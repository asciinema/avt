use rgb::RGB8;
use Color::*;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Color {
    Indexed(u8),
    RGB(RGB8),
}

impl Color {
    pub(crate) fn sgr_params(&self, base: u8) -> String {
        match self {
            Indexed(c) if *c < 8 => (base + c).to_string(),
            Indexed(c) if *c < 16 => (base + 52 + c).to_string(),
            Indexed(c) => format!("{}:5:{}", base + 8, c),
            RGB(c) => format!("{}:2:{}:{}:{}", base + 8, c.r, c.g, c.b),
        }
    }

    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::RGB(RGB8::new(r, g, b))
    }
}
