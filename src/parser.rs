// Based on Paul Williams' parser for ANSI-compatible video terminals:
// https://www.vt100.net/emu/dec_ansi_parser

use crate::charset::Charset;
use crate::dump::Dump;
use crate::ops::{Operation, Param};

const PARAMS_LEN: usize = 32;

#[derive(Debug, Default)]
pub struct Parser {
    pub state: State,
    params: [Param; PARAMS_LEN],
    cur_param: usize,
    intermediate: Option<char>,
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
        for p in &mut self.params[..=self.cur_param] {
            p.clear();
        }

        self.cur_param = 0;
        self.intermediate = None;
    }

    fn collect(&mut self, input: char) {
        self.intermediate = Some(input);
    }

    fn param(&mut self, input: char) {
        if input == ';' {
            self.cur_param += 1;

            if self.cur_param == PARAMS_LEN {
                self.cur_param = PARAMS_LEN - 1;
            }
        } else if input == ':' {
            self.params[self.cur_param].add_part();
        } else {
            self.params[self.cur_param].add_digit((input as u8) - 0x30);
        }
    }

    fn esc_dispatch(&mut self, input: char) -> Option<Operation> {
        use Operation::*;

        match (self.intermediate, input) {
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

        let ps = &self.params;

        match (self.intermediate, input) {
            (None, '@') => Some(Ich(ps[0].as_u16())),

            (None, 'A') => Some(Cuu(ps[0].as_u16())),

            (None, 'B') => Some(Cud(ps[0].as_u16())),

            (None, 'C') => Some(Cuf(ps[0].as_u16())),

            (None, 'D') => Some(Cub(ps[0].as_u16())),

            (None, 'E') => Some(Cnl(ps[0].as_u16())),

            (None, 'F') => Some(Cpl(ps[0].as_u16())),

            (None, 'G') => Some(Cha(ps[0].as_u16())),

            (None, 'H') => Some(Cup(ps[0].as_u16(), ps[1].as_u16())),

            (None, 'I') => Some(Cht(ps[0].as_u16())),

            (None, 'J') => Some(Ed(ps[0].as_u16())),

            (None, 'K') => Some(El(ps[0].as_u16())),

            (None, 'L') => Some(Il(ps[0].as_u16())),

            (None, 'M') => Some(Dl(ps[0].as_u16())),

            (None, 'P') => Some(Dch(ps[0].as_u16())),

            (None, 'S') => Some(Su(ps[0].as_u16())),

            (None, 'T') => Some(Sd(ps[0].as_u16())),

            (None, 'W') => Some(Ctc(ps[0].as_u16())),

            (None, 'X') => Some(Ech(ps[0].as_u16())),

            (None, 'Z') => Some(Cbt(ps[0].as_u16())),

            (None, '`') => Some(Cha(ps[0].as_u16())),

            (None, 'a') => Some(Cuf(ps[0].as_u16())),

            (None, 'b') => Some(Rep(ps[0].as_u16())),

            (None, 'd') => Some(Vpa(ps[0].as_u16())),

            (None, 'e') => Some(Vpr(ps[0].as_u16())),

            (None, 'f') => Some(Cup(ps[0].as_u16(), ps[1].as_u16())),

            (None, 'g') => Some(Tbc(ps[0].as_u16())),

            (None, 'h') => Some(Sm(ps[..=self.cur_param]
                .iter()
                .map(|p| p.as_u16())
                .collect())),

            (None, 'l') => Some(Rm(ps[..=self.cur_param]
                .iter()
                .map(|p| p.as_u16())
                .collect())),

            (None, 'm') => Some(Sgr(ps[..=self.cur_param].to_vec())),

            (None, 'r') => Some(Decstbm(ps[0].as_u16(), ps[1].as_u16())),

            (None, 's') => Some(Sc),

            (None, 't') => Some(Xtwinops(ps[0].as_u16(), ps[1].as_u16(), ps[2].as_u16())),

            (None, 'u') => Some(Rc),

            (Some('!'), 'p') => Some(Decstr),

            (Some('?'), 'h') => Some(PrvSm(
                ps[..=self.cur_param].iter().map(|p| p.as_u16()).collect(),
            )),

            (Some('?'), 'l') => Some(PrvRm(
                ps[..=self.cur_param].iter().map(|p| p.as_u16()).collect(),
            )),

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
            assert_eq!(self.intermediate, other.intermediate);
        }
    }
}

impl Dump for Parser {
    fn dump(&self) -> String {
        use State::*;

        let mut seq = String::new();

        match self.state {
            Ground => {}

            Escape => {
                seq.push('\u{1b}');
            }

            EscapeIntermediate => {
                let intermediates = self.intermediate.iter().collect::<String>();
                let s = format!("\u{1b}{intermediates}");
                seq.push_str(&s);
            }

            CsiEntry => {
                seq.push('\u{9b}');
            }

            CsiParam => {
                let intermediates = self.intermediate.iter().collect::<String>();

                let params = &self.params[..=self.cur_param]
                    .iter()
                    .map(|param| param.to_string())
                    .collect::<Vec<_>>()
                    .join(";");

                let s = &format!("\u{9b}{intermediates}{params}");
                seq.push_str(s);
            }

            CsiIntermediate => {
                let intermediates = self.intermediate.iter().collect::<String>();
                let s = &format!("\u{9b}{intermediates}");
                seq.push_str(s);
            }

            CsiIgnore => {
                seq.push_str("\u{9b}\u{3a}");
            }

            DcsEntry => {
                seq.push('\u{90}');
            }

            DcsIntermediate => {
                let intermediates = self.intermediate.iter().collect::<String>();
                let s = &format!("\u{90}{intermediates}");
                seq.push_str(s);
            }

            DcsParam => {
                let intermediates = self.intermediate.iter().collect::<String>();

                let params = &self.params[..=self.cur_param]
                    .iter()
                    .map(|param| param.to_string())
                    .collect::<Vec<_>>()
                    .join(";");

                let s = &format!("\u{90}{intermediates}{params}");
                seq.push_str(s);
            }

            DcsPassthrough => {
                let intermediates = self.intermediate.iter().collect::<String>();
                let s = &format!("\u{90}{intermediates}\u{40}");
                seq.push_str(s);
            }

            DcsIgnore => {
                seq.push_str("\u{90}\u{3a}");
            }

            OscString => {
                seq.push('\u{9d}');
            }

            SosPmApcString => {
                seq.push('\u{98}');
            }
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
