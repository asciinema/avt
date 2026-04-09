use rgb::RGB8;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Color {
    Indexed(u8),
    RGB(RGB8),
}

impl Color {
    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::RGB(RGB8::new(r, g, b))
    }
}
