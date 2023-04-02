use super::Pen;

#[derive(Debug, Copy, Clone)]
pub struct Cell(pub char, pub Pen);

impl Cell {
    pub fn blank() -> Cell {
        Cell(' ', Pen::new())
    }
}
