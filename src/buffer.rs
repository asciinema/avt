use crate::cell::Cell;
use crate::dump::Dump;
use crate::line::{self, Line};
use crate::pen::Pen;
use std::collections::HashSet;
use std::ops::{Index, IndexMut, Range};

#[derive(Debug)]
pub(crate) struct Buffer {
    lines: Vec<Line>,
    pub cols: usize,
    pub rows: usize,
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

type Cursor = (usize, usize);

impl Buffer {
    pub fn new(cols: usize, rows: usize) -> Self {
        let lines = vec![Line::blank(cols, Pen::default()); rows];

        Buffer { lines, cols, rows }
    }

    pub fn len(&self) -> usize {
        self.lines.len()
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

    pub fn print(&mut self, (col, row): Cursor, cell: Cell) {
        self[row].print(col, cell);
    }

    pub fn wrap(&mut self, row: usize) {
        self[row].wrapped = true;
    }

    pub fn insert(&mut self, (col, row): Cursor, mut n: usize, cell: Cell) {
        n = n.min(self.cols - col);
        self[row].insert(col, n, cell);
    }

    pub fn delete(&mut self, (col, row): Cursor, mut n: usize, pen: &Pen) {
        n = n.min(self.cols - col);
        let line = &mut self[row];
        line.delete(col, n, pen);
        line.wrapped = false;
    }

    pub fn erase(&mut self, (col, row): Cursor, mode: EraseMode, pen: &Pen) {
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

        if range.start > 0 {
            self[range.start - 1].wrapped = false;
        }

        if range.end - 1 < self.rows - 1 {
            self[range.end - 1].wrapped = false;
        }

        let end = range.end;
        self[range].rotate_left(n);
        self.clear((end - n)..end, pen);
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
        (mut cursor_col, mut cursor_row): Cursor,
    ) -> (Cursor, HashSet<usize>) {
        let mut dirty_lines = HashSet::new();
        let old_cols = self.cols;
        self.cols = new_cols;
        self.rows = new_rows;

        match self.cols.cmp(&old_cols) {
            std::cmp::Ordering::Less => {
                let rel_cursor = self.rel_cursor((cursor_col, cursor_row), old_cols);
                self.lines = line::reflow(self.lines.drain(..), self.cols);
                (cursor_col, cursor_row) = self.abs_cursor(rel_cursor, self.cols);
                let rows = self.lines.len();

                if rows > new_rows {
                    let added = rows - new_rows;
                    self.lines.rotate_left(added);
                    self.lines.truncate(new_rows);
                }

                dirty_lines.extend(0..new_rows);
            }

            std::cmp::Ordering::Equal => (),

            std::cmp::Ordering::Greater => {
                let rel_cursor = self.rel_cursor((cursor_col, cursor_row), old_cols);
                self.lines = line::reflow(self.lines.drain(..), self.cols);
                (cursor_col, cursor_row) = self.abs_cursor(rel_cursor, self.cols);
                let rows = self.lines.len();

                if rows < new_rows {
                    let line = Line::blank(self.cols, Pen::default());
                    let filler = std::iter::repeat(line).take(new_rows - rows);
                    self.lines.extend(filler);
                }

                dirty_lines.extend(0..new_rows);
            }
        }

        let rows = self.lines.len();

        match new_rows.cmp(&rows) {
            std::cmp::Ordering::Less => {
                let decrement = rows - new_rows;
                let rot = decrement - decrement.min(rows - cursor_row - 1);

                if rot > 0 {
                    self.lines.rotate_left(rot);
                    dirty_lines.extend(0..new_rows);
                }

                self.lines.truncate(new_rows);
            }

            std::cmp::Ordering::Equal => (),

            std::cmp::Ordering::Greater => {
                let increment = new_rows - rows;
                let line = Line::blank(self.cols, Pen::default());
                let filler = std::iter::repeat(line).take(increment);
                self.lines.extend(filler);

                dirty_lines.extend(rows..new_rows);
            }
        }

        ((cursor_col, cursor_row), dirty_lines)
    }

    pub fn rel_cursor(&self, (abs_col, abs_row): Cursor, cols: usize) -> (usize, usize) {
        let mut rel_col = abs_col;
        let mut rel_row = 0;
        let mut r = self.lines.len() - 1;

        while r > abs_row {
            if !self.lines[r - 1].wrapped {
                rel_row += 1;
            }

            r -= 1;
        }

        while r > 0 && self.lines[r - 1].wrapped {
            rel_col += cols;
            r -= 1;
        }

        (rel_col, rel_row)
    }

    pub fn abs_cursor(&self, (rel_col, rel_row): (usize, usize), cols: usize) -> Cursor {
        let mut abs_col = rel_col;
        let mut abs_row = self.lines.len() - 1;
        let mut r = 0;

        while r < rel_row && abs_row > 0 {
            if !self.lines[abs_row - 1].wrapped {
                r += 1;
            }

            abs_row -= 1;
        }

        while abs_row > 0 && self.lines[abs_row - 1].wrapped {
            abs_row -= 1;
        }

        while abs_col >= cols && self.lines[abs_row].wrapped {
            abs_col -= cols;
            abs_row += 1;
        }

        abs_col = abs_col.min(cols - 1);

        if self.lines.len() > self.rows {
            abs_row = self.rows - (self.lines.len() - abs_row);
        }

        (abs_col, abs_row)
    }

    pub fn view(&self) -> &[Line] {
        &self.lines[self.lines.len() - self.rows..]
    }

    fn view_mut(&mut self) -> &mut [Line] {
        let len = self.lines.len();
        &mut self.lines[len - self.rows..]
    }

    fn clear(&mut self, range: Range<usize>, pen: &Pen) {
        let line = Line::blank(self.cols, *pen);
        self.view_mut()[range].fill(line);
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

impl Index<Cursor> for Buffer {
    type Output = Cell;

    fn index(&self, (col, row): Cursor) -> &Self::Output {
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

#[cfg(test)]
mod tests {
    use super::Buffer;
    use crate::cell::Cell;
    use crate::pen::Pen;
    use pretty_assertions::assert_eq;

    #[test]
    fn text() {
        let mut buffer = Buffer::new(10, 5);
        let cell = Cell('x', Pen::default());

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
}
