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
//! use avt::{DirtyRows, Terminal};
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
mod anchor;
mod buffer;
mod cell;
mod charset;
mod color;
mod dirty_rows;
pub mod input;
mod line;
mod observer;
pub mod operation;
pub mod parser;
mod pen;
mod shell;
mod tabs;
mod terminal;
mod transcript;

#[cfg(test)]
mod tests;

pub use anchor::{Anchor, Position};
pub use cell::Cell;
pub use charset::Charset;
pub use color::{Color, Rgb};
pub use dirty_rows::DirtyRows;
pub use input::{AsInput, Input, IntoInputs};
pub use line::Line;
pub use observer::{Observation, Observer};
pub use operation::{IntoOperations, Operation};
pub use parser::{Parser, ShellIntegration};
pub use pen::Pen;
pub use shell::{
    Command, CommandId, PromptId, PromptOutcome, ShellEvent, ShellTracker, VisiblePrompt,
};
pub use terminal::{Cursor, Screen, ScreenKind};
pub use transcript::Transcript;

/// Rejected terminal dimensions. Both columns and rows must be positive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidSize {
    pub cols: usize,
    pub rows: usize,
}

impl std::fmt::Display for InvalidSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid terminal size {}x{}: columns and rows must be positive",
            self.cols, self.rows
        )
    }
}

impl std::error::Error for InvalidSize {}

/// A virtual terminal with a parser and primary and alternate screen buffers.
pub struct Terminal {
    parser: Parser,
    engine: terminal::Engine,
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
            engine: terminal::Engine::new((cols, rows), scrollback_limit),
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
    /// least one.
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

    /// Resolves even in an inactive screen. Erasure, removal, eviction, and hard
    /// reset invalidate positions. Ordinary writes leave them in place.
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
    /// visible buffers, terminal modes, and unfinished parsing. Retained scrollback
    /// and observer/anchor state are omitted; this is not a full-state snapshot.
    pub fn dump(&self) -> String {
        let mut dump = parser::dump(&self.engine.dump());
        dump.push_str(&self.parser.dump());

        dump
    }

    pub fn cursor_key_app_mode(&self) -> bool {
        self.engine.cursor_keys_app_mode()
    }
}
