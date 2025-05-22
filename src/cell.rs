use crate::pen::Pen;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Cell(char, usize, Pen);

impl Cell {
    pub(crate) fn new(ch: char, width: usize, pen: Pen) -> Self {
        Cell(ch, width, pen)
    }

    pub(crate) fn blank(pen: Pen) -> Self {
        Cell(' ', 1, pen)
    }

    pub fn is_default(&self) -> bool {
        self.0 == ' ' && self.1 == 1 && self.2.is_default()
    }

    pub fn char(&self) -> char {
        self.0
    }

    pub fn width(&self) -> usize {
        self.1
    }

    pub fn pen(&self) -> &Pen {
        &self.2
    }

    pub fn set(&mut self, ch: char, width: usize, pen: Pen) {
        self.0 = ch;
        self.1 = width;
        self.2 = pen;
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::blank(Pen::default())
    }
}
