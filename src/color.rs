#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Color {
    Indexed(u8),
    RGB(Rgb),
}

impl Color {
    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::RGB(Rgb::new(r, g, b))
    }
}
