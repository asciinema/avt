use crate::pen::Pen;

#[derive(Debug, Copy, Clone)]
pub struct Cell(pub char, pub Pen);

impl Cell {
    pub fn blank(pen: Pen) -> Self {
        Cell(' ', pen)
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::blank(Pen::default())
    }
}
