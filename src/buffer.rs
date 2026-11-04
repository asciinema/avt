use alloc::borrow::ToOwned as _;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::ops::{Index, IndexMut, Range};

use crate::anchor::{Anchor, Mark};
use crate::cell::Cell;
use crate::line::{Line, ReflowCursor, ReflowCursors};
use crate::pen::Pen;

/// Screen/history storage. Structural edits maintain row relationships while
/// individual lines maintain their cells and anchor columns.
#[derive(Debug)]
pub(crate) struct Buffer {
    lines: VecDeque<Line>,
    pub cols: usize,
    pub rows: usize,
    scrollback_limit: Option<usize>,
    // The first retained row continues the last evicted row's logical line.
    front_wrapped: bool,
}

pub(crate) enum EraseMode {
    NextChars(usize),
    FromCursorToEndOfView,
    FromStartOfViewToCursor,
    WholeView,
    FromCursorToEndOfLine,
    FromStartOfLineToCursor,
    WholeLine,
}

type VisualPosition = (usize, usize);

impl Buffer {
    pub fn new(cols: usize, rows: usize, scrollback_limit: Option<usize>, pen: Pen) -> Self {
        let mut lines = VecDeque::with_capacity(rows);

        for _ in 0..rows {
            lines.push_back(Line::blank(cols, pen));
        }

        lines.reserve(scrollback_limit.unwrap_or(1000));

        Buffer {
            lines,
            cols,
            rows,
            scrollback_limit,
            front_wrapped: false,
        }
    }

    pub fn text(&self) -> Vec<String> {
        let mut text = Vec::new();
        let mut current = String::new();

        for line in &self.lines {
            current.extend(line.chars());

            if !line.is_wrapped() {
                text.push(current.trim_end().to_owned());
                current.clear();
            }
        }

        text
    }

    pub fn replace_char(
        &mut self,
        (col, row): VisualPosition,
        ch: char,
        pen: Pen,
    ) -> Option<usize> {
        self[row].replace_char(col, ch, pen)
    }

    #[inline]
    pub fn replace_ascii_prefix(
        &mut self,
        (col, row): VisualPosition,
        text: &str,
        pen: Pen,
    ) -> usize {
        self[row].replace_ascii_prefix(col, text, pen)
    }

    pub fn insert_char(&mut self, (col, row): VisualPosition, ch: char, pen: Pen) -> Option<usize> {
        self[row].insert_char(col, ch, pen)
    }

    pub fn wrap_to_next_row(&mut self, row: usize) {
        let marks = self.wrap(row);
        self[row + 1].add_marks(marks);
    }

    pub fn wrap_and_scroll_up(&mut self, range: Range<usize>, pen: &Pen) {
        let row = range.end - 1;
        let marks = self.wrap(row);
        self.scroll_up_preserving_bottom_wrap(range, 1, pen);
        self[row].add_marks(marks);
    }

    fn wrap(&mut self, row: usize) -> Vec<Mark> {
        self[row].set_wrapped(true);
        let cols = self.cols;

        self[row].split_marks(cols)
    }

    pub fn anchor_at(&mut self, col: usize, row: isize) -> Option<Anchor> {
        let index = self.view_offset().checked_add_signed(row)?;
        let line = self.lines.get_mut(index)?;

        line.anchor_at(col)
    }

    pub fn resolve(&self, anchor: &Anchor) -> Option<(usize, isize)> {
        for (row, line) in self.lines.iter().enumerate() {
            if let Some(col) = line.anchor_column(anchor) {
                return Some((col, row as isize - self.view_offset() as isize));
            }
        }

        None
    }

    pub fn insert_blanks(&mut self, (col, row): VisualPosition, n: usize, pen: Pen) {
        self[row].insert_blanks(col, n, pen);
    }

    pub fn delete(&mut self, (col, row): VisualPosition, mut n: usize, pen: &Pen) {
        n = n.min(self.cols - col);

        if n == 0 {
            return;
        }

        let line = &mut self[row];
        line.delete(col, n, pen);
        line.set_wrapped(false);
    }

    pub fn erase(&mut self, (col, row): VisualPosition, mode: EraseMode, pen: &Pen) {
        use EraseMode::*;

        match mode {
            NextChars(mut n) => {
                n = n.min(self.cols - col);

                if n == 0 {
                    return;
                }

                let end = col + n;
                let clear_wrap = end == self.cols;
                let line = &mut self[row];
                line.clear(col..end, pen);

                if clear_wrap {
                    line.set_wrapped(false);
                }
            }

            FromCursorToEndOfView => {
                let range = col..self.cols;
                let line = &mut self[row];
                line.set_wrapped(false);
                line.clear(range, pen);
                self.clear((row + 1)..self.rows, pen);
            }

            FromStartOfViewToCursor => {
                let range = 0..(col + 1).min(self.cols);
                self[row].clear(range, pen);
                self.clear(0..row, pen);
            }

            WholeView => {
                self.clear(0..self.rows, pen);
            }

            FromCursorToEndOfLine => {
                let range = col..self.cols;
                let line = &mut self[row];
                line.clear(range, pen);
                line.set_wrapped(false);
            }

            FromStartOfLineToCursor => {
                let range = 0..(col + 1).min(self.cols);
                self[row].clear(range, pen);
            }

            WholeLine => {
                let range = 0..self.cols;
                let line = &mut self[row];
                line.clear(range, pen);
                line.set_wrapped(false);
            }
        }
    }

    pub fn scroll_up(&mut self, range: Range<usize>, n: usize, pen: &Pen) {
        if range.end < self.rows {
            self[range.end - 1].set_wrapped(false);
        }

        self.scroll_up_preserving_bottom_wrap(range, n, pen);
    }

    // Callers decide whether the old bottom row continues into the newly exposed row.
    fn scroll_up_preserving_bottom_wrap(&mut self, range: Range<usize>, mut n: usize, pen: &Pen) {
        n = n.min(range.end - range.start);

        if range.start == 0 {
            if range.end == self.rows {
                self.extend(n, self.cols, pen);
            } else {
                let line = Line::blank(self.cols, *pen);
                let index = self.lines.len() - self.rows + range.end;

                for _ in 0..n {
                    self.lines.insert(index, line.clone());
                }
            }
        } else {
            self[range.start - 1].set_wrapped(false);
            let end = range.end;
            self.visible_rows_mut(range).rotate_left(n);
            self.clear((end - n)..end, pen);
        }
    }

    /// Counts visible rows through the last occupied row or soft-wrap terminator.
    /// `previous_wrapped` indicates a continuation entering the first visible row.
    pub fn occupied_prefix_len(&self, mut previous_wrapped: bool) -> usize {
        let mut end = 0;

        for (row, line) in self.view().enumerate() {
            if previous_wrapped || line.is_wrapped() || !line.is_blank() {
                end = row + 1;
            }

            previous_wrapped = line.is_wrapped();
        }

        end
    }

    /// Moves the occupied screen prefix into history, including blank rows that
    /// finish a soft-wrapped line. Trailing ordinary blank rows stay on screen.
    pub fn scroll_out(&mut self, pen: &Pen) {
        let previous_wrapped = self
            .view_offset()
            .checked_sub(1)
            .map_or(self.front_wrapped, |row| self.lines[row].is_wrapped());

        let end = self.occupied_prefix_len(previous_wrapped);

        if end > 0 {
            self.scroll_up(0..self.rows, end, pen);
        }
    }

    pub fn scroll_down(&mut self, range: Range<usize>, mut n: usize, pen: &Pen) {
        let (start, end) = (range.start, range.end);
        n = n.min(end - start);
        self.visible_rows_mut(range).rotate_right(n);
        self.clear(start..start + n, pen);

        if start > 0 {
            self[start - 1].set_wrapped(false);
        }

        self[end - 1].set_wrapped(false);
    }

    pub fn resize(
        &mut self,
        new_cols: usize,
        new_rows: usize,
        mut cursor: VisualPosition,
        reflow_enabled: bool,
        saved: &mut Option<VisualPosition>,
    ) -> VisualPosition {
        debug_assert!(cursor.0 <= self.cols && cursor.1 < self.rows);

        let old_cols = self.cols;
        let mut old_rows = self.rows;
        let mut saved_absolute = saved.map(|(col, row)| (col, self.view_offset() + row));

        if new_cols != old_cols && reflow_enabled {
            let positions = [
                Some((cursor.0, self.view_offset() + cursor.1)),
                saved_absolute,
            ]
            .map(|position| {
                position.map(|(col, row)| {
                    (
                        ReflowCursor {
                            col: col.min(old_cols - 1),
                            after: col == old_cols,
                        },
                        row,
                    )
                })
            });

            let (lines, positions) = reflow(self.lines.drain(..), new_cols, positions);
            let position = positions[0].expect("live cursor must survive reflow");
            saved_absolute = positions[1];
            self.lines = lines;

            for line in &mut self.lines {
                line.clip_marks();
            }

            let line_count = self.lines.len();

            if line_count < old_rows {
                self.extend(old_rows - line_count, new_cols, &Pen::default());
            }

            let relative_row = position.1 as isize - (self.lines.len() - old_rows) as isize;
            cursor.0 = position.0;

            if relative_row >= 0 {
                cursor.1 = relative_row as usize;
            } else {
                cursor.1 = 0;
                old_rows += (-relative_row) as usize;
            }
        } else if new_cols != old_cols {
            for line in &mut self.lines {
                line.resize(new_cols);
            }

            cursor.0 = cursor.0.min(new_cols - 1);
        }

        let line_count = self.lines.len();
        debug_assert!(old_rows <= line_count);

        match new_rows.cmp(&old_rows) {
            Ordering::Less => {
                let height_delta = old_rows - new_rows;
                let inverted_cursor_row = old_rows - 1 - cursor.1;
                let excess = height_delta.min(inverted_cursor_row);

                if excess > 0 {
                    self.lines.truncate(line_count - excess);
                    self.lines.back_mut().unwrap().set_wrapped(false);
                }

                cursor.1 -= height_delta - excess;
            }

            Ordering::Greater => {
                let mut height_delta = new_rows - old_rows;
                let scrollback_size = line_count - old_rows;
                let cursor_row_shift = scrollback_size.min(height_delta);
                height_delta -= cursor_row_shift;

                cursor.1 += cursor_row_shift;

                if height_delta > 0 {
                    self.extend(height_delta, new_cols, &Pen::default());
                }
            }

            Ordering::Equal => (),
        }

        self.cols = new_cols;
        self.rows = new_rows;

        if reflow_enabled {
            *saved = saved_absolute.map(|(col, row)| {
                if (self.view_offset()..self.lines.len()).contains(&row) {
                    (col, row - self.view_offset())
                } else {
                    // A saved cursor never changes the viewport chosen for the live one.
                    (0, 0)
                }
            });
        }

        cursor
    }

    pub fn view(&self) -> impl ExactSizeIterator<Item = &Line> {
        self.lines.iter().skip(self.view_offset())
    }

    pub fn lines(&self) -> impl Iterator<Item = &Line> {
        self.lines.iter()
    }

    fn view_offset(&self) -> usize {
        self.lines.len() - self.rows
    }

    fn visible_rows_mut(&mut self, range: Range<usize>) -> &mut [Line] {
        let offset = self.view_offset();

        &mut self.lines.make_contiguous()[offset + range.start..offset + range.end]
    }

    #[inline]
    pub fn pop_evicted(&mut self) -> Option<Line> {
        if self
            .scrollback_limit
            .is_some_and(|limit| self.lines.len() - self.rows > limit)
        {
            let line = self.lines.pop_front()?;
            self.front_wrapped = line.is_wrapped();

            return Some(line);
        }

        None
    }

    fn clear(&mut self, range: Range<usize>, pen: &Pen) {
        let offset = self.view_offset();

        for line in self
            .lines
            .range_mut(offset + range.start..offset + range.end)
        {
            line.reset(*pen);
        }
    }

    fn extend(&mut self, n: usize, cols: usize, pen: &Pen) {
        let line = Line::blank(cols, *pen);
        let filler = core::iter::repeat_n(line, n);
        self.lines.extend(filler);
    }

    #[cfg(test)]
    pub(crate) fn assert_execution_equivalent(&self, other: &Self) {
        assert_eq!((self.cols, self.rows), (other.cols, other.rows));
        assert_eq!(self.scrollback_limit, other.scrollback_limit);
        assert_eq!(self.front_wrapped, other.front_wrapped);
        // Line equality excludes tracking registrations and allocation layout.
        assert_eq!(self.lines, other.lines);
    }

    #[cfg(test)]
    pub fn add_scrollback(&mut self, n: usize) {
        let mut line = Line::blank(self.cols, Pen::default());

        for col in 0..self.cols {
            line.replace_char(col, 's', Pen::default());
        }

        for _ in 0..n {
            self.lines.insert(0, line.clone());
        }
    }
}

impl Index<usize> for Buffer {
    type Output = Line;

    fn index(&self, index: usize) -> &Self::Output {
        &self.lines[self.view_offset() + index]
    }
}

impl Index<VisualPosition> for Buffer {
    type Output = Cell;

    fn index(&self, (col, row): VisualPosition) -> &Self::Output {
        &self.lines[self.view_offset() + row][col]
    }
}

impl IndexMut<usize> for Buffer {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        let idx = self.view_offset() + index;
        &mut self.lines[idx]
    }
}

struct Reflow<I>
where
    I: Iterator<Item = (Line, ReflowCursors)>,
{
    pub iter: I,
    pub cols: usize,
    pub rest: Option<(Line, ReflowCursors)>,
}

fn reflow(
    iter: impl Iterator<Item = Line>,
    cols: usize,
    mut cursors: [Option<(ReflowCursor, usize)>; 2],
) -> (VecDeque<Line>, [Option<VisualPosition>; 2]) {
    let iter = iter.enumerate().map(|(row, line)| {
        let tracked = core::array::from_fn(|i| {
            if cursors[i]
                .as_ref()
                .is_some_and(|(_, cursor_row)| *cursor_row == row)
            {
                cursors[i].take().map(|(cursor, _)| cursor)
            } else {
                None
            }
        });

        (line, tracked)
    });

    let reflow = Reflow {
        iter,
        cols,
        rest: None,
    };

    let mut positions = [None; 2];

    let lines: VecDeque<Line> = reflow
        .enumerate()
        .map(|(row, (line, cursor))| {
            for (position, cursor) in positions.iter_mut().zip(cursor) {
                if let Some(mut c) = cursor {
                    c.clamp(cols);
                    *position = Some((c.col + usize::from(c.after), row));
                }
            }

            line
        })
        .collect();

    assert!(lines.iter().all(|l| l.len() == cols));

    (lines, positions)
}

impl<I: Iterator<Item = (Line, ReflowCursors)>> Iterator for Reflow<I> {
    type Item = (Line, ReflowCursors);

    fn next(&mut self) -> Option<Self::Item> {
        use core::cmp::Ordering::*;

        while let Some((mut line, mut cursor)) = self.rest.take().or_else(|| self.iter.next()) {
            match self.cols.cmp(&line.len()) {
                Less => {
                    self.rest = line.split_for_reflow(self.cols, &mut cursor);

                    return Some((line, cursor));
                }

                Equal => {
                    return Some((line, cursor));
                }

                Greater => match self.iter.next() {
                    Some((next_line, next_cursor)) => {
                        self.rest =
                            line.append_for_reflow(next_line, self.cols, &mut cursor, next_cursor);

                        if line.len() == self.cols {
                            return Some((line, cursor));
                        }

                        self.rest = Some((line, cursor));
                    }

                    None => {
                        line.expand(self.cols);
                        line.set_wrapped(false);

                        return Some((line, cursor));
                    }
                },
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::{Buffer, VisualPosition};
    use crate::line::Line;
    use crate::pen::Pen;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;
    use pretty_assertions::assert_eq;

    #[test]
    fn text() {
        let mut buffer = Buffer::new(10, 5, None, Pen::default());

        assert_eq!(buffer.text(), vec!["", "", "", "", ""]);

        buffer.replace_char((0, 0), 'x', Pen::default());
        buffer.replace_char((1, 1), 'x', Pen::default());
        buffer.replace_char((2, 2), 'x', Pen::default());
        buffer.replace_char((3, 3), 'x', Pen::default());
        buffer.replace_char((4, 4), 'x', Pen::default());
        assert_eq!(buffer.text(), vec!["x", " x", "  x", "   x", "    x"]);

        buffer.wrap(0);
        buffer.wrap(3);
        assert_eq!(
            buffer.text(),
            vec!["x          x", "  x", "   x          x"]
        );
    }

    #[test]
    fn scroll_up() {
        let content = vec![
            ("aaaa", true),
            ("aaaa", true),
            ("aa  ", false),
            ("bb", false),
            ("cccc", true),
            ("cccc", true),
            ("cc", false),
        ];

        let pen = Pen::default();

        // whole view

        let mut buf = buffer(&content, None, 0);

        buf.scroll_up(0..content.len(), 1, &pen);

        assert_eq!(line(&buf[0]), "aaaa⏎");
        assert_eq!(line(&buf[1]), "aa  ");
        assert_eq!(line(&buf[2]), "bb  ");
        assert_eq!(line(&buf[3]), "cccc⏎");
        assert_eq!(line(&buf[4]), "cccc⏎");
        assert_eq!(line(&buf[5]), "cc  ");
        assert_eq!(line(&buf[6]), "    ");
        assert_eq!(buf.text().join("\n"), "aaaaaaaaaa\nbb\ncccccccccc\n");
        assert_eq!(buf.lines.len(), 8);
        assert!(buf.lines[0].is_wrapped());

        // top of the view

        let mut buf = buffer(&content, None, 0);

        buf.scroll_up(0..5, 1, &pen);

        assert_eq!(line(&buf[0]), "aaaa⏎");
        assert_eq!(line(&buf[1]), "aa  ");
        assert_eq!(line(&buf[2]), "bb  ");
        assert_eq!(line(&buf[3]), "cccc");
        assert_eq!(line(&buf[4]), "    ");
        assert_eq!(line(&buf[5]), "cccc⏎");
        assert_eq!(line(&buf[6]), "cc  ");
        assert_eq!(buf.text().join("\n"), "aaaaaaaaaa\nbb\ncccc\n\ncccccc");
        assert_eq!(buf.lines.len(), 8);
        assert!(buf.lines[0].is_wrapped());

        // bottom of the view

        let mut buf = buffer(&content, None, 0);

        buf.scroll_up(1..content.len(), 1, &pen);

        assert_eq!(line(&buf[0]), "aaaa");
        assert_eq!(line(&buf[1]), "aa  ");
        assert_eq!(line(&buf[2]), "bb  ");
        assert_eq!(line(&buf[3]), "cccc⏎");
        assert_eq!(line(&buf[4]), "cccc⏎");
        assert_eq!(line(&buf[5]), "cc  ");
        assert_eq!(line(&buf[6]), "    ");
        assert_eq!(buf.text().join("\n"), "aaaa\naa\nbb\ncccccccccc\n");
        assert_eq!(buf.lines.len(), 7);

        // no scrollback limit

        let mut buf = buffer(&content, None, 0);

        buf.scroll_up(0..content.len(), 5, &pen);

        assert_eq!(buf.lines.len(), 12);

        // scrollback limit of 0

        let mut buf = buffer(&content, Some(0), 0);

        buf.scroll_up(0..content.len(), 5, &pen);

        assert_eq!(buf.lines.len(), 12);

        while buf.pop_evicted().is_some() {}

        assert_eq!(buf.lines.len(), 7);

        // scrollback limit of 3

        let mut buf = buffer(&content, Some(3), 0);

        buf.scroll_up(0..content.len(), 5, &pen);

        assert_eq!(buf.lines.len(), 12);

        while buf.pop_evicted().is_some() {}

        assert_eq!(buf.lines.len(), 10);
    }

    fn line(line: &Line) -> String {
        let mut t = line.text();

        if line.is_wrapped() {
            t.push('⏎');
        }

        t
    }

    #[test]
    fn resize_shorter() {
        let content = vec![
            ("aa  ", false),
            ("bbbb", true),
            ("bbbb", true),
            ("bb", false),
            ("cc", false),
        ];

        // cursor at the top

        for scrollback in [0, 20] {
            let (view, cursor) = resize_buffer(scrollback, content.clone(), 4, 3, (0, 0));

            assert_eq!(cursor, (0, 0));
            assert_eq!(view, vec!["aa  ", "bbbb", "bbbb"]);
        }

        // cursor at the bottom

        for scrollback in [0, 20] {
            let (view, cursor) = resize_buffer(scrollback, content.clone(), 4, 3, (0, 4));

            assert_eq!(cursor, (0, 2));
            assert_eq!(view, vec!["bbbb", "bb  ", "cc  "]);
        }

        // cursor in the middle

        for scrollback in [0, 20] {
            let (view, cursor) = resize_buffer(scrollback, content.clone(), 4, 2, (0, 3));

            assert_eq!(cursor, (0, 1));
            assert_eq!(view, vec!["bbbb", "bb  "]);
        }
    }

    #[test]
    fn resize_taller() {
        let content = vec![
            ("aa  ", false),
            ("bbbb", true),
            ("bbbb", true),
            ("bb", false),
            ("cc", false),
        ];

        // cursor at the top, no scrollback

        let (view, cursor) = resize_buffer(0, content.clone(), 4, 7, (0, 0));

        assert_eq!(cursor, (0, 0));
        assert_eq!(
            view,
            vec!["aa  ", "bbbb", "bbbb", "bb  ", "cc  ", "    ", "    "]
        );

        // cursor at the top, with scrollback

        let (view, cursor) = resize_buffer(20, content.clone(), 4, 7, (0, 0));

        assert_eq!(cursor, (0, 2));
        assert_eq!(
            view,
            vec!["ssss", "ssss", "aa  ", "bbbb", "bbbb", "bb  ", "cc  "]
        );

        // cursor at the bottom, no scrollback

        let (view, cursor) = resize_buffer(0, content.clone(), 4, 7, (0, 4));

        assert_eq!(cursor, (0, 4));
        assert_eq!(
            view,
            vec!["aa  ", "bbbb", "bbbb", "bb  ", "cc  ", "    ", "    "]
        );

        // cursor at the bottom, with scrollback

        let (view, cursor) = resize_buffer(20, content.clone(), 4, 7, (0, 4));

        assert_eq!(cursor, (0, 6));
        assert_eq!(
            view,
            vec!["ssss", "ssss", "aa  ", "bbbb", "bbbb", "bb  ", "cc  "]
        );

        // cursor in the middle, no scrollback

        let (view, cursor) = resize_buffer(0, content.clone(), 4, 7, (0, 3));

        assert_eq!(cursor, (0, 3));
        assert_eq!(
            view,
            vec!["aa  ", "bbbb", "bbbb", "bb  ", "cc  ", "    ", "    "]
        );

        // cursor in the middle, with scrollback

        let (view, cursor) = resize_buffer(20, content.clone(), 4, 7, (0, 3));

        assert_eq!(cursor, (0, 5));
        assert_eq!(
            view,
            vec!["ssss", "ssss", "aa  ", "bbbb", "bbbb", "bb  ", "cc  "]
        );
    }

    #[test]
    fn resize_wider() {
        let content = vec![
            ("aa  ", false),
            ("bbbb", true),
            ("bbbb", true),
            ("bb", false),
            ("cc", false),
        ];

        // cursor at the top, no scrollback

        let (view, cursor) = resize_buffer(0, content.clone(), 6, 5, (0, 0));

        assert_eq!(cursor, (0, 0));
        assert_eq!(view, vec!["aa    ", "bbbbbb", "bbbb  ", "cc    ", "      "]);

        // cursor at the top, with scrollback

        let (view, cursor) = resize_buffer(20, content.clone(), 6, 5, (0, 0));

        assert_eq!(cursor, (0, 1));
        assert_eq!(view, vec!["ssss  ", "aa    ", "bbbbbb", "bbbb  ", "cc    "]);

        // cursor at the bottom, no scrollback

        let (view, cursor) = resize_buffer(0, content.clone(), 6, 5, (0, 4));

        assert_eq!(cursor, (0, 3));
        assert_eq!(view, vec!["aa    ", "bbbbbb", "bbbb  ", "cc    ", "      "]);

        // cursor at the bottom, with scrollback

        let (view, cursor) = resize_buffer(20, content.clone(), 6, 5, (0, 4));

        assert_eq!(cursor, (0, 4));
        assert_eq!(view, vec!["ssss  ", "aa    ", "bbbbbb", "bbbb  ", "cc    "]);

        // cursor in the middle, no scrollback

        let (view, cursor) = resize_buffer(0, content.clone(), 6, 5, (1, 2));

        assert_eq!(cursor, (5, 1));
        assert_eq!(view, vec!["aa    ", "bbbbbb", "bbbb  ", "cc    ", "      "]);

        // cursor in the middle, with scrollback

        let (view, cursor) = resize_buffer(20, content.clone(), 6, 5, (1, 2));

        assert_eq!(cursor, (5, 2));
        assert_eq!(view, vec!["ssss  ", "aa    ", "bbbbbb", "bbbb  ", "cc    "]);
    }

    #[test]
    fn resize_narrower() {
        let content = vec![
            ("aa  ", false),
            ("bbbb", true),
            ("bbbb", true),
            ("bb", false),
            ("cc", false),
        ];

        // cursor at the top, no scrollback

        let (view, cursor) = resize_buffer(0, content.clone(), 2, 5, (0, 0));

        assert_eq!(cursor, (0, 0));
        assert_eq!(view, vec!["aa", "bb", "bb", "bb", "bb"]);

        // cursor at the top, with scrollback

        let (view, cursor) = resize_buffer(20, content.clone(), 2, 5, (0, 0));

        assert_eq!(cursor, (0, 0));
        assert_eq!(view, vec!["aa", "bb", "bb", "bb", "bb"]);

        // cursor at the bottom, no scrollback

        let (view, cursor) = resize_buffer(0, content.clone(), 2, 5, (0, 4));

        assert_eq!(cursor, (0, 4));
        assert_eq!(view, vec!["bb", "bb", "bb", "bb", "cc"]);

        // cursor at the bottom, with scrollback

        let (view, cursor) = resize_buffer(20, content.clone(), 2, 5, (0, 4));

        assert_eq!(cursor, (0, 4));
        assert_eq!(view, vec!["bb", "bb", "bb", "bb", "cc"]);

        // cursor in the middle, no scrollback

        let (view, cursor) = resize_buffer(0, content.clone(), 2, 5, (1, 2));

        assert_eq!(cursor, (1, 1));
        assert_eq!(view, vec!["bb", "bb", "bb", "bb", "cc"]);

        // cursor in the middle, with scrollback

        let (view, cursor) = resize_buffer(20, content.clone(), 2, 5, (1, 2));

        assert_eq!(cursor, (1, 1));
        assert_eq!(view, vec!["bb", "bb", "bb", "bb", "cc"]);

        // cursor in the middle, no scrollback, last lines wrapped

        let (view, cursor) = resize_buffer(
            0,
            vec![
                ("aa  ", false),
                ("bbbb", true),
                ("bbb ", false),
                ("cccc", true),
                ("cc", false),
            ],
            2,
            5,
            (1, 2),
        );

        assert_eq!(cursor, (1, 0));
        assert_eq!(view, vec!["bb", "b ", "cc", "cc", "cc"]);
    }

    fn resize_buffer(
        scrollback_size: usize,
        content: Vec<(&str, bool)>,
        new_cols: usize,
        new_rows: usize,
        mut cursor: VisualPosition,
    ) -> (Vec<String>, VisualPosition) {
        let mut buffer = buffer(&content, None, scrollback_size);
        cursor = buffer.resize(new_cols, new_rows, cursor, true, &mut None);

        let view = buffer.view().map(|line| line.text()).collect::<Vec<_>>();

        (view, cursor)
    }

    fn buffer(
        content: &[(&str, bool)],
        scrollback_limit: Option<usize>,
        scrollback_size: usize,
    ) -> Buffer {
        let cols = content[0].0.len();
        let rows = content.len();
        let mut buffer = Buffer::new(cols, rows, scrollback_limit, Pen::default());

        if !matches!(scrollback_limit, Some(0)) {
            buffer.add_scrollback(scrollback_size);
        }

        for (row, (line, wrapped)) in content.iter().enumerate() {
            for (col, ch) in line.chars().enumerate() {
                buffer.replace_char((col, row), ch, Pen::default());
            }

            if *wrapped {
                buffer.wrap(row);
            }
        }

        buffer
    }
}
