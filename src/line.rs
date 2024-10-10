use crate::cell::Cell;
use crate::dump::Dump;
use crate::pen::Pen;
use std::ops::{Index, Range, RangeFull};

#[derive(Debug, Clone, PartialEq)]
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
        self.cells[range].fill(Cell::blank(*pen));
    }

    pub(crate) fn print(&mut self, col: usize, cell: Cell) {
        self.cells[col] = cell;
    }

    pub(crate) fn insert(&mut self, col: usize, n: usize, cell: Cell) {
        self.cells[col..].rotate_right(n);
        self.cells[col..col + n].fill(cell);
    }

    pub(crate) fn delete(&mut self, col: usize, n: usize, pen: &Pen) {
        self.cells[col..].rotate_left(n);
        let start = self.cells.len() - n;
        self.cells[start..].fill(Cell::blank(*pen));
    }

    pub(crate) fn extend(&mut self, mut other: Line, len: usize) -> (bool, Option<Line>) {
        let needed = len - self.len();

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

    pub(crate) fn contract(&mut self, len: usize) -> Option<Line> {
        if !self.wrapped {
            let trimmed_len = self.len() - self.trailers();
            self.cells.truncate(len.max(trimmed_len));
        }

        if self.len() > len {
            let mut rest = Line {
                cells: self.cells.split_off(len),
                wrapped: self.wrapped,
            };

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
        Chunks::new(self.cells.iter(), predicate)
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

    pub fn dump(&self) -> String {
        let mut s = String::new();

        for cells in self.chunks(|c1, c2| c1.pen() != c2.pen()) {
            s.push_str(&cells[0].pen().dump());

            for cell in cells {
                s.push(cell.char());
            }
        }

        s
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
    use super::{Cell, Chunks};

    fn chars(cells: &[Cell]) -> Vec<char> {
        cells.iter().map(|c| c.char()).collect()
    }

    #[test]
    fn chunks() {
        let cells = [
            '0'.into(),
            'a'.into(),
            'b'.into(),
            'C'.into(),
            'D'.into(),
            'E'.into(),
            '1'.into(),
            'F'.into(),
            'g'.into(),
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
