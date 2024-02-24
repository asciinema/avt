use std::ops::{Index, Range, RangeFull};

use crate::cell::Cell;
use crate::dump::Dump;
use crate::pen::Pen;
use crate::segment::Segment;

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

    pub fn cells(&self) -> impl Iterator<Item = (char, Pen)> + '_ {
        self.cells.iter().map(|cell| (cell.0, cell.1))
    }

    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        self.group(|_c| false)
    }

    pub fn group<'a>(
        &'a self,
        predicate: impl Fn(&char) -> bool + 'a,
    ) -> impl Iterator<Item = Segment> + '_ {
        Segments::new(self.cells.iter(), predicate)
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

struct Segments<'a, I, F>
where
    I: Iterator<Item = &'a Cell>,
    F: Fn(&char) -> bool,
{
    iter: I,
    current: Option<Segment>,
    ready: Option<Segment>,
    offset: usize,
    predicate: F,
}

impl<'a, I: Iterator<Item = &'a Cell>, F: Fn(&char) -> bool> Segments<'a, I, F> {
    fn new(iter: I, predicate: F) -> Self {
        Self {
            iter,
            current: None,
            ready: None,
            offset: 0,
            predicate,
        }
    }
}

impl<'a, I: Iterator<Item = &'a Cell>, F: Fn(&char) -> bool> Iterator for Segments<'a, I, F> {
    type Item = Segment;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(segment) = self.ready.take() {
            return Some(segment);
        }

        for cell in self.iter.by_ref() {
            self.offset += 1;

            if (self.predicate)(&cell.0) {
                let ready = Some(Segment {
                    chars: vec![cell.0],
                    pen: cell.1,
                    offset: self.offset - 1,
                });

                if let Some(segment) = self.current.take() {
                    self.ready = ready;
                    return Some(segment);
                }

                return ready;
            }

            match self.current.as_mut() {
                Some(segment) => {
                    if cell.1 != segment.pen {
                        return self.current.replace(Segment {
                            chars: vec![cell.0],
                            pen: cell.1,
                            offset: self.offset - 1,
                        });
                    }

                    segment.chars.push(cell.0);
                }

                None => {
                    self.current = Some(Segment {
                        chars: vec![cell.0],
                        pen: cell.1,
                        offset: self.offset - 1,
                    });
                }
            }
        }

        self.current.take()
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

#[cfg(test)]
mod tests {
    use super::{Cell, Segment, Segments};
    use crate::{Color, Pen};

    #[test]
    fn segments() {
        let pen1 = Pen::default();

        let pen2 = Pen {
            foreground: Some(Color::Indexed(1)),
            ..Pen::default()
        };

        let cells = vec![
            Cell('a', pen1),
            Cell('b', pen1),
            Cell('c', pen2),
            Cell('d', pen1),
            Cell('e', pen1),
        ];

        let segments: Vec<Segment> = Segments::new(cells.iter(), |_| false).collect();

        assert_eq!(&segments[0].chars, &['a', 'b']);
        assert_eq!(segments[0].pen, pen1);
        assert_eq!(segments[0].offset, 0);

        assert_eq!(&segments[1].chars, &['c']);
        assert_eq!(segments[1].pen, pen2);
        assert_eq!(segments[1].offset, 2);

        assert_eq!(&segments[2].chars, &['d', 'e']);
        assert_eq!(segments[2].pen, pen1);
        assert_eq!(segments[2].offset, 3);
    }

    #[test]
    fn segments_group() {
        let pen1 = Pen::default();

        let pen2 = Pen {
            foreground: Some(Color::Indexed(1)),
            ..Pen::default()
        };

        let cells = vec![
            Cell('a', pen1),
            Cell('b', pen1),
            Cell('c', pen1),
            Cell('d', pen1),
            Cell('e', pen2),
        ];

        let segments: Vec<Segment> = Segments::new(cells.iter(), |c| c == &'c').collect();

        assert_eq!(&segments[0].chars, &['a', 'b']);
        assert_eq!(segments[0].pen, pen1);
        assert_eq!(segments[0].offset, 0);

        assert_eq!(&segments[1].chars, &['c']);
        assert_eq!(segments[1].pen, pen1);
        assert_eq!(segments[1].offset, 2);

        assert_eq!(&segments[2].chars, &['d']);
        assert_eq!(segments[2].pen, pen1);
        assert_eq!(segments[2].offset, 3);

        assert_eq!(&segments[3].chars, &['e']);
        assert_eq!(segments[3].pen, pen2);
        assert_eq!(segments[3].offset, 4);
    }
}
