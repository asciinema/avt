use std::ops::Range;

use crate::{Observation, Observer};

/// Accumulates repaint rows for one terminal, returning sorted, unique indices.
/// The first observed call requests a full repaint. Pass this observer through
/// every subsequent mutation; cursor movement and visibility are tracked separately.
#[derive(Default)]
pub struct DirtyRows {
    rows: Vec<bool>,
}

impl DirtyRows {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rows dirtied so far in the current call, without consuming them.
    /// Useful when an enclosing observer inspects intermediate contents in `after`.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(row, &dirty)| dirty.then_some(row))
    }
}

impl Observer for DirtyRows {
    type Output = Vec<usize>;

    fn begin(&mut self, context: &mut Observation<'_>) {
        if self.rows.is_empty() {
            self.rows.resize(context.terminal().size().1, true);
        }
    }

    fn resized(&mut self, _: usize, rows: usize) {
        self.rows.resize(rows, false);
    }

    fn dirty(&mut self, rows: Range<usize>) {
        self.rows[rows].fill(true);
    }

    fn end(&mut self, _: &mut Observation<'_>) -> Self::Output {
        let rows = self.iter().collect();
        self.rows.fill(false);

        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Operation, Terminal, parser::Function};

    #[test]
    fn reports_are_per_call_and_owned() {
        let mut terminal = Terminal::new(8, 3).unwrap();
        terminal.feed("existing");
        let mut dirty = DirtyRows::new();

        // Attaching the observer requests a full repaint, even with no new input.
        let initial = terminal.feed_with("", &mut dirty);
        assert_eq!(initial, [0, 1, 2]);
        assert!(dirty.iter().next().is_none());

        // Later reports contain only the current call's changes.
        let changed = terminal.feed_with("\x1b[2;1HX", &mut dirty);
        assert_eq!(changed, [1]);
        assert!(dirty.iter().next().is_none());

        assert!(terminal.feed_with("", &mut dirty).is_empty());
        assert_eq!(initial, [0, 1, 2]);
        assert_eq!(changed, [1]);
    }

    #[test]
    fn reports_rows_in_order_without_duplicates() {
        let mut terminal = Terminal::new(8, 4).unwrap();
        let mut dirty = DirtyRows::new();
        terminal.feed_with("", &mut dirty);

        // Write to row 2, then row 0, then row 2 again.
        let rows = terminal.feed_with("\x1b[3;1HX\x1b[1;1HY\x1b[3;1HZ", &mut dirty);
        assert_eq!(rows, [0, 2]);
    }

    #[test]
    fn reports_follow_resized_geometry() {
        let mut terminal = Terminal::new(8, 6).unwrap();
        let mut dirty = DirtyRows::new();
        terminal.feed_with("", &mut dirty);

        // The row dirtied before shrinking must not survive in the report.
        let rows = terminal.execute_with(
            [
                Function::Cup(6, 1).into(),
                Function::Print('x').into(),
                Operation::Resize { cols: 8, rows: 2 },
                Function::Print('y').into(),
            ],
            &mut dirty,
        );

        assert_eq!(rows, [0, 1]);
        assert!(terminal.feed_with("", &mut dirty).is_empty());

        let rows = terminal.execute_with(
            [
                Operation::Resize { cols: 8, rows: 5 },
                Function::Cup(5, 1).into(),
                Function::Print('z').into(),
            ],
            &mut dirty,
        );

        assert_eq!(rows, [0, 1, 2, 3, 4]);
        assert!(terminal.feed_with("", &mut dirty).is_empty());
    }
}
