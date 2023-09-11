use crate::color::Color;
use crate::dump::Dump;
use serde::ser::{Serialize, SerializeMap, Serializer};

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Pen {
    pub(crate) foreground: Option<Color>,
    pub(crate) background: Option<Color>,
    pub(crate) intensity: Intensity,
    pub(crate) italic: bool,
    pub(crate) underline: bool,
    pub(crate) strikethrough: bool,
    pub(crate) blink: bool,
    pub(crate) inverse: bool,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Intensity {
    Normal,
    Bold,
    Faint,
}

impl Pen {
    pub fn foreground(&self) -> Option<Color> {
        self.foreground
    }

    pub fn background(&self) -> Option<Color> {
        self.background
    }

    pub fn is_bold(&self) -> bool {
        self.intensity == Intensity::Bold
    }

    pub fn is_faint(&self) -> bool {
        self.intensity == Intensity::Faint
    }

    pub fn is_italic(&self) -> bool {
        self.italic
    }

    pub fn is_underline(&self) -> bool {
        self.underline
    }

    pub fn is_strikethrough(&self) -> bool {
        self.strikethrough
    }

    pub fn is_blink(&self) -> bool {
        self.blink
    }

    pub fn is_inverse(&self) -> bool {
        self.inverse
    }

    pub fn is_default(&self) -> bool {
        self.foreground.is_none()
            && self.background.is_none()
            && self.intensity == Intensity::Normal
            && !self.italic
            && !self.underline
            && !self.strikethrough
            && !self.blink
            && !self.inverse
    }
}

impl Default for Pen {
    fn default() -> Self {
        Pen {
            foreground: None,
            background: None,
            intensity: Intensity::Normal,
            italic: false,
            underline: false,
            strikethrough: false,
            blink: false,
            inverse: false,
        }
    }
}

impl Dump for Pen {
    fn dump(&self) -> String {
        let mut s = "\x1b[0".to_owned();

        if let Some(c) = self.foreground {
            s.push_str(&format!(";{}", c.sgr_params(30)));
        }

        if let Some(c) = self.background {
            s.push_str(&format!(";{}", c.sgr_params(40)));
        }

        match self.intensity {
            Intensity::Normal => (),

            Intensity::Bold => {
                s.push_str(";1");
            }

            Intensity::Faint => {
                s.push_str(";2");
            }
        }

        if self.italic {
            s.push_str(";3");
        }

        if self.underline {
            s.push_str(";4");
        }

        if self.blink {
            s.push_str(";5");
        }

        if self.inverse {
            s.push_str(";7");
        }

        if self.strikethrough {
            s.push_str(";9");
        }

        s.push('m');

        s
    }
}

impl Serialize for Pen {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut len = 0;

        if self.foreground.is_some() {
            len += 1;
        }

        if self.background.is_some() {
            len += 1;
        }

        if let Intensity::Bold | Intensity::Faint = self.intensity {
            len += 1;
        }

        if self.italic {
            len += 1;
        }

        if self.underline {
            len += 1;
        }

        if self.strikethrough {
            len += 1;
        }

        if self.blink {
            len += 1;
        }

        if self.inverse {
            len += 1;
        }

        let mut map = serializer.serialize_map(Some(len))?;

        if let Some(c) = self.foreground {
            map.serialize_entry("fg", &c)?;
        }

        if let Some(c) = self.background {
            map.serialize_entry("bg", &c)?;
        }

        match self.intensity {
            Intensity::Normal => (),
            Intensity::Bold => map.serialize_entry("bold", &true)?,
            Intensity::Faint => map.serialize_entry("faint", &true)?,
        }

        if self.italic {
            map.serialize_entry("italic", &true)?;
        }

        if self.underline {
            map.serialize_entry("underline", &true)?;
        }

        if self.strikethrough {
            map.serialize_entry("strikethrough", &true)?;
        }

        if self.blink {
            map.serialize_entry("blink", &true)?;
        }

        if self.inverse {
            map.serialize_entry("inverse", &true)?;
        }

        map.end()
    }
}
