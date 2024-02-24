use crate::pen::Pen;
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Cell(pub char, pub Pen);

impl Cell {
    pub fn blank(pen: Pen) -> Self {
        Cell(' ', pen)
    }

    pub fn is_default(&self) -> bool {
        self.0 == ' ' && self.1.is_default()
    }

    pub(crate) fn char_width(&self) -> usize {
        UnicodeWidthChar::width(self.0).unwrap_or(0)
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::blank(Pen::default())
    }
}
