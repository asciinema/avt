// Based on Paul Williams' parser for ANSI-compatible video terminals:
// https://www.vt100.net/emu/dec_ansi_parser

use crate::charset::Charset;
use crate::color::Color;
use crate::dump::Dump;
use std::fmt::Display;

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

#[derive(Debug, PartialEq)]
pub enum Function {
    Bs,
    Cbt(u16),
    Cha(u16),
    Cht(u16),
    Cnl(u16),
    Cpl(u16),
    Cr,
    Ctc(CtcOp),
    Cub(u16),
    Cud(u16),
    Cuf(u16),
    Cup(u16, u16),
    Cuu(u16),
    Dch(u16),
    Decaln,
    Decrc,
    Decrst(Vec<DecMode>),
    Decsc,
    Decset(Vec<DecMode>),
    Decstbm(u16, u16),
    Decstr,
    Dl(u16),
    Ech(u16),
    Ed(EdScope),
    El(ElScope),
    G1d4(Charset),
    Gzd4(Charset),
    Ht,
    Hts,
    Ich(u16),
    Il(u16),
    Lf,
    Nel,
    Print(char),
    Rep(u16),
    Ri,
    Ris,
    Rm(Vec<AnsiMode>),
    Scorc,
    Scosc,
    Sd(u16),
    Sgr(Vec<SgrOp>),
    Si,
    Sm(Vec<AnsiMode>),
    So,
    Su(u16),
    Tbc(TbcScope),
    Vpa(u16),
    Vpr(u16),
    Xtwinops(XtwinopsOp),
}

#[derive(Debug, PartialEq)]
#[repr(u16)]
pub enum AnsiMode {
    Insert = 4,   // IRM
    NewLine = 20, // LNM
}

#[derive(Debug, PartialEq)]
pub enum CtcOp {
    Set,
    ClearCurrentColumn,
    ClearAll,
}

#[derive(Debug, PartialEq)]
#[repr(u16)]
pub enum DecMode {
    CursorKeys = 1,                   // DECCKM
    Origin = 6,                       // DECOM
    AutoWrap = 7,                     // DECAWM
    TextCursorEnable = 25,            // DECTCEM
    AltScreenBuffer = 1047,           // xterm
    SaveCursor = 1048,                // xterm
    SaveCursorAltScreenBuffer = 1049, // xterm
}

#[derive(Debug, PartialEq)]
pub enum EdScope {
    Below,
    Above,
    All,
    SavedLines,
}

#[derive(Debug, PartialEq)]
pub enum ElScope {
    ToRight,
    ToLeft,
    All,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum SgrOp {
    Reset,                     // 0
    SetBoldIntensity,          // 1
    SetFaintIntensity,         // 2
    SetItalic,                 // 3
    SetUnderline,              // 4
    SetBlink,                  // 5
    SetInverse,                // 7
    SetStrikethrough,          // 9
    ResetIntensity,            // 21, 22
    ResetItalic,               // 23
    ResetUnderline,            // 24
    ResetBlink,                // 25
    ResetInverse,              // 27
    ResetStrikethrough,        // 29
    SetForegroundColor(Color), // 30-38
    ResetForegroundColor,      // 39
    SetBackgroundColor(Color), // 40-48
    ResetBackgroundColor,      // 49
}

#[derive(Debug, PartialEq)]
pub enum TbcScope {
    CurrentColumn,
    All,
}

#[derive(Debug, PartialEq)]
pub enum XtwinopsOp {
    Resize(u16, u16),
}

impl Parser {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn feed(&mut self, input: char) -> Option<Function> {
        use State::*;

        let input2 = if input >= '\u{a0}' { '\u{41}' } else { input };

        match (&self.state, input2) {
            (Ground, '\u{20}'..='\u{7f}') => {
                return Some(Function::Print(input));
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

    fn execute(&mut self, input: char) -> Option<Function> {
        use Function::*;

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

    fn esc_dispatch(&mut self, input: char) -> Option<Function> {
        use Function::*;

        match (self.intermediate, input) {
            (None, c) if ('@'..='_').contains(&c) => self.execute(((input as u8) + 0x40) as char),

            (None, '7') => Some(Decsc),

            (None, '8') => Some(Decrc),

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

    fn csi_dispatch(&mut self, input: char) -> Option<Function> {
        use Function::*;

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

            (None, 'J') => match ps[0].as_u16() {
                0 => Some(Ed(EdScope::Below)),
                1 => Some(Ed(EdScope::Above)),
                2 => Some(Ed(EdScope::All)),
                3 => Some(Ed(EdScope::SavedLines)),
                _ => None,
            },

            (None, 'K') => match ps[0].as_u16() {
                0 => Some(El(ElScope::ToRight)),
                1 => Some(El(ElScope::ToLeft)),
                2 => Some(El(ElScope::All)),
                _ => None,
            },

            (None, 'L') => Some(Il(ps[0].as_u16())),

            (None, 'M') => Some(Dl(ps[0].as_u16())),

            (None, 'P') => Some(Dch(ps[0].as_u16())),

            (None, 'S') => Some(Su(ps[0].as_u16())),

            (None, 'T') => Some(Sd(ps[0].as_u16())),

            (None, 'W') => match ps[0].as_u16() {
                0 => Some(Ctc(CtcOp::Set)),
                2 => Some(Ctc(CtcOp::ClearCurrentColumn)),
                5 => Some(Ctc(CtcOp::ClearAll)),
                _ => None,
            },

            (None, 'X') => Some(Ech(ps[0].as_u16())),

            (None, 'Z') => Some(Cbt(ps[0].as_u16())),

            (None, '`') => Some(Cha(ps[0].as_u16())),

            (None, 'a') => Some(Cuf(ps[0].as_u16())),

            (None, 'b') => Some(Rep(ps[0].as_u16())),

            (None, 'd') => Some(Vpa(ps[0].as_u16())),

            (None, 'e') => Some(Vpr(ps[0].as_u16())),

            (None, 'f') => Some(Cup(ps[0].as_u16(), ps[1].as_u16())),

            (None, 'g') => match ps[0].as_u16() {
                0 => Some(Tbc(TbcScope::CurrentColumn)),
                3 => Some(Tbc(TbcScope::All)),
                _ => None,
            },

            (None, 'h') => Some(Sm(ps[..=self.cur_param]
                .iter()
                .filter_map(ansi_mode)
                .collect())),

            (None, 'l') => Some(Rm(ps[..=self.cur_param]
                .iter()
                .filter_map(ansi_mode)
                .collect())),

            (None, 'm') => Some(Sgr(SgrOps {
                ps: &ps[..=self.cur_param],
            }
            .collect())),

            (None, 'r') => Some(Decstbm(ps[0].as_u16(), ps[1].as_u16())),

            (None, 's') => Some(Scosc),

            (None, 't') => {
                if ps[0].as_u16() == 8 {
                    let rows = ps[1].as_u16();
                    let cols = ps[2].as_u16();

                    Some(Xtwinops(XtwinopsOp::Resize(cols, rows)))
                } else {
                    None
                }
            }

            (None, 'u') => Some(Scorc),

            (Some('!'), 'p') => Some(Decstr),

            (Some('?'), 'h') => Some(Decset(
                ps[..=self.cur_param].iter().filter_map(dec_mode).collect(),
            )),

            (Some('?'), 'l') => Some(Decrst(
                ps[..=self.cur_param].iter().filter_map(dec_mode).collect(),
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

fn ansi_mode(param: &Param) -> Option<AnsiMode> {
    use AnsiMode::*;

    match param.as_u16() {
        4 => Some(Insert),
        20 => Some(NewLine),
        _ => None,
    }
}

struct SgrOps<'a> {
    ps: &'a [Param],
}

impl<'a> Iterator for SgrOps<'a> {
    type Item = SgrOp;

    fn next(&mut self) -> Option<Self::Item> {
        use SgrOp::*;

        while let Some(param) = self.ps.first() {
            match param.parts() {
                [0] => {
                    self.ps = &self.ps[1..];

                    return Some(Reset);
                }

                [1] => {
                    self.ps = &self.ps[1..];

                    return Some(SetBoldIntensity);
                }

                [2] => {
                    self.ps = &self.ps[1..];

                    return Some(SetFaintIntensity);
                }

                [3] => {
                    self.ps = &self.ps[1..];

                    return Some(SetItalic);
                }

                [4] => {
                    self.ps = &self.ps[1..];

                    return Some(SetUnderline);
                }

                [5] => {
                    self.ps = &self.ps[1..];

                    return Some(SetBlink);
                }

                [7] => {
                    self.ps = &self.ps[1..];

                    return Some(SetInverse);
                }

                [9] => {
                    self.ps = &self.ps[1..];

                    return Some(SetStrikethrough);
                }

                [21] | [22] => {
                    self.ps = &self.ps[1..];

                    return Some(ResetIntensity);
                }

                [23] => {
                    self.ps = &self.ps[1..];

                    return Some(ResetItalic);
                }

                [24] => {
                    self.ps = &self.ps[1..];

                    return Some(ResetUnderline);
                }

                [25] => {
                    self.ps = &self.ps[1..];

                    return Some(ResetBlink);
                }

                [27] => {
                    self.ps = &self.ps[1..];

                    return Some(ResetInverse);
                }

                [29] => {
                    self.ps = &self.ps[1..];

                    return Some(ResetStrikethrough);
                }

                [param] if *param >= 30 && *param <= 37 => {
                    let color = Color::Indexed((param - 30) as u8);
                    self.ps = &self.ps[1..];

                    return Some(SetForegroundColor(color));
                }

                [38, 2, r, g, b] | [38, 2, _, r, g, b] => {
                    self.ps = &self.ps[1..];

                    return Some(SetForegroundColor(Color::rgb(*r as u8, *g as u8, *b as u8)));
                }

                [38, 5, idx] => {
                    let color = Color::Indexed(*idx as u8);
                    self.ps = &self.ps[1..];

                    return Some(SetForegroundColor(color));
                }

                [38] => match self.ps.get(1).map(|p| p.parts()) {
                    None => {
                        self.ps = &self.ps[1..];
                    }

                    Some([2]) => {
                        if let Some(b) = self.ps.get(4) {
                            let r = self.ps.get(2).unwrap().as_u16();
                            let g = self.ps.get(3).unwrap().as_u16();
                            let b = b.as_u16();
                            let color = Color::rgb(r as u8, g as u8, b as u8);
                            self.ps = &self.ps[5..];

                            return Some(SetForegroundColor(color));
                        } else {
                            self.ps = &self.ps[2..];
                        }
                    }

                    Some([5]) => {
                        if let Some(idx) = self.ps.get(2) {
                            let idx = idx.as_u16();
                            let color = Color::Indexed(idx as u8);
                            self.ps = &self.ps[3..];

                            return Some(SetForegroundColor(color));
                        } else {
                            self.ps = &self.ps[2..];
                        }
                    }

                    Some(_) => {
                        self.ps = &self.ps[1..];
                    }
                },

                [39] => {
                    self.ps = &self.ps[1..];

                    return Some(ResetForegroundColor);
                }

                [param] if *param >= 40 && *param <= 47 => {
                    let color = Color::Indexed((param - 40) as u8);
                    self.ps = &self.ps[1..];

                    return Some(SetBackgroundColor(color));
                }

                [48, 2, r, g, b] | [48, 2, _, r, g, b] => {
                    let color = Color::rgb(*r as u8, *g as u8, *b as u8);
                    self.ps = &self.ps[1..];

                    return Some(SetBackgroundColor(color));
                }

                [48, 5, idx] => {
                    let color = Color::Indexed(*idx as u8);
                    self.ps = &self.ps[1..];

                    return Some(SetBackgroundColor(color));
                }

                [48] => match self.ps.get(1).map(|p| p.parts()) {
                    None => {
                        self.ps = &self.ps[1..];
                    }

                    Some([2]) => {
                        if let Some(b) = self.ps.get(4) {
                            let r = self.ps.get(2).unwrap().as_u16();
                            let g = self.ps.get(3).unwrap().as_u16();
                            let b = b.as_u16();
                            let color = Color::rgb(r as u8, g as u8, b as u8);
                            self.ps = &self.ps[5..];

                            return Some(SetBackgroundColor(color));
                        } else {
                            self.ps = &self.ps[2..];
                        }
                    }

                    Some([5]) => {
                        if let Some(idx) = self.ps.get(2) {
                            let idx = idx.as_u16();
                            let color = Color::Indexed(idx as u8);
                            self.ps = &self.ps[3..];

                            return Some(SetBackgroundColor(color));
                        } else {
                            self.ps = &self.ps[2..];
                        }
                    }

                    Some(_) => {
                        self.ps = &self.ps[1..];
                    }
                },

                [49] => {
                    self.ps = &self.ps[1..];

                    return Some(ResetBackgroundColor);
                }

                [param] if *param >= 90 && *param <= 97 => {
                    let color = Color::Indexed((param - 90 + 8) as u8);
                    self.ps = &self.ps[1..];

                    return Some(SetForegroundColor(color));
                }

                [param] if *param >= 100 && *param <= 107 => {
                    let color = Color::Indexed((param - 100 + 8) as u8);
                    self.ps = &self.ps[1..];

                    return Some(SetBackgroundColor(color));
                }

                _ => {
                    self.ps = &self.ps[1..];
                }
            }
        }

        None
    }
}

fn dec_mode(param: &Param) -> Option<DecMode> {
    use DecMode::*;

    match param.as_u16() {
        1 => Some(CursorKeys),
        6 => Some(Origin),
        7 => Some(AutoWrap),
        25 => Some(TextCursorEnable),
        47 => Some(AltScreenBuffer), // legacy variant of 1047
        1047 => Some(AltScreenBuffer),
        1048 => Some(SaveCursor),
        1049 => Some(SaveCursorAltScreenBuffer),
        _ => None,
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

const MAX_PARAM_LEN: usize = 6;

#[derive(Debug, PartialEq, Clone)]
struct Param {
    cur_part: usize,
    pub parts: [u16; MAX_PARAM_LEN],
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

#[cfg(test)]
mod tests {
    use super::AnsiMode;
    use super::DecMode;
    use super::Function;
    use super::Function::*;
    use super::Parser;
    use super::SgrOp::*;
    use crate::color::Color;
    use crate::dump::Dump;

    fn parse(s: &str) -> Vec<Function> {
        let mut parser = Parser::new();

        s.chars().filter_map(|ch| parser.feed(ch)).collect()
    }

    #[test]
    fn parse_c0() {
        assert_eq!(parse("\x08"), [Bs]);
        assert_eq!(parse("\x0a"), [Lf]);
        assert_eq!(parse("\x0d"), [Cr]);
        assert_eq!(parse("\x0e"), [So]);
        assert_eq!(parse("\x0f"), [Si]);
    }

    #[test]
    fn parse_c1() {
        assert_eq!(parse("\u{84}"), [Lf]);
        assert_eq!(parse("\u{85}"), [Nel]);
        assert_eq!(parse("\u{88}"), [Hts]);
        assert_eq!(parse("\u{8d}"), [Ri]);
    }

    #[test]
    fn parse_esc_seq() {
        assert_eq!(parse("\x1b7"), [Decsc]);
        assert_eq!(parse("\x1bc"), [Ris]);
        assert_eq!(parse("\x1bM"), [Ri]);
    }

    #[test]
    fn parse_csi_seq() {
        assert_eq!(parse("\x1b[@"), [Ich(0)]);
        assert_eq!(parse("\x1b[3;4H"), [Cup(3, 4)]);

        assert_eq!(
            parse("\x1b[4;20h"),
            [Sm(vec![AnsiMode::Insert, AnsiMode::NewLine])]
        );

        assert_eq!(
            parse("\x1b[?6;1047h"),
            [Decset(vec![DecMode::Origin, DecMode::AltScreenBuffer])]
        );

        assert_eq!(parse("\x1b[m"), [Sgr(vec![Reset])]);
    }

    #[test]
    fn parse_sgr_seq() {
        assert_eq!(
            parse("\x1b[;1;m"),
            [Sgr(vec![Reset, SetBoldIntensity, Reset])]
        );

        assert_eq!(parse("\x1b[1m"), [Sgr(vec![SetBoldIntensity])]);
        assert_eq!(parse("\x1b[2m"), [Sgr(vec![SetFaintIntensity])]);
        assert_eq!(parse("\x1b[3m"), [Sgr(vec![SetItalic])]);
        assert_eq!(parse("\x1b[4m"), [Sgr(vec![SetUnderline])]);
        assert_eq!(parse("\x1b[5m"), [Sgr(vec![SetBlink])]);
        assert_eq!(parse("\x1b[7m"), [Sgr(vec![SetInverse])]);
        assert_eq!(parse("\x1b[9m"), [Sgr(vec![SetStrikethrough])]);
        assert_eq!(parse("\x1b[21m"), [Sgr(vec![ResetIntensity])]);
        assert_eq!(parse("\x1b[22m"), [Sgr(vec![ResetIntensity])]);
        assert_eq!(parse("\x1b[23m"), [Sgr(vec![ResetItalic])]);
        assert_eq!(parse("\x1b[24m"), [Sgr(vec![ResetUnderline])]);
        assert_eq!(parse("\x1b[25m"), [Sgr(vec![ResetBlink])]);
        assert_eq!(parse("\x1b[27m"), [Sgr(vec![ResetInverse])]);
        assert_eq!(parse("\x1b[29m"), [Sgr(vec![ResetStrikethrough])]);

        assert_eq!(
            parse("\x1b[31m"),
            [Sgr(vec![SetForegroundColor(Color::Indexed(1))])]
        );

        assert_eq!(
            parse("\x1b[38:2:1:2:3m"),
            [Sgr(vec![SetForegroundColor(Color::rgb(1, 2, 3))])]
        );

        assert_eq!(
            parse("\x1b[38:2::1:2:3m"),
            [Sgr(vec![SetForegroundColor(Color::rgb(1, 2, 3))])]
        );

        assert_eq!(
            parse("\x1b[38:5:88m"),
            [Sgr(vec![SetForegroundColor(Color::Indexed(88))])]
        );

        assert_eq!(parse("\x1b[39m"), [Sgr(vec![ResetForegroundColor])]);

        assert_eq!(
            parse("\x1b[41m"),
            [Sgr(vec![SetBackgroundColor(Color::Indexed(1))])]
        );

        assert_eq!(
            parse("\x1b[48:2:1:2:3m"),
            [Sgr(vec![SetBackgroundColor(Color::rgb(1, 2, 3))])]
        );

        assert_eq!(
            parse("\x1b[48:2::1:2:3m"),
            [Sgr(vec![SetBackgroundColor(Color::rgb(1, 2, 3))])]
        );

        assert_eq!(
            parse("\x1b[48:5:99m"),
            [Sgr(vec![SetBackgroundColor(Color::Indexed(99))])]
        );

        assert_eq!(parse("\x1b[49m"), [Sgr(vec![ResetBackgroundColor])]);

        // legacy syntax for 24-bit color, within a larger sequence
        assert_eq!(
            parse("\x1b[1;38;2;1;2;3;48;2;1;2;3;0m"),
            [Sgr(vec![
                SetBoldIntensity,
                SetForegroundColor(Color::rgb(1, 2, 3)),
                SetBackgroundColor(Color::rgb(1, 2, 3)),
                Reset,
            ])]
        );

        // legacy syntax for 8-bit color, within a larger sequence
        assert_eq!(
            parse("\x1b[1;38;5;88;48;5;99;0m"),
            [Sgr(vec![
                SetBoldIntensity,
                SetForegroundColor(Color::Indexed(88)),
                SetBackgroundColor(Color::Indexed(99)),
                Reset,
            ])]
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
