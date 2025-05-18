#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Charset {
    Ascii,
    Drawing,
}

const SPECIAL_GFX_CHARS: [char; 31] = [
    '♦', '▒', '␉', '␌', '␍', '␊', '°', '±', '␤', '␋', '┘', '┐', '┌', '└', '┼', '⎺', '⎻', '─', '⎼',
    '⎽', '├', '┤', '┴', '┬', '│', '≤', '≥', 'π', '≠', '£', '⋅',
];

const fn build_lut() -> [char; 128] {
    let mut lut = ['\0'; 128];
    let mut i = 0;

    while i < 128 {
        lut[i] = if i >= 0x60 && i < 0x7f {
            SPECIAL_GFX_CHARS[i - 0x60]
        } else {
            // `u8 as char` is always valid for 0x00‥=0x7f
            (i as u8) as char
        };

        i += 1;
    }

    lut
}

const LUT_DRAWING: [char; 128] = build_lut();

impl Charset {
    #[inline(always)]
    pub fn translate(self, c: char) -> char {
        // bail out for non-ASCII
        if c as u32 > 0x7f {
            return c;
        }

        let idx = c as u8 as usize;

        match self {
            Charset::Ascii => c,
            Charset::Drawing => LUT_DRAWING[idx],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Charset;

    #[test]
    fn translate() {
        let charset = Charset::Ascii;
        assert_eq!(charset.translate('A'), 'A');
        assert_eq!(charset.translate('a'), 'a');
        assert_eq!(charset.translate('~'), '~');

        let charset = Charset::Drawing;
        assert_eq!(charset.translate('A'), 'A');
        assert_eq!(charset.translate('a'), '▒');
        assert_eq!(charset.translate('~'), '⋅');
    }
}
