use crate::charset::Charset;
use std::fmt::Display;

const MAX_PARAM_LEN: usize = 6;

#[derive(Debug, PartialEq)]
pub enum Operation {
    Bs,
    Cbt(u16),
    Cha(u16),
    Cht(u16),
    Cnl(u16),
    Cpl(u16),
    Cr,
    Ctc(u16),
    Cub(u16),
    Cud(u16),
    Cuf(u16),
    Cup(u16, u16),
    Cuu(u16),
    Dch(u16),
    Decaln,
    Decstbm(u16, u16),
    Decstr,
    Dl(u16),
    Ech(u16),
    Ed(u16),
    El(u16),
    G1d4(Charset),
    Gzd4(Charset),
    Ht,
    Hts,
    Ich(u16),
    Il(u16),
    Lf,
    Nel,
    Print(char),
    PrvRm(Vec<u16>),
    PrvSm(Vec<u16>),
    Rc,
    Rep(u16),
    Ri,
    Ris,
    Rm(Vec<u16>),
    Sc,
    Sd(u16),
    Sgr(Vec<Param>),
    Si,
    Sm(Vec<u16>),
    So,
    Su(u16),
    Tbc(u16),
    Vpa(u16),
    Vpr(u16),
    Xtwinops(u16, u16, u16),
}

#[derive(Debug, PartialEq, Clone, Copy)]
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

    pub fn clear(&mut self) {
        self.parts[..=self.cur_part].fill(0);
        self.cur_part = 0;
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
