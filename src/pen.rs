use crate::color::Color;
use crate::dump::Dump;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Pen {
    pub(crate) foreground: Option<Color>,
    pub(crate) background: Option<Color>,
    pub(crate) intensity: Intensity,
    pub(crate) attrs: u8,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Intensity {
    Normal,
    Bold,
    Faint,
}

const ITALIC_MASK: u8 = 1;
const UNDERLINE_MASK: u8 = 1 << 1;
const STRIKETHROUGH_MASK: u8 = 1 << 2;
const BLINK_MASK: u8 = 1 << 3;
const INVERSE_MASK: u8 = 1 << 4;

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
        (self.attrs & ITALIC_MASK) != 0
    }

    pub fn is_underline(&self) -> bool {
        (self.attrs & UNDERLINE_MASK) != 0
    }

    pub fn is_strikethrough(&self) -> bool {
        (self.attrs & STRIKETHROUGH_MASK) != 0
    }

    pub fn is_blink(&self) -> bool {
        (self.attrs & BLINK_MASK) != 0
    }

    pub fn is_inverse(&self) -> bool {
        (self.attrs & INVERSE_MASK) != 0
    }

    pub fn set_italic(&mut self) {
        self.attrs |= ITALIC_MASK;
    }

    pub fn set_underline(&mut self) {
        self.attrs |= UNDERLINE_MASK;
    }

    pub fn set_blink(&mut self) {
        self.attrs |= BLINK_MASK;
    }

    pub fn set_strikethrough(&mut self) {
        self.attrs |= STRIKETHROUGH_MASK;
    }

    pub fn set_inverse(&mut self) {
        self.attrs |= INVERSE_MASK;
    }

    pub fn unset_italic(&mut self) {
        self.attrs &= !ITALIC_MASK;
    }

    pub fn unset_underline(&mut self) {
        self.attrs &= !UNDERLINE_MASK;
    }

    pub fn unset_blink(&mut self) {
        self.attrs &= !BLINK_MASK;
    }

    pub fn unset_strikethrough(&mut self) {
        self.attrs &= !STRIKETHROUGH_MASK;
    }

    pub fn unset_inverse(&mut self) {
        self.attrs &= !INVERSE_MASK;
    }

    pub fn is_default(&self) -> bool {
        self.foreground.is_none()
            && self.background.is_none()
            && self.intensity == Intensity::Normal
            && !self.is_italic()
            && !self.is_underline()
            && !self.is_strikethrough()
            && !self.is_blink()
            && !self.is_inverse()
    }
}

impl Default for Pen {
    fn default() -> Self {
        Pen {
            foreground: None,
            background: None,
            intensity: Intensity::Normal,
            attrs: 0,
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

        if self.is_italic() {
            s.push_str(";3");
        }

        if self.is_underline() {
            s.push_str(";4");
        }

        if self.is_blink() {
            s.push_str(";5");
        }

        if self.is_inverse() {
            s.push_str(";7");
        }

        if self.is_strikethrough() {
            s.push_str(";9");
        }

        s.push('m');

        s
    }
}
