//! Synchronous terminal observation and built-in observers.
//!
//! Implement [`Observer`] for custom analysis, or compose [`DirtyRows`],
//! [`Transcript`], and [`ShellTracker`] using tuples.

mod dirty_rows;
mod shell;
mod transcript;

pub use dirty_rows::DirtyRows;
pub use shell::{
    Command, CommandId, PromptId, PromptOutcome, ShellEvent, ShellTracker, VisiblePrompt,
};
pub use transcript::Transcript;

use crate::{Anchor, Operation, Position, Row, Terminal};
use core::ops::Range;

/// Read-only terminal inspection with permission to register tracking metadata.
pub struct Observation<'a> {
    terminal: &'a mut Terminal,
}

impl<'a> Observation<'a> {
    pub(crate) fn new(terminal: &'a mut Terminal) -> Self {
        Self { terminal }
    }

    pub fn terminal(&self) -> &Terminal {
        self.terminal
    }

    pub fn anchor_at_cursor(&mut self) -> Anchor {
        self.terminal.anchor_at_cursor()
    }

    pub fn anchor_at(&mut self, position: Position) -> Option<Anchor> {
        self.terminal.anchor_at(position)
    }

    pub fn resolve(&self, anchor: &Anchor) -> Option<Position> {
        self.terminal.resolve(anchor)
    }
}

/// Synchronous execution observation with observer-defined per-call results.
///
/// Each feed, resize, or explicit execution calls `begin` once and `end` once,
/// including empty feeds, incomplete sequences, and unchanged sizes. These are
/// execution-call boundaries, not analysis completion. For continuous analysis,
/// retain the observer and pass it through every mutation.
///
/// `before` and `after` surround each action, including caller-requested
/// resizes, and receive the same operation. During execution, `resized` and
/// `dirty` report effects without exposing partially updated terminal state.
/// Inspect the terminal in `after`, which runs before eviction.
/// A print action can contain many characters; its hooks surround the entire
/// run. Parser run boundaries depend on controls and input fragmentation.
///
/// Eviction runs after all actions and before `end`. Until then, excess history
/// remains inspectable and anchorable; later actions can reflow it into view.
/// Limits apply at call boundaries, not between fragments or to peak memory.
/// Use separate calls for earlier eviction callbacks.
///
/// Retain callback operations with `operation.clone().into_owned()` as
/// `Operation<'static>`. Preserve call boundaries for exact replay: eviction
/// can affect later operations, especially resizing.
pub trait Observer {
    type Output;

    fn begin(&mut self, _context: &mut Observation<'_>) {}
    fn before(&mut self, _context: &mut Observation<'_>, _operation: &Operation<'_>) {}

    /// Actual geometry changes from caller-requested resize actions. Runs
    /// before dirty notifications referring to the new dimensions. An
    /// unchanged-size request still has action hooks but no resize or dirty
    /// callbacks.
    fn resized(&mut self, _cols: usize, _rows: usize) {}

    /// Visible rows that may need repainting. Ranges can overlap or repeat, and
    /// contents may compare equal. Cursor-only changes are not included.
    /// Notification granularity may depend on input fragmentation.
    fn dirty(&mut self, _rows: Range<usize>) {}

    fn after(&mut self, _context: &mut Observation<'_>, _operation: &Operation<'_>) {}

    /// A primary row evicted by the scrollback limit, immediately after removal.
    /// Resizing can evict primary rows while the alternate screen is active.
    /// Runs after all actions in the call; the context reflects their final state
    /// with eviction in progress, not the action that moved this row into history.
    /// Rows awaiting eviction remain available for inspection. All observers
    /// borrow the original row; it is dropped after their callbacks return.
    /// Only rows still exceeding the limit at call end are evicted. Destructive
    /// operations such as RIS can discard content earlier without this callback;
    /// inspect that content in `before` if it must be preserved.
    fn evicted(&mut self, _context: &mut Observation<'_>, _row: &Row) {}

    /// Runs after all evictions and returns the per-call report, which may be `()`.
    fn end(&mut self, context: &mut Observation<'_>) -> Self::Output;
}

impl<O: Observer + ?Sized> crate::engine::Effects for O {
    fn dirty(&mut self, rows: Range<usize>) {
        Observer::dirty(self, rows);
    }

    fn resized(&mut self, cols: usize, rows: usize) {
        Observer::resized(self, cols, rows);
    }
}

impl Observer for () {
    type Output = ();

    fn end(&mut self, _: &mut Observation<'_>) {}
}

impl<O: Observer + ?Sized> Observer for &mut O {
    type Output = O::Output;

    fn begin(&mut self, context: &mut Observation<'_>) {
        (**self).begin(context);
    }

    fn before(&mut self, context: &mut Observation<'_>, operation: &Operation<'_>) {
        (**self).before(context, operation);
    }

    fn resized(&mut self, cols: usize, rows: usize) {
        (**self).resized(cols, rows);
    }

    fn dirty(&mut self, rows: Range<usize>) {
        (**self).dirty(rows);
    }

    fn after(&mut self, context: &mut Observation<'_>, operation: &Operation<'_>) {
        (**self).after(context, operation);
    }

    fn evicted(&mut self, context: &mut Observation<'_>, row: &Row) {
        (**self).evicted(context, row);
    }

    fn end(&mut self, context: &mut Observation<'_>) -> Self::Output {
        (**self).end(context)
    }
}

impl<A: Observer, B: Observer> Observer for (A, B) {
    type Output = (A::Output, B::Output);

    fn begin(&mut self, context: &mut Observation<'_>) {
        self.0.begin(context);
        self.1.begin(context);
    }

    fn before(&mut self, context: &mut Observation<'_>, operation: &Operation<'_>) {
        self.0.before(context, operation);
        self.1.before(context, operation);
    }

    fn resized(&mut self, cols: usize, rows: usize) {
        self.0.resized(cols, rows);
        self.1.resized(cols, rows);
    }

    fn dirty(&mut self, rows: Range<usize>) {
        self.0.dirty(rows.clone());
        self.1.dirty(rows);
    }

    fn after(&mut self, context: &mut Observation<'_>, operation: &Operation<'_>) {
        self.0.after(context, operation);
        self.1.after(context, operation);
    }

    fn evicted(&mut self, context: &mut Observation<'_>, row: &Row) {
        self.0.evicted(context, row);
        self.1.evicted(context, row);
    }

    fn end(&mut self, context: &mut Observation<'_>) -> Self::Output {
        (self.0.end(context), self.1.end(context))
    }
}
