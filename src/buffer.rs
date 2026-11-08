use alloc::borrow::ToOwned as _;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::ops::{Index, IndexMut, Range};

use crate::anchor::{Anchor, Mark};
use crate::cell::Cell;
use crate::pen::Pen;
use crate::row::{ReflowCursor, ReflowCursors, Row};

/// Screen/history storage. Structural edits maintain row relationships while
/// individual rows maintain their cells and anchor columns.
#[derive(Debug)]
pub(crate) struct Buffer {
    rows: VecDeque<Row>,
    pub width: usize,
    /// Visible height; `rows` also includes retained scrollback.
    pub height: usize,
    scrollback_limit: Option<usize>,
    // The first retained row continues the last evicted row's logical line.
    starts_with_continuation: bool,
}

pub(crate) enum EraseMode {
    NextChars(usize),
    FromCursorToEndOfView,
    FromStartOfViewToCursor,
    WholeView,
    FromCursorToEndOfRow,
    FromStartOfRowToCursor,
    WholeRow,
}

type VisualPosition = (usize, usize);

impl Buffer {
    pub fn new(width: usize, height: usize, scrollback_limit: Option<usize>, pen: Pen) -> Self {
        let mut rows = VecDeque::with_capacity(height);

        for _ in 0..height {
            rows.push_back(Row::blank(width, pen));
        }

        rows.reserve(scrollback_limit.unwrap_or(1000));

        Buffer {
            rows,
            width,
            height,
            scrollback_limit,
            starts_with_continuation: false,
        }
    }

    pub fn text(&self) -> Vec<String> {
        let mut text = Vec::new();
        let mut current = String::new();

        for row in &self.rows {
            current.extend(row.chars());

            if !row.wraps_to_next() {
                text.push(current.trim_end().to_owned());
                current.clear();
            }
        }

        text
    }

    pub fn replace_char(
        &mut self,
        (col, row_index): VisualPosition,
        ch: char,
        pen: Pen,
    ) -> Option<usize> {
        self[row_index].replace_char(col, ch, pen)
    }

    #[inline]
    pub fn replace_ascii_prefix(
        &mut self,
        (col, row_index): VisualPosition,
        text: &str,
        pen: Pen,
    ) -> usize {
        self[row_index].replace_ascii_prefix(col, text, pen)
    }

    pub fn insert_char(
        &mut self,
        (col, row_index): VisualPosition,
        ch: char,
        pen: Pen,
    ) -> Option<usize> {
        self[row_index].insert_char(col, ch, pen)
    }

    pub fn wrap_to_next_row(&mut self, row_index: usize) {
        let marks = self.wrap(row_index);
        self[row_index + 1].add_marks(marks);
    }

    pub fn wrap_and_scroll_up(&mut self, range: Range<usize>, pen: &Pen) {
        let row_index = range.end - 1;
        let marks = self.wrap(row_index);
        self.scroll_up_preserving_bottom_wrap(range, 1, pen);
        self[row_index].add_marks(marks);
    }

    fn wrap(&mut self, row_index: usize) -> Vec<Mark> {
        self[row_index].set_wraps_to_next(true);
        let cols = self.width;

        self[row_index].split_marks(cols)
    }

    pub fn anchor_at(&mut self, col: usize, row_coord: isize) -> Option<Anchor> {
        let buffer_index = self.view_offset().checked_add_signed(row_coord)?;
        let row = self.rows.get_mut(buffer_index)?;

        row.anchor_at(col)
    }

    pub fn resolve(&self, anchor: &Anchor) -> Option<(usize, isize)> {
        for (buffer_index, row) in self.rows.iter().enumerate() {
            if let Some(col) = row.anchor_column(anchor) {
                return Some((col, buffer_index as isize - self.view_offset() as isize));
            }
        }

        None
    }

    pub fn insert_blanks(&mut self, (col, row_index): VisualPosition, n: usize, pen: Pen) {
        self[row_index].insert_blanks(col, n, pen);
    }

    pub fn delete(&mut self, (col, row_index): VisualPosition, mut n: usize, pen: &Pen) {
        n = n.min(self.width - col);

        if n == 0 {
            return;
        }

        let row = &mut self[row_index];
        row.delete(col, n, pen);
        row.set_wraps_to_next(false);
    }

    pub fn erase(&mut self, (col, row_index): VisualPosition, mode: EraseMode, pen: &Pen) {
        use EraseMode::*;

        match mode {
            NextChars(mut n) => {
                n = n.min(self.width - col);

                if n == 0 {
                    return;
                }

                let end = col + n;
                let clear_wrap = end == self.width;
                let row = &mut self[row_index];
                row.clear(col..end, pen);

                if clear_wrap {
                    row.set_wraps_to_next(false);
                }
            }

            FromCursorToEndOfView => {
                let range = col..self.width;
                let row = &mut self[row_index];
                row.set_wraps_to_next(false);
                row.clear(range, pen);
                self.clear((row_index + 1)..self.height, pen);
            }

            FromStartOfViewToCursor => {
                let range = 0..(col + 1).min(self.width);
                self[row_index].clear(range, pen);
                self.clear(0..row_index, pen);
            }

            WholeView => {
                self.clear(0..self.height, pen);
            }

            FromCursorToEndOfRow => {
                let range = col..self.width;
                let row = &mut self[row_index];
                row.clear(range, pen);
                row.set_wraps_to_next(false);
            }

            FromStartOfRowToCursor => {
                let range = 0..(col + 1).min(self.width);
                self[row_index].clear(range, pen);
            }

            WholeRow => {
                let range = 0..self.width;
                let row = &mut self[row_index];
                row.clear(range, pen);
                row.set_wraps_to_next(false);
            }
        }
    }

    pub fn scroll_up(&mut self, range: Range<usize>, n: usize, pen: &Pen) {
        if range.end < self.height {
            self[range.end - 1].set_wraps_to_next(false);
        }

        self.scroll_up_preserving_bottom_wrap(range, n, pen);
    }

    // Callers decide whether the old bottom row continues into the newly exposed row.
    fn scroll_up_preserving_bottom_wrap(&mut self, range: Range<usize>, mut n: usize, pen: &Pen) {
        n = n.min(range.end - range.start);

        if range.start == 0 {
            if range.end == self.height {
                self.extend(n, self.width, pen);
            } else {
                let row = Row::blank(self.width, *pen);
                let index = self.rows.len() - self.height + range.end;

                for _ in 0..n {
                    self.rows.insert(index, row.clone());
                }
            }
        } else {
            self[range.start - 1].set_wraps_to_next(false);
            let end = range.end;
            self.visible_rows_mut(range).rotate_left(n);
            self.clear((end - n)..end, pen);
        }
    }

    /// Counts visible rows through the last occupied row or soft-wrap terminator.
    /// `continues_from_previous` indicates a continuation entering the first visible row.
    pub fn occupied_prefix_len(&self, mut continues_from_previous: bool) -> usize {
        let mut end = 0;

        for (row_index, row) in self.visible_rows().enumerate() {
            if continues_from_previous || row.wraps_to_next() || !row.is_blank() {
                end = row_index + 1;
            }

            continues_from_previous = row.wraps_to_next();
        }

        end
    }

    /// Moves the occupied screen prefix into history, including blank rows that
    /// finish a soft-wrapped line. Trailing ordinary blank rows stay on screen.
    pub fn scroll_out(&mut self, pen: &Pen) {
        let continues_from_previous = self
            .view_offset()
            .checked_sub(1)
            .map_or(self.starts_with_continuation, |row_index| {
                self.rows[row_index].wraps_to_next()
            });

        let end = self.occupied_prefix_len(continues_from_previous);

        if end > 0 {
            self.scroll_up(0..self.height, end, pen);
        }
    }

    pub fn scroll_down(&mut self, range: Range<usize>, mut n: usize, pen: &Pen) {
        let (start, end) = (range.start, range.end);
        n = n.min(end - start);
        self.visible_rows_mut(range).rotate_right(n);
        self.clear(start..start + n, pen);

        if start > 0 {
            self[start - 1].set_wraps_to_next(false);
        }

        self[end - 1].set_wraps_to_next(false);
    }

    pub fn resize(
        &mut self,
        new_cols: usize,
        new_rows: usize,
        mut cursor: VisualPosition,
        reflow_enabled: bool,
        saved: &mut Option<VisualPosition>,
    ) -> VisualPosition {
        debug_assert!(cursor.0 <= self.width && cursor.1 < self.height);

        let old_cols = self.width;
        let mut old_rows = self.height;

        let mut saved_absolute =
            saved.map(|(col, row_index)| (col, self.view_offset() + row_index));

        if new_cols != old_cols && reflow_enabled {
            let positions = [
                Some((cursor.0, self.view_offset() + cursor.1)),
                saved_absolute,
            ]
            .map(|position| {
                position.map(|(col, buffer_index)| {
                    (
                        ReflowCursor {
                            col: col.min(old_cols - 1),
                            after: col == old_cols,
                        },
                        buffer_index,
                    )
                })
            });

            let (rows, positions) = reflow(self.rows.drain(..), new_cols, positions);
            let position = positions[0].expect("live cursor must survive reflow");
            saved_absolute = positions[1];
            self.rows = rows;
            let row_count = self.rows.len();

            if row_count < old_rows {
                self.extend(old_rows - row_count, new_cols, &Pen::default());
            }

            let relative_row = position.1 as isize - (self.rows.len() - old_rows) as isize;
            cursor.0 = position.0;

            if relative_row >= 0 {
                cursor.1 = relative_row as usize;
            } else {
                cursor.1 = 0;
                old_rows += (-relative_row) as usize;
            }
        } else if new_cols != old_cols {
            for row in &mut self.rows {
                row.resize(new_cols);
            }

            cursor.0 = cursor.0.min(new_cols - 1);
        }

        let row_count = self.rows.len();
        debug_assert!(old_rows <= row_count);

        match new_rows.cmp(&old_rows) {
            Ordering::Less => {
                let height_delta = old_rows - new_rows;
                let inverted_cursor_row = old_rows - 1 - cursor.1;
                let excess = height_delta.min(inverted_cursor_row);

                if excess > 0 {
                    self.rows.truncate(row_count - excess);
                    self.rows.back_mut().unwrap().set_wraps_to_next(false);
                }

                cursor.1 -= height_delta - excess;
            }

            Ordering::Greater => {
                let mut height_delta = new_rows - old_rows;
                let scrollback_size = row_count - old_rows;
                let cursor_row_shift = scrollback_size.min(height_delta);
                height_delta -= cursor_row_shift;

                cursor.1 += cursor_row_shift;

                if height_delta > 0 {
                    self.extend(height_delta, new_cols, &Pen::default());
                }
            }

            Ordering::Equal => (),
        }

        self.width = new_cols;
        self.height = new_rows;

        if reflow_enabled {
            *saved = saved_absolute.map(|(col, buffer_index)| {
                if (self.view_offset()..self.rows.len()).contains(&buffer_index) {
                    (col, buffer_index - self.view_offset())
                } else {
                    // A saved cursor never changes the viewport chosen for the live one.
                    (0, 0)
                }
            });
        }

        cursor
    }

    pub fn visible_rows(&self) -> impl ExactSizeIterator<Item = &Row> {
        self.rows.iter().skip(self.view_offset())
    }

    pub fn all_rows(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter()
    }

    fn view_offset(&self) -> usize {
        self.rows.len() - self.height
    }

    fn visible_rows_mut(&mut self, range: Range<usize>) -> &mut [Row] {
        let offset = self.view_offset();

        &mut self.rows.make_contiguous()[offset + range.start..offset + range.end]
    }

    #[inline]
    pub fn pop_evicted(&mut self) -> Option<Row> {
        if self
            .scrollback_limit
            .is_some_and(|limit| self.rows.len() - self.height > limit)
        {
            let row = self.rows.pop_front()?;
            self.starts_with_continuation = row.wraps_to_next();

            return Some(row);
        }

        None
    }

    fn clear(&mut self, range: Range<usize>, pen: &Pen) {
        let offset = self.view_offset();

        for row in self
            .rows
            .range_mut(offset + range.start..offset + range.end)
        {
            row.reset(*pen);
        }
    }

    fn extend(&mut self, n: usize, cols: usize, pen: &Pen) {
        let row = Row::blank(cols, *pen);
        let filler = core::iter::repeat_n(row, n);
        self.rows.extend(filler);
    }

    #[cfg(test)]
    pub(crate) fn assert_execution_equivalent(&self, other: &Self) {
        assert_eq!((self.width, self.height), (other.width, other.height));
        assert_eq!(self.scrollback_limit, other.scrollback_limit);
        assert_eq!(
            self.starts_with_continuation,
            other.starts_with_continuation
        );
        // Row equality excludes tracking registrations and allocation layout.
        assert_eq!(self.rows, other.rows);
    }

    #[cfg(test)]
    pub fn add_scrollback(&mut self, n: usize) {
        let mut row = Row::blank(self.width, Pen::default());

        for col in 0..self.width {
            row.replace_char(col, 's', Pen::default());
        }

        for _ in 0..n {
            self.rows.insert(0, row.clone());
        }
    }
}

impl Index<usize> for Buffer {
    type Output = Row;

    fn index(&self, index: usize) -> &Self::Output {
        &self.rows[self.view_offset() + index]
    }
}

impl Index<VisualPosition> for Buffer {
    type Output = Cell;

    fn index(&self, (col, row_index): VisualPosition) -> &Self::Output {
        &self.rows[self.view_offset() + row_index][col]
    }
}

impl IndexMut<usize> for Buffer {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        let buffer_index = self.view_offset() + index;

        &mut self.rows[buffer_index]
    }
}

struct Reflow<I>
where
    I: Iterator<Item = (Row, ReflowCursors)>,
{
    pub iter: I,
    pub cols: usize,
    pub rest: Option<(Row, ReflowCursors)>,
}

fn reflow(
    iter: impl Iterator<Item = Row>,
    cols: usize,
    mut cursors: [Option<(ReflowCursor, usize)>; 2],
) -> (VecDeque<Row>, [Option<VisualPosition>; 2]) {
    let iter = iter.enumerate().map(|(buffer_index, row)| {
        let tracked = core::array::from_fn(|i| {
            if cursors[i]
                .as_ref()
                .is_some_and(|(_, cursor_row)| *cursor_row == buffer_index)
            {
                cursors[i].take().map(|(cursor, _)| cursor)
            } else {
                None
            }
        });

        (row, tracked)
    });

    let reflow = Reflow {
        iter,
        cols,
        rest: None,
    };

    let mut positions = [None; 2];

    let rows: VecDeque<Row> = reflow
        .enumerate()
        .map(|(buffer_index, (mut row, cursor))| {
            debug_assert_eq!(row.len(), cols);
            row.clip_marks();

            for (position, cursor) in positions.iter_mut().zip(cursor) {
                if let Some(mut c) = cursor {
                    c.clamp(cols);
                    *position = Some((c.col + usize::from(c.after), buffer_index));
                }
            }

            row
        })
        .collect();

    (rows, positions)
}

impl<I: Iterator<Item = (Row, ReflowCursors)>> Iterator for Reflow<I> {
    type Item = (Row, ReflowCursors);

    fn next(&mut self) -> Option<Self::Item> {
        use core::cmp::Ordering::*;

        while let Some((mut row, mut cursor)) = self.rest.take().or_else(|| self.iter.next()) {
            match self.cols.cmp(&row.len()) {
                Less => {
                    self.rest = row.split_for_reflow(self.cols, &mut cursor);

                    return Some((row, cursor));
                }

                Equal => {
                    return Some((row, cursor));
                }

                Greater => match self.iter.next() {
                    Some((next_row, next_cursor)) => {
                        self.rest =
                            row.append_for_reflow(next_row, self.cols, &mut cursor, next_cursor);

                        if row.len() == self.cols {
                            return Some((row, cursor));
                        }

                        self.rest = Some((row, cursor));
                    }

                    None => {
                        row.expand(self.cols);
                        row.set_wraps_to_next(false);

                        return Some((row, cursor));
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
    use crate::pen::Pen;
    use crate::row::Row;
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

        assert_eq!(row_text(&buf[0]), "aaaa⏎");
        assert_eq!(row_text(&buf[1]), "aa  ");
        assert_eq!(row_text(&buf[2]), "bb  ");
        assert_eq!(row_text(&buf[3]), "cccc⏎");
        assert_eq!(row_text(&buf[4]), "cccc⏎");
        assert_eq!(row_text(&buf[5]), "cc  ");
        assert_eq!(row_text(&buf[6]), "    ");
        assert_eq!(buf.text().join("\n"), "aaaaaaaaaa\nbb\ncccccccccc\n");
        assert_eq!(buf.rows.len(), 8);
        assert!(buf.rows[0].wraps_to_next());

        // top of the view

        let mut buf = buffer(&content, None, 0);

        buf.scroll_up(0..5, 1, &pen);

        assert_eq!(row_text(&buf[0]), "aaaa⏎");
        assert_eq!(row_text(&buf[1]), "aa  ");
        assert_eq!(row_text(&buf[2]), "bb  ");
        assert_eq!(row_text(&buf[3]), "cccc");
        assert_eq!(row_text(&buf[4]), "    ");
        assert_eq!(row_text(&buf[5]), "cccc⏎");
        assert_eq!(row_text(&buf[6]), "cc  ");
        assert_eq!(buf.text().join("\n"), "aaaaaaaaaa\nbb\ncccc\n\ncccccc");
        assert_eq!(buf.rows.len(), 8);
        assert!(buf.rows[0].wraps_to_next());

        // bottom of the view

        let mut buf = buffer(&content, None, 0);

        buf.scroll_up(1..content.len(), 1, &pen);

        assert_eq!(row_text(&buf[0]), "aaaa");
        assert_eq!(row_text(&buf[1]), "aa  ");
        assert_eq!(row_text(&buf[2]), "bb  ");
        assert_eq!(row_text(&buf[3]), "cccc⏎");
        assert_eq!(row_text(&buf[4]), "cccc⏎");
        assert_eq!(row_text(&buf[5]), "cc  ");
        assert_eq!(row_text(&buf[6]), "    ");
        assert_eq!(buf.text().join("\n"), "aaaa\naa\nbb\ncccccccccc\n");
        assert_eq!(buf.rows.len(), 7);

        // no scrollback limit

        let mut buf = buffer(&content, None, 0);

        buf.scroll_up(0..content.len(), 5, &pen);

        assert_eq!(buf.rows.len(), 12);

        // scrollback limit of 0

        let mut buf = buffer(&content, Some(0), 0);

        buf.scroll_up(0..content.len(), 5, &pen);

        assert_eq!(buf.rows.len(), 12);

        while buf.pop_evicted().is_some() {}

        assert_eq!(buf.rows.len(), 7);

        // scrollback limit of 3

        let mut buf = buffer(&content, Some(3), 0);

        buf.scroll_up(0..content.len(), 5, &pen);

        assert_eq!(buf.rows.len(), 12);

        while buf.pop_evicted().is_some() {}

        assert_eq!(buf.rows.len(), 10);
    }

    fn row_text(row: &Row) -> String {
        let mut t = row.text();

        if row.wraps_to_next() {
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

        // cursor in the middle, no scrollback, last rows wrapped

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

        let view = buffer
            .visible_rows()
            .map(|row| row.text())
            .collect::<Vec<_>>();

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

        for (row_index, (row_text, wraps_to_next)) in content.iter().enumerate() {
            for (col, ch) in row_text.chars().enumerate() {
                buffer.replace_char((col, row_index), ch, Pen::default());
            }

            if *wraps_to_next {
                buffer.wrap(row_index);
            }
        }

        buffer
    }
}
