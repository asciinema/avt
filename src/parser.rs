// Based on Paul Williams' parser for ANSI-compatible video terminals:
// https://www.vt100.net/emu/dec_ansi_parser

use crate::charset::Charset;
use crate::dump::Dump;
use crate::ops::{Operation, Param};
use std::mem;

#[derive(Debug, Default)]
pub struct Parser {
    pub state: State,
    params: Params,
    intermediates: Intermediates,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    DcsEntry,
    DcsParam,
    DcsIntermediate,
    DcsPassthrough,
    DcsIgnore,
    OscString,
    SosPmApcString,
}

#[derive(Debug, PartialEq)]
struct Params(Vec<Param>);

#[derive(Debug, Default, PartialEq)]
struct Intermediates(Vec<char>);

impl Parser {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn feed(&mut self, input: char) -> Option<Operation> {
        use State::*;

        let input2 = if input >= '\u{a0}' { '\u{41}' } else { input };

        match (&self.state, input2) {
            (Ground, '\u{20}'..='\u{7f}') => {
                return Some(Operation::Print(input));
            }

            (CsiParam, '\u{30}'..='\u{3b}') => {
                self.param(input);
            }

            (_, '\u{1b}') => {
                self.state = Escape;
                self.clear();
            }

            (Escape, '\u{5b}') => {
                self.state = CsiEntry;
                self.clear();
            }

            (CsiParam, '\u{40}'..='\u{7e}') => {
                self.state = Ground;
                return self.csi_dispatch(input);
            }

            (CsiEntry, '\u{30}'..='\u{39}') | (CsiEntry, '\u{3b}') => {
                self.state = CsiParam;
                self.param(input);
            }

            (Ground, '\u{00}'..='\u{17}') | (Ground, '\u{19}') | (Ground, '\u{1c}'..='\u{1f}') => {
                return self.execute(input);
            }

            (CsiEntry, '\u{40}'..='\u{7e}') => {
                self.state = Ground;
                return self.csi_dispatch(input);
            }

            (OscString, '\u{20}'..='\u{7f}') => {
                self.osc_put(input);
            }

            (Escape, '\u{20}'..='\u{2f}') => {
                self.state = EscapeIntermediate;
                self.collect(input);
            }

            (EscapeIntermediate, '\u{30}'..='\u{7e}') => {
                self.state = Ground;
                return self.esc_dispatch(input);
            }

            (CsiEntry, '\u{3c}'..='\u{3f}') => {
                self.state = CsiParam;
                self.collect(input);
            }

            (DcsPassthrough, '\u{20}'..='\u{7e}') => {
                self.put(input);
            }

            (CsiIgnore, '\u{40}'..='\u{7e}') => {
                self.state = Ground;
            }

            (CsiParam, '\u{3c}'..='\u{3f}') => {
                self.state = CsiIgnore;
            }

            (Escape, '\u{30}'..='\u{4f}')
            | (Escape, '\u{51}'..='\u{57}')
            | (Escape, '\u{59}')
            | (Escape, '\u{5a}')
            | (Escape, '\u{5c}')
            | (Escape, '\u{60}'..='\u{7e}') => {
                self.state = Ground;
                return self.esc_dispatch(input);
            }

            (Escape, '\u{5d}') => {
                self.state = OscString;
            }

            (OscString, '\u{07}') => {
                // 0x07 is xterm non-ANSI variant of transition to ground
                self.state = Ground;
            }

            (_, '\u{18}')
            | (_, '\u{1a}')
            | (_, '\u{80}'..='\u{8f}')
            | (_, '\u{91}'..='\u{97}')
            | (_, '\u{99}')
            | (_, '\u{9a}') => {
                self.state = Ground;
                return self.execute(input);
            }

            (Escape, '\u{50}') => {
                self.state = DcsEntry;
                self.clear();
            }

            (CsiParam, '\u{20}'..='\u{2f}') => {
                self.state = CsiIntermediate;
                self.collect(input);
            }

            (CsiIntermediate, '\u{40}'..='\u{7e}') => {
                self.state = Ground;
                return self.csi_dispatch(input);
            }

            (DcsParam, '\u{30}'..='\u{39}') | (DcsParam, '\u{3b}') => {
                self.param(input);
            }

            (DcsParam, '\u{40}'..='\u{7e}') => {
                self.state = DcsPassthrough;
            }

            (DcsEntry, '\u{3c}'..='\u{3f}') => {
                self.state = DcsParam;
                self.collect(input);
            }

            (CsiParam, '\u{00}'..='\u{17}')
            | (CsiParam, '\u{19}')
            | (CsiParam, '\u{1c}'..='\u{1f}') => {
                return self.execute(input);
            }

            (Escape, '\u{00}'..='\u{17}') | (Escape, '\u{19}') | (Escape, '\u{1c}'..='\u{1f}') => {
                return self.execute(input);
            }

            (DcsEntry, '\u{20}'..='\u{2f}') => {
                self.state = DcsIntermediate;
                self.collect(input);
            }

            (DcsIntermediate, '\u{40}'..='\u{7e}') => {
                self.state = DcsPassthrough;
            }

            (DcsPassthrough, '\u{00}'..='\u{17}')
            | (DcsPassthrough, '\u{19}')
            | (DcsPassthrough, '\u{1c}'..='\u{1f}') => {
                self.put(input);
            }

            (CsiEntry, '\u{00}'..='\u{17}')
            | (CsiEntry, '\u{19}')
            | (CsiEntry, '\u{1c}'..='\u{1f}') => {
                return self.execute(input);
            }

            (DcsEntry, '\u{40}'..='\u{7e}') => {
                self.state = DcsPassthrough;
            }

            (CsiIntermediate, '\u{20}'..='\u{2f}') => {
                self.collect(input);
            }

            (EscapeIntermediate, '\u{20}'..='\u{2f}') => {
                self.collect(input);
            }

            (CsiIntermediate, '\u{30}'..='\u{3f}') => {
                self.state = CsiIgnore;
            }

            (CsiEntry, '\u{20}'..='\u{2f}') => {
                self.state = CsiIntermediate;
                self.collect(input);
            }

            (EscapeIntermediate, '\u{00}'..='\u{17}')
            | (EscapeIntermediate, '\u{19}')
            | (EscapeIntermediate, '\u{1c}'..='\u{1f}') => {
                return self.execute(input);
            }

            (Escape, '\u{58}') | (Escape, '\u{5e}') | (Escape, '\u{5f}') => {
                self.state = SosPmApcString;
            }

            (_, '\u{98}') | (_, '\u{9e}') | (_, '\u{9f}') => {
                self.state = SosPmApcString;
            }

            (_, '\u{9c}') => {
                self.state = Ground;
            }

            (_, '\u{9d}') => {
                self.state = OscString;
            }

            (_, '\u{90}') => {
                self.state = DcsEntry;
                self.clear();
            }

            (_, '\u{9b}') => {
                self.state = CsiEntry;
                self.clear();
            }

            (DcsEntry, '\u{30}'..='\u{39}') | (DcsEntry, '\u{3b}') => {
                self.state = DcsParam;
                self.param(input);
            }

            (DcsIntermediate, '\u{20}'..='\u{2f}') => {
                self.collect(input);
            }

            (CsiIntermediate, '\u{00}'..='\u{17}')
            | (CsiIntermediate, '\u{19}')
            | (CsiIntermediate, '\u{1c}'..='\u{1f}') => {
                return self.execute(input);
            }

            (DcsEntry, '\u{3a}') => {
                self.state = DcsIgnore;
            }

            (DcsIntermediate, '\u{30}'..='\u{3f}') => {
                self.state = DcsIgnore;
            }

            (CsiIgnore, '\u{00}'..='\u{17}')
            | (CsiIgnore, '\u{19}')
            | (CsiIgnore, '\u{1c}'..='\u{1f}') => {
                return self.execute(input);
            }

            (DcsParam, '\u{20}'..='\u{2f}') => {
                self.state = DcsIntermediate;
                self.collect(input);
            }

            (CsiEntry, '\u{3a}') => {
                self.state = CsiIgnore;
            }

            (DcsParam, '\u{3a}') | (DcsParam, '\u{3c}'..='\u{3f}') => {
                self.state = DcsIgnore;
            }

            _ => {}
        }

        None
    }

    fn execute(&mut self, input: char) -> Option<Operation> {
        use Operation::*;

        match input {
            '\u{08}' => Some(Bs),
            '\u{09}' => Some(Ht),
            '\u{0a}' => Some(Lf),
            '\u{0b}' => Some(Lf),
            '\u{0c}' => Some(Lf),
            '\u{0d}' => Some(Cr),
            '\u{0e}' => Some(So),
            '\u{0f}' => Some(Si),
            '\u{84}' => Some(Lf),
            '\u{85}' => Some(Nel),
            '\u{88}' => Some(Hts),
            '\u{8d}' => Some(Ri),
            _ => None,
        }
    }

    fn clear(&mut self) {
        self.params = Params::default();
        self.intermediates = Intermediates::default();
    }

    fn collect(&mut self, input: char) {
        self.intermediates.0.push(input);
    }

    fn param(&mut self, input: char) {
        self.params.push(input);
    }

    fn esc_dispatch(&mut self, input: char) -> Option<Operation> {
        use Operation::*;

        match (self.intermediates.0.first(), input) {
            (None, c) if ('@'..='_').contains(&c) => self.execute(((input as u8) + 0x40) as char),

            (None, '7') => Some(Sc),

            (None, '8') => Some(Rc),

            (None, 'c') => {
                self.state = State::Ground;
                Some(Ris)
            }

            (Some('#'), '8') => Some(Decaln),

            (Some('('), '0') => Some(Gzd4(Charset::Drawing)),

            (Some('('), _) => Some(Gzd4(Charset::Ascii)),

            (Some(')'), '0') => Some(G1d4(Charset::Drawing)),

            (Some(')'), _) => Some(G1d4(Charset::Ascii)),

            _ => None,
        }
    }

    fn csi_dispatch(&mut self, input: char) -> Option<Operation> {
        use Operation::*;

        let ps = &mut self.params;

        match (self.intermediates.0.first(), input) {
            (None, '@') => Some(Ich(ps.drain().next())),

            (None, 'A') => Some(Cuu(ps.drain().next())),

            (None, 'B') => Some(Cud(ps.drain().next())),

            (None, 'C') => Some(Cuf(ps.drain().next())),

            (None, 'D') => Some(Cub(ps.drain().next())),

            (None, 'E') => Some(Cnl(ps.drain().next())),

            (None, 'F') => Some(Cpl(ps.drain().next())),

            (None, 'G') => Some(Cha(ps.drain().next())),

            (None, 'H') => {
                let mut ps = ps.drain();
                Some(Cup(ps.next(), ps.next()))
            }

            (None, 'I') => Some(Cht(ps.drain().next())),

            (None, 'J') => Some(Ed(ps.drain().next())),

            (None, 'K') => Some(El(ps.drain().next())),

            (None, 'L') => Some(Il(ps.drain().next())),

            (None, 'M') => Some(Dl(ps.drain().next())),

            (None, 'P') => Some(Dch(ps.drain().next())),

            (None, 'S') => Some(Su(ps.drain().next())),

            (None, 'T') => Some(Sd(ps.drain().next())),

            (None, 'W') => Some(Ctc(ps.drain().next())),

            (None, 'X') => Some(Ech(ps.drain().next())),

            (None, 'Z') => Some(Cbt(ps.drain().next())),

            (None, '`') => Some(Cha(ps.drain().next())),

            (None, 'a') => Some(Cuf(ps.drain().next())),

            (None, 'b') => Some(Rep(ps.drain().next())),

            (None, 'd') => Some(Vpa(ps.drain().next())),

            (None, 'e') => Some(Vpr(ps.drain().next())),

            (None, 'f') => {
                let mut ps = ps.drain();
                Some(Cup(ps.next(), ps.next()))
            }

            (None, 'g') => Some(Tbc(ps.drain().next())),

            (None, 'h') => Some(Sm(ps.take())),

            (None, 'l') => Some(Rm(ps.take())),

            (None, 'm') => Some(Sgr(ps.take())),

            (None, 'r') => {
                let mut ps = ps.drain();
                Some(Decstbm(ps.next(), ps.next()))
            }

            (None, 's') => Some(Sc),

            (None, 't') => {
                let mut ps = ps.drain();
                Some(Xtwinops(ps.next(), ps.next(), ps.next()))
            }

            (None, 'u') => Some(Rc),

            (Some('!'), 'p') => Some(Decstr),

            (Some('?'), 'h') => Some(PrvSm(ps.take())),

            (Some('?'), 'l') => Some(PrvRm(ps.take())),

            _ => None,
        }
    }

    fn put(&mut self, _input: char) {}

    fn osc_put(&mut self, _input: char) {}

    #[cfg(test)]
    pub fn assert_eq(&self, other: &Parser) {
        use State::*;

        assert_eq!(self.state, other.state);

        if self.state == CsiParam || self.state == DcsParam {
            assert_eq!(self.params, other.params);
        }

        if self.state == EscapeIntermediate
            || self.state == CsiIntermediate
            || self.state == CsiParam
            || self.state == DcsIntermediate
            || self.state == DcsParam
        {
            assert_eq!(self.intermediates, other.intermediates);
        }
    }
}

impl Params {
    fn push(&mut self, input: char) {
        if input == ';' {
            self.0.push(Param::default());
        } else if input == ':' {
            let last_idx = self.0.len() - 1;
            self.0[last_idx].add_part();
        } else {
            let last_idx = self.0.len() - 1;
            self.0[last_idx].add_digit((input as u8) - 0x30);
        }
    }

    fn iter(&self) -> std::slice::Iter<Param> {
        self.0.iter()
    }

    fn drain(&mut self) -> impl Iterator<Item = Param> + '_ {
        self.0.drain(..)
    }

    fn take(&mut self) -> Vec<Param> {
        mem::take(&mut self.0)
    }
}

impl Default for Params {
    fn default() -> Self {
        let mut params = Vec::with_capacity(8);
        params.push(Param::default());

        Self(params)
    }
}

impl From<Vec<Param>> for Params {
    fn from(values: Vec<Param>) -> Self {
        Self(values)
    }
}

impl Dump for Parser {
    fn dump(&self) -> String {
        use State::*;

        let mut seq = String::new();

        match self.state {
            Ground => (),

            Escape => seq.push('\u{1b}'),

            EscapeIntermediate => {
                let intermediates = self.intermediates.0.iter().collect::<String>();
                let s = format!("\u{1b}{intermediates}");
                seq.push_str(&s);
            }

            CsiEntry => seq.push('\u{9b}'),

            CsiParam => {
                let intermediates = self.intermediates.0.iter().collect::<String>();

                let params = self
                    .params
                    .iter()
                    .map(|param| param.to_string())
                    .collect::<Vec<_>>()
                    .join(";");

                let s = &format!("\u{9b}{intermediates}{params}");
                seq.push_str(s);
            }

            CsiIntermediate => {
                let intermediates = self.intermediates.0.iter().collect::<String>();
                let s = &format!("\u{9b}{intermediates}");
                seq.push_str(s);
            }

            CsiIgnore => seq.push_str("\u{9b}\u{3a}"),

            DcsEntry => seq.push('\u{90}'),

            DcsIntermediate => {
                let intermediates = self.intermediates.0.iter().collect::<String>();
                let s = &format!("\u{90}{intermediates}");
                seq.push_str(s);
            }

            DcsParam => {
                let intermediates = self.intermediates.0.iter().collect::<String>();

                let params = self
                    .params
                    .iter()
                    .map(|param| param.to_string())
                    .collect::<Vec<_>>()
                    .join(";");

                let s = &format!("\u{90}{intermediates}{params}");
                seq.push_str(s);
            }

            DcsPassthrough => {
                let intermediates = self.intermediates.0.iter().collect::<String>();
                let s = &format!("\u{90}{intermediates}\u{40}");
                seq.push_str(s);
            }

            DcsIgnore => seq.push_str("\u{90}\u{3a}"),

            OscString => seq.push('\u{9d}'),

            SosPmApcString => seq.push('\u{98}'),
        }

        seq
    }
}

#[cfg(test)]
mod tests {
    use super::Parser;
    use crate::dump::Dump;
    use crate::ops::{Operation, Param};
    use Operation::*;

    fn p(number: u16) -> Param {
        Param::new(number)
    }

    fn ps(numbers: &[u16]) -> Vec<Param> {
        numbers.iter().map(|n| p(*n)).collect()
    }

    fn mp(parts: &[u16]) -> Param {
        Param::from_slice(parts)
    }

    #[test]
    fn sgr() {
        let mut parser = Parser::new();

        let ops = "\x1b[;1;;23;456;m"
            .chars()
            .filter_map(|ch| parser.feed(ch))
            .collect::<Vec<_>>();

        assert_eq!(ops, vec![Sgr(ps(&[0, 1, 0, 23, 456, 0]))]);

        let ops = "\x1b[;1;;38:2:1:2:3;m"
            .chars()
            .filter_map(|ch| parser.feed(ch))
            .collect::<Vec<_>>();

        assert_eq!(
            ops,
            vec![Sgr(vec![p(0), p(1), p(0), mp(&[38, 2, 1, 2, 3]), p(0)])]
        );
    }

    #[test]
    fn dump() {
        let mut parser = Parser::new();

        for ch in "\x1b[;1;;38:2:1:2:3;".chars() {
            parser.feed(ch);
        }

        assert_eq!(parser.dump(), "\u{9b}0;1;0;38:2:1:2:3;0");
    }
}
