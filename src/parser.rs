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

pub trait Executor {
    fn execute(&mut self, op: Operation);
}

impl Executor for Vec<Operation> {
    fn execute(&mut self, op: Operation) {
        self.push(op);
    }
}

impl Parser {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn feed_str<S: AsRef<str>, E: Executor>(&mut self, input: S, executor: &mut E) {
        for ch in input.as_ref().chars() {
            self.feed(ch, executor);
        }
    }

    pub fn feed<E: Executor>(&mut self, input: char, executor: &mut E) {
        let input2 = if input >= '\u{a0}' { '\u{41}' } else { input };

        match (&self.state, input2) {
            (State::Ground, '\u{20}'..='\u{7f}') => {
                executor.execute(Operation::Print(input));
            }

            (State::CsiParam, '\u{30}'..='\u{3b}') => {
                self.param(input);
            }

            (_, '\u{1b}') => {
                self.state = State::Escape;
                self.clear();
            }

            (State::Escape, '\u{5b}') => {
                self.state = State::CsiEntry;
                self.clear();
            }

            (State::CsiParam, '\u{40}'..='\u{7e}') => {
                self.state = State::Ground;
                self.csi_dispatch(input, executor);
            }

            (State::CsiEntry, '\u{30}'..='\u{39}') | (State::CsiEntry, '\u{3b}') => {
                self.state = State::CsiParam;
                self.param(input);
            }

            (State::Ground, '\u{00}'..='\u{17}')
            | (State::Ground, '\u{19}')
            | (State::Ground, '\u{1c}'..='\u{1f}') => {
                self.execute(input, executor);
            }

            (State::CsiEntry, '\u{40}'..='\u{7e}') => {
                self.state = State::Ground;
                self.csi_dispatch(input, executor);
            }

            (State::OscString, '\u{20}'..='\u{7f}') => {
                self.osc_put(input);
            }

            (State::Escape, '\u{20}'..='\u{2f}') => {
                self.state = State::EscapeIntermediate;
                self.collect(input);
            }

            (State::EscapeIntermediate, '\u{30}'..='\u{7e}') => {
                self.state = State::Ground;
                self.esc_dispatch(input, executor);
            }

            (State::CsiEntry, '\u{3c}'..='\u{3f}') => {
                self.state = State::CsiParam;
                self.collect(input);
            }

            (State::DcsPassthrough, '\u{20}'..='\u{7e}') => {
                self.put(input);
            }

            (State::CsiIgnore, '\u{40}'..='\u{7e}') => {
                self.state = State::Ground;
            }

            (State::CsiParam, '\u{3c}'..='\u{3f}') => {
                self.state = State::CsiIgnore;
            }

            (State::Escape, '\u{30}'..='\u{4f}')
            | (State::Escape, '\u{51}'..='\u{57}')
            | (State::Escape, '\u{59}')
            | (State::Escape, '\u{5a}')
            | (State::Escape, '\u{5c}')
            | (State::Escape, '\u{60}'..='\u{7e}') => {
                self.state = State::Ground;
                self.esc_dispatch(input, executor);
            }

            (State::Escape, '\u{5d}') => {
                self.state = State::OscString;
            }

            (State::OscString, '\u{07}') => {
                // 0x07 is xterm non-ANSI variant of transition to ground
                self.state = State::Ground;
            }

            (_, '\u{18}')
            | (_, '\u{1a}')
            | (_, '\u{80}'..='\u{8f}')
            | (_, '\u{91}'..='\u{97}')
            | (_, '\u{99}')
            | (_, '\u{9a}') => {
                self.state = State::Ground;
                self.execute(input, executor);
            }

            (State::Escape, '\u{50}') => {
                self.state = State::DcsEntry;
                self.clear();
            }

            (State::CsiParam, '\u{20}'..='\u{2f}') => {
                self.state = State::CsiIntermediate;
                self.collect(input);
            }

            (State::CsiIntermediate, '\u{40}'..='\u{7e}') => {
                self.state = State::Ground;
                self.csi_dispatch(input, executor);
            }

            (State::DcsParam, '\u{30}'..='\u{39}') | (State::DcsParam, '\u{3b}') => {
                self.param(input);
            }

            (State::DcsParam, '\u{40}'..='\u{7e}') => {
                self.state = State::DcsPassthrough;
            }

            (State::DcsEntry, '\u{3c}'..='\u{3f}') => {
                self.state = State::DcsParam;
                self.collect(input);
            }

            (State::CsiParam, '\u{00}'..='\u{17}')
            | (State::CsiParam, '\u{19}')
            | (State::CsiParam, '\u{1c}'..='\u{1f}') => {
                self.execute(input, executor);
            }

            (State::Escape, '\u{00}'..='\u{17}')
            | (State::Escape, '\u{19}')
            | (State::Escape, '\u{1c}'..='\u{1f}') => {
                self.execute(input, executor);
            }

            (State::DcsEntry, '\u{20}'..='\u{2f}') => {
                self.state = State::DcsIntermediate;
                self.collect(input);
            }

            (State::DcsIntermediate, '\u{40}'..='\u{7e}') => {
                self.state = State::DcsPassthrough;
            }

            (State::DcsPassthrough, '\u{00}'..='\u{17}')
            | (State::DcsPassthrough, '\u{19}')
            | (State::DcsPassthrough, '\u{1c}'..='\u{1f}') => {
                self.put(input);
            }

            (State::CsiEntry, '\u{00}'..='\u{17}')
            | (State::CsiEntry, '\u{19}')
            | (State::CsiEntry, '\u{1c}'..='\u{1f}') => {
                self.execute(input, executor);
            }

            (State::DcsEntry, '\u{40}'..='\u{7e}') => {
                self.state = State::DcsPassthrough;
            }

            (State::CsiIntermediate, '\u{20}'..='\u{2f}') => {
                self.collect(input);
            }

            (State::EscapeIntermediate, '\u{20}'..='\u{2f}') => {
                self.collect(input);
            }

            (State::CsiIntermediate, '\u{30}'..='\u{3f}') => {
                self.state = State::CsiIgnore;
            }

            (State::CsiEntry, '\u{20}'..='\u{2f}') => {
                self.state = State::CsiIntermediate;
                self.collect(input);
            }

            (State::EscapeIntermediate, '\u{00}'..='\u{17}')
            | (State::EscapeIntermediate, '\u{19}')
            | (State::EscapeIntermediate, '\u{1c}'..='\u{1f}') => {
                self.execute(input, executor);
            }

            (State::Escape, '\u{58}') | (State::Escape, '\u{5e}') | (State::Escape, '\u{5f}') => {
                self.state = State::SosPmApcString;
            }

            (_, '\u{98}') | (_, '\u{9e}') | (_, '\u{9f}') => {
                self.state = State::SosPmApcString;
            }

            (_, '\u{9c}') => {
                self.state = State::Ground;
            }

            (_, '\u{9d}') => {
                self.state = State::OscString;
            }

            (_, '\u{90}') => {
                self.state = State::DcsEntry;
                self.clear();
            }

            (_, '\u{9b}') => {
                self.state = State::CsiEntry;
                self.clear();
            }

            (State::DcsEntry, '\u{30}'..='\u{39}') | (State::DcsEntry, '\u{3b}') => {
                self.state = State::DcsParam;
                self.param(input);
            }

            (State::DcsIntermediate, '\u{20}'..='\u{2f}') => {
                self.collect(input);
            }

            (State::CsiIntermediate, '\u{00}'..='\u{17}')
            | (State::CsiIntermediate, '\u{19}')
            | (State::CsiIntermediate, '\u{1c}'..='\u{1f}') => {
                self.execute(input, executor);
            }

            (State::DcsEntry, '\u{3a}') => {
                self.state = State::DcsIgnore;
            }

            (State::DcsIntermediate, '\u{30}'..='\u{3f}') => {
                self.state = State::DcsIgnore;
            }

            (State::CsiIgnore, '\u{00}'..='\u{17}')
            | (State::CsiIgnore, '\u{19}')
            | (State::CsiIgnore, '\u{1c}'..='\u{1f}') => {
                self.execute(input, executor);
            }

            (State::DcsParam, '\u{20}'..='\u{2f}') => {
                self.state = State::DcsIntermediate;
                self.collect(input);
            }

            (State::CsiEntry, '\u{3a}') => {
                self.state = State::CsiIgnore;
            }

            (State::DcsParam, '\u{3a}') | (State::DcsParam, '\u{3c}'..='\u{3f}') => {
                self.state = State::DcsIgnore;
            }

            _ => {}
        }
    }

    fn execute<E: Executor>(&mut self, input: char, executor: &mut E) {
        use Operation::*;

        match input {
            '\u{08}' => {
                executor.execute(Bs);
            }

            '\u{09}' => {
                executor.execute(Ht);
            }

            '\u{0a}' => {
                executor.execute(Lf);
            }

            '\u{0b}' => {
                executor.execute(Lf);
            }

            '\u{0c}' => {
                executor.execute(Lf);
            }

            '\u{0d}' => {
                executor.execute(Cr);
            }

            '\u{0e}' => {
                executor.execute(So);
            }

            '\u{0f}' => {
                executor.execute(Si);
            }

            '\u{84}' => {
                executor.execute(Lf);
            }

            '\u{85}' => {
                executor.execute(Nel);
            }

            '\u{88}' => {
                executor.execute(Hts);
            }

            '\u{8d}' => {
                executor.execute(Ri);
            }

            _ => {}
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

    fn esc_dispatch<E: Executor>(&mut self, input: char, executor: &mut E) {
        use Operation::*;

        match (self.intermediates.0.first(), input) {
            (None, c) if ('@'..='_').contains(&c) => {
                self.execute(((input as u8) + 0x40) as char, executor)
            }

            (None, '7') => {
                executor.execute(Sc);
            }

            (None, '8') => {
                executor.execute(Rc);
            }

            (None, 'c') => {
                self.state = State::Ground;
                executor.execute(Ris);
            }

            (Some('#'), '8') => {
                executor.execute(Decaln);
            }

            (Some('('), '0') => {
                executor.execute(Gzd4(Charset::Drawing));
            }

            (Some('('), _) => {
                executor.execute(Gzd4(Charset::Ascii));
            }

            (Some(')'), '0') => {
                executor.execute(G1d4(Charset::Drawing));
            }

            (Some(')'), _) => {
                executor.execute(G1d4(Charset::Ascii));
            }

            _ => {}
        }
    }

    fn csi_dispatch<E: Executor>(&mut self, input: char, executor: &mut E) {
        use Operation::*;

        let ps = &mut self.params;

        match (self.intermediates.0.first(), input) {
            (None, '@') => {
                executor.execute(Ich(ps.drain().next()));
            }

            (None, 'A') => {
                executor.execute(Cuu(ps.drain().next()));
            }

            (None, 'B') => {
                executor.execute(Cud(ps.drain().next()));
            }

            (None, 'C') => {
                executor.execute(Cuf(ps.drain().next()));
            }

            (None, 'D') => {
                executor.execute(Cub(ps.drain().next()));
            }

            (None, 'E') => {
                executor.execute(Cnl(ps.drain().next()));
            }

            (None, 'F') => {
                executor.execute(Cpl(ps.drain().next()));
            }

            (None, 'G') => {
                executor.execute(Cha(ps.drain().next()));
            }

            (None, 'H') => {
                let mut ps = ps.drain();
                executor.execute(Cup(ps.next(), ps.next()));
            }

            (None, 'I') => {
                executor.execute(Cht(ps.drain().next()));
            }

            (None, 'J') => {
                executor.execute(Ed(ps.drain().next()));
            }

            (None, 'K') => {
                executor.execute(El(ps.drain().next()));
            }

            (None, 'L') => {
                executor.execute(Il(ps.drain().next()));
            }

            (None, 'M') => {
                executor.execute(Dl(ps.drain().next()));
            }

            (None, 'P') => {
                executor.execute(Dch(ps.drain().next()));
            }

            (None, 'S') => {
                executor.execute(Su(ps.drain().next()));
            }

            (None, 'T') => {
                executor.execute(Sd(ps.drain().next()));
            }

            (None, 'W') => {
                executor.execute(Ctc(ps.drain().next()));
            }

            (None, 'X') => {
                executor.execute(Ech(ps.drain().next()));
            }

            (None, 'Z') => {
                executor.execute(Cbt(ps.drain().next()));
            }

            (None, '`') => {
                executor.execute(Cha(ps.drain().next()));
            }

            (None, 'a') => {
                executor.execute(Cuf(ps.drain().next()));
            }

            (None, 'b') => {
                executor.execute(Rep(ps.drain().next()));
            }

            (None, 'd') => {
                executor.execute(Vpa(ps.drain().next()));
            }

            (None, 'e') => {
                executor.execute(Vpr(ps.drain().next()));
            }

            (None, 'f') => {
                let mut ps = ps.drain();
                executor.execute(Cup(ps.next(), ps.next()));
            }

            (None, 'g') => {
                executor.execute(Tbc(ps.drain().next()));
            }

            (None, 'h') => {
                executor.execute(Sm(ps.take()));
            }

            (None, 'l') => {
                executor.execute(Rm(ps.take()));
            }

            (None, 'm') => {
                executor.execute(Sgr(ps.take()));
            }

            (None, 'r') => {
                let mut ps = ps.drain();
                executor.execute(Decstbm(ps.next(), ps.next()));
            }

            (None, 's') => {
                executor.execute(Sc);
            }

            (None, 't') => {
                let mut ps = ps.drain();
                executor.execute(Xtwinops(ps.next(), ps.next(), ps.next()));
            }

            (None, 'u') => {
                executor.execute(Rc);
            }

            (Some('!'), 'p') => {
                executor.execute(Decstr);
            }

            (Some('?'), 'h') => {
                executor.execute(PrvSm(ps.take()));
            }

            (Some('?'), 'l') => {
                executor.execute(PrvRm(ps.take()));
            }

            _ => {}
        }
    }

    fn put(&mut self, _input: char) {}

    fn osc_put(&mut self, _input: char) {}

    #[cfg(test)]
    pub fn assert_eq(&self, other: &Parser) {
        assert_eq!(self.state, other.state);

        if self.state == State::CsiParam || self.state == State::DcsParam {
            assert_eq!(self.params, other.params);
        }

        if self.state == State::EscapeIntermediate
            || self.state == State::CsiIntermediate
            || self.state == State::CsiParam
            || self.state == State::DcsIntermediate
            || self.state == State::DcsParam
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
        let mut seq = String::new();

        match self.state {
            State::Ground => (),

            State::Escape => seq.push('\u{1b}'),

            State::EscapeIntermediate => {
                let intermediates = self.intermediates.0.iter().collect::<String>();
                let s = format!("\u{1b}{intermediates}");
                seq.push_str(&s);
            }

            State::CsiEntry => seq.push('\u{9b}'),

            State::CsiParam => {
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

            State::CsiIntermediate => {
                let intermediates = self.intermediates.0.iter().collect::<String>();
                let s = &format!("\u{9b}{intermediates}");
                seq.push_str(s);
            }

            State::CsiIgnore => seq.push_str("\u{9b}\u{3a}"),

            State::DcsEntry => seq.push('\u{90}'),

            State::DcsIntermediate => {
                let intermediates = self.intermediates.0.iter().collect::<String>();
                let s = &format!("\u{90}{intermediates}");
                seq.push_str(s);
            }

            State::DcsParam => {
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

            State::DcsPassthrough => {
                let intermediates = self.intermediates.0.iter().collect::<String>();
                let s = &format!("\u{90}{intermediates}\u{40}");
                seq.push_str(s);
            }

            State::DcsIgnore => seq.push_str("\u{90}\u{3a}"),

            State::OscString => seq.push('\u{9d}'),

            State::SosPmApcString => seq.push('\u{98}'),
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
        let mut ops = Vec::new();

        parser.feed_str("\x1b[;1;;23;456;m", &mut ops);

        assert_eq!(ops, vec![Sgr(ps(&[0, 1, 0, 23, 456, 0]))]);

        ops.clear();

        parser.feed_str("\x1b[;1;;38:2:1:2:3;m", &mut ops);

        assert_eq!(
            ops,
            vec![Sgr(vec![p(0), p(1), p(0), mp(&[38, 2, 1, 2, 3]), p(0)])]
        );
    }

    #[test]
    fn dump() {
        let mut parser = Parser::new();
        let mut ops = Vec::new();

        parser.feed_str("\x1b[;1;;38:2:1:2:3;", &mut ops);

        assert_eq!(parser.dump(), "\u{9b}0;1;0;38:2:1:2:3;0");
    }
}
