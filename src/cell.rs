use crate::pen::Pen;

#[derive(Debug, Copy, Clone)]
pub struct Cell(pub char, pub Pen);

impl Default for Cell {
    fn default() -> Self {
        Cell(' ', Pen::default())
    }
}
