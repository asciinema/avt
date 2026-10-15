use std::cmp::Ordering;
use std::collections::VecDeque;
use std::ops::{Index, IndexMut, Range};
use std::sync::Arc;

use crate::anchor::{Anchor, Mark};
use crate::cell::Cell;
use crate::line::Line;
use crate::pen::Pen;

#[derive(Debug)]
pub(crate) struct Buffer {
    lines: VecDeque<Line>,
    pub cols: usize,
    pub rows: usize,
    scrollback_limit: Option<usize>,
    trim_needed: bool,
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

type LogicalPosition = (usize, usize);
type RelativePosition = (usize, isize);
type VisualPosition = (usize, usize);

impl Buffer {
    pub fn new(
        cols: usize,
        rows: usize,
        scrollback_limit: Option<usize>,
        pen: Option<&Pen>,
    ) -> Self {
        let default_pen = Pen::default();
        let pen = pen.unwrap_or(&default_pen);
        let mut lines = VecDeque::with_capacity(rows);

        for _ in 0..rows {
            lines.push_back(Line::blank(cols, *pen));
        }

        if let Some(limit) = scrollback_limit {
            if limit > 0 {
                lines.reserve(limit);
            }
        } else {
            lines.reserve(1000);
        }

        Buffer {
            lines,
            cols,
            rows,
            scrollback_limit,
            trim_needed: false,
            front_wrapped: false,
        }
    }

    pub fn text(&self) -> Vec<String> {
        let mut text = Vec::new();
        let mut current = String::new();

        for line in &self.lines {
            current.extend(line.chars());

            if !line.wrapped {
                text.push(current.trim_end().to_owned());
                current.clear();
            }
        }

        text
    }

    pub fn print(&mut self, (col, row): VisualPosition, ch: char, pen: Pen) -> Option<usize> {
        self[row].print(col, ch, pen)
    }

    pub fn wrap_to_next_row(&mut self, row: usize) {
        let marks = self.wrap(row);
        self[row + 1].marks.extend(marks);
    }

    pub fn wrap_and_scroll_up(&mut self, range: Range<usize>, pen: &Pen) {
        let row = range.end - 1;
        let marks = self.wrap(row);
        self.scroll_up_preserving_bottom_wrap(range, 1, pen);
        self[row].marks.extend(marks);
    }

    fn wrap(&mut self, row: usize) -> Vec<Mark> {
        self[row].wrapped = true;
        let cols = self.cols;

        self[row].split_marks(cols)
    }

    pub fn anchor_at(&mut self, col: usize, row: isize) -> Option<Anchor> {
        let index = self.view_offset().checked_add_signed(row)?;
        let line = self.lines.get_mut(index)?;

        if col > line.len() {
            return None;
        }

        line.marks.retain(Mark::live);
        let anchor = Anchor(Arc::new(()));

        line.marks.push(Mark {
            token: Arc::downgrade(&anchor.0),
            col,
        });

        Some(anchor)
    }

    pub fn resolve(&self, anchor: &Anchor) -> Option<(usize, isize)> {
        for (row, line) in self.lines.iter().enumerate() {
            if let Some(mark) = line.marks.iter().find(|mark| mark.matches(anchor)) {
                return Some((mark.col, row as isize - self.view_offset() as isize));
            }
        }

        None
    }

    pub fn shift_right(&mut self, (col, row): VisualPosition, mut n: usize, pen: Pen) {
        n = n.min(self.cols - col);
        self[row].shift_right(col, n, pen);
    }

    pub fn delete(&mut self, (col, row): VisualPosition, mut n: usize, pen: &Pen) {
        n = n.min(self.cols - col);
        let line = &mut self[row];
        line.delete(col, n, pen);
        line.wrapped = false;
    }

    pub fn erase(&mut self, (col, row): VisualPosition, mode: EraseMode, pen: &Pen) {
        use EraseMode::*;

        match mode {
            NextChars(mut n) => {
                n = n.min(self.cols - col);
                let end = col + n;
                let clear_wrap = end == self.cols;
                let line = &mut self[row];
                line.clear(col..end, pen);

                if clear_wrap {
                    line.wrapped = false;
                }
            }

            FromCursorToEndOfView => {
                let range = col..self.cols;
                let line = &mut self[row];
                line.wrapped = false;
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
                line.wrapped = false;
            }

            FromStartOfLineToCursor => {
                let range = 0..(col + 1).min(self.cols);
                self[row].clear(range, pen);
            }

            WholeLine => {
                let range = 0..self.cols;
                let line = &mut self[row];
                line.clear(range, pen);
                line.wrapped = false;
            }
        }
    }

    pub fn scroll_up(&mut self, range: Range<usize>, n: usize, pen: &Pen) {
        if range.end < self.rows {
            self[range.end - 1].wrapped = false;
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
            self[range.start - 1].wrapped = false;
            let end = range.end;
            self.visible_rows_mut(range).rotate_left(n);
            self.clear((end - n)..end, pen);
        }

        self.trim_needed = true;
    }

    /// Moves the occupied screen prefix into history, including blank rows that
    /// finish a soft-wrapped line. Trailing ordinary blank rows stay on screen.
    pub fn scroll_out(&mut self, pen: &Pen) {
        let mut end = 0;

        let mut wrapped = self
            .view_offset()
            .checked_sub(1)
            .map_or(self.front_wrapped, |row| self.lines[row].wrapped);

        for (row, line) in self.view().enumerate() {
            if wrapped || line.wrapped || !line.is_blank() {
                end = row + 1;
            }

            wrapped = line.wrapped;
        }

        self.scroll_up(0..self.rows, end, pen);
    }

    pub fn scroll_down(&mut self, range: Range<usize>, mut n: usize, pen: &Pen) {
        let (start, end) = (range.start, range.end);
        n = n.min(end - start);
        self.visible_rows_mut(range).rotate_right(n);
        self.clear(start..start + n, pen);

        if start > 0 {
            self[start - 1].wrapped = false;
        }

        self[end - 1].wrapped = false;
    }

    pub fn resize(
        &mut self,
        new_cols: usize,
        new_rows: usize,
        mut cursor: VisualPosition,
    ) -> VisualPosition {
        debug_assert!(cursor.0 <= self.cols && cursor.1 < self.rows);

        let old_cols = self.cols;
        let mut old_rows = self.rows;

        if new_cols != old_cols {
            let cursor_log_pos = self.logical_position(cursor, old_cols, old_rows);
            self.lines = reflow(self.lines.drain(..), new_cols);

            for line in &mut self.lines {
                line.clip_marks();
            }

            let line_count = self.lines.len();

            if line_count < old_rows {
                self.extend(old_rows - line_count, new_cols, &Pen::default());
            }

            let cursor_rel_pos = self.relative_position(cursor_log_pos, new_cols, old_rows);
            cursor.0 = cursor_rel_pos.0;

            if cursor_rel_pos.1 >= 0 {
                cursor.1 = cursor_rel_pos.1 as usize;
            } else {
                cursor.1 = 0;
                old_rows += (-cursor_rel_pos.1) as usize;
            }
        }

        let line_count = self.lines.len();

        match new_rows.cmp(&old_rows) {
            Ordering::Less => {
                let height_delta = old_rows - new_rows;
                let inverted_cursor_row = old_rows - 1 - cursor.1;
                let excess = height_delta.min(inverted_cursor_row);

                if excess > 0 {
                    self.lines.truncate(line_count - excess);
                    self.lines.back_mut().unwrap().wrapped = false;
                }

                cursor.1 -= height_delta - excess;
            }

            Ordering::Greater => {
                let mut height_delta = new_rows - old_rows;
                let scrollback_size = line_count - old_rows.min(line_count);
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
        self.trim_needed = true;

        cursor
    }

    fn logical_position(&self, pos: VisualPosition, cols: usize, rows: usize) -> LogicalPosition {
        let vis_row_offset = self.lines.len() - rows;
        let mut log_col_offset = 0;
        let abs_row = pos.1 + vis_row_offset;
        let mut log_row = 0;

        for line in self.lines.iter().take(abs_row) {
            if line.wrapped {
                log_col_offset += cols;
            } else {
                log_col_offset = 0;
                log_row += 1;
            }
        }

        (pos.0 + log_col_offset, log_row)
    }

    fn relative_position(
        &self,
        pos: LogicalPosition,
        cols: usize,
        rows: usize,
    ) -> RelativePosition {
        let mut rel_col = pos.0;
        let mut rel_row = 0;
        let mut r = 0;
        let last_row = self.lines.len() - 1;

        while r < pos.1 && rel_row < last_row {
            if !self.lines[rel_row].wrapped {
                r += 1;
            }

            rel_row += 1;
        }

        while rel_col >= cols && self.lines[rel_row].wrapped {
            rel_col -= cols;
            rel_row += 1;
        }

        rel_col = rel_col.min(cols - 1);
        let rel_row_offset = self.lines.len() - rows;

        (rel_col, (rel_row as isize - rel_row_offset as isize))
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

    pub fn pop_evicted(&mut self) -> Option<Line> {
        if self.trim_needed {
            if self
                .scrollback_limit
                .is_some_and(|limit| self.lines.len() - self.rows > limit)
            {
                let line = self.lines.pop_front()?;
                self.front_wrapped = line.wrapped;

                return Some(line);
            }

            self.trim_needed = false;
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
        let filler = std::iter::repeat_n(line, n);
        self.lines.extend(filler);
    }

    #[cfg(test)]
    pub(crate) fn assert_execution_equivalent(&self, other: &Self) {
        assert_eq!((self.cols, self.rows), (other.cols, other.rows));
        assert_eq!(self.scrollback_limit, other.scrollback_limit);
        assert_eq!(self.front_wrapped, other.front_wrapped);
        // Line equality excludes tracking registrations. Allocation layout and
        // the trim optimization flag are not part of execution equivalence.
        assert_eq!(self.lines, other.lines);
    }

    #[cfg(test)]
    pub fn add_scrollback(&mut self, n: usize) {
        let mut line = Line::blank(self.cols, Pen::default());

        for col in 0..self.cols {
            line.print(col, 's', Pen::default());
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
    I: Iterator<Item = Line>,
{
    pub iter: I,
    pub cols: usize,
    pub rest: Option<Line>,
}

pub(crate) fn reflow<I: Iterator<Item = Line>>(iter: I, cols: usize) -> VecDeque<Line> {
    let lines: VecDeque<Line> = Reflow {
        iter,
        cols,
        rest: None,
    }
    .collect();

    assert!(lines.iter().all(|l| l.len() == cols));

    lines
}

impl<I: Iterator<Item = Line>> Iterator for Reflow<I> {
    type Item = Line;

    fn next(&mut self) -> Option<Self::Item> {
        use std::cmp::Ordering::*;

        while let Some(mut line) = self.rest.take().or_else(|| self.iter.next()) {
            match self.cols.cmp(&line.len()) {
                Less => {
                    self.rest = line.contract(self.cols);

                    return Some(line);
                }

                Equal => {
                    return Some(line);
                }

                Greater => match self.iter.next() {
                    Some(next_line) => {
                        self.rest = line.extend(next_line, self.cols);

                        if line.len() == self.cols {
                            return Some(line);
                        }

                        self.rest = Some(line);
                    }

                    None => {
                        line.expand(self.cols);
                        line.wrapped = false;

                        return Some(line);
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
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;

    #[test]
    fn text() {
        let mut buffer = Buffer::new(10, 5, None, None);

        assert_eq!(buffer.text(), vec!["", "", "", "", ""]);

        buffer.print((0, 0), 'x', Pen::default());
        buffer.print((1, 1), 'x', Pen::default());
        buffer.print((2, 2), 'x', Pen::default());
        buffer.print((3, 3), 'x', Pen::default());
        buffer.print((4, 4), 'x', Pen::default());
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
        assert!(buf.lines[0].wrapped);

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
        assert!(buf.lines[0].wrapped);

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

        if line.wrapped {
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

    proptest! {
        #[test]
        fn prop_cursor_translation(scrollback_size in 0..20usize, wrapped in prop::collection::vec(prop::bool::ANY, 5), col in 0..10usize, row in 0..5usize) {
            let cols = 10;
            let rows = 5;
            let mut buffer = Buffer::new(cols, rows, None, None);
            buffer.add_scrollback(scrollback_size);

            for (i, w) in wrapped.iter().enumerate() {
                if *w {
                    buffer.wrap(i);
                }
            }

            let rel_cur = buffer.logical_position((col, row), cols, rows);

            assert_eq!(buffer.relative_position(rel_cur, cols, rows), (col, row as isize));
        }
    }

    fn resize_buffer(
        scrollback_size: usize,
        content: Vec<(&str, bool)>,
        new_cols: usize,
        new_rows: usize,
        mut cursor: VisualPosition,
    ) -> (Vec<String>, VisualPosition) {
        let mut buffer = buffer(&content, None, scrollback_size);
        cursor = buffer.resize(new_cols, new_rows, cursor);

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
        let mut buffer = Buffer::new(cols, rows, scrollback_limit, None);

        if !matches!(scrollback_limit, Some(0)) {
            buffer.add_scrollback(scrollback_size);
        }

        for (row, (line, wrapped)) in content.iter().enumerate() {
            for (col, ch) in line.chars().enumerate() {
                buffer.print((col, row), ch, Pen::default());
            }

            if *wrapped {
                buffer.wrap(row);
            }
        }

        buffer
    }
}
