use crate::cell::Cell;
use crate::dump::Dump;
use crate::line::Line;
use crate::pen::Pen;
use std::cmp::Ordering;
use std::ops::{Index, IndexMut, Range};

#[derive(Debug)]
pub(crate) struct Buffer {
    lines: Vec<Line>,
    pub cols: usize,
    pub rows: usize,
    scrollback_limit: Option<ScrollbackLimit>,
    trim_needed: bool,
}

#[derive(Debug)]
struct ScrollbackLimit {
    soft: usize,
    hard: usize,
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
        let mut lines = vec![Line::blank(cols, *pen); rows];

        if let Some(limit) = scrollback_limit {
            if limit > 0 {
                lines.reserve(limit);
            }
        } else {
            lines.reserve(1000);
        }

        let scrollback_limit = scrollback_limit.map(|l| ScrollbackLimit {
            soft: l,
            hard: l + l / 10, // 10% bigger than soft
        });

        Buffer {
            lines,
            cols,
            rows,
            scrollback_limit,
            trim_needed: false,
        }
    }

    pub fn text(&self) -> Vec<String> {
        let mut text = Vec::new();
        let mut current = String::new();

        for line in &self.lines {
            current.push_str(&line.text());

            if !line.wrapped {
                text.push(current.trim_end().to_owned());
                current.clear();
            }
        }

        if !current.is_empty() {
            text.push(current.trim_end().to_owned());
        }

        text
    }

    pub fn print(&mut self, (col, row): VisualPosition, cell: Cell) {
        self[row].print(col, cell);
    }

    pub fn wrap(&mut self, row: usize) {
        self[row].wrapped = true;
    }

    pub fn insert(&mut self, (col, row): VisualPosition, mut n: usize, cell: Cell) {
        n = n.min(self.cols - col);
        self[row].insert(col, n, cell);
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

    pub fn scroll_up(&mut self, range: Range<usize>, mut n: usize, pen: &Pen) {
        n = n.min(range.end - range.start);

        if range.end - 1 < self.rows - 1 {
            self[range.end - 1].wrapped = false;
        }

        if range.start == 0 {
            if range.end == self.rows {
                self.extend(n, self.cols);
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
            self[range].rotate_left(n);
            self.clear((end - n)..end, pen);
        }

        self.trim_needed = true;
    }

    pub fn scroll_down(&mut self, range: Range<usize>, mut n: usize, pen: &Pen) {
        let (start, end) = (range.start, range.end);
        n = n.min(end - start);
        self[range].rotate_right(n);
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
        let old_cols = self.cols;
        let mut old_rows = self.rows;
        let cursor_log_pos = self.logical_position(cursor, old_cols, old_rows);

        if new_cols != old_cols {
            self.lines = reflow(self.lines.drain(..), new_cols);
            let line_count = self.lines.len();

            if line_count < old_rows {
                self.extend(old_rows - line_count, new_cols);
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
                    self.lines.last_mut().unwrap().wrapped = false;
                }

                cursor.1 -= height_delta - excess;
            }

            Ordering::Greater => {
                let mut height_delta = new_rows - old_rows;
                let scrollback_size = line_count - old_rows.min(line_count);
                let cursor_row_shift = scrollback_size.min(height_delta);
                height_delta -= cursor_row_shift;

                if cursor.1 < old_rows {
                    cursor.1 += cursor_row_shift;
                }

                if height_delta > 0 {
                    self.extend(height_delta, new_cols);
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
        let last_available_row = abs_row.min(self.lines.len());
        let mut log_row = abs_row - last_available_row;

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

    pub fn view(&self) -> &[Line] {
        &self.lines[self.lines.len() - self.rows..]
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines[..]
    }

    pub fn gc(&mut self) -> Option<impl Iterator<Item = Line> + '_> {
        if self.trim_needed {
            self.trim_needed = false;
            self.trim_scrollback()
        } else {
            None
        }
    }

    fn view_mut(&mut self) -> &mut [Line] {
        let len = self.lines.len();
        &mut self.lines[len - self.rows..]
    }

    fn clear(&mut self, range: Range<usize>, pen: &Pen) {
        let line = Line::blank(self.cols, *pen);
        self.view_mut()[range].fill(line);
    }

    fn extend(&mut self, n: usize, cols: usize) {
        let line = Line::blank(cols, Pen::default());
        let filler = std::iter::repeat(line).take(n);
        self.lines.extend(filler);
    }

    fn trim_scrollback(&mut self) -> Option<impl Iterator<Item = Line> + '_> {
        if let Some(limit) = &self.scrollback_limit {
            let line_count = self.lines.len();
            let scrollback_size = line_count - self.rows;

            if scrollback_size > limit.hard {
                let excess = scrollback_size - limit.soft;
                return Some(self.lines.drain(..excess));
            }
        }

        None
    }

    #[cfg(test)]
    pub fn add_scrollback(&mut self, n: usize) {
        let mut line = Line::blank(self.cols, Pen::default());

        for col in 0..self.cols {
            line.print(col, 's'.into());
        }

        for _ in 0..n {
            self.lines.insert(0, line.clone());
        }
    }
}

impl Index<usize> for Buffer {
    type Output = Line;

    fn index(&self, index: usize) -> &Self::Output {
        &self.view()[index]
    }
}

impl Index<Range<usize>> for Buffer {
    type Output = [Line];

    fn index(&self, range: Range<usize>) -> &Self::Output {
        &self.view()[range]
    }
}

impl Index<VisualPosition> for Buffer {
    type Output = Cell;

    fn index(&self, (col, row): VisualPosition) -> &Self::Output {
        &self.view()[row][col]
    }
}

impl IndexMut<usize> for Buffer {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.view_mut()[index]
    }
}

impl IndexMut<Range<usize>> for Buffer {
    fn index_mut(&mut self, range: Range<usize>) -> &mut Self::Output {
        &mut self.view_mut()[range]
    }
}

impl Dump for Buffer {
    fn dump(&self) -> String {
        let last = self.rows - 1;

        self.view()
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let mut dump = line.dump();

                if i < last && !line.wrapped {
                    dump.push('\r');
                    dump.push('\n');
                }

                dump
            })
            .collect()
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

pub(crate) fn reflow<I: Iterator<Item = Line>>(iter: I, cols: usize) -> Vec<Line> {
    let lines: Vec<Line> = Reflow {
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
                    Some(next_line) => match line.extend(next_line, self.cols) {
                        (true, Some(rest)) => {
                            self.rest = Some(rest);
                            return Some(line);
                        }

                        (true, None) => {
                            return Some(line);
                        }

                        (false, _) => {
                            self.rest = Some(line);
                        }
                    },

                    None => {
                        line.expand(self.cols, &Pen::default());
                        line.wrapped = false;
                        return Some(line);
                    }
                },
            }
        }

        self.rest.take().map(|mut line| {
            line.expand(self.cols, &Pen::default());
            line.wrapped = false;

            line
        })
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
        let cell = 'x'.into();

        assert_eq!(buffer.text(), vec!["", "", "", "", ""]);

        buffer.print((0, 0), cell);
        buffer.print((1, 1), cell);
        buffer.print((2, 2), cell);
        buffer.print((3, 3), cell);
        buffer.print((4, 4), cell);
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

        buf.gc();

        assert_eq!(buf.lines.len(), 7);

        // scrollback limit of 3

        let mut buf = buffer(&content, Some(3), 0);

        buf.scroll_up(0..content.len(), 5, &pen);

        assert_eq!(buf.lines.len(), 12);

        buf.gc();

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

        // cursor below last row

        for scrollback in [0, 20] {
            let (_, cursor) = resize_buffer(scrollback, content.clone(), 4, 8, (2, 6));

            assert_eq!(cursor, (2, 6));
        }
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

        let view = buffer
            .view()
            .iter()
            .map(|line| line.text())
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
        let mut buffer = Buffer::new(cols, rows, scrollback_limit, None);

        if !matches!(scrollback_limit, Some(0)) {
            buffer.add_scrollback(scrollback_size);
        }

        for (row, (line, wrapped)) in content.iter().enumerate() {
            for (col, ch) in line.chars().enumerate() {
                buffer.print((col, row), ch.into());
            }

            if *wrapped {
                buffer.wrap(row);
            }
        }

        buffer
    }
}
