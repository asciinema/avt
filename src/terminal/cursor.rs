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

impl From<Cursor> for Option<(usize, usize)> {
    fn from(cursor: Cursor) -> Self {
        if cursor.visible {
            Some((cursor.col, cursor.row))
        } else {
            None
        }
    }
}

impl PartialEq<(usize, usize)> for Cursor {
    fn eq(&self, (other_col, other_row): &(usize, usize)) -> bool {
        *other_col == self.col && *other_row == self.row
    }
}
