use crate::color::Color;
use crate::dump::Dump;
use crate::pen::Pen;
use serde::{ser::Serializer, Serialize};

#[derive(Debug, Serialize)]
pub struct Segment {
    #[serde(rename = "text", serialize_with = "serialize_chars")]
    pub(crate) chars: Vec<char>,
    pub(crate) pen: Pen,
    pub(crate) offset: usize,
}

impl Segment {
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn foreground(&self) -> Option<Color> {
        self.pen.foreground()
    }

    pub fn background(&self) -> Option<Color> {
        self.pen.background()
    }

    pub fn is_bold(&self) -> bool {
        self.pen.is_bold()
    }

    pub fn is_faint(&self) -> bool {
        self.pen.is_faint()
    }

    pub fn is_italic(&self) -> bool {
        self.pen.is_italic()
    }

    pub fn is_underline(&self) -> bool {
        self.pen.is_underline()
    }

    pub fn is_strikethrough(&self) -> bool {
        self.pen.is_strikethrough()
    }

    pub fn is_blink(&self) -> bool {
        self.pen.is_blink()
    }

    pub fn is_inverse(&self) -> bool {
        self.pen.is_inverse()
    }
}

impl Dump for Segment {
    fn dump(&self) -> String {
        let mut s = self.pen.dump();
        let text = self.chars.iter().collect::<String>();
        s.push_str(&text);

        s
    }
}

fn serialize_chars<S: Serializer>(chars: &[char], serializer: S) -> Result<S::Ok, S::Error> {
    let s: String = chars.iter().collect();
    serializer.serialize_str(&s)
}
