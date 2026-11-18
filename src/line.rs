use crate::Row;
use alloc::collections::VecDeque;
use core::ops::Range;

/// Borrowed view of a logical line's physical rows.
///
/// Obtained through [`crate::Screen::line_at`]; the original beginning may have
/// been evicted.
#[derive(Debug, Clone, Copy)]
pub struct Line<'a> {
    rows: &'a VecDeque<Row>,
    start: usize,
    end: usize,
    offset: usize,
}

impl<'a> Line<'a> {
    pub(crate) fn new(rows: &'a VecDeque<Row>, range: Range<usize>, offset: usize) -> Self {
        Self {
            rows,
            start: range.start,
            end: range.end,
            offset,
        }
    }

    /// Physical rows, from top to bottom, borrowing the buffer rather than this view.
    pub fn rows(self) -> impl ExactSizeIterator<Item = &'a Row> {
        self.rows.range(self.start..self.end)
    }

    /// Physical-row range in [`crate::Position::row`] coordinates:
    /// zero is the first visible row; negative rows are scrollback.
    pub fn row_range(self) -> Range<isize> {
        (self.start as isize - self.offset as isize)..(self.end as isize - self.offset as isize)
    }
}

#[cfg(test)]
mod tests {
    use crate::ansi::{DecMode, DecModes, Function::*, SgrOp::SetForegroundColor};
    use crate::{Color, Row, Screen, Terminal};
    use alloc::vec::Vec;
    use core::ops::Range;

    fn assert_line(screen: Screen<'_>, range: Range<isize>, text: &[&str]) {
        for coordinate in range.clone() {
            let line = screen.line_at(coordinate).unwrap();
            assert_eq!(line.row_range(), range);
            assert_eq!(line.rows().len(), text.len());
            assert_eq!(line.rows().map(Row::text).collect::<Vec<_>>(), text);
        }
    }

    #[test]
    fn finds_chains_across_viewport_and_history() {
        let mut terminal = Terminal::with_scrollback(6, 3, None).unwrap();
        terminal.execute([Print("abcdefghijklmn".into()), Cr, Lf, Print("next".into())]);
        assert_line(terminal.screen(), -1..2, &["abcdef", "ghijkl", "mn    "]);
        assert_line(terminal.screen(), 2..3, &["next  "]);
        terminal.execute([Cr, Lf, Print("last".into()), Cr, Lf, Print("end".into())]);
        assert_line(terminal.screen(), -3..0, &["abcdef", "ghijkl", "mn    "]);
        assert_line(terminal.screen(), 0..1, &["next  "]);
        assert_line(terminal.screen(), 2..3, &["end   "]);
    }

    #[test]
    fn rejects_invalid_coordinates() {
        let mut terminal = Terminal::with_scrollback(6, 3, None).unwrap();

        for functions in [
            &[][..],
            &[Print("abcdefghijklmn".into()), Cr, Lf, Print("next".into())],
        ] {
            terminal.execute(functions.iter().cloned());

            for coordinate in [isize::MIN, -2, 3, isize::MAX] {
                assert!(terminal.screen().line_at(coordinate).is_none());
            }
        }
    }

    #[test]
    fn blanks_and_full_rows_follow_wrap_flags() {
        let mut terminal = Terminal::new(4, 4).unwrap();
        assert_line(terminal.screen(), 0..1, &["    "]);
        terminal.feed("abcd");
        assert_line(terminal.screen(), 0..1, &["abcd"]);

        terminal.execute([Cr, Lf, Print("efgh".into()), Cr, Lf, Print("     ".into())]);

        assert_line(terminal.screen(), 0..1, &["abcd"]);
        assert_line(terminal.screen(), 1..2, &["efgh"]);
        assert_line(terminal.screen(), 2..4, &["    ", "    "]);
    }

    #[test]
    fn borrows_original_styled_wide_rows_through_temporaries() {
        let mut terminal = Terminal::new(4, 3).unwrap();

        terminal.execute([
            Sgr([SetForegroundColor(Color::Indexed(1))].as_slice().into()),
            Print("abc界 ".into()),
        ]);

        let rows = terminal.screen().line_at(1).unwrap().rows();
        assert_eq!(rows.len(), 2);

        for (row, original) in rows.zip(terminal.screen().visible_rows()) {
            assert!(core::ptr::eq(row, original));
        }

        assert_line(terminal.screen(), 0..2, &["abc ", "界  "]);
        let line = terminal.screen().line_at(0).unwrap();
        let wide = line.rows().nth(1).unwrap();
        assert_eq!(wide.cells()[0].width(), 2);
        assert_eq!(wide.cells()[1].width(), 0);
        assert_eq!(wide.cells()[0].pen().foreground(), Some(Color::Indexed(1)));
    }

    #[test]
    fn follows_primary_reflow_and_viewport_changes() {
        let mut terminal = Terminal::with_scrollback(6, 3, None).unwrap();
        terminal.execute([Print("abcdefghijklmn".into()), Cr, Lf, Print("next".into())]);
        terminal.resize(4, 3);
        assert_line(terminal.screen(), -2..2, &["abcd", "efgh", "ijkl", "mn  "]);
        assert_line(terminal.screen(), 2..3, &["next"]);
        terminal.resize(8, 3);
        assert_line(terminal.screen(), 0..2, &["abcdefgh", "ijklmn  "]);
        assert_line(terminal.screen(), 2..3, &["next    "]);
    }

    #[test]
    fn returns_retained_portion_after_eviction() {
        for (limit, range, text) in [
            (0, 0..1, &["mn    "][..]),
            (1, -1..1, &["ghijkl", "mn    "][..]),
        ] {
            let mut terminal = Terminal::with_scrollback(6, 2, Some(limit)).unwrap();
            terminal.execute([Print("abcdefghijklmn".into()), Cr, Lf, Print("next".into())]);
            assert_line(terminal.screen(), range.clone(), text);
            assert!(terminal.screen().line_at(range.start - 1).is_none());
            assert_line(terminal.screen(), 1..2, &["next  "]);
        }
    }

    #[test]
    fn alternate_and_inactive_primary_use_their_own_rows() {
        let mut terminal = Terminal::with_scrollback(6, 3, None).unwrap();
        terminal.execute([Print("abcdefghijklmn".into()), Cr, Lf, Print("next".into())]);

        terminal.execute([
            Decset(DecModes::one(DecMode::SaveCursorAltScreenBuffer)),
            Cup(1, 1),
            Print("ABCDEFGH".into()),
        ]);

        assert_line(terminal.screen(), 0..2, &["ABCDEF", "GH    "]);

        assert_line(
            terminal.primary_screen(),
            -1..2,
            &["abcdef", "ghijkl", "mn    "],
        );

        assert!(terminal.screen().line_at(-1).is_none());
        terminal.resize(4, 3);
        assert_line(terminal.screen(), 0..2, &["ABCD", "GH  "]);

        assert_line(
            terminal.primary_screen(),
            -2..2,
            &["abcd", "efgh", "ijkl", "mn  "],
        );

        terminal.execute([Cup(3, 1), Print("ijklmnopq".into())]);
        assert_line(terminal.screen(), 0..3, &["ijkl", "mnop", "q   "]);
        assert!(terminal.screen().line_at(-1).is_none());
    }
}
