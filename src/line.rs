use unicode_width::UnicodeWidthChar;

use crate::cell::{Cell, Occupancy};
use crate::pen::Pen;
use std::ops::{Index, Range, RangeFull};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum CharWidth {
    Single,
    Double,
}

impl CharWidth {
    fn as_usize(self) -> usize {
        match self {
            CharWidth::Single => 1,
            CharWidth::Double => 2,
        }
    }
}

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

    pub(crate) fn reset(&mut self, cols: usize, pen: Pen) {
        self.cells.clear();
        self.cells.resize(cols, Cell::blank(pen));
        self.wrapped = false;
    }

    pub(crate) fn clear(&mut self, range: Range<usize>, pen: &Pen) {
        if range.start == self.len() {
            return;
        }

        let start_col = range.start;
        let end_col = range.end;

        if self.cells[start_col].occupancy() == Occupancy::WideTail {
            self.cells[start_col - 1].set(' ', Occupancy::Single, *pen);
        }

        self.cells[range].fill(Cell::blank(*pen));

        if let Some(next_cell) = self.cells.get_mut(end_col) {
            if next_cell.occupancy() == Occupancy::WideTail {
                next_cell.set(' ', Occupancy::Single, *pen);
            }
        }
    }

    pub(crate) fn print(&mut self, col: usize, ch: char, pen: Pen) -> Option<usize> {
        let cell_occupancy = self.cells[col].occupancy();
        let char_width = self.char_display_width(ch);
        let remaining_cols = self.len() as isize - 1 - col as isize;

        match (cell_occupancy, char_width, remaining_cols) {
            (Occupancy::Single, CharWidth::Single, _) => {
                self.cells[col].set(ch, Occupancy::Single, pen);
            }

            (Occupancy::Single, CharWidth::Double, 0) => {
                self.cells[col].set(' ', Occupancy::Single, pen);
                return None;
            }

            (Occupancy::Single, CharWidth::Double, 1) => {
                debug_assert_eq!(self.cells[col + 1].occupancy(), Occupancy::Single);

                self.cells[col].set(ch, Occupancy::WideHead, pen);
                self.cells[col + 1].set(' ', Occupancy::WideTail, pen);
            }

            (Occupancy::Single, CharWidth::Double, _right) => {
                self.cells[col].set(ch, Occupancy::WideHead, pen);

                if self.cells[col + 1].occupancy() == Occupancy::WideHead {
                    self.cells[col + 2].set(' ', Occupancy::Single, pen);
                }

                self.cells[col + 1].set(' ', Occupancy::WideTail, pen);
            }

            (Occupancy::WideHead, CharWidth::Single, right) => {
                debug_assert!(right >= 1);
                debug_assert_eq!(self.cells[col + 1].occupancy(), Occupancy::WideTail);

                self.cells[col].set(ch, Occupancy::Single, pen);
                self.cells[col + 1].set(' ', Occupancy::Single, pen);
            }

            (Occupancy::WideHead, CharWidth::Double, right) => {
                debug_assert!(right >= 1);
                debug_assert_eq!(self.cells[col + 1].occupancy(), Occupancy::WideTail);

                self.cells[col].set(ch, Occupancy::WideHead, pen);
                self.cells[col + 1].set(' ', Occupancy::WideTail, pen);
            }

            (Occupancy::WideTail, CharWidth::Single, _right) => {
                debug_assert!(col > 0);
                debug_assert_eq!(self.cells[col - 1].occupancy(), Occupancy::WideHead);

                self.cells[col - 1].set(' ', Occupancy::Single, pen);
                self.cells[col].set(ch, Occupancy::Single, pen);
            }

            (Occupancy::WideTail, CharWidth::Double, 0) => {
                debug_assert!(col > 0);
                debug_assert_eq!(self.cells[col - 1].occupancy(), Occupancy::WideHead);

                return None;
            }

            (Occupancy::WideTail, CharWidth::Double, 1) => {
                debug_assert!(col > 0);
                debug_assert_eq!(self.cells[col - 1].occupancy(), Occupancy::WideHead);

                self.cells[col + 1].set(' ', Occupancy::Single, pen);
                return None;
            }

            (Occupancy::WideTail, CharWidth::Double, _right) => {
                debug_assert!(col > 0);
                debug_assert_eq!(self.cells[col - 1].occupancy(), Occupancy::WideHead);

                self.cells[col - 1].set(' ', Occupancy::Single, pen);
                self.cells[col].set(ch, Occupancy::WideHead, pen);

                if self.cells[col + 1].occupancy() == Occupancy::WideHead {
                    self.cells[col + 2].set(' ', Occupancy::Single, pen);
                }

                self.cells[col + 1].set(' ', Occupancy::WideTail, pen);
            }
        }

        Some(char_width.as_usize())
    }

    pub(crate) fn shift_right(&mut self, col: usize, n: usize, pen: Pen) {
        let col = col.min(self.len() - 1);
        let cur_cell = &mut self.cells[col];

        if cur_cell.occupancy() == Occupancy::WideTail {
            cur_cell.set(' ', Occupancy::Single, pen);
            self.cells[col - 1].set(' ', Occupancy::Single, pen);
        }

        self.cells[col..].rotate_right(n);

        let cur_cell = &mut self.cells[col];

        if cur_cell.occupancy() == Occupancy::WideTail {
            cur_cell.set(' ', Occupancy::Single, pen);
            self.cells
                .last_mut()
                .unwrap()
                .set(' ', Occupancy::Single, pen);
        }
    }

    pub(crate) fn delete(&mut self, col: usize, n: usize, pen: &Pen) {
        if self.cells[col].occupancy() == Occupancy::WideTail {
            self.cells[col - 1].set(' ', Occupancy::Single, *pen);
        }

        self.cells[col..].rotate_left(n);

        let cur_cell = &mut self.cells[col];

        if cur_cell.occupancy() == Occupancy::WideTail {
            cur_cell.set(' ', Occupancy::Single, *pen);
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
            if other[needed].occupancy() == Occupancy::WideTail {
                needed -= 1;
                let pen = self.cells[self.len() - 1].pen();
                self.cells.push(Cell::new(' ', Occupancy::Single, *pen));
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
        let filler = std::iter::repeat_n(tpl, len - self.len());
        self.cells.extend(filler);
    }

    pub(crate) fn contract(&mut self, mut len: usize) -> Option<Line> {
        if !self.wrapped {
            let trimmed_len = self.len() - self.trailers();
            self.cells.truncate(len.max(trimmed_len));
        }

        if self.len() > len {
            let wide_char_boundary = self.cells[len].occupancy() == Occupancy::WideTail;

            if wide_char_boundary {
                len -= 1;
            }

            let mut rest = Line {
                cells: self.cells.split_off(len),
                wrapped: self.wrapped,
            };

            if wide_char_boundary {
                let pen = self.cells[self.cells.len() - 1].pen();
                self.cells.push(Cell::new(' ', Occupancy::Single, *pen));
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
    ) -> impl Iterator<Item = Vec<Cell>> + 'a {
        let i = self
            .cells
            .iter()
            .filter(|c| c.occupancy() != Occupancy::WideTail);

        Chunks::new(i, predicate)
    }

    pub fn chars(&self) -> impl Iterator<Item = char> + '_ {
        self.cells.iter().filter_map(|c| {
            if c.occupancy() != Occupancy::WideTail {
                Some(c.char())
            } else {
                None
            }
        })
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

    fn char_display_width(&self, ch: char) -> CharWidth {
        if ch <= '\u{7e}' || ch.width().unwrap_or(1) != 2 {
            CharWidth::Single
        } else {
            CharWidth::Double
        }
    }
}

impl std::fmt::Debug for Line {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = self.text();

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
    use super::{Cell, Chunks, Occupancy, Pen};

    fn chars(cells: &[Cell]) -> Vec<char> {
        cells.iter().map(|c| c.char()).collect()
    }

    #[test]
    fn chunks() {
        let pen = Pen::default();

        let cells = [
            Cell::new('0', Occupancy::Single, pen),
            Cell::new('a', Occupancy::Single, pen),
            Cell::new('b', Occupancy::Single, pen),
            Cell::new('C', Occupancy::Single, pen),
            Cell::new('D', Occupancy::Single, pen),
            Cell::new('E', Occupancy::Single, pen),
            Cell::new('1', Occupancy::Single, pen),
            Cell::new('F', Occupancy::Single, pen),
            Cell::new('g', Occupancy::Single, pen),
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
