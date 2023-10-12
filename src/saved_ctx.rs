use crate::pen::Pen;

#[derive(Debug, PartialEq, Eq)]
pub struct SavedCtx {
    pub cursor_col: usize,
    pub cursor_row: usize,
    pub pen: Pen,
    pub origin_mode: bool,
    pub auto_wrap_mode: bool,
}

impl Default for SavedCtx {
    fn default() -> Self {
        SavedCtx {
            cursor_col: 0,
            cursor_row: 0,
            pen: Pen::default(),
            origin_mode: false,
            auto_wrap_mode: true,
        }
    }
}
