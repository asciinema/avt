#![no_std]

//! A virtual terminal with composable observers.
//!
//! This crate is `no_std` with `alloc`, requiring an allocator and pointer-width atomics.
//!
//! [`Terminal`] interprets application output, including text and control
//! sequences, and maintains primary and alternate screen buffers.
//! Plain mutations return `()`; `_with` variants return an
//! [`Observer`]'s output.
//!
//! A [`Row`] is one physical terminal row. [`Screen::visible_rows`] and
//! [`Screen::all_rows`] expose physical rows; [`Screen::text`] joins soft-wrapped
//! rows into logical text lines. [`Screen::line_at`] returns a borrowed [`Line`].
//!
//! Stateful observers should be reused across calls when collecting
//! dirty rows, transcripts, or shell integration events. Feed boundaries
//! enforce scrollback retention, but do not finish parsing or logical lines.
//!
//! ```
//! use avt::observer::DirtyRows;
//! use avt::Terminal;
//!
//! let mut terminal = Terminal::new(80, 24)?;
//! let mut dirty = DirtyRows::new();
//! let row_indices = terminal.feed_with(["hello", "\r\nworld"], &mut dirty);
//! assert_eq!(row_indices.len(), 24); // First observation requests a full repaint.
//! assert_eq!(terminal.screen().row(1).unwrap().text().trim_end(), "world");
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
pub mod ansi;
mod buffer;
mod cell;
mod charset;
mod color;
mod engine;
pub mod input;
mod line;
pub mod observer;
pub mod operation;
mod pen;
mod row;
mod tabs;

pub use anchor::Anchor;
pub use ansi::Parser;
pub use cell::Cell;
pub use charset::Charset;
pub use color::{Color, Rgb};
pub use engine::{Cursor, Position, Screen, ScreenKind};
pub use input::{AsInput, Input, IntoInputs};
pub use line::Line;
pub use observer::{Observation, Observer};
pub use operation::{IntoOperations, Operation};
pub use pen::Pen;
pub use row::Row;

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
    /// Limits are enforced at the end of each feed, execute, or resize call.
    /// History can temporarily exceed the limit while processing a call.
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
    /// additional begin/end hooks. They can split print runs, so operation hooks
    /// may differ with input fragmentation even when the terminal state agrees.
    /// Eviction follows the entire input; see [`Observer`] for callback ordering.
    pub fn feed_with<I, Kind, O>(&mut self, input: I, mut observer: O) -> O::Output
    where
        I: IntoInputs<Kind>,
        O: Observer,
    {
        observer.begin(&mut Observation::new(self));

        for item in input.into_inputs() {
            self.feed_fragment(item.as_input(), &mut observer);
        }

        self.evict(&mut observer);

        observer.end(&mut Observation::new(self))
    }

    fn feed_fragment<O: Observer>(&mut self, input: Input<'_>, observer: &mut O) {
        let mut encoded = [0; 4];

        let mut text = match input {
            Input::Text(text) => text,
            Input::Char(ch) => ch.encode_utf8(&mut encoded),
        };

        while let Some(function) = self.parser.next_function(&mut text) {
            self.apply(Operation::Function(function), observer);
        }
    }

    /// Executes a function, operation, or flat iterable of either. Actions are
    /// moved into execution, but their print text may borrow input.
    /// Use `.iter().cloned()` to execute operations
    /// from a borrowed collection. Pending text-parser state is preserved.
    pub fn execute<'a, I, Kind>(&mut self, operations: I)
    where
        I: IntoOperations<'a, Kind>,
    {
        self.execute_with(operations, ());
    }

    /// Executes operations with the supplied observer and returns its result,
    /// including for an empty iterable. Actions are processed incrementally.
    /// Eviction runs after the iterator is exhausted. See [`Observer`] for callback
    /// ordering and call-boundary requirements for exact replay.
    pub fn execute_with<'a, I, Kind, O>(&mut self, operations: I, mut observer: O) -> O::Output
    where
        I: IntoOperations<'a, Kind>,
        O: Observer,
    {
        observer.begin(&mut Observation::new(self));

        for operation in operations.into_operations() {
            self.apply(operation.into(), &mut observer);
        }

        self.evict(&mut observer);

        observer.end(&mut Observation::new(self))
    }

    /// Changes the virtual terminal's dimensions. Dimensions are clamped to at
    /// least one. The primary screen reflows even while the alternate screen is
    /// active. The alternate screen clips or pads rows without reflow,
    /// preserving soft-wrap flags. Intermediate resizes can discard content
    /// even if the original size is restored. Reflow to one column erases wide
    /// characters that cannot fit, preserving their style and invalidating
    /// anchors in the erased cells.
    ///
    /// Any size change resets scrolling margins to the full screen and preserves
    /// origin mode. Resizing to the current dimensions does nothing.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.resize_with(cols, rows, ());
    }

    pub fn resize_with<O: Observer>(&mut self, cols: usize, rows: usize, observer: O) -> O::Output {
        self.execute_with(Operation::Resize { cols, rows }, observer)
    }

    #[inline]
    fn apply<O: Observer>(&mut self, mut operation: Operation<'_>, observer: &mut O) {
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
    }

    fn evict<O: Observer>(&mut self, observer: &mut O) {
        self.engine.evict_alternate();

        while let Some(row) = self.engine.pop_evicted() {
            observer.evicted(&mut Observation::new(self), &row);
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

    /// Escape-sequence initialization for a fresh avt terminal of the same size,
    /// including visible buffers, cursor state, modes, and unfinished parsing.
    /// Uses DECRSPS cursor-information restoration, including pending wrap.
    /// Retained scrollback and observer/anchor state are omitted; this is not a
    /// full-state snapshot.
    pub fn dump(&self) -> String {
        let mut dump = ansi::encode(&self.engine.dump());
        dump.push_str(&self.parser.dump());

        dump
    }

    pub fn cursor_key_app_mode(&self) -> bool {
        self.engine.cursor_keys_app_mode()
    }

    /// Whether bracketed paste mode (DEC private mode 2004) is enabled.
    /// Disabled initially and by RIS; preserved by soft resets.
    pub fn bracketed_paste_mode(&self) -> bool {
        self.engine.bracketed_paste_mode()
    }
}

#[cfg(test)]
mod tests {
    use crate::ansi::{
        self, CursorPresentation, DecMode, DecModes, EdScope, Function, SgrOp, SgrOps,
    };
    use crate::observer::{DirtyRows, PromptId, ShellEvent, ShellTracker, Transcript};
    use crate::{Input, IntoInputs, Observation, Observer, Operation, ScreenKind, Terminal};
    use alloc::borrow::ToOwned as _;
    use alloc::format;
    use alloc::string::{String, ToString as _};
    use alloc::vec;
    use alloc::vec::Vec;
    use proptest::prelude::*;

    const PROMPT: &str = "\x1b]133;A\x07$ \x1b]133;B\x07";
    const SUBMIT: &str = "\x1b]133;C\x07";

    fn print(ch: char) -> Function<'static> {
        Function::Print(String::from(ch).into())
    }

    #[test]
    fn bracketed_paste_tracks_settings_and_resets() {
        use Function::*;

        let mut terminal = Terminal::new(8, 3).unwrap();
        assert!(!terminal.bracketed_paste_mode());

        for (function, enabled) in [
            (Decrst(DecModes::one(DecMode::BracketedPaste)), false),
            (Decset(DecModes::one(DecMode::BracketedPaste)), true),
            (Decset(DecModes::one(DecMode::BracketedPaste)), true),
            (Decstr, true),
            (Decrst(DecModes::one(DecMode::BracketedPaste)), false),
            (Decrst(DecModes::one(DecMode::BracketedPaste)), false),
            (Decset(DecModes::one(DecMode::BracketedPaste)), true),
            (Ris, false),
        ] {
            terminal.execute(function.clone());
            assert_eq!(terminal.bracketed_paste_mode(), enabled, "{function:?}");
        }
    }

    #[test]
    fn bracketed_paste_is_terminal_wide() {
        use Function::*;

        for enabled in [false, true] {
            for screen_mode in [DecMode::AltScreenBuffer, DecMode::SaveCursorAltScreenBuffer] {
                let mut terminal = Terminal::new(8, 3).unwrap();

                if enabled {
                    terminal.execute(Decset(DecModes::one(DecMode::BracketedPaste)));
                }

                terminal.resize(12, 4);
                assert_eq!(terminal.bracketed_paste_mode(), enabled);
                terminal.execute(Decset(DecModes::one(screen_mode)));
                assert_eq!(terminal.bracketed_paste_mode(), enabled);
                terminal.resize(6, 2);
                assert_eq!(terminal.bracketed_paste_mode(), enabled);

                // Changing the mode on the alternate screen affects the primary too.
                terminal.execute(if enabled {
                    Decrst(DecModes::one(DecMode::BracketedPaste))
                } else {
                    Decset(DecModes::one(DecMode::BracketedPaste))
                });

                terminal.execute(Decrst(DecModes::one(screen_mode)));
                assert_eq!(terminal.bracketed_paste_mode(), !enabled);
            }

            for (save, restore) in [
                (Decsc, Decrc),
                (Scosc, Scorc),
                (
                    Decset(DecModes::one(DecMode::SaveCursor)),
                    Decrst(DecModes::one(DecMode::SaveCursor)),
                ),
            ] {
                let mut terminal = Terminal::new(8, 3).unwrap();

                if enabled {
                    terminal.execute(Decset(DecModes::one(DecMode::BracketedPaste)));
                }

                terminal.execute(save);

                terminal.execute(if enabled {
                    Decrst(DecModes::one(DecMode::BracketedPaste))
                } else {
                    Decset(DecModes::one(DecMode::BracketedPaste))
                });

                terminal.execute(restore);
                assert_eq!(terminal.bracketed_paste_mode(), !enabled);
            }
        }
    }

    #[test]
    fn observers_see_complete_grouped_paste_mode_changes() {
        use Function::*;

        #[derive(Default)]
        struct Modes(Vec<(bool, ScreenKind, crate::Cursor)>);

        impl Observer for Modes {
            type Output = ();

            fn before(&mut self, context: &mut Observation<'_>, _: &Operation<'_>) {
                let terminal = context.terminal();

                self.0.push((
                    terminal.bracketed_paste_mode(),
                    terminal.active_screen_kind(),
                    terminal.cursor(),
                ));
            }

            fn after(&mut self, context: &mut Observation<'_>, operation: &Operation<'_>) {
                self.before(context, operation);
            }

            fn end(&mut self, _: &mut Observation<'_>) {}
        }

        let mut terminal = Terminal::new(8, 3).unwrap();
        let cursor = terminal.cursor();

        for (function, before, after) in [
            (Decset(DecModes::one(DecMode::BracketedPaste)), false, true),
            (Decset(DecModes::one(DecMode::BracketedPaste)), true, true),
            (Decrst(DecModes::one(DecMode::BracketedPaste)), true, false),
            (Decrst(DecModes::one(DecMode::BracketedPaste)), false, false),
            (Decset(DecModes::one(DecMode::BracketedPaste)), false, true),
            (Ris, true, false),
        ] {
            let mut modes = Modes::default();
            terminal.execute_with(function, &mut modes);

            assert_eq!(
                modes.0,
                [
                    (before, ScreenKind::Primary, cursor),
                    (after, ScreenKind::Primary, cursor),
                ]
            );
        }

        let mode_orders = [
            [DecMode::SaveCursorAltScreenBuffer, DecMode::BracketedPaste],
            [DecMode::BracketedPaste, DecMode::SaveCursorAltScreenBuffer],
        ];

        for set in mode_orders {
            for reset in mode_orders {
                let mut terminal = Terminal::new(8, 3).unwrap();
                terminal.feed("ab");
                let primary_cursor = terminal.cursor();
                let mut modes = Modes::default();
                terminal.execute_with(Decset(DecModes::collect(set)), &mut modes);

                assert_eq!(
                    modes.0,
                    [
                        (false, ScreenKind::Primary, primary_cursor),
                        (true, ScreenKind::Alternate, primary_cursor),
                    ]
                );

                terminal.execute(Cup(2, 4));
                let alternate_cursor = terminal.cursor();
                modes.0.clear();
                terminal.execute_with(Decrst(DecModes::collect(reset)), &mut modes);

                assert_eq!(
                    modes.0,
                    [
                        (true, ScreenKind::Alternate, alternate_cursor),
                        (false, ScreenKind::Primary, primary_cursor),
                    ]
                );
            }
        }
    }

    #[test]
    fn dump_preserves_bracketed_paste_mode() {
        use Function::*;

        for alternate in [false, true] {
            let mut original = Terminal::new(16, 3).unwrap();

            original.execute([
                Decset(DecModes::one(DecMode::BracketedPaste)),
                Print("primary".into()),
            ]);

            if alternate {
                original.execute([
                    Decset(DecModes::one(DecMode::SaveCursorAltScreenBuffer)),
                    Cup(1, 1),
                    Print("alt".into()),
                ]);
            }

            let mut restored = Terminal::new(16, 3).unwrap();
            restored.feed(original.dump());
            assert!(restored.bracketed_paste_mode());
            assert_dump_equivalent(&original, &restored);
        }
    }

    #[test]
    fn dump_preserves_title_continuation_at_every_boundary() {
        for end in ["\x1b\\", "\u{9c}"] {
            let title = format!("\x1bkuser@host$ echo '日'; pwd\x07\r\nignored{end}");

            for split in title.char_indices().map(|(i, _)| i).chain([title.len()]) {
                let mut original = Terminal::new(16, 3).unwrap();
                original.feed("before");
                let before = original.dump();
                let mut dirty = DirtyRows::new();
                original.feed_with("", &mut dirty);
                assert!(original.feed_with(&title[..split], &mut dirty).is_empty());
                let mut restored = Terminal::new(16, 3).unwrap();
                restored.feed(original.dump());
                assert_dump_equivalent(&original, &restored);
                assert!(original.feed_with(&title[split..], &mut dirty).is_empty());
                restored.feed(&title[split..]);
                assert_eq!(original.dump(), before);
                assert_dump_equivalent(&original, &restored);
                original.feed("OK");
                restored.feed("OK");
                assert_dump_equivalent(&original, &restored);
                assert_eq!(original.screen().text()[0], "beforeOK");
            }

            let mut terminal = Terminal::new(16, 3).unwrap();

            for ch in title.chars() {
                terminal.feed(ch);
            }

            terminal.feed("OK");
            assert_eq!(terminal.screen().text(), ["OK", "", ""]);
        }
    }

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
        assert_eq!(terminal.screen().visible_rows().len(), 2);
        assert_eq!(terminal.screen().all_rows().count(), 3);
        assert_eq!(terminal.screen().row(0).unwrap().text(), "bb");
        assert!(terminal.screen().row(2).is_none());
    }

    #[test]
    fn control_sequences_resume_printing_across_feed_boundaries() {
        // CAN cancels CSI; OSC accepts BEL/ST; DCS/APC payload is not printed.
        // Use an unhandled DCS final ('x') to exercise payload skipping.
        for (input, expected, col) in [
            ("\x7fA\x7f\x7fB\x7f", "AB  ", 2),
            ("A\x1b[31\x18mB", "AmB ", 3),
            ("A\x1b]2;hidden\x07B", "AB  ", 2),
            ("A\x1b]2;hidden\x1b\\B", "AB  ", 2),
            ("A\x1bP0xhidden\x1b\\B", "AB  ", 2),
            ("A\x1b_hidden\x1b\\B", "AB  ", 2),
        ] {
            let check = |terminal: &Terminal| {
                let row_texts: Vec<_> = terminal
                    .screen()
                    .visible_rows()
                    .map(crate::Row::text)
                    .collect();

                assert_eq!(row_texts, [expected, "    "], "{input:?}");
                assert_eq!(terminal.cursor().position(), Some((col, 0)), "{input:?}");

                for row in terminal.screen().visible_rows() {
                    assert!(!row.wraps_to_next(), "{input:?}");

                    for cell in row.cells() {
                        assert!(cell.pen().is_default(), "{input:?}");
                    }
                }
            };

            for split in input.char_indices().map(|(i, _)| i).chain([input.len()]) {
                let mut terminal = Terminal::new(4, 2).unwrap();
                terminal.feed(&input[..split]);
                terminal.feed(&input[split..]);
                check(&terminal);
            }

            let mut terminal = Terminal::new(4, 2).unwrap();

            for ch in input.chars() {
                terminal.feed(ch);
            }

            check(&terminal);
        }
    }

    #[test]
    fn del_preserves_pending_wrap() {
        for charset in ["\x1b(B", "\x1b(0"] {
            let mut terminal = Terminal::new(2, 2).unwrap();
            terminal.feed([charset, "AB"]);
            terminal.feed("\x7f");
            assert_eq!(terminal.cursor().position(), Some((2, 0)));
            assert_eq!(terminal.screen().row(0).unwrap().text(), "AB");
            assert_eq!(terminal.screen().row(1).unwrap().text(), "  ");
            assert!(!terminal.screen().row(0).unwrap().wraps_to_next());

            terminal.feed("C");
            assert_eq!(terminal.cursor().position(), Some((1, 1)));
            assert_eq!(terminal.screen().row(0).unwrap().text(), "AB");
            assert_eq!(terminal.screen().row(1).unwrap().text(), "C ");
            assert!(terminal.screen().row(0).unwrap().wraps_to_next());
        }
    }

    #[test]
    fn del_preserves_repeated_character() {
        let mut terminal = Terminal::new(8, 2).unwrap();
        terminal.feed("a\x7f\x1b[3b");
        assert_eq!(terminal.screen().row(0).unwrap().text(), "aaaa    ");
        assert_eq!(terminal.cursor().position(), Some((4, 0)));
    }

    #[test]
    fn del_does_not_insert_or_dirty_rows() {
        for setup in ["ABCD\r\x1b[4h", "ABCD", "ABCD\x1b[?7l"] {
            let mut terminal = Terminal::new(4, 2).unwrap();
            let mut dirty = DirtyRows::new();
            terminal.feed_with(setup, &mut dirty);
            let before = terminal.dump();
            let rows = terminal.feed_with("\x7f\x7f", &mut dirty);
            assert!(rows.is_empty());
            assert_eq!(terminal.dump(), before);
        }
    }

    #[test]
    fn screens_address_buffers_explicitly() {
        let mut terminal = Terminal::new(8, 2).unwrap();
        terminal.feed(["shell", "\x1b[?1047h", "\x1b[Hvim"]);
        assert_eq!(terminal.active_screen_kind(), ScreenKind::Alternate);
        assert_eq!(terminal.screen().row(0).unwrap().text().trim_end(), "vim");
        assert_eq!(terminal.primary_screen().text()[0], "shell");
        terminal.feed("\x1b[?1047l");
        assert_eq!(terminal.active_screen_kind(), ScreenKind::Primary);

        assert_eq!(terminal.screen().row(0).unwrap().text().trim_end(), "shell");
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

                for (cols, rows) in [(3, 4), (4, 1), (2, 3), (4, 3), (1, 2), (4, 3)] {
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
    fn resize_keeps_saved_cursors_valid_across_buffer_switches() {
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
    fn explicit_home_save_reflows_but_absent_save_stays_default() {
        for saved in [false, true] {
            let mut original = Terminal::with_scrollback(4, 2, None).unwrap();
            original.feed("aa\r\nbb\r\ncc\x1b[H");

            if saved {
                original.feed("\x1b7");
            }

            // Dumps omit history; check absence versus an explicit default save
            // before testing history-driven relocation on the original.
            let mut restored = Terminal::new(4, 2).unwrap();
            restored.feed(original.dump());
            assert_dump_equivalent(&original, &restored);
            original.resize(4, 3);
            original.feed("\x1b8");
            assert_eq!(original.cursor().position(), Some((0, usize::from(saved))));
        }
    }

    #[test]
    fn dump_preserves_saved_wrap_after_edge_edits() {
        for alternate in [false, true] {
            for edit in ["", "\x1b[H\x1b[2K", "\x1b[HAB界", "\x1b[4GZ"] {
                for inactive in [false, true] {
                    let mut original = Terminal::new(4, 3).unwrap();

                    if alternate {
                        original.feed("\x1b[?1047h");
                    }

                    original.feed(["\x1b[31mABCD\x1b7\x1b[0m", edit, "\x1b[2;2H"]);

                    if inactive {
                        original.feed(if alternate {
                            "\x1b[?1047l"
                        } else {
                            "\x1b[?1047h"
                        });
                    }

                    let mut restored = Terminal::new(4, 3).unwrap();
                    restored.feed(original.dump());
                    assert_dump_equivalent(&original, &restored);

                    for terminal in [&mut original, &mut restored] {
                        if inactive {
                            terminal.feed(if alternate {
                                "\x1b[?1047h"
                            } else {
                                "\x1b[?1047l"
                            });
                        }

                        terminal.feed("\x1b8X");
                        assert_eq!(terminal.cursor().position(), Some((1, 1)));
                        assert_eq!(terminal.screen().row(1).unwrap().cells()[0].char(), 'X');
                    }

                    assert_dump_equivalent(&original, &restored);
                }
            }
        }
    }

    #[test]
    fn dump_restores_saved_wrap_outside_origin_margins() {
        for saved_autowrap in [false, true] {
            for current_autowrap in [false, true] {
                let mut original = Terminal::new(4, 4).unwrap();
                original.feed("\x1b[?6hABCD");

                if !saved_autowrap {
                    original.feed("\x1b[?7l");
                }

                original.feed("\x1b7\x1b[2;4r\x1b8");
                original.feed(if current_autowrap {
                    "\x1b[?7h"
                } else {
                    "\x1b[?7l"
                });
                assert_eq!(original.cursor().position(), Some((4, 0)));
                let mut restored = Terminal::new(4, 4).unwrap();
                restored.feed(original.dump());
                assert_dump_equivalent(&original, &restored);

                for terminal in [&mut original, &mut restored] {
                    terminal.feed("\x1b[?7hX");
                    assert_eq!(terminal.cursor().position(), Some((1, 1)));
                    assert_eq!(terminal.screen().row(0).unwrap().text(), "ABCD");
                    assert_eq!(terminal.screen().row(1).unwrap().text(), "X   ");
                }

                assert_dump_equivalent(&original, &restored);
            }
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
    fn zwj_emoji_is_stored_in_one_two_column_cell() {
        let mut terminal = Terminal::new(8, 1).unwrap();
        terminal.feed("🐻‍❄️x");
        let row = terminal.screen().row(0).unwrap();
        assert_eq!(row.text().trim_end(), "🐻‍❄️x");
        assert_eq!(row.cells()[0].contents(), "🐻‍❄️");
        assert_eq!(row.cells()[0].width(), 2);
        assert_eq!(row.cells()[1].width(), 0);
        assert_eq!(terminal.cursor().position(), Some((3, 0)));
    }

    #[test]
    fn feed_and_char_feed_form_the_same_grapheme_clusters() {
        let cluster = "🐻‍❄️";
        let mut via_str = Terminal::new(8, 1).unwrap();
        let mut via_chars = Terminal::new(8, 1).unwrap();
        via_str.feed(cluster);
        for ch in cluster.chars() {
            via_chars.feed(ch);
        }
        assert_eq!(
            via_chars.screen().row(0).unwrap().cells()[0].contents(),
            cluster
        );
        via_str.engine.assert_dump_equivalent(&via_chars.engine);
    }

    #[test]
    fn combining_mark_spans_feed_calls() {
        let mut terminal = Terminal::new(8, 1).unwrap();
        terminal.feed("e");
        terminal.feed("\u{0301}");
        let row = terminal.screen().row(0).unwrap();
        assert_eq!(row.cells()[0].contents(), "e\u{0301}");
        assert_eq!(terminal.cursor().position(), Some((1, 0)));
    }

    #[test]
    fn dump_preserves_grapheme_clusters() {
        let mut original = Terminal::new(8, 1).unwrap();
        let mut restored = Terminal::new(8, 1).unwrap();
        original.feed("🐻‍❄️x");
        restored.feed(original.dump());
        assert_dump_equivalent(&original, &restored);
    }

    #[test]
    fn dump_does_not_attach_combining_mark_across_wrap() {
        let mut original = Terminal::new(10, 5).unwrap();
        let mut restored = Terminal::new(10, 5).unwrap();
        original.feed("⺀\x1b[5b\r\u{2de0}");
        restored.feed(original.dump());
        assert_dump_equivalent(&original, &restored);
    }

    #[test]
    fn dump_plain_ascii_keeps_letters_contiguous() {
        let mut terminal = Terminal::new(8, 1).unwrap();
        terminal.feed("hello");
        assert!(
            terminal.dump().contains("hello"),
            "dump inserted SGR between same-pen ASCII cells: {:?}",
            terminal.dump()
        );
    }

    #[test]
    fn controls_keep_a_combining_only_cell_separate() {
        let mut original = Terminal::new(8, 1).unwrap();
        let mut restored = Terminal::new(8, 1).unwrap();
        original.feed(" \x1b[50m\u{f71}");
        restored.feed(original.dump());
        assert_dump_equivalent(&original, &restored);
    }

    #[test]
    fn dump_preserves_saved_wrap_with_autowrap_disabled() {
        let mut original = Terminal::new(4, 2).unwrap();
        original.feed("ABCD\x1b[?7l\x1b7\x1b[?7h\x1b[H");
        let mut restored = Terminal::new(4, 2).unwrap();
        restored.feed(original.dump());
        assert_dump_equivalent(&original, &restored);

        for terminal in [&mut original, &mut restored] {
            terminal.feed("\x1b8");
            assert_eq!(terminal.cursor().position(), Some((4, 0)));
            terminal.feed("X");
            assert_eq!(terminal.screen().row(0).unwrap().text(), "ABCX");
        }

        assert_dump_equivalent(&original, &restored);
    }

    #[test]
    fn dump_preserves_origin_after_width_reflow() {
        let mut original = Terminal::new(1, 7).unwrap();
        original.feed("\x1b[2;0r界\x08\x1b[?6h");
        // Reflow moves the cursor above the former top margin.
        original.resize(2, 7);
        assert_eq!(original.cursor().position(), Some((1, 0)));
        let mut restored = Terminal::new(2, 7).unwrap();
        restored.feed(original.dump());
        assert_dump_equivalent(&original, &restored);
    }

    #[test]
    fn dump_preserves_origin_after_buffer_switch_above_margin() {
        for content in ["", "A", "0123456789"] {
            let mut original = Terminal::new(10, 5).unwrap();
            original.feed("\x1b[?6;1048h\x1b[?1047h\x1b[2;3r \x1b[?1049l\x1b[?1047h");
            assert_eq!(original.cursor().position(), Some((0, 0)));
            original.feed(content);
            let mut restored = Terminal::new(10, 5).unwrap();
            restored.feed(original.dump());
            assert_dump_equivalent(&original, &restored);

            for tail in ["X", "\x1b8q", "\x1b[?1047l\x1b8q", "\x1b[?1047h\x1b8q"] {
                original.feed(tail);
                restored.feed(tail);
                assert_dump_equivalent(&original, &restored);
            }
        }
    }

    #[test]
    fn dump_resumes_cursor_reports_at_every_boundary() {
        let setup = "AB界\x1b7\x1b[?1047h\x1b[Halt\x1b7\x1b[2;3r";

        for report in [
            "\x1bP1$t1;4;1;@;@;I;0;2;@;BBBB\x1b\\",
            "\u{90}1$t1;4;1;@;@;I;0;2;@;BBBB\u{9c}",
            "\x1bP1$t1;4;1;@;@;I;0;2;@;BBBB\x1b[31m",
        ] {
            for split in report.char_indices().map(|(i, _)| i).chain([report.len()]) {
                let mut original = Terminal::new(4, 4).unwrap();
                original.feed([setup, &report[..split]]);
                let mut restored = Terminal::new(4, 4).unwrap();
                restored.feed(original.dump());
                assert_dump_equivalent(&original, &restored);

                for tail in [&report[split..], "q", "\x1b8Z", "\x1b[?1047l\x1b8W"] {
                    original.feed(tail);
                    restored.feed(tail);
                    assert_dump_equivalent(&original, &restored);
                }
            }
        }
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

    pub(crate) fn gen_functions(max_len: usize) -> impl Strategy<Value = Vec<Function<'static>>> {
        prop::collection::vec(gen_function(), 0..=max_len)
    }

    fn gen_size() -> impl Strategy<Value = (usize, usize)> {
        (1..20usize, 1..8usize)
    }

    fn gen_param() -> impl Strategy<Value = u16> {
        prop_oneof![9 => 0..=20u16, 1 => any::<u16>()]
    }

    fn gen_function() -> impl Strategy<Value = Function<'static>> {
        use Function::*;
        use ansi::{
            AnsiMode, AnsiModes, CtcOp, DecMode, DecModes, ElScope, OscOp, ShellMarker, TbcScope,
            XtwinopsOp,
        };

        let counted = (
            prop::sample::select(vec![
                Cbt as fn(u16) -> Function<'static>,
                Cha,
                Cht,
                Cnl,
                Cpl,
                Cub,
                Cud,
                Cuf,
                Cuu,
                Dch,
                Dl,
                Ech,
                Ich,
                Il,
                Sd,
                Su,
                Vpa,
                Vpr,
            ]),
            gen_param(),
        )
            .prop_map(|(function, count)| function(count));

        let ansi_modes = prop::collection::vec(
            prop::sample::select(vec![AnsiMode::Insert, AnsiMode::NewLine]),
            0..=6,
        )
        .prop_map(AnsiModes::from);

        let dec_modes = prop::collection::vec(
            prop::sample::select(vec![
                DecMode::CursorKeys,
                DecMode::Origin,
                DecMode::AutoWrap,
                DecMode::TextCursorEnable,
                DecMode::AltScreenBuffer,
                DecMode::SaveCursor,
                DecMode::SaveCursorAltScreenBuffer,
                DecMode::BracketedPaste,
            ]),
            0..=6,
        )
        .prop_map(DecModes::from);

        // Favor printing so independently selected edits and modes encounter
        // populated rows. REP stays bounded because it performs every repeat.
        prop_oneof![
            20 => prop::collection::vec(gen_char(), 1..=16)
                .prop_map(|chars| Print(chars.into_iter().collect::<String>().into())),
            10 => counted,
            1 => (0..=20u16).prop_map(Rep),
            4 => prop::sample::select(vec![
                Bs, Cr, Ctc(CtcOp::Set), Ctc(CtcOp::ClearCurrentColumn),
                Ctc(CtcOp::ClearAll), Decaln, Decrc, Decsc, Decst8c, Decstr,
                Ed(EdScope::Below), Ed(EdScope::Above), Ed(EdScope::All),
                Ed(EdScope::ScrollAll),
                Ed(EdScope::SavedLines), El(ElScope::ToRight), El(ElScope::ToLeft),
                El(ElScope::All), Ht, Hts, Lf, Nel, Ri, Ris, Scorc, Scosc, Si, So,
                Tbc(TbcScope::CurrentColumn), Tbc(TbcScope::All),
            ]),
            2 => (gen_param(), gen_param()).prop_map(|(row_index, col)| Cup(row_index, col)),
            2 => gen_cursor_presentation().prop_map(Decrsps),
            1 => (gen_param(), gen_param()).prop_map(|(top, bottom)| Decstbm(top, bottom)),
            2 => (any::<bool>(), ansi_modes).prop_map(|(set, modes)| {
                if set { Sm(modes) } else { Rm(modes) }
            }),
            2 => (any::<bool>(), dec_modes).prop_map(|(set, modes)| {
                if set { Decset(modes) } else { Decrst(modes) }
            }),
            2 => prop::collection::vec(gen_sgr_op(), 0..=6)
                .prop_map(|ops| Sgr(SgrOps::from(ops))),
            1 => (
                any::<bool>(),
                prop::sample::select(vec![crate::Charset::Ascii, crate::Charset::Drawing]),
            ).prop_map(|(g1, charset)| {
                if g1 { G1d4(charset) } else { Gzd4(charset) }
            }),
            1 => prop_oneof![
                prop::sample::select(vec![
                    ShellMarker::PromptStart,
                    ShellMarker::CommandStart,
                    ShellMarker::CommandExecuted,
                ]),
                prop::option::of(any::<u8>()).prop_map(|exit_status| {
                    ShellMarker::CommandFinished { exit_status }
                }),
            ].prop_map(|kind| Function::Osc(OscOp::ShellIntegration(kind))),
            1 => (gen_param(), gen_param())
                .prop_map(|(cols, rows)| Xtwinops(XtwinopsOp::Resize(cols, rows))),
        ]
    }

    pub(crate) fn gen_cursor_presentation() -> impl Strategy<Value = CursorPresentation> {
        (gen_param(), gen_param(), any::<bool>(), any::<bool>()).prop_map(
            |(row_index, col, origin_mode, pending_wrap)| CursorPresentation {
                row: row_index,
                col,
                origin_mode,
                pending_wrap,
            },
        )
    }

    fn gen_char() -> impl Strategy<Value = char> {
        // Weight wide characters to exercise cell-pair edge cases.
        prop_oneof![
            4 => (0x20..=0x7eu8).prop_map(char::from),
            2 => prop::sample::select(vec!['界', '日', '🦀']),
            1 => (0xa0..=0xd7ffu32).prop_map(|v| char::from_u32(v).unwrap()),
            1 => (0xe000..=0x10ffffu32).prop_map(|v| char::from_u32(v).unwrap()),
        ]
    }

    fn gen_sgr_op() -> impl Strategy<Value = SgrOp> {
        use SgrOp::*;

        prop_oneof![
            prop::sample::select(vec![
                Reset,
                SetBoldIntensity,
                SetFaintIntensity,
                SetItalic,
                SetUnderline,
                SetBlink,
                SetInverse,
                SetStrikethrough,
                ResetIntensity,
                ResetItalic,
                ResetUnderline,
                ResetBlink,
                ResetInverse,
                ResetStrikethrough,
                ResetForegroundColor,
                ResetBackgroundColor,
            ]),
            gen_color().prop_map(SetForegroundColor),
            gen_color().prop_map(SetBackgroundColor),
        ]
    }

    fn gen_color() -> impl Strategy<Value = crate::Color> {
        prop_oneof![
            any::<u8>().prop_map(crate::Color::Indexed),
            (any::<u8>(), any::<u8>(), any::<u8>())
                .prop_map(|(r, g, b)| crate::Color::rgb(r, g, b)),
        ]
    }

    fn execute_and_verify(terminal: &mut Terminal, functions: &[Function<'_>]) {
        for function in functions {
            terminal.execute(function.clone());
            terminal.engine.verify();
        }
    }

    proptest! {
        #[test]
        fn prop_save_restore_identity(
            cols in 2..15usize,
            chars in prop::collection::vec(prop::sample::select(vec!['a', 'b', '界']), 1..60),
        ) {
            let mut original = Terminal::new(cols, 128).unwrap();
            let mut restored = Terminal::new(cols, 128).unwrap();
            original.feed(chars.clone());
            restored.feed(chars);
            restored.feed("\x1b7\x1b[H\x1b8");
            prop_assert_eq!(original.cursor(), restored.cursor());
            original.feed("X");
            restored.feed("X");
            prop_assert_eq!(original.screen().text(), restored.screen().text());
        }

        #[test]
        fn prop_resize_preserves_printing_continuity(
            old_cols in 2..15usize,
            cols in 1..15usize,
            input in "[a-z]{1,60}",
        ) {
            let mut terminal = Terminal::new(old_cols, 128).unwrap();
            terminal.feed(&input);
            terminal.resize(cols, 256);
            terminal.feed("X");
            prop_assert_eq!(terminal.screen().text().join(""), format!("{input}X"));
        }

        #[test]
        fn prop_saved_and_live_cursors_reflow_equally(
            old_cols in 2..15usize,
            cols in 1..15usize,
            chars in prop::collection::vec(prop::sample::select(vec!['a', 'b', '界']), 1..60),
        ) {
            let mut live = Terminal::new(old_cols, 128).unwrap();
            let mut saved = Terminal::new(old_cols, 128).unwrap();
            live.feed(chars.clone());
            saved.feed(chars);
            saved.feed("\x1b7\x1b[H");
            live.resize(cols, 256);
            saved.resize(cols, 256);
            saved.feed("\x1b8");
            prop_assert_eq!(live.cursor(), saved.cursor());
            live.feed("X");
            saved.feed("X");
            prop_assert_eq!(live.screen().text(), saved.screen().text());
        }

        #[test]
        fn prop_lossy_utf8_input_does_not_panic(
            (cols, rows) in gen_size(),
            bytes in prop::collection::vec(any::<u8>(), 0..4096),
        ) {
            let mut terminal = Terminal::new(cols, rows).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            terminal.feed(text.as_ref());
            terminal.engine.verify();
        }

        #[test]
        fn prop_sanity_checks_across_retention_policies(
            (cols, rows) in gen_size(),
            input in gen_functions(100),
        ) {
            for limit in [Some(0), Some(3), None] {
                let mut terminal = Terminal::with_scrollback(cols, rows, limit).unwrap();

                for function in &input {
                    terminal.execute(function.clone());
                    terminal.engine.verify();
                    let (_, rows) = terminal.size();
                    let retained = terminal.primary_screen().all_rows().count();

                    match limit {
                        Some(limit) => assert!((rows..=rows + limit).contains(&retained)),

                        None => assert!(retained >= rows),
                    }

                    if terminal.active_screen_kind() == ScreenKind::Alternate {
                        assert_eq!(terminal.screen().all_rows().count(), rows);
                    }
                }
            }
        }

        #[test]
        fn prop_resizing(
            (cols, rows) in gen_size(),
            (new_cols, new_rows) in gen_size(),
            input1 in gen_functions(100),
            input2 in gen_functions(100)
        ) {
            let mut terminal = Terminal::with_scrollback(cols, rows, None).unwrap();
            execute_and_verify(&mut terminal, &input1);
            terminal.resize(new_cols, new_rows);
            terminal.engine.verify();
            execute_and_verify(&mut terminal, &input2);
            assert!(terminal.primary_screen().all_rows().count() >= terminal.size().1);
        }

        #[test]
        fn prop_dump(
            (cols, rows) in gen_size(),
            input in gen_functions(100),
        ) {
            let mut original = Terminal::new(cols, rows).unwrap();
            let mut restored = Terminal::new(cols, rows).unwrap();
            execute_and_verify(&mut original, &input);
            restored.feed(original.dump());
            restored.engine.verify();
            assert_dump_equivalent(&original, &restored);
        }

        #[test]
        fn prop_dump_with_resizes(
            (cols, rows) in gen_size(),
            steps in prop::collection::vec((gen_functions(100), gen_size()), 1..8),
        ) {
            let mut original = Terminal::new(cols, rows).unwrap();
            let mut restored = Terminal::new(cols, rows).unwrap();

            for (input, (cols, rows)) in steps {
                execute_and_verify(&mut original, &input);
                execute_and_verify(&mut restored, &input);
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

        #[test]
        fn prop_soft_reset_preserves_content_and_restores_modes(
            (cols, rows) in gen_size(),
            input in gen_functions(100),
            limit in prop::option::of(0..32usize),
        ) {
            let mut terminal = Terminal::with_scrollback(cols, rows, limit).unwrap();
            execute_and_verify(&mut terminal, &input);
            let active = terminal.active_screen_kind();
            let cursor = terminal.cursor();
            let visible: Vec<_> = terminal.screen().all_rows().cloned().collect();
            let primary: Vec<_> = terminal.primary_screen().all_rows().cloned().collect();
            let bracketed_paste = terminal.bracketed_paste_mode();
            terminal.execute(Function::Decstr);
            terminal.engine.verify();
            prop_assert_eq!(terminal.size(), (cols, rows));
            prop_assert_eq!(terminal.active_screen_kind(), active);
            prop_assert_eq!(terminal.cursor().position(), Some((cursor.col, cursor.row)));
            prop_assert!(!terminal.cursor_key_app_mode());
            prop_assert_eq!(terminal.bracketed_paste_mode(), bracketed_paste);
            prop_assert!(terminal.screen().all_rows().eq(visible.iter()));
            prop_assert!(terminal.primary_screen().all_rows().eq(primary.iter()));

            // Absolute positioning, default rendition/charset, and pending
            // wrap check the reset modes through their observable behavior.
            terminal.execute(Function::Cup(1, cols as u16));
            prop_assert_eq!(terminal.cursor().position(), Some((cols - 1, 0)));
            terminal.execute(print('q'));
            prop_assert_eq!(terminal.cursor().position(), Some((cols, 0)));
            let row = terminal.screen().row(0).unwrap();
            prop_assert_eq!(row.cells()[cols - 1].char(), 'q');
            prop_assert!(row.cells()[cols - 1].pen().is_default());
            terminal.engine.verify();
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
        before: Vec<crate::Row>,
        after: Vec<crate::Row>,
        size_before: (usize, usize),
        size_after: (usize, usize),
        operations: Vec<Operation<'static>>,
        hooks: Vec<&'static str>,
        notifications: Vec<Notification>,
    }

    impl CallReport {
        fn dirty_rows(&self) -> std::collections::BTreeSet<usize> {
            self.notifications
                .iter()
                .flat_map(|notification| match notification {
                    Notification::Dirty(rows) => rows.clone(),
                    _ => 0..0,
                })
                .collect()
        }
    }

    #[derive(Default)]
    struct Calls {
        notifications: Vec<Notification>,
        number: usize,
        before: Vec<crate::Row>,
        size_before: (usize, usize),
        operations: Vec<Operation<'static>>,
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
                .visible_rows()
                .cloned()
                .collect();

            self.size_before = context.terminal().size();
            self.hooks.push("begin");
        }

        fn before(&mut self, _: &mut Observation<'_>, operation: &Operation<'_>) {
            self.check_order();
            assert!(self.active);
            assert!(!self.in_action);
            self.in_action = true;
            self.operations.push(operation.clone().into_owned());
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

        fn after(&mut self, context: &mut Observation<'_>, operation: &Operation<'_>) {
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

        fn evicted(&mut self, _: &mut Observation<'_>, row: &crate::Row) {
            self.check_order();
            assert!(self.active);
            assert!(!self.in_action);
            self.notifications.push(Notification::Evicted(row.text()));
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
                    .visible_rows()
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
            let mut expected_report = expected.feed_with(text, Calls::default());
            let mut report = actual.feed_with(input, Calls::default());
            assert_eq!(report.before, expected_report.before);
            assert_eq!(report.after, expected_report.after);
            assert_eq!(report.size_before, expected_report.size_before);
            assert_eq!(report.size_after, expected_report.size_after);

            // Fragmentation changes print-run boundaries, not repaint coverage.
            assert_eq!(report.dirty_rows(), expected_report.dirty_rows());

            for report in [&mut report, &mut expected_report] {
                report
                    .notifications
                    .retain(|notification| !matches!(notification, Notification::Dirty(_)));
            }

            assert_eq!(report.notifications, expected_report.notifications);
            assert_eq!(report.number, 1);

            let hooks: Vec<_> = core::iter::once("begin")
                .chain(report.operations.iter().flat_map(|_| ["before", "after"]))
                .chain(["end"])
                .collect();

            assert_eq!(report.hooks, hooks);
            actual.engine.assert_execution_equivalent(&expected.engine);

            let mut replay = Terminal::new(16, 3).unwrap();
            replay.execute(report.operations);
            replay.engine.assert_execution_equivalent(&actual.engine);

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
    fn observers_surround_print_runs_at_fragment_boundaries() {
        let mut terminal = Terminal::new(16, 2).unwrap();
        let report = terminal.feed_with(["abc", "dé界"], Calls::default());

        assert_eq!(
            report.operations,
            [
                Function::Print("abc".into()).into(),
                Function::Print("dé界".into()).into()
            ]
        );

        assert_eq!(
            report.hooks,
            ["begin", "before", "after", "before", "after", "end"]
        );
    }

    #[test]
    fn observers_receive_print_runs_without_del() {
        let mut terminal = Terminal::new(16, 2).unwrap();
        let report = terminal.feed_with(["a\x7fb", "\x7f"], Calls::default());

        assert_eq!(
            report.operations,
            [
                Function::Print("a".into()).into(),
                Function::Print("b".into()).into()
            ]
        );

        assert_eq!(terminal.screen().text(), ["ab", ""]);
        assert_eq!(terminal.cursor().position(), Some((2, 0)));

        let mut replay = Terminal::new(16, 2).unwrap();
        replay.execute(report.operations);
        replay.engine.assert_execution_equivalent(&terminal.engine);
    }

    #[test]
    fn long_print_runs_evict_at_call_end_and_preserve_text() {
        struct Retention {
            runs: usize,
            printed: usize,
            expected: usize,
            evicted: usize,
        }

        impl Observer for Retention {
            type Output = (usize, usize);

            fn before(&mut self, _: &mut Observation<'_>, _: &Operation<'_>) {
                assert_eq!(self.evicted, 0);
            }

            fn after(&mut self, context: &mut Observation<'_>, operation: &Operation<'_>) {
                let Operation::Function(Function::Print(text)) = operation else {
                    panic!("expected print run");
                };

                assert!(!text.is_empty());
                assert!(context.terminal().primary_screen().all_rows().count() > 2);
                self.printed += text.chars().count();
                self.runs += 1;
            }

            fn evicted(&mut self, _: &mut Observation<'_>, _: &crate::Row) {
                assert_eq!(self.printed, self.expected);
                self.evicted += 1;
            }

            fn end(&mut self, context: &mut Observation<'_>) -> Self::Output {
                assert_eq!(context.terminal().primary_screen().all_rows().count(), 2);

                (self.runs, self.evicted)
            }
        }

        let text = "ab界🦀".repeat(2048);
        let mut terminal = Terminal::new(8, 2).unwrap();
        let mut transcript = Transcript::new();

        let retention = Retention {
            runs: 0,
            printed: 0,
            expected: text.chars().count(),
            evicted: 0,
        };

        let ((runs, evicted), mut lines) = terminal.feed_with(&text, (retention, &mut transcript));

        assert_eq!(runs, 1);
        assert!(evicted > 0);
        lines.extend(transcript.finish(&terminal));
        assert_eq!(lines, [text.as_str()]);
        let mut split = Terminal::new(8, 2).unwrap();
        split.feed(text.chars());
        terminal.engine.assert_execution_equivalent(&split.engine);
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

            fn after(&mut self, _: &mut Observation<'_>, operation: &Operation<'_>) {
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
        use ansi::XtwinopsOp;

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
            Action(Operation<'static>),
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

            assert!(replay.screen().visible_rows().eq(&report.before));
            assert_eq!(replay.size(), report.size_before);
            replay.execute(report.operations);
            assert!(replay.screen().visible_rows().eq(&report.after));
            assert_eq!(replay.size(), report.size_after);
            replay.engine.assert_execution_equivalent(&terminal.engine);
        }
    }

    #[test]
    fn evicted_rows_are_borrowed_and_released_one_at_a_time() {
        struct Evictions {
            anchors: Vec<crate::Anchor>,
            addresses: Vec<*const crate::Cell>,
            row_texts: Vec<String>,
            active: crate::ScreenKind,
        }

        impl Observer for Evictions {
            type Output = Vec<String>;

            fn after(&mut self, context: &mut Observation<'_>, _: &Operation<'_>) {
                for anchor in &self.anchors {
                    assert!(context.resolve(anchor).is_some());
                }
            }

            fn evicted(&mut self, context: &mut Observation<'_>, row: &crate::Row) {
                assert_eq!(context.terminal().active_screen_kind(), self.active);
                let index = self.row_texts.len();
                assert_eq!(row.cells().as_ptr(), self.addresses[index]);
                assert!(context.resolve(&self.anchors[index]).is_none());

                for anchor in &self.anchors[..index] {
                    // Retaining an original evicted row would keep its weak marks alive.
                    assert_eq!(std::sync::Arc::weak_count(&anchor.0), 0);
                }

                for anchor in &self.anchors[index + 1..] {
                    assert!(context.resolve(anchor).is_some());
                }

                self.row_texts.push(row.text().trim_end().to_owned());
            }

            fn end(&mut self, context: &mut Observation<'_>) -> Self::Output {
                for anchor in &self.anchors[..self.row_texts.len()] {
                    assert!(context.resolve(anchor).is_none());
                    assert_eq!(std::sync::Arc::weak_count(&anchor.0), 0);
                }

                assert_eq!(
                    context.terminal().primary_screen().all_rows().count(),
                    context.terminal().size().1
                );

                std::mem::take(&mut self.row_texts)
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
                .map(|row_index| {
                    terminal
                        .anchor_at(crate::Position {
                            screen: crate::ScreenKind::Primary,
                            col: 0,
                            row: row_index,
                        })
                        .unwrap()
                })
                .collect();

            let mut evictions = Evictions {
                anchors,
                addresses: terminal
                    .screen()
                    .visible_rows()
                    .map(|row| row.cells().as_ptr())
                    .collect(),
                row_texts: Vec::new(),
                active: if alternate {
                    crate::ScreenKind::Alternate
                } else {
                    crate::ScreenKind::Primary
                },
            };

            let mut other = Evictions {
                anchors: evictions.anchors.clone(),
                addresses: evictions.addresses.clone(),
                row_texts: Vec::new(),
                active: evictions.active,
            };

            if alternate {
                terminal.feed("\x1b[?1047h");
            }

            let (rows, (row_texts, other_row_texts)) =
                terminal.execute_with(operation, (DirtyRows::new(), (&mut evictions, &mut other)));

            assert_eq!(row_texts, other_row_texts);
            assert_eq!(row_texts, expected);
            assert_eq!(rows, (0..visible.len()).collect::<Vec<_>>());
            assert_eq!(terminal.primary_screen().text(), visible);
        }
    }

    proptest! {
        #[test]
        fn prop_observed_execution_preserves_engine_state(
            (initial_cols, initial_rows) in gen_size(),
            first in gen_functions(100),
            second in gen_functions(100),
            (cols, rows) in gen_size(),
        ) {
            let mut plain = Terminal::new(initial_cols, initial_rows).unwrap();
            let mut observed = Terminal::new(initial_cols, initial_rows).unwrap();
            let mut shell = ShellTracker::new();
            let mut transcript = Transcript::new();
            execute_and_verify(&mut plain, &first);

            let (mut events, mut lines) =
                observed.execute_with(first.iter().cloned(), (&mut shell, &mut transcript));

            plain.resize(cols, rows);
            plain.engine.verify();
            let (batch, text) = observed.resize_with(cols, rows, (&mut shell, &mut transcript));
            events.extend(batch);
            lines.extend(text);
            execute_and_verify(&mut plain, &second);

            let (batch, text) =
                observed.execute_with(second.iter().cloned(), (&mut shell, &mut transcript));

            events.extend(batch);
            lines.extend(text);
            lines.extend(transcript.finish(&observed));
            observed.engine.verify();
            plain.engine.assert_execution_equivalent(&observed.engine);
            plain.parser.assert_continuation_equivalent(&observed.parser);

            let mut chunked = Terminal::new(initial_cols, initial_rows).unwrap();
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

                // Serialized functions also exercise the feed path, including
                // parser continuation across individual-character calls.
                for ch in ansi::encode(input).chars() {
                    let (batch, text) =
                        chunked.feed_with(ch, (&mut chunked_shell, &mut chunked_transcript));

                    chunked_events.extend(batch);
                    chunked_lines.extend(text);
                    chunked.engine.verify();
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

            let mut fed = Terminal::new(initial_cols, initial_rows).unwrap();
            fed.feed(ansi::encode(&first));
            fed.resize(cols, rows);
            fed.feed(ansi::encode(&second));
            plain.engine.assert_execution_equivalent(&fed.engine);
            plain.parser.assert_continuation_equivalent(&fed.parser);
        }
    }

    #[test]
    fn buffer_switch_preserves_retention_across_feed_boundaries() {
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
    fn batched_resizes_can_restore_excess_history_and_anchors() {
        for alternate in [false, true] {
            let mut batched = Terminal::new(4, 3).unwrap();
            let mut separate = Terminal::new(4, 3).unwrap();
            let anchor = batched.anchor_at_cursor();

            for terminal in [&mut batched, &mut separate] {
                terminal.feed("aa\r\nbb\r\ncc");

                if alternate {
                    terminal.feed("\x1b[?1049h");
                }
            }

            let actions = [
                Operation::Resize { cols: 4, rows: 1 },
                Operation::Resize { cols: 4, rows: 3 },
            ];

            let mut transcript = Transcript::new();
            let lines = batched.execute_with(actions.clone(), &mut transcript);
            assert!(lines.is_empty());
            assert_eq!(batched.primary_screen().text(), ["aa", "bb", "cc"]);
            assert_eq!(batched.resolve(&anchor).unwrap().row, 0);
            assert_eq!(transcript.finish(&batched), ["aa", "bb", "cc"]);

            for action in actions {
                separate.execute(action);
            }

            assert_eq!(separate.primary_screen().text(), ["cc", "", ""]);
        }
    }

    #[test]
    fn eviction_observes_final_reflow_and_active_screen() {
        struct Evictions(Vec<String>);

        impl Observer for Evictions {
            type Output = Vec<String>;

            fn before(&mut self, _: &mut Observation<'_>, _: &Operation<'_>) {
                assert!(self.0.is_empty());
            }

            fn evicted(&mut self, context: &mut Observation<'_>, row: &crate::Row) {
                assert_eq!(context.terminal().size(), (2, 1));

                assert_eq!(
                    context.terminal().active_screen_kind(),
                    ScreenKind::Alternate
                );

                self.0.push(row.text());
            }

            fn end(&mut self, _: &mut Observation<'_>) -> Self::Output {
                core::mem::take(&mut self.0)
            }
        }

        let mut terminal = Terminal::new(4, 1).unwrap();
        let anchor = terminal.anchor_at_cursor();
        let mut parser = crate::Parser::new();
        let switch = parser.feed("\x1b[?1049h").next().unwrap();

        let actions = [
            Function::Print("abcdefgh".into()).into(),
            Operation::Resize { cols: 2, rows: 1 },
            switch.into(),
        ];

        let mut transcript = Transcript::new();

        let (evicted, mut lines) =
            terminal.execute_with(actions, (Evictions(Vec::new()), &mut transcript));

        assert_eq!(evicted, ["ab", "cd", "ef"]);
        assert!(terminal.resolve(&anchor).is_none());
        assert_eq!(terminal.primary_screen().text(), ["gh"]);
        lines.extend(transcript.finish(&terminal));
        assert_eq!(lines, ["abcdefgh"]);
    }

    #[test]
    fn feed_evicts_after_all_fragments_and_cleans_inactive_alternate() {
        struct Pending {
            anchor: crate::Anchor,
            evicted: usize,
        }

        impl Observer for Pending {
            type Output = usize;

            fn before(&mut self, context: &mut Observation<'_>, _: &Operation<'_>) {
                assert!(context.resolve(&self.anchor).is_some());
                assert_eq!(self.evicted, 0);
            }

            fn evicted(&mut self, _: &mut Observation<'_>, _: &crate::Row) {
                self.evicted += 1;
            }

            fn end(&mut self, context: &mut Observation<'_>) -> Self::Output {
                assert!(context.resolve(&self.anchor).is_none());
                assert!(!self.anchor.is_registered());

                self.evicted
            }
        }

        for alternate in [false, true] {
            let mut terminal = Terminal::new(4, 1).unwrap();

            if alternate {
                terminal.feed("\x1b[?1049h");
            }

            let anchor = terminal.anchor_at_cursor();

            let switch = if alternate {
                "\x1b[?1049l"
            } else {
                "\x1b[?1049h"
            };

            let pending = Pending { anchor, evicted: 0 };
            let evicted = terminal.feed_with(["aa\r\n", "bb\r\n", switch], pending);
            assert_eq!(evicted, if alternate { 0 } else { 2 });
        }
    }

    #[test]
    fn alternate_history_is_removed_before_primary_eviction() {
        #[derive(Default)]
        struct Evictions {
            alternate: Option<crate::Anchor>,
            row_texts: Vec<String>,
        }

        impl Observer for Evictions {
            type Output = Vec<String>;

            fn before(&mut self, context: &mut Observation<'_>, operation: &Operation<'_>) {
                if matches!(operation, Operation::Function(Function::Print(text)) if text == "alt")
                {
                    self.alternate = Some(context.anchor_at_cursor());
                }
            }

            fn after(&mut self, context: &mut Observation<'_>, _: &Operation<'_>) {
                assert!(self.row_texts.is_empty());

                if let Some(anchor) = &self.alternate {
                    assert!(context.resolve(anchor).is_some());
                    assert!(anchor.is_registered());
                }
            }

            fn evicted(&mut self, context: &mut Observation<'_>, row: &crate::Row) {
                let anchor = self.alternate.as_ref().unwrap();
                assert!(context.resolve(anchor).is_none());
                assert!(!anchor.is_registered());
                self.row_texts.push(row.text());
            }

            fn end(&mut self, _: &mut Observation<'_>) -> Self::Output {
                core::mem::take(&mut self.row_texts)
            }
        }

        for switch in ["", "\x1b[?1047l"] {
            let mut terminal = Terminal::new(4, 1).unwrap();

            let row_texts = terminal.feed_with(
                ["one\r\ntwo\r\n", "\x1b[?1047h", "alt", "\r\n", switch],
                Evictions::default(),
            );

            assert_eq!(row_texts, ["one ", "two "]);
        }
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
            assert_eq!(terminal.primary_screen().all_rows().count(), retained);

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
            assert_eq!(terminal.primary_screen().all_rows().count(), retained);

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
    fn alternate_full_ed_erases_without_archiving() {
        use crate::ansi::{DecMode, DecModes, Function::*};

        for clear in [Ed(EdScope::All), Ed(EdScope::ScrollAll)] {
            let mut terminal = Terminal::with_scrollback(8, 2, None).unwrap();
            let mut transcript = Transcript::new();

            terminal.execute_with(
                [
                    Print("history".into()),
                    Cr,
                    Lf,
                    Print("one".into()),
                    Cr,
                    Lf,
                    Print("two".into()),
                    Decset(DecModes::one(DecMode::SaveCursorAltScreenBuffer)),
                    Print("alt".into()),
                ],
                &mut transcript,
            );

            let cursor = terminal.cursor();
            let anchor = terminal.anchor_at_cursor();
            assert!(terminal.execute_with(clear, &mut transcript).is_empty());
            assert_eq!(terminal.cursor(), cursor);
            assert!(terminal.resolve(&anchor).is_none());

            assert!(terminal.screen().visible_rows().all(|row| row.is_blank()));

            assert_eq!(terminal.screen().all_rows().count(), 2);
            assert_eq!(terminal.primary_screen().text(), ["history", "one", "two"]);

            terminal.execute_with(
                Decrst(DecModes::one(DecMode::SaveCursorAltScreenBuffer)),
                &mut transcript,
            );

            assert_eq!(transcript.finish(&terminal), ["history", "one", "two"]);
        }
    }

    #[test]
    fn dump_restores_visible_state_and_parser_without_history() {
        let mut original = Terminal::with_scrollback(8, 2, Some(2)).unwrap();
        original.feed("one\r\ntwo\r\nthree\r\nfour\r\nfive\x1b[31");
        let mut restored = Terminal::with_scrollback(8, 2, Some(2)).unwrap();
        restored.feed(original.dump());
        // Dump initializes visible buffers; retained history is not serialized.
        assert_eq!(original.primary_screen().all_rows().count(), 4);
        assert_eq!(restored.primary_screen().all_rows().count(), 2);

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
        send_sync::<crate::Row>();
        send_sync::<crate::Anchor>();
        send_sync::<crate::Parser>();
        send_sync::<ShellTracker>();
        send_sync::<Transcript>();
    }

    #[test]
    fn all_operation_forms_share_one_execution_lifecycle() {
        fn check<'a, I, Kind>(actions: I, text: &str)
        where
            I: crate::IntoOperations<'a, Kind>,
        {
            let mut expected = Terminal::new(16, 3).unwrap();
            let mut actual = Terminal::new(16, 3).unwrap();
            let reference = expected.feed_with(text, Calls::default());
            let report = actual.execute_with(actions, Calls::default());
            assert_eq!(report, reference);
        }

        check(print('x'), "x");
        check(Operation::Function(print('x')), "x");
        let functions = [print('x'), Function::Cr, Function::Lf];
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

        check(parser.feed("\x1b[31mhello"), "\x1b[31mhello");
    }

    #[test]
    fn explicit_execution_preserves_unfinished_text_parsing() {
        let mut terminal = Terminal::new(8, 2).unwrap();
        terminal.feed("\x1b[31");
        terminal.execute(print('x'));
        terminal.feed("mY");
        let cells = terminal.screen().row(0).unwrap().cells();
        assert_eq!(cells[0].char(), 'x');
        assert_eq!(cells[0].pen().foreground(), None);
        assert_eq!(cells[1].char(), 'Y');
        assert_eq!(cells[1].pen().foreground(), Some(crate::Color::Indexed(1)));
    }

    #[test]
    fn execute_evicts_after_exhausting_iterator() {
        // A local-only observer also verifies that observers need no Send bound.
        struct Evictions {
            count: std::rc::Rc<std::cell::Cell<usize>>,
            exhausted: std::rc::Rc<std::cell::Cell<bool>>,
        }

        impl Observer for Evictions {
            type Output = usize;

            fn evicted(&mut self, _: &mut Observation<'_>, _: &crate::Row) {
                assert!(self.exhausted.get());
                self.count.set(self.count.get() + 1);
            }

            fn end(&mut self, _: &mut Observation<'_>) -> Self::Output {
                self.count.get()
            }
        }

        let count = std::rc::Rc::new(std::cell::Cell::new(0));
        let exhausted = std::rc::Rc::new(std::cell::Cell::new(false));
        let mut remaining = 3;

        let actions = std::iter::from_fn(|| {
            assert_eq!(count.get(), 0);

            if remaining == 0 {
                exhausted.set(true);

                return None;
            }

            remaining -= 1;

            Some(Function::Lf)
        });

        let mut terminal = Terminal::new(4, 1).unwrap();

        let evictions = Evictions {
            count: count.clone(),
            exhausted: exhausted.clone(),
        };

        assert_eq!(terminal.execute_with(actions, evictions), 3);
    }

    #[test]
    fn execution_hooks_borrow_original_sgr_parameters() {
        struct Parameters {
            address: *const SgrOp,
            seen: usize,
        }

        impl Observer for Parameters {
            type Output = usize;

            fn before(&mut self, _: &mut Observation<'_>, operation: &Operation<'_>) {
                let Operation::Function(Function::Sgr(ops)) = operation else {
                    panic!("expected SGR")
                };

                assert_eq!(ops.as_slice().as_ptr(), self.address);
                self.seen += 1;
            }

            fn after(&mut self, context: &mut Observation<'_>, operation: &Operation<'_>) {
                self.before(context, operation);
            }

            fn end(&mut self, _: &mut Observation<'_>) -> Self::Output {
                self.seen
            }
        }

        let ops = SgrOps::from(vec![SgrOp::SetBoldIntensity; 8]);

        let observer = Parameters {
            address: ops.as_slice().as_ptr(),
            seen: 0,
        };

        let mut terminal = Terminal::new(4, 2).unwrap();
        assert_eq!(terminal.execute_with([Function::Sgr(ops)], observer), 2);
    }

    #[test]
    fn feed_hooks_borrow_print_text_and_record_owned_operations() {
        use alloc::borrow::Cow;

        #[derive(Default)]
        struct BorrowedPrints(Vec<Operation<'static>>);

        impl Observer for BorrowedPrints {
            type Output = ();

            fn before(&mut self, _: &mut Observation<'_>, operation: &Operation<'_>) {
                assert!(matches!(
                    operation,
                    Operation::Function(Function::Print(Cow::Borrowed(_)))
                ));

                self.0.push(operation.clone().into_owned());
            }

            fn end(&mut self, _: &mut Observation<'_>) {}
        }

        let mut terminal = Terminal::new(16, 2).unwrap();
        let mut observer = BorrowedPrints::default();
        terminal.feed_with(String::from("hé"), &mut observer);
        terminal.feed_with('界', &mut observer);
        terminal.feed_with([String::from("🦀"), String::from("!")], &mut observer);
        assert_eq!(terminal.screen().text()[0], "hé界🦀!");

        for operation in &observer.0 {
            assert!(matches!(
                operation,
                Operation::Function(Function::Print(Cow::Owned(_)))
            ));
        }

        let mut replay = Terminal::new(16, 2).unwrap();
        replay.execute(observer.0);
        terminal.engine.assert_execution_equivalent(&replay.engine);
    }

    #[test]
    fn insert_wrap_reports_both_rows_dirty() {
        let mut terminal = Terminal::new(2, 2).unwrap();
        let mut dirty = DirtyRows::new();
        terminal.feed_with("\x1b[4h界\x1b[C", &mut dirty);
        let rows = terminal.execute_with(print('界'), &mut dirty);
        assert_eq!(rows, [0, 1]);
        assert_eq!(terminal.screen().row(0).unwrap().text(), "界");
        assert_eq!(terminal.screen().row(1).unwrap().text(), "界");
        terminal.engine.verify();
    }

    #[test]
    fn print_and_repeat_report_one_range_during_the_action() {
        use Function::{Print, Rep};

        for (size, setup, function, range) in [
            ((4, 5), "", Print("".into()), None),
            ((4, 5), "", Print("abc".into()), Some(0..1)),
            ((4, 5), "\x1b[2;1H", Print("abcdefghij".into()), Some(1..4)),
            // Include the starting row even when the run wraps immediately.
            ((4, 5), "abcd", Print("界".into()), Some(0..2)),
            ((4, 5), "abc", Print("界".into()), Some(0..2)),
            (
                (4, 5),
                "\x1b[4hxx界\x1b[1;4H",
                Print("界ab".into()),
                Some(0..2),
            ),
            (
                (4, 5),
                "\x1b[5;1H",
                Print("abcdefghijklmnop".into()),
                Some(0..5),
            ),
            (
                (4, 5),
                "\x1b[2;4r\x1b[4;1H",
                Print("abcdefghijklmnop".into()),
                Some(1..4),
            ),
            (
                (4, 5),
                "\x1b[2;4r\x1b[1;1H",
                Print("abcdefghijklmnopqrst".into()),
                Some(0..4),
            ),
            (
                (4, 5),
                "\x1b[2;4r\x1b[5;1H",
                Print("abcdefgh".into()),
                Some(4..5),
            ),
            ((1, 3), "", Print("界界界".into()), Some(0..3)),
            ((4, 1), "", Print("abcdefgh".into()), Some(0..1)),
            ((4, 5), "\x1b[?7labcd", Print("界xy".into()), Some(0..1)),
            ((4, 5), "", Rep(10), None),
            ((4, 5), "a", Rep(0), Some(0..1)),
            ((4, 5), "a", Rep(10), Some(0..3)),
            ((4, 5), "\x1b[2;4r\x1b[4;1H界", Rep(10), Some(1..4)),
        ] {
            let mut terminal = Terminal::with_scrollback(size.0, size.1, None).unwrap();
            terminal.feed(setup);
            // Calls asserts that every dirty callback is between before/after.
            let report = terminal.execute_with(function.clone(), Calls::default());

            assert_eq!(
                report.notifications,
                range
                    .map(Notification::Dirty)
                    .into_iter()
                    .collect::<Vec<_>>(),
                "size {size:?}, setup {setup:?}, function {function:?}"
            );

            let dirty = report.dirty_rows();

            for (row_index, (before, after)) in report.before.iter().zip(&report.after).enumerate()
            {
                assert!(before.cells() == after.cells() || dirty.contains(&row_index));
            }

            terminal.engine.verify();
        }
    }

    proptest! {
        #[test]
        fn prop_dirty_observer_is_batching_independent(
            (initial_cols, initial_rows) in gen_size(),
            first in gen_functions(100),
            second in gen_functions(100),
            first_size in gen_size(),
            second_size in gen_size(),
            limit in prop::option::of(0..32usize),
        ) {
            let mut terminal = Terminal::with_scrollback(initial_cols, initial_rows, limit).unwrap();
            let mut dirty = DirtyRows::new();
            let mut split = Terminal::with_scrollback(initial_cols, initial_rows, limit).unwrap();
            let mut split_dirty = DirtyRows::new();

            prop_assert_eq!(
                terminal.feed_with("", &mut dirty),
                split.feed_with("", &mut split_dirty)
            );

            for (functions, (cols, rows)) in [(first, first_size), (second, second_size)] {
                // Split print runs into characters as well as splitting actions.
                let mut expected = std::collections::BTreeSet::new();

                for function in &functions {
                    let before: Vec<_> = split
                        .screen()
                        .visible_rows()
                        .map(|row| row.cells().to_vec())
                        .collect();

                    let dirty_rows = match function {
                        Function::Print(text) => {
                            split.execute_with(text.chars().map(print), &mut split_dirty)
                        }

                        _ => split.execute_with(function.clone(), &mut split_dirty),
                    };

                    split.engine.verify();

                    for (row_index, (before, after)) in before
                        .iter()
                        .zip(split.screen().visible_rows())
                        .enumerate()
                    {
                        prop_assert!(
                            before == after.cells() || dirty_rows.contains(&row_index),
                            "missing repaint: row {}, function {:?}", row_index, function
                        );
                    }

                    expected.extend(dirty_rows);
                }

                prop_assert_eq!(
                    terminal.execute_with(functions, &mut dirty),
                    expected.into_iter().collect::<Vec<_>>()
                );

                let before_size = split.size();

                let before: Vec<_> = split
                    .screen()
                    .visible_rows()
                    .map(|row| row.cells().to_vec())
                    .collect();

                let dirty_rows = split.resize_with(cols, rows, &mut split_dirty);

                prop_assert_eq!(
                    &terminal.resize_with(cols, rows, &mut dirty),
                    &dirty_rows
                );

                for (row_index, row) in split.screen().visible_rows().enumerate() {
                    prop_assert!(
                        before.get(row_index).is_some_and(|cells| cells == row.cells())
                            || dirty_rows.contains(&row_index),
                        "missing repaint: row {}, resize {:?} -> {:?}",
                        row_index, before_size, (cols, rows)
                    );
                }

                prop_assert!(dirty_rows.iter().all(|&row_index| row_index < rows));

                if before_size == (cols, rows) {
                    prop_assert!(dirty_rows.is_empty());
                }

                terminal.engine.verify();
                split.engine.verify();
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
                print('x').into(),
                Operation::Resize { cols: 8, rows: 2 },
                print('y').into(),
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
                print('z').into(),
            ],
            Geometry::default(),
        );

        assert_eq!(sizes, [(8, 5), (4, 5)]);
    }
}
