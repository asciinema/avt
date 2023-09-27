use std::ops::{Index, Range, RangeFull};

use crate::cell::Cell;
use crate::dump::Dump;
use crate::pen::Pen;
use crate::segment::Segment;

#[derive(Debug, Clone)]
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

    pub fn cells(&self) -> impl Iterator<Item = (char, Pen)> + '_ {
        self.cells.iter().map(|cell| (cell.0, cell.1))
    }

    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        Chunk {
            iter: self.cells.iter(),
            segment: None,
        }
    }

    pub fn chars(&self) -> impl Iterator<Item = char> + '_ {
        self.cells.iter().map(|cell| cell.0)
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
}

struct Chunk<'a, I>
where
    I: Iterator<Item = &'a Cell>,
{
    iter: I,
    segment: Option<Segment>,
}

impl<'a, I: Iterator<Item = &'a Cell>> Iterator for Chunk<'a, I> {
    type Item = Segment;

    fn next(&mut self) -> Option<Self::Item> {
        for cell in self.iter.by_ref() {
            match self.segment.as_mut() {
                Some(segment) => {
                    if cell.1 == segment.1 {
                        segment.0.push(cell.0);
                    } else {
                        return self.segment.replace(Segment(vec![cell.0], cell.1));
                    }
                }

                None => {
                    self.segment = Some(Segment(vec![cell.0], cell.1));
                }
            }
        }

        self.segment.take()
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

impl Dump for Line {
    fn dump(&self) -> String {
        self.segments().map(|s| s.dump()).collect()
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
