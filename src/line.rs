use crate::cell::Cell;
use crate::pen::Pen;
use crate::segment::Segment;

#[derive(Debug, Clone)]
pub struct Line(pub(crate) Vec<Cell>);

impl Line {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn cells(&self) -> impl Iterator<Item = (char, Pen)> + '_ {
        self.0.iter().map(|cell| (cell.0, cell.1))
    }

    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        Chunk {
            iter: self.0.iter(),
            segment: None,
        }
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
