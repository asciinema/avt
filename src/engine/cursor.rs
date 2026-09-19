#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Cursor {
    pub col: usize,
    pub row: usize,
    pub visible: bool,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            col: 0,
            row: 0,
            visible: true,
        }
    }
}

impl Cursor {
    pub fn position(self) -> Option<(usize, usize)> {
        if self.visible {
            Some((self.col, self.row))
        } else {
            None
        }
    }
}
