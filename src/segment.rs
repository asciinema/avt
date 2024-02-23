use crate::color::Color;
use crate::dump::Dump;
use crate::pen::Pen;
use serde::ser::{Serialize, SerializeTuple, Serializer};

#[derive(Debug)]
pub struct Segment(pub(crate) Vec<char>, pub(crate) Pen, pub(crate) usize);

impl Segment {
    pub fn text(&self) -> String {
        self.0.iter().collect()
    }

    pub fn foreground(&self) -> Option<Color> {
        self.1.foreground()
    }

    pub fn background(&self) -> Option<Color> {
        self.1.background()
    }

    pub fn is_bold(&self) -> bool {
        self.1.is_bold()
    }

    pub fn is_faint(&self) -> bool {
        self.1.is_faint()
    }

    pub fn is_italic(&self) -> bool {
        self.1.is_italic()
    }

    pub fn is_underline(&self) -> bool {
        self.1.is_underline()
    }

    pub fn is_strikethrough(&self) -> bool {
        self.1.is_strikethrough()
    }

    pub fn is_blink(&self) -> bool {
        self.1.is_blink()
    }

    pub fn is_inverse(&self) -> bool {
        self.1.is_inverse()
    }
}

impl Dump for Segment {
    fn dump(&self) -> String {
        let mut s = self.1.dump();
        let text = self.0.iter().collect::<String>();
        s.push_str(&text);

        s
    }
}

impl Serialize for Segment {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut tup = serializer.serialize_tuple(3)?;
        let text: String = self.0.iter().collect();
        tup.serialize_element(&text)?;
        tup.serialize_element(&self.1)?;
        tup.serialize_element(&self.2)?;
        tup.end()
    }
}
