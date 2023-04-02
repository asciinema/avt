use super::Pen;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Cell(pub(crate) char, pub(crate) Pen);

impl Cell {
    pub(crate) fn blank() -> Cell {
        Cell(' ', Pen::new())
    }
}
