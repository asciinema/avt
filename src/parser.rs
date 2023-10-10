// Based on Paul Williams' parser for ANSI-compatible video terminals:
// https://www.vt100.net/emu/dec_ansi_parser

use crate::{charset::Charset, dump::Dump};

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
pub struct Params(Vec<u16>);

#[derive(Debug, Default, PartialEq)]
pub(crate) struct Intermediates(Vec<char>);

pub trait Executor {
    fn print(&mut self, _input: char) {}

    fn bs(&mut self) {}
    fn cbt(&mut self, _params: &Params) {}
    fn cha(&mut self, _params: &Params) {}
    fn cht(&mut self, _params: &Params) {}
    fn cnl(&mut self, _params: &Params) {}
    fn cpl(&mut self, _params: &Params) {}
    fn cr(&mut self) {}
    fn ctc(&mut self, _params: &Params) {}
    fn cub(&mut self, _params: &Params) {}
    fn cud(&mut self, _params: &Params) {}
    fn cuf(&mut self, _params: &Params) {}
    fn cup(&mut self, _params: &Params) {}
    fn cuu(&mut self, _params: &Params) {}
    fn dch(&mut self, _params: &Params) {}
    fn decaln(&mut self) {}
    fn decstbm(&mut self, _params: &Params) {}
    fn decstr(&mut self) {}
    fn dl(&mut self, _params: &Params) {}
    fn ech(&mut self, _params: &Params) {}
    fn ed(&mut self, _params: &Params) {}
    fn el(&mut self, _params: &Params) {}
    fn g1d4(&mut self, _charset: Charset) {}
    fn gzd4(&mut self, _charset: Charset) {}
    fn ht(&mut self) {}
    fn hts(&mut self) {}
    fn ich(&mut self, _params: &Params) {}
    fn il(&mut self, _params: &Params) {}
    fn lf(&mut self) {}
    fn nel(&mut self) {}
    fn prv_rm(&mut self, _params: &Params) {}
    fn prv_sm(&mut self, _params: &Params) {}
    fn rc(&mut self) {}
    fn rep(&mut self, _params: &Params) {}
    fn ri(&mut self) {}
    fn ris(&mut self) {}
    fn rm(&mut self, _params: &Params) {}
    fn sc(&mut self) {}
    fn sd(&mut self, _params: &Params) {}
    fn sgr(&mut self, _params: &Params) {}
    fn si(&mut self) {}
    fn sm(&mut self, _params: &Params) {}
    fn so(&mut self) {}
    fn su(&mut self, _params: &Params) {}
    fn tbc(&mut self, _params: &Params) {}
    fn vpa(&mut self, _params: &Params) {}
    fn vpr(&mut self, _params: &Params) {}
    fn xtwinops(&mut self, _params: &Params) {}
}

impl Parser {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn feed_str<E: Executor, S: AsRef<str>>(&mut self, input: S, executor: &mut E) {
        for ch in input.as_ref().chars() {
            self.feed(ch, executor);
        }
    }

    pub fn feed<E: Executor>(&mut self, input: char, executor: &mut E) {
        let input2 = if input >= '\u{a0}' { '\u{41}' } else { input };

        match (&self.state, input2) {
            (State::Ground, '\u{20}'..='\u{7f}') => {
                executor.print(input);
            }

            (State::CsiParam, '\u{30}'..='\u{39}') | (State::CsiParam, '\u{3b}') => {
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
                self.csi_dispatch(executor, input);
            }

            (State::CsiEntry, '\u{30}'..='\u{39}') | (State::CsiEntry, '\u{3b}') => {
                self.state = State::CsiParam;
                self.param(input);
            }

            (State::Ground, '\u{00}'..='\u{17}')
            | (State::Ground, '\u{19}')
            | (State::Ground, '\u{1c}'..='\u{1f}') => {
                self.execute(executor, input);
            }

            (State::CsiEntry, '\u{40}'..='\u{7e}') => {
                self.state = State::Ground;
                self.csi_dispatch(executor, input);
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
                self.esc_dispatch(executor, input);
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

            (State::CsiParam, '\u{3a}') | (State::CsiParam, '\u{3c}'..='\u{3f}') => {
                self.state = State::CsiIgnore;
            }

            (State::Escape, '\u{30}'..='\u{4f}')
            | (State::Escape, '\u{51}'..='\u{57}')
            | (State::Escape, '\u{59}')
            | (State::Escape, '\u{5a}')
            | (State::Escape, '\u{5c}')
            | (State::Escape, '\u{60}'..='\u{7e}') => {
                self.state = State::Ground;
                self.esc_dispatch(executor, input);
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
                self.execute(executor, input);
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
                self.csi_dispatch(executor, input);
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
                self.execute(executor, input);
            }

            (State::Escape, '\u{00}'..='\u{17}')
            | (State::Escape, '\u{19}')
            | (State::Escape, '\u{1c}'..='\u{1f}') => {
                self.execute(executor, input);
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
                self.execute(executor, input);
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
                self.execute(executor, input);
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
                self.execute(executor, input);
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
                self.execute(executor, input);
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

            _ => (),
        }
    }

    fn execute<E: Executor>(&mut self, executor: &mut E, input: char) {
        match input {
            '\u{08}' => executor.bs(),
            '\u{09}' => executor.ht(),
            '\u{0a}' => executor.lf(),
            '\u{0b}' => executor.lf(),
            '\u{0c}' => executor.lf(),
            '\u{0d}' => executor.cr(),
            '\u{0e}' => executor.so(),
            '\u{0f}' => executor.si(),
            '\u{84}' => executor.lf(),
            '\u{85}' => executor.nel(),
            '\u{88}' => executor.hts(),
            '\u{8d}' => executor.ri(),
            _ => (),
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
        if input == ';' {
            self.params.0.push(0);
        } else {
            let n = self.params.0.len() - 1;
            let p = &mut self.params.0[n];
            *p = (10 * (*p as u32) + (input as u32) - 0x30) as u16;
        }
    }

    fn esc_dispatch<E: Executor>(&mut self, executor: &mut E, input: char) {
        match (self.intermediates.0.first(), input) {
            (None, c) if ('@'..='_').contains(&c) => {
                self.execute(executor, ((input as u8) + 0x40) as char)
            }

            (None, '7') => executor.sc(),
            (None, '8') => executor.rc(),

            (None, 'c') => {
                self.state = State::Ground;
                executor.ris();
            }

            (Some('#'), '8') => executor.decaln(),
            (Some('('), '0') => executor.gzd4(Charset::Drawing),
            (Some('('), _) => executor.gzd4(Charset::Ascii),
            (Some(')'), '0') => executor.g1d4(Charset::Drawing),
            (Some(')'), _) => executor.g1d4(Charset::Ascii),
            _ => (),
        }
    }

    fn csi_dispatch<E: Executor>(&mut self, executor: &mut E, input: char) {
        match (self.intermediates.0.first(), input) {
            (None, '@') => executor.ich(&self.params),
            (None, 'A') => executor.cuu(&self.params),
            (None, 'B') => executor.cud(&self.params),
            (None, 'C') => executor.cuf(&self.params),
            (None, 'D') => executor.cub(&self.params),
            (None, 'E') => executor.cnl(&self.params),
            (None, 'F') => executor.cpl(&self.params),
            (None, 'G') => executor.cha(&self.params),
            (None, 'H') => executor.cup(&self.params),
            (None, 'I') => executor.cht(&self.params),
            (None, 'J') => executor.ed(&self.params),
            (None, 'K') => executor.el(&self.params),
            (None, 'L') => executor.il(&self.params),
            (None, 'M') => executor.dl(&self.params),
            (None, 'P') => executor.dch(&self.params),
            (None, 'S') => executor.su(&self.params),
            (None, 'T') => executor.sd(&self.params),
            (None, 'W') => executor.ctc(&self.params),
            (None, 'X') => executor.ech(&self.params),
            (None, 'Z') => executor.cbt(&self.params),
            (None, '`') => executor.cha(&self.params),
            (None, 'a') => executor.cuf(&self.params),
            (None, 'b') => executor.rep(&self.params),
            (None, 'd') => executor.vpa(&self.params),
            (None, 'e') => executor.vpr(&self.params),
            (None, 'f') => executor.cup(&self.params),
            (None, 'g') => executor.tbc(&self.params),
            (None, 'h') => executor.sm(&self.params),
            (None, 'l') => executor.rm(&self.params),
            (None, 'm') => executor.sgr(&self.params),
            (None, 'r') => executor.decstbm(&self.params),
            (None, 's') => executor.sc(),
            (None, 't') => executor.xtwinops(&self.params),
            (None, 'u') => executor.rc(),
            (Some('!'), 'p') => executor.decstr(),
            (Some('?'), 'h') => executor.prv_sm(&self.params),
            (Some('?'), 'l') => executor.prv_rm(&self.params),
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
    pub fn iter(&self) -> std::slice::Iter<u16> {
        self.0.iter()
    }

    pub fn as_slice(&self) -> &[u16] {
        &self.0[..]
    }

    pub fn get(&self, i: usize, default: usize) -> usize {
        let param = *self.0.get(i).unwrap_or(&0);

        if param == 0 {
            default
        } else {
            param as usize
        }
    }
}

impl Default for Params {
    fn default() -> Self {
        let mut params = Vec::with_capacity(8);
        params.push(0);

        Self(params)
    }
}

impl From<Vec<u16>> for Params {
    fn from(values: Vec<u16>) -> Self {
        Params(values)
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
    use super::{Executor, Params, Parser};

    struct TestExecutor {
        params: Vec<u16>,
    }

    impl Executor for TestExecutor {
        fn sgr(&mut self, params: &Params) {
            self.params = params.as_slice().to_vec();
        }
    }

    #[test]
    fn params() {
        let mut parser = Parser::new();
        let mut executor = TestExecutor { params: Vec::new() };

        parser.feed_str("\x1b[;1;;23;456;m", &mut executor);

        assert_eq!(executor.params, vec![0, 1, 0, 23, 456, 0]);
    }
}
