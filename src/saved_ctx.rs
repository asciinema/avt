use super::Pen;

#[derive(Debug, PartialEq)]
pub struct SavedCtx {
    pub cursor_x: usize,
    pub cursor_y: usize,
    pub pen: Pen,
    pub origin_mode: bool,
    pub auto_wrap_mode: bool,
}

impl SavedCtx {
    pub fn new() -> SavedCtx {
        SavedCtx {
            cursor_x: 0,
            cursor_y: 0,
            pen: Pen::new(),
            origin_mode: false,
            auto_wrap_mode: true,
        }
    }
}
