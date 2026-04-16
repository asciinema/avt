// Based on Paul Williams' parser for ANSI-compatible video terminals:
// https://www.vt100.net/emu/dec_ansi_parser

use crate::charset::Charset;
use crate::color::Color;
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
    Sgr(SgrOps),
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

/// Number of SgrOp values that fit inline without heap allocation.
pub const SGR_OPS_INLINE_CAP: usize = 4;

/// Small-buffer-optimized collection of SgrOp values.
#[derive(Debug, Clone, PartialEq)]
pub struct SgrOps(SgrOpsStorage);

#[derive(Debug, Clone, PartialEq)]
enum SgrOpsStorage {
    Inline {
        ops: [SgrOp; SGR_OPS_INLINE_CAP],
        len: u8,
    },
    Heap(Vec<SgrOp>),
}

impl SgrOps {
    pub(crate) fn new() -> Self {
        Self(SgrOpsStorage::Inline {
            ops: [SgrOp::Reset; SGR_OPS_INLINE_CAP],
            len: 0,
        })
    }

    pub(crate) fn collect<I: IntoIterator<Item = SgrOp>>(iter: I) -> Self {
        let mut ops = Self::new();

        for op in iter {
            ops.push(op);
        }

        ops
    }

    pub(crate) fn len(&self) -> usize {
        match &self.0 {
            SgrOpsStorage::Inline { len, .. } => *len as usize,
            SgrOpsStorage::Heap(v) => v.len(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn as_slice(&self) -> &[SgrOp] {
        match &self.0 {
            SgrOpsStorage::Inline { ops, len } => &ops[..*len as usize],
            SgrOpsStorage::Heap(v) => v.as_slice(),
        }
    }

    pub(crate) fn push(&mut self, op: SgrOp) {
        match &mut self.0 {
            SgrOpsStorage::Inline { ops, len } => {
                if (*len as usize) < SGR_OPS_INLINE_CAP {
                    ops[*len as usize] = op;
                    *len += 1;
                } else {
                    let mut v = Vec::with_capacity(SGR_OPS_INLINE_CAP * 2);
                    v.extend_from_slice(ops);
                    v.push(op);
                    self.0 = SgrOpsStorage::Heap(v);
                }
            }
            SgrOpsStorage::Heap(v) => v.push(op),
        }
    }
}

impl From<Vec<SgrOp>> for SgrOps {
    fn from(v: Vec<SgrOp>) -> Self {
        if v.len() <= SGR_OPS_INLINE_CAP {
            Self::collect(v)
        } else {
            Self(SgrOpsStorage::Heap(v))
        }
    }
}

impl From<&[SgrOp]> for SgrOps {
    fn from(v: &[SgrOp]) -> Self {
        Self::collect(v.iter().copied())
    }
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

            (CsiParam | CsiEntry | CsiIntermediate, '\u{40}'..='\u{7e}') => {
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

            (OscString, '\u{20}'..='\u{7f}') => {
                self.osc_put(input);
            }

            (Escape, '\u{20}'..='\u{2f}') => {
                self.state = EscapeIntermediate;
                self.collect(input);
            }

            (EscapeIntermediate, '\u{30}'..='\u{7e}')
            | (Escape, '\u{30}'..='\u{4f}')
            | (Escape, '\u{51}'..='\u{57}')
            | (Escape, '\u{59}')
            | (Escape, '\u{5a}')
            | (Escape, '\u{5c}')
            | (Escape, '\u{60}'..='\u{7e}') => {
                self.state = Ground;
                return self.esc_dispatch(input);
            }

            (CsiEntry, '\u{3c}'..='\u{3f}') => {
                self.state = CsiParam;
                self.collect(input);
            }

            (DcsPassthrough, '\u{00}'..='\u{17}')
            | (DcsPassthrough, '\u{19}')
            | (DcsPassthrough, '\u{1c}'..='\u{7e}') => {
                self.put(input);
            }

            (CsiIgnore, '\u{40}'..='\u{7e}') => {
                self.state = Ground;
            }

            (CsiParam, '\u{3c}'..='\u{3f}')
            | (CsiIntermediate, '\u{30}'..='\u{3f}')
            | (CsiEntry, '\u{3a}') => {
                self.state = CsiIgnore;
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

            (CsiParam | CsiEntry, '\u{20}'..='\u{2f}') => {
                self.state = CsiIntermediate;
                self.collect(input);
            }

            (DcsParam, '\u{30}'..='\u{39}') | (DcsParam, '\u{3b}') => {
                self.param(input);
            }

            (DcsEntry, '\u{3c}'..='\u{3f}') => {
                self.state = DcsParam;
                self.collect(input);
            }

            (DcsEntry | DcsParam | DcsIntermediate, '\u{40}'..='\u{7e}') => {
                self.state = DcsPassthrough;
            }

            (DcsEntry | DcsParam, '\u{20}'..='\u{2f}') => {
                self.state = DcsIntermediate;
                self.collect(input);
            }

            (CsiIntermediate | EscapeIntermediate | DcsIntermediate, '\u{20}'..='\u{2f}') => {
                self.collect(input);
            }

            (DcsEntry, '\u{3a}')
            | (DcsIntermediate, '\u{30}'..='\u{3f}')
            | (DcsParam, '\u{3a}')
            | (DcsParam, '\u{3c}'..='\u{3f}') => {
                self.state = DcsIgnore;
            }

            (DcsEntry, '\u{30}'..='\u{39}') | (DcsEntry, '\u{3b}') => {
                self.state = DcsParam;
                self.param(input);
            }

            (Escape | EscapeIntermediate
            | CsiEntry | CsiParam | CsiIntermediate | CsiIgnore,
            '\u{00}'..='\u{17}')
            | (Escape | EscapeIntermediate
            | CsiEntry | CsiParam | CsiIntermediate | CsiIgnore,
            '\u{19}')
            | (Escape | EscapeIntermediate
            | CsiEntry | CsiParam | CsiIntermediate | CsiIgnore,
            '\u{1c}'..='\u{1f}') => {
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

            // DEL (0x7F) is ignored in all states except Ground and OscString
            (Escape | EscapeIntermediate
            | CsiEntry | CsiParam | CsiIntermediate | CsiIgnore
            | DcsEntry | DcsParam | DcsIntermediate | DcsPassthrough | DcsIgnore
            | SosPmApcString, '\u{7f}')

            // CsiIgnore: params and intermediates range ignored
            | (CsiIgnore, '\u{20}'..='\u{3f}')

            // C0 controls ignored in DCS entry/param/intermediate
            | (DcsEntry | DcsParam | DcsIntermediate, '\u{00}'..='\u{17}')
            | (DcsEntry | DcsParam | DcsIntermediate, '\u{19}')
            | (DcsEntry | DcsParam | DcsIntermediate, '\u{1c}'..='\u{1f}')

            // C0 controls and printable range ignored in DcsIgnore and SosPmApcString
            | (DcsIgnore | SosPmApcString, '\u{00}'..='\u{17}')
            | (DcsIgnore | SosPmApcString, '\u{19}')
            | (DcsIgnore | SosPmApcString, '\u{1c}'..='\u{7e}')

            // Some C0 controls ignored in OscString (0x07 handled above as xterm ST)
            | (OscString, '\u{00}'..='\u{06}')
            | (OscString, '\u{08}'..='\u{17}')
            | (OscString, '\u{19}')
            | (OscString, '\u{1c}'..='\u{1f}') => {}

            // input2 is always < 0xA0 due to the mapping above
            (Ground | Escape | EscapeIntermediate
            | CsiEntry | CsiParam | CsiIntermediate | CsiIgnore
            | DcsEntry | DcsParam | DcsIntermediate | DcsPassthrough | DcsIgnore
            | OscString | SosPmApcString, '\u{a0}'..='\u{10ffff}') => {
                unreachable!()
            }
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

            (None, 'm') => Some(Sgr(SgrOps::collect(SgrOpsDecoder {
                ps: &ps[..=self.cur_param],
            }))),

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

    pub(crate) fn dump(&self) -> String {
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

pub(crate) fn dump(funs: &[Function]) -> String {
    let mut seq = String::new();

    for fun in funs {
        dump_function(&mut seq, fun);
    }

    seq
}

pub(crate) fn dump_sgr_color(color: Color, base: u8) -> String {
    match color {
        Color::Indexed(c) if c < 8 => (base + c).to_string(),
        Color::Indexed(c) if c < 16 => (base + 52 + c).to_string(),
        Color::Indexed(c) => format!("{}:5:{}", base + 8, c),
        Color::RGB(c) => format!("{}:2:{}:{}:{}", base + 8, c.r, c.g, c.b),
    }
}

fn dump_function(seq: &mut String, fun: &Function) {
    use AnsiMode::*;
    use CtcOp::*;
    use DecMode::*;
    use EdScope::*;
    use ElScope::*;
    use Function::*;
    use SgrOp::*;
    use TbcScope::*;
    use XtwinopsOp::*;

    match fun {
        Bs => seq.push('\u{08}'),
        Cbt(n) => push_csi(seq, None, &[n.to_string()], 'Z'),
        Cha(n) => push_csi(seq, None, &[n.to_string()], 'G'),
        Cht(n) => push_csi(seq, None, &[n.to_string()], 'I'),
        Cnl(n) => push_csi(seq, None, &[n.to_string()], 'E'),
        Cpl(n) => push_csi(seq, None, &[n.to_string()], 'F'),
        Cr => seq.push('\r'),

        Ctc(op) => {
            let param = match op {
                Set => 0,
                ClearCurrentColumn => 2,
                ClearAll => 5,
            };

            push_csi(seq, None, &[param.to_string()], 'W');
        }

        Cub(n) => push_csi(seq, None, &[n.to_string()], 'D'),
        Cud(n) => push_csi(seq, None, &[n.to_string()], 'B'),
        Cuf(n) => push_csi(seq, None, &[n.to_string()], 'C'),
        Cup(row, col) => push_csi(seq, None, &[row.to_string(), col.to_string()], 'H'),
        Cuu(n) => push_csi(seq, None, &[n.to_string()], 'A'),
        Dch(n) => push_csi(seq, None, &[n.to_string()], 'P'),
        Decaln => push_esc(seq, Some('#'), '8'),
        Decrc => push_esc(seq, None, '8'),

        Decrst(modes) => {
            let params = modes
                .iter()
                .map(|mode| match mode {
                    CursorKeys => 1,
                    Origin => 6,
                    AutoWrap => 7,
                    TextCursorEnable => 25,
                    AltScreenBuffer => 1047,
                    SaveCursor => 1048,
                    SaveCursorAltScreenBuffer => 1049,
                })
                .map(|param| param.to_string())
                .collect::<Vec<_>>();

            push_csi(seq, Some('?'), &params, 'l');
        }

        Decsc => push_esc(seq, None, '7'),

        Decset(modes) => {
            let params = modes
                .iter()
                .map(|mode| match mode {
                    CursorKeys => 1,
                    Origin => 6,
                    AutoWrap => 7,
                    TextCursorEnable => 25,
                    AltScreenBuffer => 1047,
                    SaveCursor => 1048,
                    SaveCursorAltScreenBuffer => 1049,
                })
                .map(|param| param.to_string())
                .collect::<Vec<_>>();

            push_csi(seq, Some('?'), &params, 'h');
        }

        Decstbm(top, bottom) => {
            push_csi(seq, None, &[top.to_string(), bottom.to_string()], 'r');
        }

        Decstr => push_csi(seq, Some('!'), &[], 'p'),
        Dl(n) => push_csi(seq, None, &[n.to_string()], 'M'),
        Ech(n) => push_csi(seq, None, &[n.to_string()], 'X'),

        Ed(scope) => {
            let param = match scope {
                Below => 0,
                Above => 1,
                EdScope::All => 2,
                SavedLines => 3,
            };

            push_csi(seq, None, &[param.to_string()], 'J');
        }

        El(scope) => {
            let param = match scope {
                ToRight => 0,
                ToLeft => 1,
                ElScope::All => 2,
            };

            push_csi(seq, None, &[param.to_string()], 'K');
        }

        G1d4(charset) => push_esc(
            seq,
            Some(')'),
            match charset {
                Charset::Drawing => '0',
                Charset::Ascii => 'B',
            },
        ),

        Gzd4(charset) => push_esc(
            seq,
            Some('('),
            match charset {
                Charset::Drawing => '0',
                Charset::Ascii => 'B',
            },
        ),

        Ht => seq.push('\t'),
        Hts => push_esc(seq, None, 'H'),
        Ich(n) => push_csi(seq, None, &[n.to_string()], '@'),
        Il(n) => push_csi(seq, None, &[n.to_string()], 'L'),
        Lf => seq.push('\n'),
        Nel => push_esc(seq, None, 'E'),
        Print(ch) => seq.push(*ch),
        Rep(n) => push_csi(seq, None, &[n.to_string()], 'b'),
        Ri => push_esc(seq, None, 'M'),
        Ris => push_esc(seq, None, 'c'),

        Rm(modes) => {
            let params = modes
                .iter()
                .map(|mode| match mode {
                    Insert => 4,
                    NewLine => 20,
                })
                .map(|param| param.to_string())
                .collect::<Vec<_>>();

            push_csi(seq, None, &params, 'l');
        }

        Scorc => push_csi(seq, None, &[], 'u'),
        Scosc => push_csi(seq, None, &[], 's'),
        Sd(n) => push_csi(seq, None, &[n.to_string()], 'T'),

        Sgr(ops) => {
            if ops.is_empty() {
                // `CSI m` roundtrips to `Sgr([Reset])`, so we need a syntactically
                // valid but semantically incomplete SGR sequence for `Sgr([])`.
                seq.push_str("\x1b[38;2m");
            } else {
                let params = ops
                    .as_slice()
                    .iter()
                    .map(|op| match op {
                        Reset => "0".to_owned(),
                        SetBoldIntensity => "1".to_owned(),
                        SetFaintIntensity => "2".to_owned(),
                        SetItalic => "3".to_owned(),
                        SetUnderline => "4".to_owned(),
                        SetBlink => "5".to_owned(),
                        SetInverse => "7".to_owned(),
                        SetStrikethrough => "9".to_owned(),
                        ResetIntensity => "22".to_owned(),
                        ResetItalic => "23".to_owned(),
                        ResetUnderline => "24".to_owned(),
                        ResetBlink => "25".to_owned(),
                        ResetInverse => "27".to_owned(),
                        ResetStrikethrough => "29".to_owned(),
                        SetForegroundColor(color) => dump_sgr_color(*color, 30),
                        ResetForegroundColor => "39".to_owned(),
                        SetBackgroundColor(color) => dump_sgr_color(*color, 40),
                        ResetBackgroundColor => "49".to_owned(),
                    })
                    .collect::<Vec<_>>();

                push_csi(seq, None, &params, 'm');
            }
        }

        Si => seq.push('\u{0f}'),

        Sm(modes) => {
            let params = modes
                .iter()
                .map(|mode| match mode {
                    Insert => 4,
                    NewLine => 20,
                })
                .map(|param| param.to_string())
                .collect::<Vec<_>>();

            push_csi(seq, None, &params, 'h');
        }

        So => seq.push('\u{0e}'),
        Su(n) => push_csi(seq, None, &[n.to_string()], 'S'),

        Tbc(scope) => {
            let param = match scope {
                CurrentColumn => 0,
                TbcScope::All => 3,
            };

            push_csi(seq, None, &[param.to_string()], 'g');
        }

        Vpa(n) => push_csi(seq, None, &[n.to_string()], 'd'),
        Vpr(n) => push_csi(seq, None, &[n.to_string()], 'e'),

        Xtwinops(Resize(cols, rows)) => {
            push_csi(
                seq,
                None,
                &["8".to_owned(), rows.to_string(), cols.to_string()],
                't',
            );
        }
    }
}

fn push_esc(seq: &mut String, intermediate: Option<char>, final_char: char) {
    seq.push('\u{1b}');

    if let Some(intermediate) = intermediate {
        seq.push(intermediate);
    }

    seq.push(final_char);
}

fn push_csi(seq: &mut String, intermediate: Option<char>, params: &[String], final_char: char) {
    seq.push('\u{1b}');
    seq.push('[');

    if let Some(intermediate) = intermediate {
        seq.push(intermediate);
    }

    if let Some((first, rest)) = params.split_first() {
        seq.push_str(first);

        for param in rest {
            seq.push(';');
            seq.push_str(param);
        }
    }

    seq.push(final_char);
}

fn ansi_mode(param: &Param) -> Option<AnsiMode> {
    use AnsiMode::*;

    match param.as_u16() {
        4 => Some(Insert),
        20 => Some(NewLine),
        _ => None,
    }
}

struct SgrOpsDecoder<'a> {
    ps: &'a [Param],
}

impl<'a> Iterator for SgrOpsDecoder<'a> {
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
    use super::CtcOp;
    use super::DecMode;
    use super::EdScope;
    use super::ElScope;
    use super::Function;
    use super::Function::*;
    use super::Parser;
    use super::SgrOp::*;
    use super::SgrOps;
    use super::State;
    use super::TbcScope;
    use super::XtwinopsOp;
    use crate::charset::Charset;
    use crate::color::Color;
    use proptest::prelude::*;

    fn parse(s: &str) -> Vec<Function> {
        let mut parser = Parser::new();

        s.chars().filter_map(|ch| parser.feed(ch)).collect()
    }

    fn emit(parser: &mut Parser, input: &[char]) -> Vec<Function> {
        input.iter().filter_map(|ch| parser.feed(*ch)).collect()
    }

    fn feed(parser: &mut Parser, s: &str) {
        for ch in s.chars() {
            assert_eq!(parser.feed(ch), None);
        }
    }

    fn sgr_ops<const N: usize>(ops: [super::SgrOp; N]) -> SgrOps {
        SgrOps::from(&ops[..])
    }

    fn assert_dump(input: &str, state: State, dump: &str) {
        let mut parser = Parser::new();

        feed(&mut parser, input);

        assert_eq!(parser.state, state);
        assert_eq!(parser.dump(), dump);
    }

    fn gen_parser_char() -> impl Strategy<Value = char> {
        prop_oneof![
            prop::sample::select(vec![
                '\x1b', '\x18', '\x1a', '\u{9b}', '\u{9c}', '\u{9d}', '\u{90}', '\u{98}', '\u{9e}',
                '\u{9f}', '[', ']', 'P', 'X', '^', '_', '?', '!', ';', ':', ' ', '#', '(', ')',
                '@', 'A', 'B', 'C', 'D', 'H', 'J', 'K', 'L', 'M', 'P', 'S', 'T', 'W', 'X', 'Z',
                '`', 'a', 'b', 'd', 'e', 'f', 'g', 'h', 'l', 'm', 'p', 'r', 's', 't', 'u', '0',
                '1', '2', '3', '4', '5', '6', '7', '8', '9', '\x08', '\x09', '\x0a', '\x0d',
                '\x0e', '\x0f',
            ]),
            (0x20u8..=0x7eu8).prop_map(|b| b as char),
            prop::sample::select(vec!['日', '▒', 'ハ']),
        ]
    }

    fn gen_parser_input(max_len: usize) -> impl Strategy<Value = Vec<char>> {
        prop::collection::vec(gen_parser_char(), 0..=max_len)
    }

    fn gen_printable_text(max_len: usize) -> impl Strategy<Value = Vec<char>> {
        prop::collection::vec(
            prop_oneof![
                (0x20u8..=0x7eu8).prop_map(|b| b as char),
                prop::sample::select(vec!['日', '▒', 'ハ']),
            ],
            0..=max_len,
        )
    }

    fn gen_non_ground_prefix() -> impl Strategy<Value = Vec<char>> {
        prop_oneof![
            Just("\x1b".chars().collect()),
            Just("\x1b[".chars().collect()),
            Just("\x1b[12".chars().collect()),
            Just("\x1b[1;".chars().collect()),
            Just("\x1b[?".chars().collect()),
            Just("\x1b[ ".chars().collect()),
            Just("\x1b[:".chars().collect()),
            Just("\x1b(".chars().collect()),
            Just("\x1b]".chars().collect()),
            Just("\x1b]title".chars().collect()),
            Just("\x1bP".chars().collect()),
            Just("\x1bP1;2".chars().collect()),
            Just("\x1bP:".chars().collect()),
            Just("\x1bX".chars().collect()),
            Just("\x1bXabc".chars().collect()),
            Just("\u{9b}".chars().collect()),
            Just("\u{9b}12".chars().collect()),
            Just("\u{9d}title".chars().collect()),
            Just("\u{90}".chars().collect()),
            Just("\u{90}1;2".chars().collect()),
            Just("\u{98}".chars().collect()),
            Just("\u{98}abc".chars().collect()),
        ]
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
        assert_eq!(parse("\x1b8"), [Decrc]);
        assert_eq!(parse("\x1bc"), [Ris]);
        assert_eq!(parse("\x1bM"), [Ri]);
        assert_eq!(parse("\x1b#8"), [Decaln]);
        assert_eq!(parse("\x1b(0"), [Gzd4(Charset::Drawing)]);
        assert_eq!(parse("\x1b(B"), [Gzd4(Charset::Ascii)]);
        assert_eq!(parse("\x1b)0"), [G1d4(Charset::Drawing)]);
        assert_eq!(parse("\x1b)B"), [G1d4(Charset::Ascii)]);
    }

    #[test]
    fn parse_csi_seq() {
        // Cursor movement and positioning.
        assert_eq!(parse("\x1b[A"), [Cuu(0)]);
        assert_eq!(parse("\x1b[B"), [Cud(0)]);
        assert_eq!(parse("\x1b[2B"), [Cud(2)]);
        assert_eq!(parse("\x1b[C"), [Cuf(0)]);
        assert_eq!(parse("\x1b[3C"), [Cuf(3)]);
        assert_eq!(parse("\x1b[D"), [Cub(0)]);
        assert_eq!(parse("\x1b[4D"), [Cub(4)]);
        assert_eq!(parse("\x1b[5E"), [Cnl(5)]);
        assert_eq!(parse("\x1b[6F"), [Cpl(6)]);
        assert_eq!(parse("\x1b[G"), [Cha(0)]);
        assert_eq!(parse("\x1b[7G"), [Cha(7)]);
        assert_eq!(parse("\x1b[H"), [Cup(0, 0)]);
        assert_eq!(parse("\x1b[3;4H"), [Cup(3, 4)]);
        assert_eq!(parse("\u{9b}3;4H"), [Cup(3, 4)]);
        assert_eq!(parse("\x1b[8I"), [Cht(8)]);
        assert_eq!(parse("\x1b[2Z"), [Cbt(2)]);
        assert_eq!(parse("\x1b[`"), [Cha(0)]);
        assert_eq!(parse("\x1b[9`"), [Cha(9)]);
        assert_eq!(parse("\x1b[10a"), [Cuf(10)]);
        assert_eq!(parse("\x1b[11b"), [Rep(11)]);
        assert_eq!(parse("\x1b[12d"), [Vpa(12)]);
        assert_eq!(parse("\x1b[13e"), [Vpr(13)]);
        assert_eq!(parse("\x1b[f"), [Cup(0, 0)]);
        assert_eq!(parse("\x1b[14;15f"), [Cup(14, 15)]);

        // Erase, insert/delete, scrolling, and tab control.
        assert_eq!(parse("\x1b[@"), [Ich(0)]);
        assert_eq!(parse("\x1b[J"), [Ed(EdScope::Below)]);
        assert_eq!(parse("\x1b[0J"), [Ed(EdScope::Below)]);
        assert_eq!(parse("\x1b[1J"), [Ed(EdScope::Above)]);
        assert_eq!(parse("\x1b[2J"), [Ed(EdScope::All)]);
        assert_eq!(parse("\u{9b}2J"), [Ed(EdScope::All)]);
        assert_eq!(parse("\x1b[3J"), [Ed(EdScope::SavedLines)]);
        assert_eq!(parse("\x1b[K"), [El(ElScope::ToRight)]);
        assert_eq!(parse("\x1b[0K"), [El(ElScope::ToRight)]);
        assert_eq!(parse("\x1b[1K"), [El(ElScope::ToLeft)]);
        assert_eq!(parse("\x1b[2K"), [El(ElScope::All)]);
        assert_eq!(parse("\x1b[16L"), [Il(16)]);
        assert_eq!(parse("\x1b[17M"), [Dl(17)]);
        assert_eq!(parse("\x1b[18P"), [Dch(18)]);
        assert_eq!(parse("\x1b[19S"), [Su(19)]);
        assert_eq!(parse("\x1b[20T"), [Sd(20)]);
        assert_eq!(parse("\x1b[W"), [Ctc(CtcOp::Set)]);
        assert_eq!(parse("\x1b[2W"), [Ctc(CtcOp::ClearCurrentColumn)]);
        assert_eq!(parse("\x1b[5W"), [Ctc(CtcOp::ClearAll)]);
        assert_eq!(parse("\x1b[21X"), [Ech(21)]);
        assert_eq!(parse("\x1b[g"), [Tbc(TbcScope::CurrentColumn)]);
        assert_eq!(parse("\x1b[3g"), [Tbc(TbcScope::All)]);

        // ANSI mode setting and generic CSI operations.
        assert_eq!(
            parse("\x1b[4;20h"),
            [Sm(vec![AnsiMode::Insert, AnsiMode::NewLine])]
        );
        assert_eq!(
            parse("\x1b[4;20l"),
            [Rm(vec![AnsiMode::Insert, AnsiMode::NewLine])]
        );
        assert_eq!(parse("\x1b[m"), [Sgr(sgr_ops([Reset]))]);
        assert_eq!(parse("\x1b[2;5r"), [Decstbm(2, 5)]);
        assert_eq!(
            parse("\x1b[8;24;80t"),
            [Xtwinops(XtwinopsOp::Resize(80, 24))]
        );
        assert_eq!(parse("\x1b[s"), [Scosc]);
        assert_eq!(parse("\x1b[u"), [Scorc]);
        assert_eq!(parse("\x1b[!p"), [Decstr]);

        // DEC private modes.
        assert_eq!(parse("\x1b[?7h"), [Decset(vec![DecMode::AutoWrap])]);
        assert_eq!(parse("\u{9b}?7h"), [Decset(vec![DecMode::AutoWrap])]);
        assert_eq!(
            parse("\x1b[?6;1047h"),
            [Decset(vec![DecMode::Origin, DecMode::AltScreenBuffer])]
        );
        assert_eq!(parse("\x1b[?47h"), [Decset(vec![DecMode::AltScreenBuffer])]);
        assert_eq!(
            parse("\x1b[?1049h"),
            [Decset(vec![DecMode::SaveCursorAltScreenBuffer])]
        );
        assert_eq!(
            parse("\u{9b}?1049h"),
            [Decset(vec![DecMode::SaveCursorAltScreenBuffer])]
        );
        assert_eq!(parse("\x1b[?7l"), [Decrst(vec![DecMode::AutoWrap])]);
        assert_eq!(parse("\u{9b}?7l"), [Decrst(vec![DecMode::AutoWrap])]);
        assert_eq!(parse("\x1b[?47l"), [Decrst(vec![DecMode::AltScreenBuffer])]);
        assert_eq!(
            parse("\x1b[?6;1049l"),
            [Decrst(vec![
                DecMode::Origin,
                DecMode::SaveCursorAltScreenBuffer,
            ])]
        );
        assert_eq!(
            parse("\u{9b}?6;1049l"),
            [Decrst(vec![
                DecMode::Origin,
                DecMode::SaveCursorAltScreenBuffer,
            ])]
        );
    }

    #[test]
    fn parse_partial_and_interrupted_seq() {
        let mut parser = Parser::new();

        assert_eq!(parser.feed('\x1b'), None);
        assert_eq!(parser.feed('['), None);
        assert_eq!(parser.feed('3'), None);
        assert_eq!(parser.feed(';'), None);
        assert_eq!(parser.feed('4'), None);
        assert_eq!(parser.feed('H'), Some(Cup(3, 4)));

        assert_eq!(parser.feed('\x1b'), None);
        assert_eq!(parser.feed('['), None);
        assert_eq!(parser.feed('3'), None);
        assert_eq!(parser.feed('\x1b'), None);
        assert_eq!(parser.feed('M'), Some(Ri));

        feed(&mut parser, "\x1b");
        assert_eq!(parser.state, State::Escape);
        assert_eq!(parser.feed('\u{18}'), None);
        assert_eq!(parser.state, State::Ground);
        assert_eq!(parser.feed('A'), Some(Print('A')));

        feed(&mut parser, "\x1b[12");
        assert_eq!(parser.state, State::CsiParam);
        assert_eq!(parser.feed('\u{1a}'), None);
        assert_eq!(parser.state, State::Ground);
        assert_eq!(parser.feed('B'), Some(Print('B')));

        feed(&mut parser, "\x1b]title");
        assert_eq!(parser.state, State::OscString);
        assert_eq!(parser.feed('\u{18}'), None);
        assert_eq!(parser.state, State::Ground);
        assert_eq!(parser.feed('C'), Some(Print('C')));

        feed(&mut parser, "\x1bP1;2");
        assert_eq!(parser.state, State::DcsParam);
        assert_eq!(parser.feed('\u{1a}'), None);
        assert_eq!(parser.state, State::Ground);
        assert_eq!(parser.feed('D'), Some(Print('D')));
    }

    #[test]
    fn ignore_unsupported_seq() {
        assert_eq!(parse("\x1b[4q"), []);
        assert_eq!(parse("\x1b[9W"), []);
        assert_eq!(parse("\x1b[?9999h"), [Decset(vec![])]);
        assert_eq!(parse("\x1b[:m"), []);
        assert_eq!(parse("\x1b[1?m"), []);
        assert_eq!(parse("\x1b[ 1H"), []);
        assert_eq!(parse("\x1b[ 1m"), []);
        assert_eq!(parse("\x1b[1 q"), []);
        assert_eq!(parse("\x1b[38;2m"), [Sgr(sgr_ops([]))]);
        assert_eq!(parse("\x1b[48;5m"), [Sgr(sgr_ops([]))]);
    }

    #[test]
    fn parse_sgr_seq() {
        assert_eq!(
            parse("\x1b[;1;m"),
            [Sgr(sgr_ops([Reset, SetBoldIntensity, Reset]))]
        );

        assert_eq!(parse("\x1b[1m"), [Sgr(sgr_ops([SetBoldIntensity]))]);
        assert_eq!(parse("\x1b[2m"), [Sgr(sgr_ops([SetFaintIntensity]))]);
        assert_eq!(parse("\x1b[3m"), [Sgr(sgr_ops([SetItalic]))]);
        assert_eq!(parse("\x1b[4m"), [Sgr(sgr_ops([SetUnderline]))]);
        assert_eq!(parse("\x1b[5m"), [Sgr(sgr_ops([SetBlink]))]);
        assert_eq!(parse("\x1b[7m"), [Sgr(sgr_ops([SetInverse]))]);
        assert_eq!(parse("\x1b[9m"), [Sgr(sgr_ops([SetStrikethrough]))]);
        assert_eq!(parse("\x1b[21m"), [Sgr(sgr_ops([ResetIntensity]))]);
        assert_eq!(parse("\x1b[22m"), [Sgr(sgr_ops([ResetIntensity]))]);
        assert_eq!(parse("\x1b[23m"), [Sgr(sgr_ops([ResetItalic]))]);
        assert_eq!(parse("\x1b[24m"), [Sgr(sgr_ops([ResetUnderline]))]);
        assert_eq!(parse("\x1b[25m"), [Sgr(sgr_ops([ResetBlink]))]);
        assert_eq!(parse("\x1b[27m"), [Sgr(sgr_ops([ResetInverse]))]);
        assert_eq!(parse("\x1b[29m"), [Sgr(sgr_ops([ResetStrikethrough]))]);

        assert_eq!(
            parse("\x1b[31m"),
            [Sgr(sgr_ops([SetForegroundColor(Color::Indexed(1))]))]
        );

        assert_eq!(
            parse("\x1b[38:2:1:2:3m"),
            [Sgr(sgr_ops([SetForegroundColor(Color::rgb(1, 2, 3))]))]
        );

        assert_eq!(
            parse("\x1b[38:2::1:2:3m"),
            [Sgr(sgr_ops([SetForegroundColor(Color::rgb(1, 2, 3))]))]
        );

        assert_eq!(
            parse("\x1b[38:5:88m"),
            [Sgr(sgr_ops([SetForegroundColor(Color::Indexed(88))]))]
        );

        assert_eq!(parse("\x1b[39m"), [Sgr(sgr_ops([ResetForegroundColor]))]);

        assert_eq!(
            parse("\x1b[41m"),
            [Sgr(sgr_ops([SetBackgroundColor(Color::Indexed(1))]))]
        );

        assert_eq!(
            parse("\x1b[91m"),
            [Sgr(sgr_ops([SetForegroundColor(Color::Indexed(9))]))]
        );

        assert_eq!(
            parse("\x1b[48:2:1:2:3m"),
            [Sgr(sgr_ops([SetBackgroundColor(Color::rgb(1, 2, 3))]))]
        );

        assert_eq!(
            parse("\x1b[48:2::1:2:3m"),
            [Sgr(sgr_ops([SetBackgroundColor(Color::rgb(1, 2, 3))]))]
        );

        assert_eq!(
            parse("\x1b[48:5:99m"),
            [Sgr(sgr_ops([SetBackgroundColor(Color::Indexed(99))]))]
        );

        assert_eq!(parse("\x1b[49m"), [Sgr(sgr_ops([ResetBackgroundColor]))]);

        assert_eq!(
            parse("\x1b[104m"),
            [Sgr(sgr_ops([SetBackgroundColor(Color::Indexed(12))]))]
        );

        // legacy syntax for 24-bit color, within a larger sequence
        assert_eq!(
            parse("\x1b[1;38;2;1;2;3;48;2;1;2;3;0m"),
            [Sgr(sgr_ops([
                SetBoldIntensity,
                SetForegroundColor(Color::rgb(1, 2, 3)),
                SetBackgroundColor(Color::rgb(1, 2, 3)),
                Reset,
            ]))]
        );

        // legacy syntax for 8-bit color, within a larger sequence
        assert_eq!(
            parse("\x1b[1;38;5;88;48;5;99;0m"),
            [Sgr(sgr_ops([
                SetBoldIntensity,
                SetForegroundColor(Color::Indexed(88)),
                SetBackgroundColor(Color::Indexed(99)),
                Reset,
            ]))]
        );
    }

    #[test]
    fn parse_string_seq() {
        assert_eq!(parse("\x1b]title\x07A"), [Print('A')]);
        assert_eq!(parse("\x1b]title\x1b\\A"), [Print('A')]);
        assert_eq!(parse("\u{9d}title\u{9c}A"), [Print('A')]);
        assert_eq!(parse("\x1bPabc\u{9c}A"), [Print('A')]);
        assert_eq!(parse("\x1bPabc\x1b\\A"), [Print('A')]);
        assert_eq!(parse("\u{90}abc\u{9c}A"), [Print('A')]);
        assert_eq!(parse("\x1bXabc\u{9c}A"), [Print('A')]);
        assert_eq!(parse("\x1bXabc\x1b\\A"), [Print('A')]);
        assert_eq!(parse("\x1b^abc\u{9c}A"), [Print('A')]);
        assert_eq!(parse("\x1b^abc\x1b\\A"), [Print('A')]);
        assert_eq!(parse("\x1b_abc\u{9c}A"), [Print('A')]);
        assert_eq!(parse("\x1b_abc\x1b\\A"), [Print('A')]);
        assert_eq!(parse("\u{98}abc\u{9c}A"), [Print('A')]);
        assert_eq!(parse("\u{9e}abc\u{9c}A"), [Print('A')]);
        assert_eq!(parse("\u{9f}abc\u{9c}A"), [Print('A')]);
    }

    #[test]
    fn parse_unicode() {
        assert_eq!(parse("日"), [Print('日')]);
        assert_eq!(parse("\x1b[日A"), [Print('A')]);
    }

    #[test]
    fn dump_non_ground_states() {
        assert_dump("\x1b", State::Escape, "\x1b");
        assert_dump("\x1b(", State::EscapeIntermediate, "\x1b(");
        assert_dump("\x1b[", State::CsiEntry, "\u{9b}");
        assert_dump("\x1b[ ", State::CsiIntermediate, "\u{9b} ");
        assert_dump("\x1b[:", State::CsiIgnore, "\u{9b}\u{3a}");
        assert_dump("\x1bP", State::DcsEntry, "\u{90}");
        assert_dump("\x1bP ", State::DcsIntermediate, "\u{90} ");
        assert_dump("\x1bP1;2", State::DcsParam, "\u{90}1;2");
        assert_dump("\x1bPz", State::DcsPassthrough, "\u{90}\u{40}");
        assert_dump("\x1bP:", State::DcsIgnore, "\u{90}\u{3a}");
        assert_dump("\x1b]", State::OscString, "\u{9d}");
        assert_dump("\x1bX", State::SosPmApcString, "\u{98}");
    }

    #[test]
    fn dump() {
        let mut parser = Parser::new();

        for ch in "\x1b[;1;;38:2:1:2:3;".chars() {
            parser.feed(ch);
        }

        assert_eq!(parser.dump(), "\u{9b}0;1;0;38:2:1:2:3;0");
    }

    #[test]
    fn dump_functions_roundtrip() {
        let functions = vec![
            Bs,
            Cbt(2),
            Cha(7),
            Cht(8),
            Cnl(5),
            Cpl(6),
            Cr,
            Ctc(CtcOp::Set),
            Ctc(CtcOp::ClearCurrentColumn),
            Ctc(CtcOp::ClearAll),
            Cub(4),
            Cud(2),
            Cuf(3),
            Cup(3, 4),
            Cuu(1),
            Dch(18),
            Decaln,
            Decrc,
            Decrst(vec![]),
            Decrst(vec![
                DecMode::CursorKeys,
                DecMode::Origin,
                DecMode::AutoWrap,
                DecMode::TextCursorEnable,
                DecMode::AltScreenBuffer,
                DecMode::SaveCursor,
                DecMode::SaveCursorAltScreenBuffer,
            ]),
            Decsc,
            Decset(vec![]),
            Decset(vec![
                DecMode::CursorKeys,
                DecMode::Origin,
                DecMode::AutoWrap,
                DecMode::TextCursorEnable,
                DecMode::AltScreenBuffer,
                DecMode::SaveCursor,
                DecMode::SaveCursorAltScreenBuffer,
            ]),
            Decstbm(2, 5),
            Decstr,
            Dl(17),
            Ech(21),
            Ed(EdScope::Below),
            Ed(EdScope::Above),
            Ed(EdScope::All),
            Ed(EdScope::SavedLines),
            El(ElScope::ToRight),
            El(ElScope::ToLeft),
            El(ElScope::All),
            G1d4(Charset::Drawing),
            G1d4(Charset::Ascii),
            Gzd4(Charset::Drawing),
            Gzd4(Charset::Ascii),
            Ht,
            Hts,
            Ich(16),
            Il(16),
            Lf,
            Nel,
            Print('A'),
            Print('日'),
            Rep(11),
            Ri,
            Ris,
            Rm(vec![]),
            Rm(vec![AnsiMode::Insert, AnsiMode::NewLine]),
            Scorc,
            Scosc,
            Sd(20),
            Sgr(sgr_ops([])),
            Sgr(sgr_ops([
                Reset,
                SetBoldIntensity,
                SetFaintIntensity,
                SetItalic,
                SetUnderline,
                SetBlink,
                SetInverse,
                SetStrikethrough,
                ResetIntensity,
                ResetItalic,
                ResetUnderline,
                ResetBlink,
                ResetInverse,
                ResetStrikethrough,
                SetForegroundColor(Color::Indexed(1)),
                ResetForegroundColor,
                SetBackgroundColor(Color::rgb(1, 2, 3)),
                ResetBackgroundColor,
            ])),
            Si,
            Sm(vec![]),
            Sm(vec![AnsiMode::Insert, AnsiMode::NewLine]),
            So,
            Su(19),
            Tbc(TbcScope::CurrentColumn),
            Tbc(TbcScope::All),
            Vpa(12),
            Vpr(13),
            Xtwinops(XtwinopsOp::Resize(80, 24)),
        ];

        assert_eq!(parse(&super::dump(&functions)), functions);
    }

    proptest! {
        #[test]
        fn prop_dump_resume_equivalence(
            input in gen_parser_input(64),
            split in 0usize..65,
        ) {
            let split = split.min(input.len());
            let (prefix, suffix) = input.split_at(split);

            let mut parser1 = Parser::new();
            let _ = emit(&mut parser1, prefix);

            let mut parser2 = Parser::new();
            let dumped = parser1.dump();
            let dump_output = dumped
                .chars()
                .filter_map(|ch| parser2.feed(ch))
                .collect::<Vec<_>>();

            prop_assert!(dump_output.is_empty());
            parser1.assert_eq(&parser2);

            let suffix_output1 = emit(&mut parser1, suffix);
            let suffix_output2 = emit(&mut parser2, suffix);

            prop_assert_eq!(suffix_output1, suffix_output2);
            parser1.assert_eq(&parser2);
        }

        #[test]
        fn prop_cancel_then_continue(
            prefix in gen_non_ground_prefix(),
            cancel in prop::sample::select(vec!['\x18', '\x1a']),
            suffix in gen_printable_text(16),
        ) {
            let mut parser = Parser::new();

            let prefix_output = emit(&mut parser, &prefix);

            prop_assert!(prefix_output.is_empty());
            prop_assert_ne!(parser.state, State::Ground);

            let cancel_output = parser.feed(cancel);

            prop_assert_eq!(cancel_output, None);
            prop_assert_eq!(parser.state, State::Ground);

            let suffix_output = emit(&mut parser, &suffix);
            let expected = suffix.iter().copied().map(Function::Print).collect::<Vec<_>>();

            prop_assert_eq!(suffix_output, expected);
            prop_assert_eq!(parser.state, State::Ground);
        }
    }
}
