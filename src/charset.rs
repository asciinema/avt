#[derive(Debug, PartialEq)]
pub enum Charset {
    Ascii,
    Drawing,
}

const SPECIAL_GFX_CHARS: [char; 31] = [
    '♦', '▒', '␉', '␌', '␍', '␊', '°', '±', '␤', '␋', '┘', '┐', '┌', '└', '┼', '⎺', '⎻', '─', '⎼',
    '⎽', '├', '┤', '┴', '┬', '│', '≤', '≥', 'π', '≠', '£', '⋅',
];

impl Charset {
    pub fn translate(&self, input: char) -> char {
        if ('\x60'..'\x7f').contains(&input) {
            match self {
                Charset::Ascii => input,
                Charset::Drawing => SPECIAL_GFX_CHARS[(input as usize) - 0x60],
            }
        } else {
            input
        }
    }
}
