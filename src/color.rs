use rgb::RGB8;
use serde::ser::{Serialize, Serializer};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Color {
    Indexed(u8),
    RGB(RGB8),
}

impl Color {
    pub(crate) fn sgr_params(&self, base: u8) -> String {
        match self {
            Color::Indexed(c) if *c < 8 => {
                format!("{}", base + c)
            }

            Color::Indexed(c) if *c < 16 => {
                format!("{}", base + 52 + c)
            }

            Color::Indexed(c) => {
                format!("{};5;{}", base + 8, c)
            }

            Color::RGB(c) => {
                format!("{};2;{};{};{}", base + 8, c.r, c.g, c.b)
            }
        }
    }
}

impl Serialize for Color {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Color::Indexed(c) => serializer.serialize_u8(*c),

            Color::RGB(c) => serializer.serialize_str(&format!("rgb({},{},{})", c.r, c.g, c.b)),
        }
    }
}
