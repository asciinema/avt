use super::Pen;

#[derive(Debug, PartialEq)]
pub struct SavedCtx {
    pub(crate) cursor_x: usize,
    pub(crate) cursor_y: usize,
    pub(crate) pen: Pen,
    pub(crate) origin_mode: bool,
    pub(crate) auto_wrap_mode: bool,
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
