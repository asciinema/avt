use crate::line::Line;
use crate::parser::Parser;
use crate::terminal::{Cursor, Terminal};

#[derive(Debug)]
pub struct Vt {
    parser: Parser,
    terminal: Terminal,
}

impl Vt {
    pub fn builder() -> Builder {
        Builder::default()
    }

    pub fn new(cols: usize, rows: usize) -> Vt {
        Self::builder().size(cols, rows).build()
    }

    pub fn feed_str(&mut self, s: &str) -> Changes<'_> {
        s.chars()
            .filter_map(|ch| self.parser.feed(ch))
            .for_each(|op| self.terminal.execute(op));

        let lines = self.terminal.changes();
        let scrollback = self.terminal.gc();

        Changes { lines, scrollback }
    }

    pub fn feed(&mut self, input: char) {
        if let Some(op) = self.parser.feed(input) {
            self.terminal.execute(op);
        }
    }

    pub fn size(&self) -> (usize, usize) {
        self.terminal.size()
    }

    pub fn resize(&mut self, cols: usize, rows: usize) -> Changes<'_> {
        self.terminal.resize(cols, rows);

        let lines = self.terminal.changes();
        let scrollback = self.terminal.gc();

        Changes { lines, scrollback }
    }

    pub fn view(&self) -> impl Iterator<Item = &Line> {
        self.terminal.view()
    }

    pub fn lines(&self) -> impl Iterator<Item = &Line> {
        self.terminal.lines()
    }

    pub fn line(&self, n: usize) -> &Line {
        self.terminal.line(n)
    }

    pub fn text(&self) -> Vec<String> {
        self.terminal.text()
    }

    pub fn cursor(&self) -> Cursor {
        self.terminal.cursor()
    }

    pub fn cursor_key_app_mode(&self) -> bool {
        self.terminal.cursor_keys_app_mode()
    }

    pub fn dump(&self) -> String {
        let mut seq = self.terminal.dump();
        seq.push_str(&self.parser.dump());

        seq
    }
}

pub struct Builder {
    size: (usize, usize),
    scrollback_limit: Option<usize>,
}

impl Builder {
    pub fn size(&mut self, cols: usize, rows: usize) -> &mut Self {
        self.size = (cols, rows);

        self
    }

    pub fn scrollback_limit(&mut self, limit: usize) -> &mut Self {
        self.scrollback_limit = Some(limit);

        self
    }

    pub fn build(&self) -> Vt {
        Vt {
            parser: Parser::new(),
            terminal: Terminal::new(self.size, self.scrollback_limit),
        }
    }
}

impl Default for Builder {
    fn default() -> Self {
        Builder {
            size: (80, 24),
            scrollback_limit: None,
        }
    }
}

pub struct Changes<'a> {
    pub lines: Vec<usize>,
    pub scrollback: Box<dyn Iterator<Item = Line> + 'a>,
}

#[cfg(test)]
mod tests {
    use super::Vt;
    use proptest::prelude::*;
    use std::env;
    use std::fs;

    #[test]
    fn feed_str_returns_changed_lines() {
        let mut vt = Vt::builder().size(2, 2).build();

        vt.feed_str("");

        let (lines, scrollback) = {
            let changes = vt.feed_str("aa\r\nbb\r\ncc");

            let scrollback = changes
                .scrollback
                .map(|line| line.text())
                .collect::<Vec<_>>();

            (changes.lines, scrollback)
        };

        assert_eq!(lines, vec![0, 1]);
        assert_eq!(scrollback, Vec::<String>::new());
    }

    #[test]
    fn feed_str_updates_accessors() {
        let mut vt = Vt::builder().size(2, 2).build();

        vt.feed_str("");
        vt.feed_str("aa\r\nbb\r\ncc");

        assert_eq!(vt.size(), (2, 2));
        assert_eq!(vt.cursor(), (2, 1));

        assert_eq!(
            vt.text(),
            vec!["aa".to_owned(), "bb".to_owned(), "cc".to_owned()]
        );

        assert_eq!(vt.view().count(), 2);
        assert!(vt.lines().count() >= 2);
        assert_eq!(vt.line(0).chars().take(2).collect::<String>(), "bb");
    }

    #[test]
    fn feed_str_returns_trimmed_scrollback() {
        let mut vt = Vt::builder().size(2, 2).scrollback_limit(0).build();

        vt.feed_str("");

        let scrollback = {
            let changes = vt.feed_str("aa\r\nbb\r\ncc");

            changes
                .scrollback
                .map(|line| line.text())
                .collect::<Vec<_>>()
        };

        assert_eq!(scrollback, vec!["aa".to_owned()]);
        assert_eq!(vt.text(), vec!["bb".to_owned(), "cc".to_owned()]);
    }

    #[test]
    fn resize_returns_changed_lines() {
        let mut vt = Vt::new(4, 2);

        vt.feed_str("");

        let (lines, scrollback_count) = {
            let changes = vt.resize(4, 3);

            (changes.lines, changes.scrollback.count())
        };

        assert_eq!(lines, vec![0, 1, 2]);
        assert_eq!(scrollback_count, 0);
    }

    #[test]
    fn resize_updates_size_accessor() {
        let mut vt = Vt::new(4, 2);

        vt.resize(4, 3);

        assert_eq!(vt.size(), (4, 3));
    }

    #[test]
    fn dump_initial() {
        let vt1 = Vt::new(10, 4);
        let mut vt2 = Vt::new(10, 4);

        vt2.feed_str(&vt1.dump());

        assert_vts_eq(&vt1, &vt2);
    }

    #[test]
    fn dump_modified() {
        let mut vt1 = Vt::new(10, 4);
        let mut vt2 = Vt::new(10, 4);

        vt1.feed_str("hello\n\rworld 日\u{9b}5W\u{9b}7`\u{1b}[W\u{9b}?6h");
        vt1.feed_str("\u{9b}2;4r\u{9b}1;5H\x1b[1;31;41m\u{9b}?25l\u{9b}4h");
        vt1.feed_str("\u{9b}?7l\u{9b}20h\u{9b}\u{3a}\x1b(0\x1b)0\u{0e}");

        vt2.feed_str(&vt1.dump());

        assert_vts_eq(&vt1, &vt2);
    }

    #[test]
    fn dump_with_file() {
        if let Ok((w, h, input, step)) = setup_dump_with_file() {
            let mut vt1 = Vt::new(w, h);

            let mut s = 0;

            for c in input.chars().take(1_000_000) {
                vt1.feed(c);

                if s == 0 {
                    let d = vt1.dump();
                    let mut vt2 = Vt::new(w, h);

                    vt2.feed_str(&d);

                    assert_vts_eq(&vt1, &vt2);
                }

                s = (s + 1) % step;
            }
        }
    }

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
        (gen_csi_intermediate(), gen_csi_sm_rm_param()).prop_map(|(inters, params)| {
            flatten(vec![vec!['\x1b', '['], inters, params, vec!['h']])
        })
    }

    fn gen_csi_rm_seq() -> impl Strategy<Value = Vec<char>> {
        (gen_csi_intermediate(), gen_csi_sm_rm_param()).prop_map(|(inters, params)| {
            flatten(vec![vec!['\x1b', '['], inters, params, vec!['l']])
        })
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
            let mut vt = Vt::builder().size(10, 5).build();

            vt.feed_str(&(input.into_iter().collect::<String>()));

            vt.terminal.verify();
            assert!(vt.lines().count() >= vt.size().1);
        }

        #[test]
        fn prop_sanity_checks_no_scrollback(input in gen_input(25)) {
            let mut vt = Vt::builder().size(10, 5).scrollback_limit(0).build();

            vt.feed_str(&(input.into_iter().collect::<String>()));

            vt.terminal.verify();
            assert!(vt.lines().count() == vt.size().1);
        }

        #[test]
        fn prop_sanity_checks_fixed_scrollback(input in gen_input(25)) {
            let scrollback_limit = 3;
            let mut vt = Vt::builder().size(10, 5).scrollback_limit(scrollback_limit).build();

            vt.feed_str(&(input.into_iter().collect::<String>()));
            let (_, rows) = vt.size();

            vt.terminal.verify();
            assert!(vt.lines().count() >= rows && vt.lines().count() <= rows + scrollback_limit);
        }

        #[test]
        fn prop_resizing(new_cols in 2..15usize, new_rows in 2..8usize, input1 in gen_input(25), input2 in gen_input(25)) {
            let mut vt = Vt::builder().size(10, 5).build();

            vt.feed_str(&(input1.into_iter().collect::<String>()));
            vt.resize(new_cols, new_rows);
            vt.feed_str(&(input2.into_iter().collect::<String>()));

            vt.terminal.verify();
            assert!(vt.lines().count() >= vt.size().1);
        }

        #[test]
        fn prop_dump(input in gen_input(25)) {
            let mut vt1 = Vt::new(10, 5);
            let mut vt2 = Vt::new(10, 5);

            vt1.feed_str(&(input.into_iter().collect::<String>()));
            vt2.feed_str(&vt1.dump());

            assert_vts_eq(&vt1, &vt2);
        }
    }

    fn setup_dump_with_file() -> Result<(usize, usize, String, usize), env::VarError> {
        let path = env::var("P")?;
        let input = fs::read_to_string(path).unwrap();
        let w: usize = env::var("W").unwrap().parse::<usize>().unwrap();
        let h: usize = env::var("H").unwrap().parse::<usize>().unwrap();

        let step: usize = env::var("S")
            .unwrap_or("1".to_owned())
            .parse::<usize>()
            .unwrap();

        Ok((w, h, input, step))
    }

    fn assert_vts_eq(vt1: &Vt, vt2: &Vt) {
        vt1.parser.assert_eq(&vt2.parser);
        vt1.terminal.assert_eq(&vt2.terminal);
    }
}
