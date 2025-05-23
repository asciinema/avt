use crate::pen::Pen;
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Cell(char, Pen);

impl Cell {
    pub(crate) fn new(ch: char, pen: Pen) -> Self {
        Cell(ch, pen)
    }

    pub(crate) fn blank(pen: Pen) -> Self {
        Cell(' ', pen)
    }

    pub fn is_default(&self) -> bool {
        self.0 == ' ' && self.1.is_default()
    }

    pub fn char(&self) -> char {
        self.0
    }

    pub fn pen(&self) -> &Pen {
        &self.1
    }

    pub fn width(&self) -> usize {
        self.0.width().unwrap_or(0)
    }

    pub fn set(&mut self, ch: char, pen: Pen) {
        self.0 = ch;
        self.1 = pen;
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::blank(Pen::default())
    }
}

impl From<char> for Cell {
    fn from(value: char) -> Self {
        Self::new(value, Pen::default())
    }
}
