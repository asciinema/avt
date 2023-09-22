use crate::cell::Cell;
use crate::dump::Dump;
use crate::line::{self, Line};
use crate::pen::Pen;
use std::collections::HashSet;
use std::ops::{Index, Range};

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

    pub fn lines(&self) -> impl Iterator<Item = &Line> {
        self.lines.iter()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn print(&mut self, (col, row): Cursor, cell: Cell) {
        self.lines[row].print(col, cell);
    }

    pub fn wrap(&mut self, row: usize) {
        self.lines[row].wrapped = true;
    }

    pub fn insert(&mut self, (col, row): Cursor, mut n: usize, cell: Cell) {
        n = n.min(self.cols - col);
        self.lines[row].insert(col, n, cell);
    }

    pub fn delete(&mut self, (col, row): Cursor, mut n: usize, pen: &Pen) {
        n = n.min(self.cols - col);
        self.lines[row].delete(col, n, pen);
        self.lines[row].wrapped = false;
    }

    pub fn erase(&mut self, (col, row): Cursor, mode: EraseMode, pen: &Pen) {
        use EraseMode::*;

        match mode {
            NextChars(mut n) => {
                n = n.min(self.cols - col);
                let end = col + n;
                self.lines[row].clear(col..end, pen);

                if end == self.cols {
                    self.lines[row].wrapped = false;
                }
            }

            FromCursorToEndOfView => {
                self.lines[row].clear(col..self.cols, pen);
                self.clear_lines((row + 1)..self.rows, pen);
                self.lines[row].wrapped = false;
            }

            FromStartOfViewToCursor => {
                self.lines[row].clear(0..(col + 1).min(self.cols), pen);
                self.clear_lines(0..row, pen);
            }

            WholeView => {
                self.clear_lines(0..self.rows, pen);
            }

            FromCursorToEndOfLine => {
                self.lines[row].clear(col..self.cols, pen);
                self.lines[row].wrapped = false;
            }

            FromStartOfLineToCursor => {
                self.lines[row].clear(0..(col + 1).min(self.cols), pen);
            }

            WholeLine => {
                self.lines[row].clear(0..self.cols, pen);
                self.lines[row].wrapped = false;
            }
        }
    }

    pub fn scroll_up(&mut self, range: Range<usize>, mut n: usize, pen: &Pen) {
        n = n.min(range.end - range.start);

        if range.start > 0 {
            self.lines[range.start - 1].wrapped = false;
        }

        if range.end - 1 < self.rows - 1 {
            self.lines[range.end - 1].wrapped = false;
        }

        let end = range.end;
        self.lines[range].rotate_left(n);
        self.clear_lines((end - n)..end, pen);
    }

    pub fn scroll_down(&mut self, range: Range<usize>, mut n: usize, pen: &Pen) {
        n = n.min(range.end - range.start);
        self.lines[range.clone()].rotate_right(n);
        self.clear_lines(range.start..range.start + n, pen);

        if range.start > 0 {
            self.lines[range.start - 1].wrapped = false;
        }

        self.lines[range.end - 1].wrapped = false;
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

    fn clear_lines(&mut self, range: Range<usize>, pen: &Pen) {
        self.lines[range].fill(Line::blank(self.cols, *pen));
    }
}

impl Index<usize> for Buffer {
    type Output = Line;

    fn index(&self, index: usize) -> &Self::Output {
        &self.lines[index]
    }
}

impl Index<Cursor> for Buffer {
    type Output = Cell;

    fn index(&self, (col, row): Cursor) -> &Self::Output {
        &self.lines[row][col]
    }
}

impl Dump for Buffer {
    fn dump(&self) -> String {
        let last = self.lines.len() - 1;

        self.lines
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
