use std::ops::Range;

use crate::cell::Cell;
use crate::dump::Dump;
use crate::pen::Pen;
use crate::segment::Segment;

#[derive(Debug, Clone, Default)]
pub struct Line(pub(crate) Vec<Cell>);

impl Line {
    pub(crate) fn blank(cols: usize, pen: Pen) -> Self {
        if pen.is_default() {
            Line(Vec::new())
        } else {
            Line(vec![Cell::blank(pen); cols])
        }
    }

    pub(crate) fn clear(&mut self, range: Range<usize>, pen: &Pen) {
        assert!(range.start <= range.end);

        if pen.is_default() && range.end >= self.0.len() {
            self.0.truncate(range.start);
        } else {
            let cell = Cell::blank(*pen);

            for col in range {
                self.print(col, cell);
            }
        }
    }

    pub(crate) fn clear_from(&mut self, start: usize, pen: &Pen) {
        if start < self.0.len() {
            self.clear(start..self.0.len(), pen);
        }
    }

    pub(crate) fn print(&mut self, col: usize, cell: Cell) -> bool {
        if col >= self.0.len() && cell.is_default() {
            return false;
        }

        match col.cmp(&self.0.len()) {
            std::cmp::Ordering::Less => {
                self.0[col] = cell;
            }

            std::cmp::Ordering::Equal => {
                if !cell.is_default() {
                    self.0.push(cell);
                }
            }

            std::cmp::Ordering::Greater => {
                if !cell.is_default() {
                    for _ in (self.0.len())..col {
                        self.0.push(Cell::default());
                    }

                    self.0.push(cell);
                }
            }
        }

        true
    }

    pub(crate) fn insert(&mut self, col: usize, n: usize, cell: Cell) {
        if col >= self.0.len() {
            if cell.is_default() {
                return;
            }

            let blank = Cell::default();

            for _ in 0..col - self.0.len() {
                self.0.push(blank);
            }

            for _ in 0..n {
                self.0.push(cell);
            }
        } else {
            for _ in 0..n {
                self.0.insert(col, cell);
            }
        }
    }

    pub(crate) fn delete(&mut self, col: usize, mut n: usize) -> bool {
        if col < self.0.len() {
            n = n.min(self.0.len() - col);
            self.0[col..].rotate_left(n);
            self.0.truncate(self.0.len() - n);

            true
        } else {
            false
        }
    }

    pub(crate) fn repeat(&mut self, col: usize, n: usize, pen: &Pen) -> bool {
        if col == 0 || col > self.0.len() {
            return false;
        }

        let ch = self.0[col - 1].0;
        let mut changed = false;

        for c in col..col + n {
            let changed_ = self.print(c, Cell(ch, *pen));
            changed = changed || changed_;
        }

        changed
    }

    pub(crate) fn expand(&mut self, increment: usize, pen: &Pen) {
        let tpl = Cell::blank(*pen);
        let filler = std::iter::repeat(tpl).take(increment);
        self.0.extend(filler);
    }

    pub(crate) fn contract(&mut self, len: usize) {
        self.0.truncate(len);
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn cells(&self) -> impl Iterator<Item = (char, Pen)> + '_ {
        self.0.iter().map(|cell| (cell.0, cell.1))
    }

    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        Segments {
            iter: self.0.iter(),
            current: None,
        }
    }

    pub fn segments_until(&self, col: usize) -> impl Iterator<Item = Segment> + '_ {
        Segments {
            iter: self.0.iter().take(col),
            current: None,
        }
    }

    pub fn chars(&self) -> impl Iterator<Item = char> + '_ {
        self.0.iter().map(|cell| cell.0)
    }

    pub fn text(&self) -> String {
        self.chars().collect()
    }
}

struct Segments<'a, I>
where
    I: Iterator<Item = &'a Cell>,
{
    iter: I,
    current: Option<Segment>,
}

impl<'a, I: Iterator<Item = &'a Cell>> Iterator for Segments<'a, I> {
    type Item = Segment;

    fn next(&mut self) -> Option<Self::Item> {
        for cell in self.iter.by_ref() {
            match self.current.as_mut() {
                Some(segment) => {
                    if cell.1 == segment.1 {
                        segment.0.push(cell.0);
                    } else {
                        return self.current.replace(Segment(vec![cell.0], cell.1));
                    }
                }

                None => {
                    self.current = Some(Segment(vec![cell.0], cell.1));
                }
            }
        }

        self.current.take()
    }
}

impl Dump for Line {
    fn dump(&self) -> String {
        self.segments().map(|s| s.dump()).collect()
    }
}
