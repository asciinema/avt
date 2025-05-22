use unicode_width::UnicodeWidthChar;

use crate::cell::Cell;
use crate::pen::Pen;
use std::ops::{Index, Range, RangeFull};

#[derive(Clone, PartialEq)]
pub struct Line {
    pub(crate) cells: Vec<Cell>,
    pub(crate) wrapped: bool,
}

impl Line {
    pub(crate) fn blank(cols: usize, pen: Pen) -> Self {
        Line {
            cells: vec![Cell::blank(pen); cols],
            wrapped: false,
        }
    }

    pub(crate) fn clear(&mut self, range: Range<usize>, pen: &Pen) {
        if range.start == self.len() {
            return;
        }

        let start_col = range.start;
        let end_col = range.end;

        if self.cells[start_col].width() == 0 {
            self.cells[start_col - 1].set(' ', 1, *pen);
        }

        self.cells[range].fill(Cell::blank(*pen));

        if let Some(next_cell) = self.cells.get_mut(end_col) {
            if next_cell.width() == 0 {
                next_cell.set(' ', 1, *pen);
            }
        }
    }

    pub(crate) fn print(&mut self, col: usize, ch: char, pen: Pen) -> usize {
        let cell_width = self.cells[col].width();
        let char_width = self.char_display_width(ch);
        let remaining_cols = self.len() as isize - 1 - col as isize;

        match (cell_width, char_width, remaining_cols) {
            (1, 1, _) => {
                self.cells[col].set(ch, 1, pen);
            }

            (1, 2, 0) => {
                self.cells[col].set(' ', 1, pen);
                return 0;
            }

            (1, 2, 1) => {
                debug_assert!(self.cells[col + 1].width() < 2);

                self.cells[col].set(ch, 2, pen);
                self.cells[col + 1].set(' ', 0, pen);
            }

            (1, 2, _right) => {
                self.cells[col].set(ch, 2, pen);

                if self.cells[col + 1].width() == 2 {
                    self.cells[col + 2].set(' ', 1, pen);
                }

                self.cells[col + 1].set(' ', 0, pen);
            }

            (2, 1, right) => {
                debug_assert!(right >= 1);
                debug_assert!(self.cells[col + 1].width() == 0);

                self.cells[col].set(ch, 1, pen);
                self.cells[col + 1].set(' ', 1, pen);
            }

            (2, 2, right) => {
                debug_assert!(right >= 1);
                debug_assert!(self.cells[col + 1].width() == 0);

                self.cells[col].set(ch, 2, pen);
                self.cells[col + 1].set(' ', 0, pen);
            }

            (0, 1, _right) => {
                debug_assert!(col > 0);
                debug_assert!(self.cells[col - 1].width() == 2);

                self.cells[col - 1].set(' ', 1, pen);
                self.cells[col].set(ch, 1, pen);
            }

            (0, 2, 0) => {
                debug_assert!(col > 0);
                debug_assert!(self.cells[col - 1].width() == 2);

                return 0;
            }

            (0, 2, 1) => {
                debug_assert!(col > 0);
                debug_assert!(self.cells[col - 1].width() == 2);

                self.cells[col + 1].set(' ', 1, pen);
                return 0;
            }

            (0, 2, _right) => {
                debug_assert!(col > 0);
                debug_assert!(self.cells[col - 1].width() == 2);

                self.cells[col - 1].set(' ', 1, pen);
                self.cells[col].set(ch, 2, pen);

                if self.cells[col + 1].width() == 2 {
                    self.cells[col + 2].set(' ', 1, pen);
                }

                self.cells[col + 1].set(' ', 0, pen);
            }

            _ => {
                unreachable!();
            }
        }

        char_width
    }

    pub(crate) fn shift_right(&mut self, col: usize, n: usize, pen: Pen) {
        let col = col.min(self.len() - 1);
        let cur_cell = &mut self.cells[col];

        if cur_cell.width() == 0 {
            cur_cell.set(' ', 1, pen);
            self.cells[col - 1].set(' ', 1, pen);
        }

        self.cells[col..].rotate_right(n);

        let cur_cell = &mut self.cells[col];

        if cur_cell.width() == 0 {
            cur_cell.set(' ', 1, pen);
            self.cells.last_mut().unwrap().set(' ', 1, pen);
        }
    }

    pub(crate) fn delete(&mut self, col: usize, n: usize, pen: &Pen) {
        if self.cells[col].width() == 0 {
            self.cells[col - 1].set(' ', 1, *pen);
        }

        self.cells[col..].rotate_left(n);

        let cur_cell = &mut self.cells[col];

        if cur_cell.width() == 0 {
            cur_cell.set(' ', 1, *pen);
        }

        let fill_start = self.cells.len() - n;
        self.cells[fill_start..].fill(Cell::blank(*pen));
    }

    pub(crate) fn extend(&mut self, mut other: Line, len: usize) -> (bool, Option<Line>) {
        let mut needed = len - self.len();

        if needed == 0 {
            return (true, Some(other));
        }

        if !self.wrapped {
            self.expand(len, &Pen::default());

            return (true, Some(other));
        }

        if !other.wrapped {
            other.trim();
        }

        if needed < other.len() {
            if other[needed].width() == 0 {
                needed -= 1;
                let pen = self.cells[self.len() - 1].pen();
                self.cells.push(Cell::new(' ', 1, *pen));
            }

            self.cells.extend(&other[0..needed]);
            let mut cells = other.cells;
            cells.rotate_left(needed);
            cells.truncate(cells.len() - needed);

            return (
                true,
                Some(Line {
                    cells,
                    wrapped: other.wrapped,
                }),
            );
        }

        self.cells.extend(&other[..]);

        if !other.wrapped {
            self.wrapped = false;

            if self.len() < len {
                self.expand(len, &Pen::default());
            }

            (true, None)
        } else {
            (false, None)
        }
    }

    pub(crate) fn expand(&mut self, len: usize, pen: &Pen) {
        let tpl = Cell::blank(*pen);
        let filler = std::iter::repeat(tpl).take(len - self.len());
        self.cells.extend(filler);
    }

    pub(crate) fn contract(&mut self, mut len: usize) -> Option<Line> {
        if !self.wrapped {
            let trimmed_len = self.len() - self.trailers();
            self.cells.truncate(len.max(trimmed_len));
        }

        if self.len() > len {
            let wide_char_boundary = self.cells[len].width() == 0;

            if wide_char_boundary {
                len -= 1;
            }

            let mut rest = Line {
                cells: self.cells.split_off(len),
                wrapped: self.wrapped,
            };

            if wide_char_boundary {
                let pen = self.cells[self.cells.len() - 1].pen();
                self.cells.push(Cell::new(' ', 1, *pen));
            }

            if !self.wrapped {
                rest.trim();
            }

            if rest.cells.is_empty() {
                None
            } else {
                self.wrapped = true;

                Some(rest)
            }
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn chunks<'a>(
        &'a self,
        predicate: impl Fn(&Cell, &Cell) -> bool + 'a,
    ) -> impl Iterator<Item = Vec<Cell>> + '_ {
        let i = self.cells.iter().filter(|c| c.width() > 0);
        Chunks::new(i, predicate)
    }

    pub fn chars(&self) -> impl Iterator<Item = char> + '_ {
        self.cells.iter().map(Cell::char)
    }

    pub fn text(&self) -> String {
        self.chars().collect()
    }

    fn trim(&mut self) {
        let trailers = self.trailers();

        if trailers > 0 {
            self.cells.truncate(self.len() - trailers);
        }
    }

    fn trailers(&self) -> usize {
        self.cells
            .iter()
            .rev()
            .take_while(|cell| cell.is_default())
            .count()
    }

    pub(crate) fn is_blank(&self) -> bool {
        self.cells.iter().all(|c| c.is_default())
    }

    fn char_display_width(&self, ch: char) -> usize {
        if ch.width().unwrap_or(1) == 2 {
            2
        } else {
            1
        }
    }
}

impl std::fmt::Debug for Line {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = String::new();

        for cells in self.chunks(|c1, c2| c1.pen() != c2.pen()) {
            s.push_str(&cells[0].pen().dump());

            for cell in cells {
                if cell.width() > 0 {
                    s.push(cell.char());
                }
            }
        }

        if self.wrapped {
            s.push('⏎');
        }

        write!(f, "{:?}", s)
    }
}

struct Chunks<'a, I, F>
where
    I: Iterator<Item = &'a Cell>,
    F: Fn(&Cell, &Cell) -> bool,
{
    iter: I,
    predicate: F,
    cells: Vec<Cell>,
}

impl<'a, I: Iterator<Item = &'a Cell>, F: Fn(&Cell, &Cell) -> bool> Chunks<'a, I, F> {
    fn new(iter: I, predicate: F) -> Self {
        Self {
            iter,
            predicate,
            cells: Vec::new(),
        }
    }
}

impl<'a, I: Iterator<Item = &'a Cell>, F: Fn(&Cell, &Cell) -> bool> Iterator for Chunks<'a, I, F> {
    type Item = Vec<Cell>;

    fn next(&mut self) -> Option<Self::Item> {
        for cell in self.iter.by_ref() {
            if self.cells.is_empty() {
                self.cells.push(*cell);
                continue;
            }

            if (self.predicate)(self.cells.last().unwrap(), cell) {
                let cells = std::mem::take(&mut self.cells);
                self.cells.push(*cell);
                return Some(cells);
            } else {
                self.cells.push(*cell);
            }
        }

        if self.cells.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.cells))
        }
    }
}

impl Index<usize> for Line {
    type Output = Cell;

    fn index(&self, index: usize) -> &Self::Output {
        &self.cells[index]
    }
}

impl Index<Range<usize>> for Line {
    type Output = [Cell];

    fn index(&self, range: Range<usize>) -> &Self::Output {
        &self.cells[range]
    }
}

impl Index<RangeFull> for Line {
    type Output = [Cell];

    fn index(&self, range: RangeFull) -> &Self::Output {
        &self.cells[range]
    }
}

#[cfg(test)]
mod tests {
    use super::{Cell, Chunks, Pen};

    fn chars(cells: &[Cell]) -> Vec<char> {
        cells.iter().map(|c| c.char()).collect()
    }

    #[test]
    fn chunks() {
        let pen = Pen::default();

        let cells = [
            Cell::new('0', 1, pen),
            Cell::new('a', 1, pen),
            Cell::new('b', 1, pen),
            Cell::new('C', 1, pen),
            Cell::new('D', 1, pen),
            Cell::new('E', 1, pen),
            Cell::new('1', 1, pen),
            Cell::new('F', 1, pen),
            Cell::new('g', 1, pen),
        ];

        let chunks: Vec<Vec<Cell>> = Chunks::new(cells.iter(), |c1, c2| {
            c1.char().is_ascii_digit()
                || c2.char().is_ascii_digit()
                || (c1.char().is_lowercase() && c2.char().is_uppercase())
                || (c1.char().is_uppercase() && c2.char().is_lowercase())
        })
        .collect();

        assert_eq!(&chars(&chunks[0]), &['0']);
        assert_eq!(&chars(&chunks[1]), &['a', 'b']);
        assert_eq!(&chars(&chunks[2]), &['C', 'D', 'E']);
        assert_eq!(&chars(&chunks[3]), &['1']);
        assert_eq!(&chars(&chunks[4]), &['F']);
        assert_eq!(&chars(&chunks[5]), &['g']);
    }
}
