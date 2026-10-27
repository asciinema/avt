#![no_std]

//! A virtual terminal with composable observers.
//!
//! [`Terminal`] interprets application output, including text and control
//! sequences, and maintains primary and alternate screen buffers.
//! Plain mutations return `()`; `_with` variants return an
//! [`Observer`]'s output.
//!
//! Stateful observers should be reused across calls when collecting
//! dirty rows, transcripts, or shell integration events. Feed boundaries
//! do not finish parsing or logical lines.
//!
//! ```
//! use avt::observer::DirtyRows;
//! use avt::Terminal;
//!
//! let mut terminal = Terminal::new(80, 24)?;
//! let mut dirty = DirtyRows::new();
//! let rows = terminal.feed_with(["hello", "\r\nworld"], &mut dirty);
//! assert_eq!(rows.len(), 24); // First observation requests a full repaint.
//! assert_eq!(terminal.screen().line(1).unwrap().text().trim_end(), "world");
//!
//! # Ok::<(), avt::InvalidSize>(())
//! ```
//!
//! [`Parser`] can also be used independently, with parsed functions passed to
//! [`Terminal::execute`]. Construction is fallible and defaults to zero history;
//! [`Terminal::with_scrollback`] enables bounded or unlimited primary history.
//! [`Terminal::dump`] initializes visible state and pending parsing, omitting
//! retained history and analysis state. See the repository examples for rendering,
//! transcripts, shell tracking, replay, and dependency-based slicing.

extern crate alloc;
#[cfg(test)]
extern crate std;

mod anchor;
mod buffer;
mod cell;
mod charset;
mod color;
mod engine;
pub mod input;
mod line;
pub mod observer;
pub mod operation;
pub mod parser;
mod pen;
mod tabs;

pub use anchor::Anchor;
pub use cell::Cell;
pub use charset::Charset;
pub use color::{Color, Rgb};
pub use engine::{Cursor, Position, Screen, ScreenKind};
pub use input::{AsInput, Input, IntoInputs};
pub use line::Line;
pub use observer::{Observation, Observer};
pub use operation::{IntoOperations, Operation};
pub use parser::Parser;
pub use pen::Pen;

use alloc::string::String;

/// Rejected terminal dimensions. Both columns and rows must be positive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidSize {
    pub cols: usize,
    pub rows: usize,
}

impl core::fmt::Display for InvalidSize {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "invalid terminal size {}x{}: columns and rows must be positive",
            self.cols, self.rows
        )
    }
}

impl core::error::Error for InvalidSize {}

/// A virtual terminal that interprets text and control sequences, maintaining
/// screen contents, cursor state, and optional scrollback.
pub struct Terminal {
    parser: Parser,
    engine: engine::Engine,
}

impl Terminal {
    /// Creates a terminal with no retained scrollback.
    /// Returns `InvalidSize` if either dimension is zero.
    pub fn new(cols: usize, rows: usize) -> Result<Self, InvalidSize> {
        Self::with_scrollback(cols, rows, Some(0))
    }

    /// Creates a terminal with zero (`Some(0)`), bounded (`Some(limit)`), or
    /// unlimited (`None`) primary scrollback.
    /// Returns `InvalidSize` if either dimension is zero.
    pub fn with_scrollback(
        cols: usize,
        rows: usize,
        scrollback_limit: Option<usize>,
    ) -> Result<Self, InvalidSize> {
        if cols == 0 || rows == 0 {
            return Err(InvalidSize { cols, rows });
        }

        Ok(Self {
            parser: Parser::new(),
            engine: engine::Engine::new((cols, rows), scrollback_limit),
        })
    }

    /// Processes text and control sequences, updating the virtual terminal
    /// state. Accepts a string, character, `Input`, or flat sequence of
    /// fragments. Iterators are processed incrementally, with one call
    /// lifecycle for the whole sequence, including an empty sequence.
    pub fn feed<I, Kind>(&mut self, input: I)
    where
        I: IntoInputs<Kind>,
    {
        self.feed_with(input, ());
    }

    /// Processes input with the supplied observer and returns its result.
    /// Fragment boundaries do not finish parsing, flush analysis, or invoke
    /// additional begin/end hooks.
    pub fn feed_with<I, Kind, O>(&mut self, input: I, mut observer: O) -> O::Output
    where
        I: IntoInputs<Kind>,
        O: Observer,
    {
        observer.begin(&mut Observation::new(self));

        for item in input.into_inputs() {
            self.feed_fragment(item.as_input(), &mut observer);
        }

        observer.end(&mut Observation::new(self))
    }

    fn feed_fragment<O: Observer>(&mut self, input: Input<'_>, observer: &mut O) {
        match input {
            Input::Text(text) => {
                for ch in text.chars() {
                    self.feed_char(ch, observer);
                }
            }

            Input::Char(ch) => self.feed_char(ch, observer),
        }
    }

    fn feed_char<O: Observer>(&mut self, ch: char, observer: &mut O) {
        if let Some(function) = self.parser.feed(ch) {
            self.apply(Operation::Function(function), observer);
        }
    }

    /// Executes a function, operation, or flat iterable of either. Owned values
    /// are moved into execution; use `.iter().cloned()` to execute operations
    /// from a borrowed collection. Pending text-parser state is preserved.
    pub fn execute<I, Kind>(&mut self, operations: I)
    where
        I: IntoOperations<Kind>,
    {
        self.execute_with(operations, ());
    }

    /// Executes operations with the supplied observer and returns its result,
    /// including for an empty iterable. Actions are processed incrementally.
    /// Before requesting the next operation, any primary-buffer lines removed
    /// to enforce the scrollback limit are passed to [`Observer::evicted`].
    pub fn execute_with<I, Kind, O>(&mut self, operations: I, mut observer: O) -> O::Output
    where
        I: IntoOperations<Kind>,
        O: Observer,
    {
        observer.begin(&mut Observation::new(self));

        for operation in operations.into_operations() {
            self.apply(operation.into(), &mut observer);
        }

        observer.end(&mut Observation::new(self))
    }

    /// Changes the virtual terminal's dimensions. Dimensions are clamped to at
    /// least one. The primary screen reflows even while the alternate screen is
    /// active. The alternate screen clips or pads rows without reflow,
    /// preserving soft-wrap flags. Intermediate resizes can discard content
    /// even if the original size is restored. Reflow to one column erases wide
    /// characters that cannot fit, preserving their style and invalidating
    /// anchors in the erased cells.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.resize_with(cols, rows, ());
    }

    pub fn resize_with<O: Observer>(&mut self, cols: usize, rows: usize, observer: O) -> O::Output {
        self.execute_with(Operation::Resize { cols, rows }, observer)
    }

    #[inline]
    fn apply<O: Observer>(&mut self, mut operation: Operation, observer: &mut O) {
        if let Operation::Resize { cols, rows } = &mut operation {
            *cols = (*cols).max(1);
            *rows = (*rows).max(1);
        }

        observer.before(&mut Observation::new(self), &operation);

        match &operation {
            Operation::Function(function) => self.engine.execute_with(function, observer),

            Operation::Resize { cols, rows } => {
                self.engine.resize_with(*cols, *rows, observer);
            }
        }

        observer.after(&mut Observation::new(self), &operation);
        self.evict(observer);
    }

    fn evict<O: Observer>(&mut self, observer: &mut O) {
        while let Some(line) = self.engine.pop_evicted() {
            observer.evicted(&mut Observation::new(self), &line);
        }
    }

    pub fn size(&self) -> (usize, usize) {
        self.engine.size()
    }

    pub fn cursor_position(&self) -> Position {
        let cursor = self.cursor();

        Position {
            screen: self.active_screen_kind(),
            col: cursor.col,
            row: cursor.row as isize,
        }
    }

    pub fn anchor_at_cursor(&mut self) -> Anchor {
        self.anchor_at(self.cursor_position())
            .expect("cursor is in the active screen")
    }

    /// Registers a positional anchor. Invalid coordinates return None.
    pub fn anchor_at(&mut self, position: Position) -> Option<Anchor> {
        self.engine.anchor_at(position)
    }

    /// Resolves even in an inactive screen. Destructive erasure, removal, eviction,
    /// and hard reset invalidate positions. Ordinary writes leave them in place.
    /// Primary-screen ED2 moves anchors in preserved rows into history, subject
    /// to normal retention and eviction.
    pub fn resolve(&self, anchor: &Anchor) -> Option<Position> {
        self.engine.resolve(anchor)
    }

    /// Returns a read-only view of the active screen.
    pub fn screen(&self) -> Screen<'_> {
        self.engine.active_screen()
    }

    /// Returns a read-only view of the primary screen, even when inactive.
    pub fn primary_screen(&self) -> Screen<'_> {
        self.engine.primary_screen()
    }

    pub fn cursor(&self) -> Cursor {
        self.engine.cursor()
    }

    /// Returns which screen is active.
    pub fn active_screen_kind(&self) -> ScreenKind {
        self.engine.active_screen_kind()
    }

    /// Escape-sequence initialization for a terminal of the same size, including
    /// visible buffers, cursor state, terminal modes, and unfinished parsing.
    /// Retained scrollback and observer/anchor state are omitted; this is not a
    /// full-state snapshot.
    pub fn dump(&self) -> String {
        let mut dump = parser::dump(&self.engine.dump());
        dump.push_str(&self.parser.dump());

        dump
    }

    pub fn cursor_key_app_mode(&self) -> bool {
        self.engine.cursor_keys_app_mode()
    }
}

#[cfg(test)]
mod tests {

    use crate::observer::{DirtyRows, PromptId, ShellEvent, ShellTracker, Transcript};
    use crate::parser::{EdScope, Function};
    use crate::{Input, IntoInputs, Observation, Observer, Operation, ScreenKind, Terminal};
    use alloc::borrow::ToOwned as _;
    use alloc::format;
    use alloc::string::{String, ToString as _};
    use alloc::vec;
    use alloc::vec::Vec;
    use proptest::prelude::*;

    const PROMPT: &str = "\x1b]133;A\x07$ \x1b]133;B\x07";
    const SUBMIT: &str = "\x1b]133;C\x07";

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
        terminal.feed(["shell", "\x1b[?1047h", "\x1b[Hvim"]);
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
    fn dump_restores_primary_after_alternate_resize() {
        for mode in [1047, 1049] {
            let mut original = Terminal::new(4, 2).unwrap();
            original.feed("abcd\r\nefgh\x1b[H");
            original.feed(format!("\x1b[?{mode}habcdEF"));
            original.resize(2, 2);
            let mut restored = Terminal::new(2, 2).unwrap();
            restored.feed(original.dump());
            assert_dump_equivalent(&original, &restored);

            for terminal in [&mut original, &mut restored] {
                terminal.feed(format!("\x1b[?{mode}l"));
                assert_eq!(terminal.screen().text(), ["abcd"]);
            }

            assert_dump_equivalent(&original, &restored);
        }
    }

    #[test]
    fn dump_preserves_hidden_primary_cursor_for_further_resizes() {
        for mode in [1047, 1049] {
            for text in ["abcd", "ab界", "ab\x1b[31m界", "abcd\x1b[H", "ab\x1b[3;4H"] {
                let mut original = Terminal::new(4, 3).unwrap();
                original.feed(text);
                original.feed(format!("\x1b[?{mode}h\x1b[2;2Halt"));
                let mut restored = Terminal::new(4, 3).unwrap();
                restored.feed(original.dump());
                assert_dump_equivalent(&original, &restored);

                for (cols, rows) in [(4, 1), (2, 3), (4, 3), (1, 2), (4, 3)] {
                    original.resize(cols, rows);
                    restored.resize(cols, rows);
                    original.engine.verify();
                    restored.engine.verify();
                    assert_dump_equivalent(&original, &restored);
                }

                original.feed(format!("\x1b[?{mode}lX"));
                restored.feed(format!("\x1b[?{mode}lX"));
                assert_dump_equivalent(&original, &restored);
            }
        }
    }

    #[test]
    fn resize_clamps_saved_cursors_in_both_buffers() {
        for alternate in [false, true] {
            let mut original = Terminal::new(8, 4).unwrap();
            original.feed("\x1b[4;8H\x1b7\x1b[?1047h\x1b[4;8H\x1b7");

            if !alternate {
                original.feed("\x1b[?1047l");
            }

            original.resize(2, 2);
            original.resize(8, 4);
            let mut restored = Terminal::new(8, 4).unwrap();
            restored.feed(original.dump());
            assert_dump_equivalent(&original, &restored);

            for input in ["\x1b[?1047h\x1b8", "\x1b[?1047l\x1b8"] {
                original.feed(input);
                restored.feed(input);
                assert_eq!(original.cursor().position(), Some((1, 1)));
                assert_dump_equivalent(&original, &restored);
            }
        }
    }

    #[test]
    fn dump_restores_saved_cursors_in_both_buffers() {
        let mut original = Terminal::new(8, 4).unwrap();
        let mut restored = Terminal::new(8, 4).unwrap();
        original.feed(["\x1b[3;4H\x1b7", "\x1b[?1047h", "\x1b[2;2H\x1b7"]);
        restored.feed(original.dump());
        assert_dump_equivalent(&original, &restored);

        for input in ["\x1b8", "\x1b[?1047l\x1b8"] {
            original.feed(input);
            restored.feed(input);
            assert_dump_equivalent(&original, &restored);
        }
    }

    #[test]
    fn dump_initial() {
        let original = Terminal::new(10, 4).unwrap();
        let mut restored = Terminal::new(10, 4).unwrap();
        restored.feed(original.dump());
        assert_dump_equivalent(&original, &restored);
    }

    #[test]
    fn dump_modified() {
        let mut original = Terminal::new(10, 4).unwrap();
        let mut restored = Terminal::new(10, 4).unwrap();

        original.feed([
            "hello\n\rworld 日\u{9b}5W\u{9b}7`\u{1b}[W\u{9b}?6h",
            "\u{9b}2;4r\u{9b}1;5H\x1b[1;31;41m\u{9b}?25l\u{9b}4h",
            "\u{9b}?7l\u{9b}20h\u{9b}\u{3a}\x1b(0\x1b)0\u{0e}",
        ]);

        restored.feed(original.dump());
        assert_dump_equivalent(&original, &restored);
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
        fn prop_lossy_utf8_input_does_not_panic(
            bytes in prop::collection::vec(any::<u8>(), 0..4096),
        ) {
            let mut terminal = Terminal::new(10, 4).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            terminal.feed(text.as_ref());
            terminal.engine.verify();
        }

        #[test]
        fn prop_sanity_checks_across_retention_policies(input in gen_input(25)) {
            for limit in [Some(0), Some(3), None] {
                let mut terminal = Terminal::with_scrollback(10, 5, limit).unwrap();
                terminal.feed(&input);
                terminal.engine.verify();
                let (_, rows) = terminal.size();
                let retained = terminal.primary_screen().all_lines().count();

                match limit {
                    Some(limit) => assert!((rows..=rows + limit).contains(&retained)),

                    None => assert!(retained >= rows),
                }

                if terminal.active_screen_kind() == ScreenKind::Alternate {
                    assert_eq!(terminal.screen().all_lines().count(), rows);
                }
            }
        }

        #[test]
        fn prop_resizing(
            new_cols in 1..15usize,
            new_rows in 1..8usize,
            input1 in gen_input(25),
            input2 in gen_input(25)
        ) {
            let mut terminal = Terminal::with_scrollback(10, 5, None).unwrap();
            terminal.feed(input1);
            terminal.resize(new_cols, new_rows);
            terminal.feed(input2);
            terminal.engine.verify();
            assert!(terminal.primary_screen().all_lines().count() >= terminal.size().1);
        }

        #[test]
        fn prop_dump(input in gen_input(25)) {
            let mut original = Terminal::new(10, 5).unwrap();
            let mut restored = Terminal::new(10, 5).unwrap();
            original.feed(input);
            restored.feed(original.dump());
            assert_dump_equivalent(&original, &restored);
        }

        #[test]
        fn prop_dump_with_resizes(
            steps in prop::collection::vec((gen_input(8), 1..15usize, 1..8usize), 1..8)
        ) {
            let mut original = Terminal::new(10, 5).unwrap();
            let mut restored = Terminal::new(10, 5).unwrap();

            for (input, cols, rows) in steps {
                original.feed(&input);
                restored.feed(&input);
                original.resize(cols, rows);
                restored.resize(cols, rows);
                original.engine.verify();
                restored.engine.verify();
                assert_dump_equivalent(&original, &restored);
                restored = Terminal::new(cols, rows).unwrap();
                restored.feed(original.dump());
                assert_dump_equivalent(&original, &restored);
            }
        }
    }

    fn assert_dump_equivalent(original: &Terminal, restored: &Terminal) {
        original
            .parser
            .assert_continuation_equivalent(&restored.parser);

        original.engine.assert_dump_equivalent(&restored.engine);
    }

    fn collect(input: &str, split: usize) -> (Vec<ShellEvent>, Vec<String>) {
        let mut terminal = Terminal::new(24, 4).unwrap();
        let mut shell = ShellTracker::new();
        let mut transcript = Transcript::new();
        let mut events = Vec::new();
        let mut lines = Vec::new();

        for chunk in [&input[..split], &input[split..]] {
            let (shell_events, text) = terminal.feed_with(chunk, (&mut shell, &mut transcript));
            events.extend(shell_events);
            lines.extend(text);
        }

        lines.extend(transcript.finish(&terminal));

        (events, lines)
    }

    #[test]
    fn paired_observers_are_independent_of_every_input_split() {
        let input = format!(
            "old\r\n{PROMPT}wrong\x1b[H\x1b[2J{PROMPT}echo 界{SUBMIT}\r\nresult\r\n\x1b]133;D;0\x1b\\{PROMPT}"
        );

        for (input, expected_lines) in [
            (
                input.as_str(),
                &["old", "$ wrong", "$ echo 界", "result", "$"][..],
            ),
            (
                "before\r\n$ 界\x1b[H\x1b[2J$ corrected",
                &["before", "$ 界", "$ corrected"][..],
            ),
        ] {
            let expected = collect(input, 0);
            assert_eq!(expected.1, expected_lines);

            for split in input
                .char_indices()
                .map(|(index, _)| index)
                .chain([input.len()])
            {
                assert_eq!(collect(input, split), expected, "split at byte {split}");
            }

            let mut terminal = Terminal::new(24, 4).unwrap();
            let mut shell = ShellTracker::new();
            let mut transcript = Transcript::new();
            let mut events = Vec::new();
            let mut lines = Vec::new();

            for ch in input.chars() {
                let (batch, text) = terminal.feed_with(ch, (&mut shell, &mut transcript));
                events.extend(batch);
                lines.extend(text);
            }

            lines.extend(transcript.finish(&terminal));
            assert_eq!((events, lines), expected);
        }
    }

    #[test]
    fn paired_observers_reconstruct_shell_sessions_in_bulk_and_chunks() {
        const COMMANDS: usize = 32;

        let input: String = (0..COMMANDS)
            .map(|n| format!("{PROMPT}echo item-{n:04}{SUBMIT}\r\nitem-{n:04}\r\n\x1b]133;D;0\x07"))
            .collect();

        let expected: Vec<_> = (0..COMMANDS)
            .flat_map(|n| [format!("$ echo item-{n:04}"), format!("item-{n:04}")])
            .collect();

        for chunk_size in [input.len(), 32] {
            let offsets: Vec<_> = input
                .char_indices()
                .step_by(chunk_size)
                .map(|(offset, _)| offset)
                .chain([input.len()])
                .collect();

            let mut terminal = Terminal::new(80, 24).unwrap();
            let mut shell = ShellTracker::new();
            let mut transcript = Transcript::new();
            let mut commands = 0;
            let mut lines = Vec::new();

            for pair in offsets.windows(2) {
                let (events, output) =
                    terminal.feed_with(&input[pair[0]..pair[1]], (&mut shell, &mut transcript));

                for event in events {
                    if let ShellEvent::CommandSubmitted(command) = event {
                        assert_eq!(command.text, Some(format!("echo item-{commands:04}")));
                        commands += 1;
                    }
                }

                lines.extend(output);
            }

            lines.extend(transcript.finish(&terminal));
            assert_eq!(commands, COMMANDS);
            assert_eq!(lines, expected);
        }
    }

    #[derive(Debug, PartialEq)]
    enum Notification {
        Resized(usize, usize),
        Dirty(std::ops::Range<usize>),
        Evicted(String),
    }

    // Observer outputs need not implement Default.
    #[derive(Debug, PartialEq)]
    struct CallReport {
        number: usize,
        before: Vec<crate::Line>,
        after: Vec<crate::Line>,
        size_before: (usize, usize),
        size_after: (usize, usize),
        operations: Vec<Operation>,
        hooks: Vec<&'static str>,
        notifications: Vec<Notification>,
    }

    #[derive(Default)]
    struct Calls {
        notifications: Vec<Notification>,
        number: usize,
        before: Vec<crate::Line>,
        size_before: (usize, usize),
        operations: Vec<Operation>,
        hooks: Vec<&'static str>,
        active: bool,
        in_action: bool,
        order: Option<(usize, std::rc::Rc<std::cell::Cell<usize>>)>,
    }

    impl Calls {
        fn check_order(&self) {
            if let Some((index, next)) = &self.order {
                assert_eq!(next.get(), *index);
                next.set(1 - index);
            }
        }
    }

    impl Observer for Calls {
        type Output = CallReport;

        fn begin(&mut self, context: &mut Observation<'_>) {
            self.check_order();
            assert!(!self.active);
            self.active = true;
            self.number += 1;

            self.before = context
                .terminal()
                .screen()
                .visible_lines()
                .cloned()
                .collect();

            self.size_before = context.terminal().size();
            self.hooks.push("begin");
        }

        fn before(&mut self, _: &mut Observation<'_>, operation: &Operation) {
            self.check_order();
            assert!(self.active);
            assert!(!self.in_action);
            self.in_action = true;
            self.operations.push(operation.clone());
            self.hooks.push("before");
        }

        fn resized(&mut self, cols: usize, rows: usize) {
            self.check_order();
            assert!(self.in_action);
            self.notifications.push(Notification::Resized(cols, rows));
        }

        fn dirty(&mut self, rows: std::ops::Range<usize>) {
            self.check_order();
            assert!(self.in_action);
            self.notifications.push(Notification::Dirty(rows));
        }

        fn after(&mut self, context: &mut Observation<'_>, operation: &Operation) {
            self.check_order();
            assert!(self.active);
            assert!(self.in_action);
            self.in_action = false;
            assert_eq!(self.operations.last(), Some(operation));

            if let Operation::Resize { cols, rows } = operation {
                assert_eq!(context.terminal().size(), (*cols, *rows));
            }

            self.hooks.push("after");
        }

        fn evicted(&mut self, _: &mut Observation<'_>, line: &crate::Line) {
            self.check_order();
            assert!(self.active);
            assert!(!self.in_action);
            self.notifications.push(Notification::Evicted(line.text()));
        }

        fn end(&mut self, context: &mut Observation<'_>) -> Self::Output {
            self.check_order();
            assert!(self.active);
            assert!(!self.in_action);
            self.active = false;
            self.hooks.push("end");

            CallReport {
                number: self.number,
                before: std::mem::take(&mut self.before),
                after: context
                    .terminal()
                    .screen()
                    .visible_lines()
                    .cloned()
                    .collect(),
                size_before: self.size_before,
                size_after: context.terminal().size(),
                operations: std::mem::take(&mut self.operations),
                hooks: std::mem::take(&mut self.hooks),
                notifications: std::mem::take(&mut self.notifications),
            }
        }
    }

    #[test]
    #[allow(clippy::needless_borrows_for_generic_args)] // Exercise borrowed forms too.
    fn all_input_forms_share_one_feed_lifecycle() {
        fn check<I, Kind>(input: I, text: &str)
        where
            I: IntoInputs<Kind>,
        {
            let mut expected = Terminal::new(16, 3).unwrap();
            let mut actual = Terminal::new(16, 3).unwrap();
            let expected_report = expected.feed_with(text, Calls::default());
            let report = actual.feed_with(input, Calls::default());
            assert_eq!(report, expected_report);
            assert_eq!(report.number, 1);

            actual
                .parser
                .assert_continuation_equivalent(&expected.parser);
        }

        let text = "hé界";
        let owned = text.to_owned();
        check(text, text);
        check(&owned, text);
        check(owned.clone(), text);
        check('界', "界");
        check(&'界', "界");
        check('界'.encode_utf8(&mut [0; 4]), "界");
        check(Input::Text(text), text);
        check(Input::Char('界'), "界");
        check(Input::from(&owned), text);
        check(Input::from('界'), "界");

        let chunks = ["hé", "界"];
        check(chunks, text);
        check(&chunks[..], text);
        check(chunks.iter(), text);
        check(chunks.to_vec(), text);
        let chunks: Vec<String> = chunks.iter().map(|text| text.to_string()).collect();
        check(&chunks, text);
        check(&chunks[..], text);
        check(chunks.iter(), text);
        check(chunks, text);
        check(text.chars(), text);
        check(['h', 'é', '界'], text);
        check(&['h', 'é', '界'][..], text);
        check((0..3).map(|n| format!("{n}")), "012");

        let parts = [Input::Char('h'), Input::Text("é界")];
        check(parts, text);
        check(&parts[..], text);
        check(parts.iter(), text);
        check(["\x1b[3", "1m", "X"], "\x1b[31mX");
        check(Vec::<String>::new(), "");
        check(std::iter::empty::<Input<'_>>(), "");
    }

    #[test]
    fn feed_processes_owned_chunks_before_advancing_iterator() {
        use std::cell::Cell;

        struct Progress<'a> {
            produced: &'a Cell<usize>,
            printed: usize,
        }

        impl Observer for Progress<'_> {
            type Output = usize;

            fn begin(&mut self, _: &mut Observation<'_>) {
                assert_eq!(self.produced.get(), 0);
            }

            fn after(&mut self, _: &mut Observation<'_>, operation: &Operation) {
                assert!(matches!(operation, Operation::Function(Function::Print(_))));
                self.printed += 1;
                assert_eq!(self.produced.get(), self.printed);
            }

            fn end(&mut self, _: &mut Observation<'_>) -> Self::Output {
                self.printed
            }
        }

        let produced = Cell::new(0);

        let chunks = (0..3).map(|n| {
            produced.set(n + 1);

            n.to_string()
        });

        let mut terminal = Terminal::new(8, 2).unwrap();

        let printed = terminal.feed_with(
            chunks,
            Progress {
                produced: &produced,
                printed: 0,
            },
        );

        assert_eq!(printed, 3);
        assert_eq!(terminal.screen().text()[0], "012");
    }

    #[test]
    fn shell_events_and_transcript_survive_chunking_and_reflow() {
        fn replay(
            character_fragments: bool,
            separate_calls: bool,
        ) -> (Vec<ShellEvent>, Vec<String>, Terminal) {
            let first = [
                "pré\r\n\x1b]13",
                "3;A\x07$ \x1b]133;B\x07ecxo\x1b[2Dh\x1b[C\x1b]133;C",
            ];

            let second = [
                "\x07\x1b[2J\x1b[Hdone\r\n\x1b]133;D;0\x07",
                "\x1b]133;A\x07$ \x1b]133;B\x07pwd\x1b]133;C\x07\r\n\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07cancel\x1b]133;D\x07",
            ];

            let mut terminal = Terminal::new(24, 5).unwrap();
            let mut shell = ShellTracker::new();
            let mut transcript = Transcript::new();
            let mut events = Vec::new();
            let mut lines = Vec::new();

            for (index, parts) in [first, second].into_iter().enumerate() {
                if index == 1 {
                    let (batch, text) = terminal.resize_with(4, 8, (&mut shell, &mut transcript));
                    events.extend(batch);
                    lines.extend(text);
                }

                let fragments: Vec<_> = if character_fragments {
                    parts
                        .into_iter()
                        .flat_map(str::chars)
                        .map(Input::Char)
                        .collect()
                } else {
                    parts.into_iter().map(Input::Text).collect()
                };

                if separate_calls {
                    for input in fragments {
                        let (batch, text) =
                            terminal.feed_with(input, (&mut shell, &mut transcript));

                        events.extend(batch);
                        lines.extend(text);
                    }
                } else {
                    let (batch, text) =
                        terminal.feed_with(fragments, (&mut shell, &mut transcript));

                    events.extend(batch);
                    lines.extend(text);
                }
            }

            lines.extend(transcript.finish(&terminal));

            (events, lines, terminal)
        }

        let expected = replay(false, false);

        let commands: Vec<_> = expected
            .0
            .iter()
            .filter_map(|event| match event {
                ShellEvent::CommandSubmitted(command) => {
                    Some((command.prompt, command.text.as_deref()))
                }

                _ => None,
            })
            .collect();

        assert_eq!(
            commands,
            [
                (Some(PromptId(0)), Some("echo")),
                (Some(PromptId(1)), Some("pwd")),
            ]
        );

        for (characters, separate) in [(false, true), (true, false), (true, true)] {
            let actual = replay(characters, separate);
            assert_eq!(actual.0, expected.0);
            assert_eq!(actual.1, expected.1);

            actual
                .2
                .engine
                .assert_execution_equivalent(&expected.2.engine);

            actual
                .2
                .parser
                .assert_continuation_equivalent(&expected.2.parser);
        }
    }

    #[test]
    fn dump_restoration_preserves_pending_shell_marker_recognition() {
        let mut original = Terminal::new(24, 4).unwrap();
        original.feed("\x1b]133;");
        let mut restored = Terminal::new(24, 4).unwrap();
        restored.feed(original.dump());
        let mut original_shell = ShellTracker::new();
        let mut restored_shell = ShellTracker::new();
        let tail = "A\x07$ \x1b]133;B\x07cmd\x1b]133;C\x07";
        let original_events = original.feed_with(tail, &mut original_shell);
        let restored_events = restored.feed_with(tail, &mut restored_shell);
        assert_eq!(original_events, restored_events);

        assert_eq!(restored_events[0], ShellEvent::PromptStarted(PromptId(0)));

        assert!(matches!(
            &restored_events[1],
            ShellEvent::CommandSubmitted(command)
                if command.prompt == Some(PromptId(0))
                    && command.text.as_deref() == Some("cmd")
        ));

        original.engine.assert_dump_equivalent(&restored.engine);

        original
            .parser
            .assert_continuation_equivalent(&restored.parser);
    }

    #[test]
    fn tuple_observers_preserve_call_lifecycle_and_output_order() {
        let mut terminal = Terminal::new(3, 2).unwrap();
        // Every callback must visit the left observer before the right observer.
        let next = std::rc::Rc::new(std::cell::Cell::new(0));

        let mut first = Calls {
            order: Some((0, next.clone())),
            ..Calls::default()
        };

        let mut second = Calls {
            order: Some((1, next.clone())),
            ..Calls::default()
        };

        let (empty, other) = terminal.feed_with("", (&mut first, &mut second));
        assert_eq!(empty, other);
        assert_eq!(empty.number, 1);
        assert_eq!(empty.before, empty.after);
        assert_eq!(empty.hooks, ["begin", "end"]);
        assert!(empty.notifications.is_empty());

        let (partial, other) = terminal.feed_with("\x1b[3", (&mut first, &mut second));
        assert_eq!(partial, other);
        assert_eq!(partial.number, 2);
        assert_eq!(partial.before, empty.after);
        assert_eq!(partial.before, partial.after); // Incomplete input leaves the screen alone.
        assert_eq!(partial.hooks, ["begin", "end"]);
        assert!(partial.notifications.is_empty());

        let (complete, other) = terminal.feed_with("1mX", (&mut first, &mut second));
        assert_eq!(complete, other);
        assert_eq!(complete.before, partial.after);

        assert_eq!(
            complete.hooks,
            ["begin", "before", "after", "before", "after", "end"]
        );

        assert_eq!(complete.notifications, [Notification::Dirty(0..1)]);

        let (resize, other) = terminal.resize_with(3, 2, (&mut first, &mut second));
        assert_eq!(resize, other);
        assert_eq!(resize.before, complete.after);
        assert_eq!(resize.before, resize.after);
        assert_eq!(resize.hooks, ["begin", "before", "after", "end"]);
        assert_eq!(resize.operations, [Operation::Resize { cols: 3, rows: 2 }]);
        assert!(resize.notifications.is_empty());

        let (reset, other) = terminal.execute_with(Function::Ris, (&mut first, &mut second));
        assert_eq!(reset, other);
        assert_eq!(reset.number, 5);
        assert_eq!(reset.before, resize.after);
        assert_eq!(reset.hooks, ["begin", "before", "after", "end"]);
        assert_eq!(reset.operations, [Operation::Function(Function::Ris)]);
        assert_eq!(reset.notifications, [Notification::Dirty(0..2)]);
        assert_eq!(empty.hooks, ["begin", "end"]); // Earlier reports remain owned.

        terminal.feed_with("a\r\nb", (&mut first, &mut second));
        second.number = 10; // Distinguish outputs to detect swapped tuple results.
        let (resize, other) = terminal.resize_with(3, 1, (&mut first, &mut second));
        assert_eq!((resize.number, other.number), (7, 11));

        for report in [resize, other] {
            assert_eq!(report.size_before, (3, 2));
            assert_eq!(report.size_after, (3, 1));
            assert_eq!(report.hooks, ["begin", "before", "after", "end"]);
            assert_eq!(report.operations, [Operation::Resize { cols: 3, rows: 1 }]);

            assert_eq!(
                report.notifications,
                [
                    Notification::Resized(3, 1),
                    Notification::Dirty(0..1),
                    Notification::Evicted("a  ".to_owned()),
                ]
            );
        }

        assert_eq!(next.get(), 0);
    }

    #[test]
    fn resize_matches_explicit_operation_lifecycle_and_clamping() {
        for (cols, rows) in [(8, 4), (4, 3), (0, 0), (0, 2)] {
            let mut convenience = Terminal::new(4, 3).unwrap();
            let mut explicit = Terminal::new(4, 3).unwrap();

            for terminal in [&mut convenience, &mut explicit] {
                terminal.feed("abcd\r\nefgh\r\nlast");
            }

            let report = convenience.resize_with(cols, rows, Calls::default());

            let other_report =
                explicit.execute_with(Operation::Resize { cols, rows }, Calls::default());

            assert_eq!(report, other_report);
            assert_eq!(report.number, 1);
            assert_eq!(report.hooks, ["begin", "before", "after", "end"]);
            assert_eq!(report.size_before, (4, 3));
            assert_eq!(report.size_after, (cols.max(1), rows.max(1)));

            assert_eq!(
                report.operations,
                [Operation::Resize {
                    cols: cols.max(1),
                    rows: rows.max(1)
                }]
            );

            convenience
                .engine
                .assert_execution_equivalent(&explicit.engine);

            convenience
                .parser
                .assert_continuation_equivalent(&explicit.parser);
        }
    }

    #[test]
    fn only_host_resize_changes_geometry() {
        use crate::parser::XtwinopsOp;

        let mut terminal = Terminal::new(4, 3).unwrap();
        let mut calls = Calls::default();
        let request = terminal.feed_with("\x1b[8;7;11t", &mut calls);

        assert_eq!(
            request.operations,
            [Operation::Function(Function::Xtwinops(XtwinopsOp::Resize(
                11, 7
            )))]
        );

        // Only host resize actions change terminal geometry.
        assert_eq!(request.size_before, (4, 3));
        assert_eq!(request.size_after, (4, 3));

        let resize = terminal.resize_with(11, 7, &mut calls);
        assert_eq!(resize.operations, [Operation::Resize { cols: 11, rows: 7 }]);
        assert_eq!(resize.size_before, (4, 3));
        assert_eq!(resize.size_after, (11, 7));
        assert_eq!(resize.hooks, request.hooks);
    }

    #[test]
    fn observed_functions_and_host_resizes_form_one_replayable_stream() {
        enum Event {
            Output(&'static str),
            Action(Operation),
        }

        let mut terminal = Terminal::new(12, 5).unwrap();
        let mut calls = Calls::default();
        let mut replay = Terminal::new(12, 5).unwrap();

        // Exercise recording a heap-backed operation across both hooks.
        for event in [
            Event::Output("hello\x1b[1;2;3;4;5;7;9m界\r\nworld"),
            Event::Action(Operation::Resize { cols: 8, rows: 3 }),
            Event::Action(Function::Cup(1, 1).into()),
            Event::Output("\x1b[?1049h\x1b[31malt"),
            Event::Action(Operation::Resize { cols: 6, rows: 4 }),
            Event::Output("\x1b[?1049l\x1b[0m!"),
        ] {
            let report = match event {
                Event::Output(text) => terminal.feed_with(text, &mut calls),
                Event::Action(operation) => terminal.execute_with(operation, &mut calls),
            };

            assert!(replay.screen().visible_lines().eq(&report.before));
            assert_eq!(replay.size(), report.size_before);
            replay.execute(report.operations);
            assert!(replay.screen().visible_lines().eq(&report.after));
            assert_eq!(replay.size(), report.size_after);
            replay.engine.assert_execution_equivalent(&terminal.engine);
        }
    }

    #[test]
    fn evicted_lines_are_borrowed_and_released_one_at_a_time() {
        struct Evictions {
            anchors: Vec<crate::Anchor>,
            addresses: Vec<*const crate::Cell>,
            lines: Vec<String>,
            active: crate::ScreenKind,
        }

        impl Observer for Evictions {
            type Output = Vec<String>;

            fn after(&mut self, context: &mut Observation<'_>, _: &Operation) {
                for anchor in &self.anchors {
                    assert!(context.resolve(anchor).is_some());
                }
            }

            fn evicted(&mut self, context: &mut Observation<'_>, line: &crate::Line) {
                assert_eq!(context.terminal().active_screen_kind(), self.active);
                let index = self.lines.len();
                assert_eq!(line.cells().as_ptr(), self.addresses[index]);
                assert!(context.resolve(&self.anchors[index]).is_none());

                for anchor in &self.anchors[..index] {
                    // Retaining an original evicted line would keep its weak marks alive.
                    assert_eq!(std::sync::Arc::weak_count(&anchor.0), 0);
                }

                for anchor in &self.anchors[index + 1..] {
                    assert!(context.resolve(anchor).is_some());
                }

                self.lines.push(line.text().trim_end().to_owned());
            }

            fn end(&mut self, context: &mut Observation<'_>) -> Self::Output {
                for anchor in &self.anchors[..self.lines.len()] {
                    assert!(context.resolve(anchor).is_none());
                    assert_eq!(std::sync::Arc::weak_count(&anchor.0), 0);
                }

                assert_eq!(
                    context.terminal().primary_screen().all_lines().count(),
                    context.terminal().size().1
                );

                std::mem::take(&mut self.lines)
            }
        }

        for (operation, expected, visible, alternate) in [
            (
                Operation::Resize { cols: 6, rows: 1 },
                vec!["first", "second", "third"],
                vec!["last"],
                false,
            ),
            (
                Operation::Resize { cols: 6, rows: 1 },
                vec!["first", "second", "third"],
                vec!["last"],
                true,
            ),
            (
                Function::Ed(EdScope::All).into(),
                vec!["first", "second", "third", "last"],
                vec!["", "", "", ""],
                false,
            ),
        ] {
            let mut terminal = Terminal::new(6, 4).unwrap();
            terminal.feed("first\r\nsecond\r\nthird\r\nlast");

            let anchors = (0..4)
                .map(|row| {
                    terminal
                        .anchor_at(crate::Position {
                            screen: crate::ScreenKind::Primary,
                            col: 0,
                            row,
                        })
                        .unwrap()
                })
                .collect();

            let mut evictions = Evictions {
                anchors,
                addresses: terminal
                    .screen()
                    .visible_lines()
                    .map(|line| line.cells().as_ptr())
                    .collect(),
                lines: Vec::new(),
                active: if alternate {
                    crate::ScreenKind::Alternate
                } else {
                    crate::ScreenKind::Primary
                },
            };

            let mut other = Evictions {
                anchors: evictions.anchors.clone(),
                addresses: evictions.addresses.clone(),
                lines: Vec::new(),
                active: evictions.active,
            };

            if alternate {
                terminal.feed("\x1b[?1047h");
            }

            let (rows, (lines, other_lines)) =
                terminal.execute_with(operation, (DirtyRows::new(), (&mut evictions, &mut other)));

            assert_eq!(lines, other_lines);
            assert_eq!(lines, expected);
            assert_eq!(rows, (0..visible.len()).collect::<Vec<_>>());
            assert_eq!(terminal.primary_screen().text(), visible);
        }
    }

    fn input() -> impl Strategy<Value = String> {
        prop::collection::vec(
            prop::sample::select(vec![
                "abc",
                "界",
                "\r\n",
                "\x1b[H",
                "\x1b[2J",
                "\x1b[31m",
                "\x1b[0m",
                "\x1b7",
                "\x1b8",
                "\x1b[?1049h",
                "\x1b[?1049l",
                "\x1b[2;4r",
                "\x1b[?25l",
                "\x1b[?25h",
                "\x1b[2L",
                "\x1b[2M",
                "\x1bc",
                PROMPT,
                SUBMIT,
                "\x1b]133;D;42\x07",
                "\x1b[3",
            ]),
            0..40,
        )
        .prop_map(|parts| parts.concat())
    }

    proptest! {
        #[test]
        fn prop_observed_execution_preserves_engine_state(
            first in input(),
            second in input(),
            cols in 1..20usize,
            rows in 1..8usize
        ) {
            let mut plain = Terminal::new(12, 5).unwrap();
            let mut observed = Terminal::new(12, 5).unwrap();
            let mut shell = ShellTracker::new();
            let mut transcript = Transcript::new();
            plain.feed(&first);
            let (mut events, mut lines) = observed.feed_with(&first, (&mut shell, &mut transcript));
            plain.resize(cols, rows);
            let (batch, text) = observed.resize_with(cols, rows, (&mut shell, &mut transcript));
            events.extend(batch);
            lines.extend(text);
            plain.feed(&second);
            let (batch, text) = observed.feed_with(&second, (&mut shell, &mut transcript));
            events.extend(batch);
            lines.extend(text);
            lines.extend(transcript.finish(&observed));
            observed.engine.verify();
            plain.engine.assert_execution_equivalent(&observed.engine);
            plain.parser.assert_continuation_equivalent(&observed.parser);

            let mut chunked = Terminal::new(12, 5).unwrap();
            let mut chunked_shell = ShellTracker::new();
            let mut chunked_transcript = Transcript::new();
            let mut chunked_events = Vec::new();
            let mut chunked_lines = Vec::new();

            for (index, input) in [&first, &second].into_iter().enumerate() {
                if index == 1 {
                    let (batch, text) =
                        chunked.resize_with(cols, rows, (&mut chunked_shell, &mut chunked_transcript));

                    chunked_events.extend(batch);
                    chunked_lines.extend(text);
                }

                for ch in input.chars() {
                    let (batch, text) =
                        chunked.feed_with(ch, (&mut chunked_shell, &mut chunked_transcript));

                    chunked_events.extend(batch);
                    chunked_lines.extend(text);
                }
            }

            chunked_lines.extend(chunked_transcript.finish(&chunked));
            prop_assert_eq!(events, chunked_events);
            prop_assert_eq!(lines, chunked_lines);

            prop_assert_eq!(
                shell.visible_prompts(&observed).collect::<Vec<_>>(),
                chunked_shell.visible_prompts(&chunked).collect::<Vec<_>>()
            );

            plain.engine.assert_execution_equivalent(&chunked.engine);
            plain.parser.assert_continuation_equivalent(&chunked.parser);

            // Standalone parsing and explicit execution preserve the same state.
            let mut parsed = Terminal::new(12, 5).unwrap();
            let mut parser = crate::Parser::new();
            parsed.execute(first.chars().filter_map(|ch| parser.feed(ch)));
            parsed.resize(cols, rows);
            parsed.execute(second.chars().filter_map(|ch| parser.feed(ch)));
            plain.engine.assert_execution_equivalent(&parsed.engine);
            plain.parser.assert_continuation_equivalent(&parser);
        }
    }

    #[test]
    fn eviction_before_buffer_switch_is_independent_of_feed_boundaries() {
        let mut batched = Terminal::new(12, 5).unwrap();
        let mut split = Terminal::new(12, 5).unwrap();
        batched.feed("\x1b[2M\x1b[?1049h");
        split.feed("\x1b[2M");
        split.feed("\x1b[?1049h");

        for terminal in [&mut batched, &mut split] {
            terminal.resize(2, 6);
            terminal.feed("\x1b[?1049l");
        }

        batched.engine.assert_execution_equivalent(&split.engine);
        batched.parser.assert_continuation_equivalent(&split.parser);
        assert_eq!(batched.cursor().position(), Some((0, 0)));
    }

    #[test]
    fn constructors_validate_both_dimensions_for_every_retention_policy() {
        for (cols, rows) in [(0, 0), (0, 3), (4, 0)] {
            let error = Some(crate::InvalidSize { cols, rows });
            assert_eq!(Terminal::new(cols, rows).err(), error);

            for limit in [Some(0), Some(100), None] {
                assert_eq!(Terminal::with_scrollback(cols, rows, limit).err(), error);
            }
        }

        for limit in [Some(0), Some(100), None] {
            assert_eq!(
                Terminal::with_scrollback(1, 1, limit).unwrap().size(),
                (1, 1)
            );
        }
    }

    #[test]
    fn retention_controls_eviction_anchors_and_transcript_delivery() {
        for (limit, evicted, retained) in [
            (Some(0), vec!["aa", "bb"], 2),
            (Some(1), vec!["aa"], 3),
            (Some(2), vec![], 4),
            (None, vec![], 4),
        ] {
            let mut terminal = Terminal::with_scrollback(4, 2, limit).unwrap();
            let anchor = terminal.anchor_at_cursor();
            let mut transcript = Transcript::new();
            let mut text = terminal.feed_with("aa\r\nbb\r\ncc\r\ndd", &mut transcript);
            assert_eq!(text, evicted);
            assert_eq!(terminal.primary_screen().all_lines().count(), retained);

            if retained == 4 {
                assert_eq!(terminal.resolve(&anchor).unwrap().row, -2);
            } else {
                assert!(terminal.resolve(&anchor).is_none());
            }

            text.extend(terminal.resize_with(4, 1, &mut transcript));
            text.extend(transcript.finish(&terminal));
            assert_eq!(text, ["aa", "bb", "cc", "dd"]);
        }
    }

    #[test]
    fn hidden_primary_resize_emits_evicted_transcript_immediately() {
        for (limit, evicted, retained) in [
            (Some(0), vec!["aa", "bb", "cc"], 1),
            (Some(1), vec!["aa", "bb"], 2),
            (None, vec![], 4),
        ] {
            let mut terminal = Terminal::with_scrollback(4, 4, limit).unwrap();
            let anchor = terminal.anchor_at_cursor();
            let mut transcript = Transcript::new();
            terminal.feed_with("aa\r\nbb\r\ncc\r\ndd\x1b[?1049h", &mut transcript);
            let mut text = terminal.resize_with(4, 1, &mut transcript);
            assert_eq!(text, evicted);
            assert_eq!(terminal.primary_screen().all_lines().count(), retained);

            if limit.is_none() {
                assert_eq!(terminal.resolve(&anchor).unwrap().row, -3);
            } else {
                assert!(terminal.resolve(&anchor).is_none());
            }

            assert!(
                terminal
                    .feed_with("\x1b[?1049l", &mut transcript)
                    .is_empty()
            );

            text.extend(transcript.finish(&terminal));
            assert_eq!(text, ["aa", "bb", "cc", "dd"]);
        }
    }

    #[test]
    fn ed2_preserved_wrapped_text_and_anchors_reflow_with_history() {
        for limit in [Some(0), Some(1), Some(10), None] {
            let mut terminal = Terminal::with_scrollback(4, 3, limit).unwrap();
            let mut transcript = Transcript::new();
            let mut lines = terminal.feed_with("abcd界ef", &mut transcript);

            let anchor = terminal
                .anchor_at(crate::Position {
                    screen: crate::ScreenKind::Primary,
                    col: 0,
                    row: 1,
                })
                .unwrap();

            lines.extend(terminal.feed_with("\x1b[H\x1b[2Jnew", &mut transcript));
            lines.extend(terminal.resize_with(8, 3, &mut transcript));

            match limit {
                Some(0) => assert!(terminal.resolve(&anchor).is_none()),

                _ => {
                    let position = terminal.resolve(&anchor).unwrap();
                    assert_eq!(position.row, -1);
                    assert_eq!(position.col, if limit == Some(1) { 0 } else { 4 });
                }
            }

            lines.extend(transcript.finish(&terminal));
            assert_eq!(lines, ["abcd界ef", "new"]);
            terminal.engine.verify();
        }
    }

    #[test]
    fn alternate_screen_ed2_erases_without_archiving() {
        let mut terminal = Terminal::with_scrollback(8, 2, None).unwrap();
        let mut transcript = Transcript::new();
        terminal.feed_with("history\r\none\r\ntwo\x1b[?1049halt", &mut transcript);
        let cursor = terminal.cursor();
        let anchor = terminal.anchor_at_cursor();
        assert!(terminal.feed_with("\x1b[2J", &mut transcript).is_empty());
        assert_eq!(terminal.cursor(), cursor);
        assert!(terminal.resolve(&anchor).is_none());

        assert!(
            terminal
                .screen()
                .visible_lines()
                .all(|line| line.is_blank())
        );

        assert_eq!(terminal.screen().all_lines().count(), 2);
        assert_eq!(terminal.primary_screen().text(), ["history", "one", "two"]);
        terminal.feed_with("\x1b[?1049l", &mut transcript);
        assert_eq!(transcript.finish(&terminal), ["history", "one", "two"]);
    }

    #[test]
    fn dump_restores_visible_state_and_parser_without_history() {
        let mut original = Terminal::with_scrollback(8, 2, Some(2)).unwrap();
        original.feed("one\r\ntwo\r\nthree\r\nfour\r\nfive\x1b[31");
        let mut restored = Terminal::with_scrollback(8, 2, Some(2)).unwrap();
        restored.feed(original.dump());
        // Dump initializes visible buffers; retained history is not serialized.
        assert_eq!(original.primary_screen().all_lines().count(), 4);
        assert_eq!(restored.primary_screen().all_lines().count(), 2);

        for tail in ["", "m!", "\r\nsix", "\x1b[?1049halt\x1b[?1049l"] {
            original.feed(tail);
            restored.feed(tail);
            original.engine.assert_dump_equivalent(&restored.engine);

            original
                .parser
                .assert_continuation_equivalent(&restored.parser);
        }
    }

    #[test]
    fn terminal_and_owned_types_are_send_sync() {
        fn send_sync<T: Send + Sync>() {}

        send_sync::<Terminal>();
        send_sync::<std::sync::RwLock<Terminal>>();
        send_sync::<crate::Line>();
        send_sync::<crate::Anchor>();
        send_sync::<crate::Parser>();
        send_sync::<ShellTracker>();
        send_sync::<Transcript>();
    }

    #[test]
    fn all_operation_forms_share_one_execution_lifecycle() {
        fn check<I, Kind>(actions: I, text: &str)
        where
            I: crate::IntoOperations<Kind>,
        {
            let mut expected = Terminal::new(16, 3).unwrap();
            let mut actual = Terminal::new(16, 3).unwrap();
            let reference = expected.feed_with(text, Calls::default());
            let report = actual.execute_with(actions, Calls::default());
            assert_eq!(report, reference);
        }

        check(Function::Print('x'), "x");
        check(Operation::Function(Function::Print('x')), "x");
        let functions = [Function::Print('x'), Function::Cr, Function::Lf];
        check(functions.clone(), "x\r\n");
        check(functions.to_vec(), "x\r\n");
        check(functions.iter().cloned(), "x\r\n");

        check(
            functions
                .into_iter()
                .map(Operation::from)
                .collect::<Vec<_>>(),
            "x\r\n",
        );

        check(Vec::<Function>::new(), "");
        check(std::iter::empty::<Operation>(), "");

        let mut parser = crate::Parser::new();

        check(
            "\x1b[31mhello".chars().filter_map(|ch| parser.feed(ch)),
            "\x1b[31mhello",
        );
    }

    #[test]
    fn explicit_execution_preserves_unfinished_text_parsing() {
        let mut terminal = Terminal::new(8, 2).unwrap();
        terminal.feed("\x1b[31");
        terminal.execute(Function::Print('x'));
        terminal.feed("mY");
        let cells = terminal.screen().line(0).unwrap().cells();
        assert_eq!(cells[0].char(), 'x');
        assert_eq!(cells[0].pen().foreground(), None);
        assert_eq!(cells[1].char(), 'Y');
        assert_eq!(cells[1].pen().foreground(), Some(crate::Color::Indexed(1)));
    }

    #[test]
    fn execute_finishes_eviction_before_advancing_iterator() {
        // A local-only observer also verifies that observers need no Send bound.
        struct Evictions(std::rc::Rc<std::cell::Cell<usize>>);

        impl Observer for Evictions {
            type Output = usize;

            fn evicted(&mut self, _: &mut Observation<'_>, _: &crate::Line) {
                self.0.set(self.0.get() + 1);
            }

            fn end(&mut self, _: &mut Observation<'_>) -> Self::Output {
                self.0.get()
            }
        }

        let count = std::rc::Rc::new(std::cell::Cell::new(0));

        let actions = (0..3).map(|index| {
            assert_eq!(count.get(), index);

            Function::Lf
        });

        let mut terminal = Terminal::new(4, 1).unwrap();
        assert_eq!(terminal.execute_with(actions, Evictions(count.clone())), 3);
    }

    #[test]
    fn execution_hooks_borrow_original_sgr_parameters() {
        use crate::parser::SgrOp;

        struct Parameters {
            address: *const SgrOp,
            seen: usize,
        }

        impl Observer for Parameters {
            type Output = usize;

            fn before(&mut self, _: &mut Observation<'_>, operation: &Operation) {
                let Operation::Function(Function::Sgr(ops)) = operation else {
                    panic!("expected SGR")
                };

                assert_eq!(ops.as_slice().as_ptr(), self.address);
                self.seen += 1;
            }

            fn after(&mut self, context: &mut Observation<'_>, operation: &Operation) {
                self.before(context, operation);
            }

            fn end(&mut self, _: &mut Observation<'_>) -> Self::Output {
                self.seen
            }
        }

        let ops = crate::parser::SgrOps::from(vec![SgrOp::SetBoldIntensity; 8]);

        let observer = Parameters {
            address: ops.as_slice().as_ptr(),
            seen: 0,
        };

        let mut terminal = Terminal::new(4, 2).unwrap();
        assert_eq!(terminal.execute_with([Function::Sgr(ops)], observer), 2);
    }

    proptest! {
        #[test]
        fn prop_dirty_observer_is_batching_independent(
            first in input(),
            second in input(),
            cols in 1..20usize,
            rows in 1..8usize,
            limit in prop::option::of(0..32usize),
        ) {
            let mut terminal = Terminal::with_scrollback(12, 5, limit).unwrap();
            let mut dirty = DirtyRows::new();
            let mut split = Terminal::with_scrollback(12, 5, limit).unwrap();
            let mut split_dirty = DirtyRows::new();

            prop_assert_eq!(
                terminal.feed_with("", &mut dirty),
                split.feed_with("", &mut split_dirty)
            );

            for text in [first, second] {
                // Compare a batched report with the union of per-character reports.
                let mut expected = std::collections::BTreeSet::new();

                for ch in text.chars() {
                    let before: Vec<_> = split
                        .screen()
                        .visible_lines()
                        .map(|line| line.cells().to_vec())
                        .collect();

                    let dirty_rows = split.feed_with(ch, &mut split_dirty);

                    for (row, (before, after)) in before
                        .iter()
                        .zip(split.screen().visible_lines())
                        .enumerate()
                    {
                        prop_assert!(
                            before == after.cells() || dirty_rows.contains(&row),
                            "missing repaint: row {}, character {:?}", row, ch
                        );
                    }

                    expected.extend(dirty_rows);
                }

                prop_assert_eq!(
                    terminal.feed_with(text, &mut dirty),
                    expected.into_iter().collect::<Vec<_>>()
                );

                prop_assert_eq!(
                    terminal.resize_with(cols, rows, &mut dirty),
                    split.resize_with(cols, rows, &mut split_dirty)
                );

                terminal.engine.assert_execution_equivalent(&split.engine);
                terminal.parser.assert_continuation_equivalent(&split.parser);
            }
        }
    }

    #[test]
    fn cursor_only_changes_emit_no_dirty_notifications() {
        let mut terminal = Terminal::new(8, 3).unwrap();
        let report = terminal.feed_with("\x1b[2;2H\x1b[?25l", Calls::default());
        assert!(report.notifications.is_empty());
    }

    #[test]
    fn resize_notifications_precede_dirty_ranges() {
        #[derive(Default)]
        struct Geometry {
            rows: usize,
            sizes: Vec<(usize, usize)>,
        }

        impl Observer for Geometry {
            type Output = Vec<(usize, usize)>;

            fn begin(&mut self, context: &mut Observation<'_>) {
                self.rows = context.terminal().size().1;
            }

            fn resized(&mut self, cols: usize, rows: usize) {
                self.rows = rows;
                self.sizes.push((cols, rows));
            }

            fn dirty(&mut self, rows: std::ops::Range<usize>) {
                assert!(rows.end <= self.rows);
            }

            fn end(&mut self, _: &mut Observation<'_>) -> Self::Output {
                std::mem::take(&mut self.sizes)
            }
        }

        let mut terminal = Terminal::new(8, 6).unwrap();

        let sizes = terminal.execute_with(
            [
                Function::Cup(6, 1).into(),
                Function::Print('x').into(),
                Operation::Resize { cols: 8, rows: 2 },
                Function::Print('y').into(),
                Operation::Resize { cols: 8, rows: 2 },
            ],
            Geometry::default(),
        );

        assert_eq!(sizes, [(8, 2)]);

        let sizes = terminal.execute_with(
            [
                Operation::Resize { cols: 8, rows: 5 },
                Operation::Resize { cols: 4, rows: 5 },
                Function::Cup(5, 1).into(),
                Function::Print('z').into(),
            ],
            Geometry::default(),
        );

        assert_eq!(sizes, [(8, 5), (4, 5)]);
    }
}
