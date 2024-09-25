use crate::charset::Charset;
use std::fmt::Display;

const MAX_PARAM_LEN: usize = 6;

#[derive(Debug, PartialEq)]
pub enum Operation {
    Bs,
    Cbt(Option<Param>),
    Cha(Option<Param>),
    Cht(Option<Param>),
    Cnl(Option<Param>),
    Cpl(Option<Param>),
    Cr,
    Ctc(Option<Param>),
    Cub(Option<Param>),
    Cud(Option<Param>),
    Cuf(Option<Param>),
    Cup(Option<Param>, Option<Param>),
    Cuu(Option<Param>),
    Dch(Option<Param>),
    Decaln,
    Decstbm(Option<Param>, Option<Param>),
    Decstr,
    Dl(Option<Param>),
    Ech(Option<Param>),
    Ed(Option<Param>),
    El(Option<Param>),
    G1d4(Charset),
    Gzd4(Charset),
    Ht,
    Hts,
    Ich(Option<Param>),
    Il(Option<Param>),
    Lf,
    Nel,
    Print(char),
    PrvRm(Vec<Param>),
    PrvSm(Vec<Param>),
    Rc,
    Rep(Option<Param>),
    Ri,
    Ris,
    Rm(Vec<Param>),
    Sc,
    Sd(Option<Param>),
    Sgr(Vec<Param>),
    Si,
    Sm(Vec<Param>),
    So,
    Su(Option<Param>),
    Tbc(Option<Param>),
    Vpa(Option<Param>),
    Vpr(Option<Param>),
    Xtwinops(Option<Param>, Option<Param>, Option<Param>),
}

#[derive(Debug, PartialEq, Clone)]
pub struct Param {
    cur_part: usize,
    parts: [u16; MAX_PARAM_LEN],
}

impl Param {
    pub fn new(number: u16) -> Self {
        Self {
            cur_part: 0,
            parts: [number, 0, 0, 0, 0, 0],
        }
    }

    #[cfg(test)]
    pub fn from_slice(numbers: &[u16]) -> Self {
        let mut parts = [0; 6];

        for (i, part) in numbers.iter().enumerate() {
            parts[i] = *part;
        }

        Self {
            cur_part: numbers.len() - 1,
            parts,
        }
    }

    pub fn add_part(&mut self) {
        self.cur_part = (self.cur_part + 1).min(5);
    }

    pub fn add_digit(&mut self, input: u8) {
        let number = &mut self.parts[self.cur_part];
        *number = (10 * (*number as u32) + (input as u32)) as u16;
    }

    pub fn as_u16(&self) -> u16 {
        self.parts[0]
    }

    pub fn parts(&self) -> &[u16] {
        &self.parts[..=self.cur_part]
    }
}

impl Display for Param {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.parts() {
            [] => unreachable!(),

            [part] => write!(f, "{}", part),

            [first, rest @ ..] => {
                write!(f, "{first}")?;

                for part in rest {
                    write!(f, ":{part}")?;
                }

                Ok(())
            }
        }
    }
}

impl Default for Param {
    fn default() -> Self {
        Self::new(0)
    }
}

impl From<u16> for Param {
    fn from(value: u16) -> Self {
        Self::new(value)
    }
}

impl From<Vec<u16>> for Param {
    fn from(values: Vec<u16>) -> Self {
        let mut parts = [0u16; MAX_PARAM_LEN];
        let mut cur_part = 0;

        for (i, v) in values.iter().take(MAX_PARAM_LEN).enumerate() {
            cur_part = i;
            parts[i] = *v;
        }

        Self { cur_part, parts }
    }
}

impl PartialEq<u16> for Param {
    fn eq(&self, other: &u16) -> bool {
        self.parts[0] == *other
    }
}

impl PartialEq<Vec<u16>> for Param {
    fn eq(&self, other: &Vec<u16>) -> bool {
        self.parts[..=self.cur_part] == other[..]
    }
}
