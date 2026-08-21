use crate::pen::Pen;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    ch: char,
    occupancy: Occupancy,
    pen: Pen,
    /// Zero-width combining marks attached to this cell's base
    /// character. They render after the base, never get their own
    /// column. Empty for the overwhelming majority of cells, which
    /// is why this lives behind an allocation rather than inline.
    combining: Vec<char>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum Occupancy {
    Single,
    WideHead,
    WideTail,
}

impl Occupancy {
    pub(crate) fn width(&self) -> u8 {
        match self {
            Occupancy::Single => 1,
            Occupancy::WideHead => 2,
            Occupancy::WideTail => 0,
        }
    }
}

impl Cell {
    pub(crate) fn new(ch: char, occupancy: Occupancy, pen: Pen) -> Self {
        Cell {
            ch,
            occupancy,
            pen,
            combining: Vec::new(),
        }
    }

    pub(crate) fn blank(pen: Pen) -> Self {
        Self::new(' ', Occupancy::Single, pen)
    }

    pub fn is_default(&self) -> bool {
        self.ch == ' '
            && self.occupancy == Occupancy::Single
            && self.pen.is_default()
            && self.combining.is_empty()
    }

    pub fn char(&self) -> char {
        self.ch
    }

    pub fn combining(&self) -> &[char] {
        &self.combining
    }

    pub(crate) fn occupancy(&self) -> Occupancy {
        self.occupancy
    }

    pub fn width(&self) -> u8 {
        self.occupancy.width()
    }

    pub fn pen(&self) -> &Pen {
        &self.pen
    }

    pub(crate) fn set(&mut self, ch: char, occupancy: Occupancy, pen: Pen) {
        self.ch = ch;
        self.occupancy = occupancy;
        self.pen = pen;
        self.combining.clear();
    }

    pub(crate) fn push_combining(&mut self, ch: char) {
        self.combining.push(ch);
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::blank(Pen::default())
    }
}
