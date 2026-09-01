use crate::{Line, Operation, Terminal};
use std::ops::Range;

/// Read-only terminal inspection during observation.
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
pub trait Observer {
    type Output;

    fn begin(&mut self, _context: &mut Observation<'_>) {}
    fn before(&mut self, _context: &mut Observation<'_>, _operation: &Operation) {}

    /// Actual geometry changes from caller-requested resize actions. Runs
    /// before dirty notifications referring to the new dimensions. An
    /// unchanged-size request still has action hooks but no `resized` callback.
    fn resized(&mut self, _cols: usize, _rows: usize) {}

    /// Visible rows that may need repainting. Ranges can overlap or repeat, and
    /// contents may compare equal. Cursor-only changes are not included.
    fn dirty(&mut self, _rows: Range<usize>) {}

    fn after(&mut self, _context: &mut Observation<'_>, _operation: &Operation) {}

    /// A primary line evicted by the scrollback limit, immediately after removal.
    /// Lines awaiting eviction remain available for inspection. All observers
    /// borrow the original line; it is dropped after their callbacks return.
    fn evicted(&mut self, _context: &mut Observation<'_>, _line: &Line) {}

    /// Runs after all evictions and returns the per-call report, which may be `()`.
    fn end(&mut self, context: &mut Observation<'_>) -> Self::Output;
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

    fn before(&mut self, context: &mut Observation<'_>, operation: &Operation) {
        (**self).before(context, operation);
    }

    fn resized(&mut self, cols: usize, rows: usize) {
        (**self).resized(cols, rows);
    }

    fn dirty(&mut self, rows: Range<usize>) {
        (**self).dirty(rows);
    }

    fn after(&mut self, context: &mut Observation<'_>, operation: &Operation) {
        (**self).after(context, operation);
    }

    fn evicted(&mut self, context: &mut Observation<'_>, line: &Line) {
        (**self).evicted(context, line);
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

    fn before(&mut self, context: &mut Observation<'_>, operation: &Operation) {
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

    fn after(&mut self, context: &mut Observation<'_>, operation: &Operation) {
        self.0.after(context, operation);
        self.1.after(context, operation);
    }

    fn evicted(&mut self, context: &mut Observation<'_>, line: &Line) {
        self.0.evicted(context, line);
        self.1.evicted(context, line);
    }

    fn end(&mut self, context: &mut Observation<'_>) -> Self::Output {
        (self.0.end(context), self.1.end(context))
    }
}
