//! Terminal state and dump regressions.
use crate::{ScreenKind, Terminal};
use proptest::prelude::*;
use std::{env, fs};

#[test]
fn cursor_position_is_none_when_hidden() {
    let mut terminal = Terminal::new(4, 2).unwrap();
    terminal.feed("ab");
    assert_eq!(terminal.cursor().position(), Some((2, 0)));
    terminal.feed("\x1b[?25l");
    assert_eq!(terminal.cursor().position(), None);
}

#[test]
fn feed_updates_screen_accessors_with_retained_history() {
    let mut terminal = Terminal::with_scrollback(2, 2, None).unwrap();
    terminal.feed("aa\r\nbb\r\ncc");
    assert_eq!(terminal.size(), (2, 2));
    assert_eq!(terminal.cursor().position(), Some((2, 1)));
    assert_eq!(terminal.primary_screen().text(), ["aa", "bb", "cc"]);
    assert_eq!(terminal.screen().visible_lines().len(), 2);
    assert_eq!(terminal.screen().all_lines().count(), 3);
    assert_eq!(terminal.screen().line(0).unwrap().text(), "bb");
    assert!(terminal.screen().line(2).is_none());
}

#[test]
fn screens_address_buffers_explicitly() {
    let mut terminal = Terminal::new(8, 2).unwrap();
    terminal.feed(&["shell", "\x1b[?1047h", "\x1b[Hvim"].concat());
    assert_eq!(terminal.active_screen_kind(), ScreenKind::Alternate);
    assert_eq!(terminal.screen().line(0).unwrap().text().trim_end(), "vim");
    assert_eq!(terminal.primary_screen().text()[0], "shell");
    terminal.feed("\x1b[?1047l");
    assert_eq!(terminal.active_screen_kind(), ScreenKind::Primary);

    assert_eq!(
        terminal.screen().line(0).unwrap().text().trim_end(),
        "shell"
    );
}

#[test]
fn primary_reflows_when_reactivated_after_alternate_resize() {
    let mut terminal = Terminal::new(4, 2).unwrap();
    terminal.feed(&["abcd", "\x1b[?1047h"].concat());
    terminal.resize(2, 2);
    assert_eq!(terminal.size(), (2, 2));
    assert_eq!(terminal.primary_screen().line(0).unwrap().len(), 4);
    terminal.feed("\x1b[?1047l");
    assert_eq!(terminal.screen().line(0).unwrap().text(), "ab");
    assert_eq!(terminal.screen().line(1).unwrap().text(), "cd");
}

#[test]
fn alternate_saved_cursor_persists_across_sessions() {
    let mut terminal = Terminal::new(8, 4).unwrap();

    terminal.feed(
        &[
            "\x1b[?1047h",
            "\x1b[2;3H\x1b7",
            "\x1b[?1047l",
            "\x1b[?1047h",
            "\x1b8",
        ]
        .concat(),
    );

    assert_eq!(terminal.cursor().position(), Some((2, 1)));
}

#[test]
fn alternate_buffer_starts_blank_on_each_entry() {
    let mut terminal = Terminal::new(8, 2).unwrap();

    terminal.feed(&["\x1b[?1047h", "hello", "\x1b[?1047l", "\x1b[41m\x1b[?1047h"].concat());

    assert_eq!(terminal.screen().line(0).unwrap().text().trim_end(), "");
    let pen = *terminal.screen().line(0).unwrap().cells()[0].pen();
    assert_eq!(pen.background(), Some(crate::Color::Indexed(1)));
}

#[test]
fn reselecting_active_buffer_preserves_content_and_dirty_state() {
    let mut terminal = Terminal::new(8, 2).unwrap();
    terminal.feed(&["\x1b[?1047h", "hello", "\x1b[?1047h"].concat());

    assert_eq!(
        terminal.screen().line(0).unwrap().text().trim_end(),
        "hello"
    );
}

#[test]
fn dump_restores_saved_cursors_in_both_buffers() {
    let mut original = Terminal::new(8, 4).unwrap();
    let mut restored = Terminal::new(8, 4).unwrap();
    original.feed(&["\x1b[3;4H\x1b7", "\x1b[?1047h", "\x1b[2;2H\x1b7"].concat());
    restored.feed(&original.dump());
    assert_terminals_eq(&original, &restored);

    for input in ["\x1b8", "\x1b[?1047l\x1b8"] {
        original.feed(input);
        restored.feed(input);
        assert_terminals_eq(&original, &restored);
    }
}

#[test]
fn dump_initial() {
    let original = Terminal::new(10, 4).unwrap();
    let mut restored = Terminal::new(10, 4).unwrap();
    restored.feed(&original.dump());
    assert_terminals_eq(&original, &restored);
}

#[test]
fn dump_modified() {
    let mut original = Terminal::new(10, 4).unwrap();
    let mut restored = Terminal::new(10, 4).unwrap();

    original.feed(
        &[
            "hello\n\rworld 日\u{9b}5W\u{9b}7`\u{1b}[W\u{9b}?6h",
            "\u{9b}2;4r\u{9b}1;5H\x1b[1;31;41m\u{9b}?25l\u{9b}4h",
            "\u{9b}?7l\u{9b}20h\u{9b}\u{3a}\x1b(0\x1b)0\u{0e}",
        ]
        .concat(),
    );

    restored.feed(&original.dump());
    assert_terminals_eq(&original, &restored);
}

#[test]
fn dump_with_file() {
    if let Ok((cols, rows, input, step)) = setup_dump_with_file() {
        let mut original = Terminal::new(cols, rows).unwrap();

        for (index, ch) in input.chars().take(1_000_000).enumerate() {
            original.feed(ch.encode_utf8(&mut [0; 4]));

            if index % step == 0 {
                let mut restored = Terminal::new(cols, rows).unwrap();
                restored.feed(&original.dump());
                assert_terminals_eq(&original, &restored);
            }
        }
    }
}

// Keep the original generators so the historical regression seeds are replayed.
fn gen_input(max_len: usize) -> impl Strategy<Value = Vec<char>> {
    prop::collection::vec(
        prop_oneof![gen_ctl_seq(), gen_esc_seq(), gen_csi_seq(), gen_text()],
        1..=max_len,
    )
    .prop_map(flatten)
}

fn gen_ctl_seq() -> impl Strategy<Value = Vec<char>> {
    let ctl_chars = vec![0x00..0x18, 0x19..0x1a, 0x1c..0x20];

    prop::sample::select(flatten(ctl_chars)).prop_map(|v: u8| vec![v as char])
}

fn gen_esc_seq() -> impl Strategy<Value = Vec<char>> {
    (
        prop::collection::vec(gen_esc_intermediate(), 0..=2),
        gen_esc_finalizer(),
    )
        .prop_map(|(inters, fin)| flatten(vec![vec!['\x1b'], inters, vec![fin]]))
}

fn gen_csi_seq() -> impl Strategy<Value = Vec<char>> {
    prop_oneof![
        gen_csi_sgr_seq(),
        gen_csi_sm_seq(),
        gen_csi_rm_seq(),
        gen_csi_any_seq(),
    ]
}

fn gen_text() -> impl Strategy<Value = Vec<char>> {
    prop::collection::vec(gen_char(), 1..10)
}

fn gen_esc_intermediate() -> impl Strategy<Value = char> {
    (0x20..0x30u8).prop_map(|v| v as char)
}

fn gen_esc_finalizer() -> impl Strategy<Value = char> {
    let finalizers = vec![
        0x30..0x50,
        0x51..0x58,
        0x59..0x5a,
        0x5a..0x5b,
        0x5c..0x5d,
        0x60..0x7f,
    ];

    prop::sample::select(flatten(finalizers)).prop_map(|v: u8| v as char)
}

fn gen_csi_sgr_seq() -> impl Strategy<Value = Vec<char>> {
    gen_csi_params().prop_map(|params| flatten(vec![vec!['\x1b', '['], params, vec!['m']]))
}

fn gen_csi_sm_seq() -> impl Strategy<Value = Vec<char>> {
    (gen_csi_intermediate(), gen_csi_sm_rm_param())
        .prop_map(|(inters, params)| flatten(vec![vec!['\x1b', '['], inters, params, vec!['h']]))
}

fn gen_csi_rm_seq() -> impl Strategy<Value = Vec<char>> {
    (gen_csi_intermediate(), gen_csi_sm_rm_param())
        .prop_map(|(inters, params)| flatten(vec![vec!['\x1b', '['], inters, params, vec!['l']]))
}

fn gen_csi_any_seq() -> impl Strategy<Value = Vec<char>> {
    (gen_csi_params(), gen_csi_finalizer())
        .prop_map(|(params, fin)| flatten(vec![vec!['\x1b', '['], params, vec![fin]]))
}

fn gen_csi_intermediate() -> impl Strategy<Value = Vec<char>> {
    prop::collection::vec(prop::sample::select(vec!['?', '!']), 0..=1)
}

fn gen_csi_params() -> impl Strategy<Value = Vec<char>> {
    prop::collection::vec(
        prop_oneof![
            gen_csi_param(),
            gen_csi_param(),
            prop::sample::select(vec![';'])
        ],
        0..=5,
    )
}

fn gen_csi_param() -> impl Strategy<Value = char> {
    (0x30..0x3au8).prop_map(|v| v as char)
}

fn gen_csi_sm_rm_param() -> impl Strategy<Value = Vec<char>> {
    let modes = vec![1, 4, 6, 7, 20, 25, 47, 1047, 1048, 1049];

    prop_oneof![
        prop::sample::select(modes).prop_map(|n| n.to_string().chars().collect()),
        prop::collection::vec(gen_csi_param(), 1..=4)
    ]
}

fn gen_csi_finalizer() -> impl Strategy<Value = char> {
    (0x40..0x7fu8).prop_map(|v| v as char)
}

fn gen_char() -> impl Strategy<Value = char> {
    prop_oneof![
        gen_ascii_char(),
        gen_ascii_char(),
        gen_ascii_char(),
        gen_ascii_char(),
        gen_ascii_char(),
        (0x80..=0xd7ffu32).prop_map(|v| char::from_u32(v).unwrap()),
        (0xf900..=0xffffu32).prop_map(|v| char::from_u32(v).unwrap())
    ]
}

fn gen_ascii_char() -> impl Strategy<Value = char> {
    (0x20..=0x7fu8).prop_map(|v| v as char)
}

fn flatten<T, I: IntoIterator<Item = T>>(seqs: Vec<I>) -> Vec<T> {
    seqs.into_iter().flatten().collect()
}

proptest! {
    #[test]
    fn prop_sanity_checks_infinite_scrollback(input in gen_input(25)) {
        let mut terminal = Terminal::with_scrollback(10, 5, None).unwrap();
        terminal.feed(&input.into_iter().collect::<String>());
        terminal.engine.verify();
        assert!(terminal.screen().all_lines().count() >= terminal.size().1);
    }

    #[test]
    fn prop_sanity_checks_no_scrollback(input in gen_input(25)) {
        let mut terminal = Terminal::new(10, 5).unwrap();
        terminal.feed(&input.into_iter().collect::<String>());
        terminal.engine.verify();
        assert_eq!(terminal.screen().all_lines().count(), terminal.size().1);
    }

    #[test]
    fn prop_sanity_checks_fixed_scrollback(input in gen_input(25)) {
        let limit = 3;
        let mut terminal = Terminal::with_scrollback(10, 5, Some(limit)).unwrap();
        terminal.feed(&input.into_iter().collect::<String>());
        let (_, rows) = terminal.size();
        terminal.engine.verify();
        assert!((rows..=rows + limit).contains(&terminal.screen().all_lines().count()));
    }

    #[test]
    fn prop_resizing(
        new_cols in 2..15usize,
        new_rows in 2..8usize,
        input1 in gen_input(25),
        input2 in gen_input(25)
    ) {
        let mut terminal = Terminal::with_scrollback(10, 5, None).unwrap();
        terminal.feed(&input1.into_iter().collect::<String>());
        terminal.resize(new_cols, new_rows);
        terminal.feed(&input2.into_iter().collect::<String>());
        terminal.engine.verify();
        assert!(terminal.screen().all_lines().count() >= terminal.size().1);
    }

    #[test]
    fn prop_dump(input in gen_input(25)) {
        let mut original = Terminal::new(10, 5).unwrap();
        let mut restored = Terminal::new(10, 5).unwrap();
        original.feed(&input.into_iter().collect::<String>());
        restored.feed(&original.dump());
        assert_terminals_eq(&original, &restored);
    }
}

fn setup_dump_with_file() -> Result<(usize, usize, String, usize), env::VarError> {
    let path = env::var("P")?;
    let input = fs::read_to_string(path).unwrap();
    let cols = env::var("W").unwrap().parse().unwrap();
    let rows = env::var("H").unwrap().parse().unwrap();
    let step = env::var("S").unwrap_or("1".to_owned()).parse().unwrap();
    assert!(step > 0, "S must be positive");

    Ok((cols, rows, input, step))
}

fn assert_terminals_eq(original: &Terminal, restored: &Terminal) {
    original.parser.assert_eq(&restored.parser);
    original.engine.assert_eq(&restored.engine);
}
