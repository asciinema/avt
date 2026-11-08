//! ANSI-compatible terminal functions and parsing, including DEC and xterm extensions.
//!
//! [`Parser`] turns text and control sequences into [`Function`] values, which
//! can also be constructed directly for [`crate::Terminal::execute`].

// Based on Paul Williams' parser for ANSI-compatible video terminals:
// https://www.vt100.net/emu/dec_ansi_parser

mod dcs;

use crate::charset::Charset;
use crate::color::Color;
use alloc::borrow::{Cow, ToOwned as _};
use alloc::format;
use alloc::string::String;
use alloc::string::ToString as _;
use alloc::vec::Vec;
use core::fmt::Display;

const PARAMS_LEN: usize = 32;
const OSC_BUFFER_LIMIT: usize = 1024;

/// Incremental parser for text and ANSI-compatible terminal control sequences.
/// Incomplete sequences continue across calls to [`Parser::feed`].
#[derive(Debug, Default)]
pub struct Parser {
    pub state: ParserState,
    params: [Param; PARAMS_LEN],
    cur_param: usize,
    intermediate: Option<char>,
    osc_buffer: String,
    dcs: dcs::Handler,
}

/// A state-machine phase of [`Parser`], excluding buffered sequence data.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum ParserState {
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

/// A terminal function. Print text may borrow the input.
#[derive(Debug, Clone, PartialEq)]
pub enum Function<'a> {
    /// Backspace.
    Bs,
    /// Cursor Backward Tabulation: tab-stop count.
    Cbt(u16),
    /// Cursor Character Absolute: column.
    Cha(u16),
    /// Cursor Forward Tabulation: tab-stop count.
    Cht(u16),
    /// Cursor Next Line: row count; moves to the first column.
    Cnl(u16),
    /// Cursor Preceding Line: row count; moves to the first column.
    Cpl(u16),
    /// Carriage Return.
    Cr,
    /// Cursor Tabulation Control.
    Ctc(CtcOp),
    /// Cursor Backward: column count.
    Cub(u16),
    /// Cursor Down: row count.
    Cud(u16),
    /// Cursor Forward: column count.
    Cuf(u16),
    /// Cursor Position: `(row, column)`.
    Cup(u16, u16),
    /// Cursor Up: row count.
    Cuu(u16),
    /// Delete Character: column count.
    Dch(u16),
    /// DEC Screen Alignment Test: fill the visible screen with `E` characters.
    Decaln,
    /// DEC Restore Cursor.
    Decrc,
    /// DEC Restore Presentation State; see [`CursorPresentation`].
    Decrsps(CursorPresentation),
    /// DEC Reset Mode.
    Decrst(DecModes),
    /// DEC Save Cursor.
    Decsc,
    /// DEC Set Mode.
    Decset(DecModes),
    /// DEC Set Tab at Every 8 Columns: replace all tab stops with defaults.
    Decst8c,
    /// DEC Set Top and Bottom Margins: `(top, bottom)`, inclusive.
    /// Zero selects the first row for `top` and the last for `bottom`.
    Decstbm(u16, u16),
    /// DEC Soft Terminal Reset.
    Decstr,
    /// Delete Line: row count.
    Dl(u16),
    /// Erase Character: column count.
    Ech(u16),
    /// Erase in Display.
    Ed(EdScope),
    /// Erase in Line.
    El(ElScope),
    /// G1 Designate 94-Set: assign the character set selected by [`Function::So`].
    G1d4(Charset),
    /// G0 Designate 94-Set: assign the character set selected by [`Function::Si`].
    Gzd4(Charset),
    /// Horizontal Tabulation: next tab stop.
    Ht,
    /// Horizontal Tabulation Set: current column.
    Hts,
    /// Insert Character: blank-column count.
    Ich(u16),
    /// Insert Line: blank-row count.
    Il(u16),
    /// Line Feed; also moves to the first column in new-line mode.
    Lf,
    /// Next Line: advance one row to the first column, scrolling if needed.
    Nel,
    /// Operating System Command.
    Osc(OscOp),
    Print(Cow<'a, str>),
    /// Repeat: count of copies of the character preceding the cursor.
    Rep(u16),
    /// Reverse Index: move up one row, scrolling down at the top margin.
    Ri,
    /// Reset to Initial State (hard reset).
    Ris,
    /// Reset Mode.
    Rm(AnsiModes),
    /// SCO Restore Cursor.
    Scorc,
    /// SCO Save Cursor.
    Scosc,
    /// Scroll Down: row count within the scrolling region.
    Sd(u16),
    /// Select Graphic Rendition.
    Sgr(SgrOps),
    /// Shift In: select the G0 character set.
    Si,
    /// Set Mode.
    Sm(AnsiModes),
    /// Shift Out: select the G1 character set.
    So,
    /// Scroll Up: row count within the scrolling region.
    Su(u16),
    /// Tabulation Clear.
    Tbc(TbcScope),
    /// Vertical Position Absolute: row.
    Vpa(u16),
    /// Vertical Position Relative: row count downward.
    Vpr(u16),
    /// Xterm Window Operations.
    Xtwinops(XtwinopsOp),
}

impl Function<'_> {
    /// Makes the function independent of its input, copying borrowed print text
    /// and moving already-owned data without cloning it.
    pub fn into_owned(self) -> Function<'static> {
        match self {
            Self::Bs => Function::Bs,
            Self::Cbt(n) => Function::Cbt(n),
            Self::Cha(n) => Function::Cha(n),
            Self::Cht(n) => Function::Cht(n),
            Self::Cnl(n) => Function::Cnl(n),
            Self::Cpl(n) => Function::Cpl(n),
            Self::Cr => Function::Cr,
            Self::Ctc(op) => Function::Ctc(op),
            Self::Cub(n) => Function::Cub(n),
            Self::Cud(n) => Function::Cud(n),
            Self::Cuf(n) => Function::Cuf(n),
            Self::Cup(row, col) => Function::Cup(row, col),
            Self::Cuu(n) => Function::Cuu(n),
            Self::Dch(n) => Function::Dch(n),
            Self::Decaln => Function::Decaln,
            Self::Decrc => Function::Decrc,
            Self::Decrsps(presentation) => Function::Decrsps(presentation),
            Self::Decrst(modes) => Function::Decrst(modes),
            Self::Decsc => Function::Decsc,
            Self::Decset(modes) => Function::Decset(modes),
            Self::Decst8c => Function::Decst8c,
            Self::Decstbm(top, bottom) => Function::Decstbm(top, bottom),
            Self::Decstr => Function::Decstr,
            Self::Dl(n) => Function::Dl(n),
            Self::Ech(n) => Function::Ech(n),
            Self::Ed(scope) => Function::Ed(scope),
            Self::El(scope) => Function::El(scope),
            Self::G1d4(charset) => Function::G1d4(charset),
            Self::Gzd4(charset) => Function::Gzd4(charset),
            Self::Ht => Function::Ht,
            Self::Hts => Function::Hts,
            Self::Ich(n) => Function::Ich(n),
            Self::Il(n) => Function::Il(n),
            Self::Lf => Function::Lf,
            Self::Nel => Function::Nel,
            Self::Osc(osc) => Function::Osc(osc),
            Self::Print(text) => Function::Print(Cow::Owned(text.into_owned())),
            Self::Rep(n) => Function::Rep(n),
            Self::Ri => Function::Ri,
            Self::Ris => Function::Ris,
            Self::Rm(modes) => Function::Rm(modes),
            Self::Scorc => Function::Scorc,
            Self::Scosc => Function::Scosc,
            Self::Sd(n) => Function::Sd(n),
            Self::Sgr(ops) => Function::Sgr(ops),
            Self::Si => Function::Si,
            Self::Sm(modes) => Function::Sm(modes),
            Self::So => Function::So,
            Self::Su(n) => Function::Su(n),
            Self::Tbc(scope) => Function::Tbc(scope),
            Self::Vpa(n) => Function::Vpa(n),
            Self::Vpr(n) => Function::Vpr(n),
            Self::Xtwinops(op) => Function::Xtwinops(op),
        }
    }
}

/// The dump-specific cursor-information form of DECRSPS.
///
/// Coordinates are one-based physical screen positions, independent of origin
/// mode and margins. Execution ignores the entire operation if either coordinate
/// is zero or outside the screen. Pending wrap is honored only at the last column,
/// independently of autowrap mode; it does not change cells or line-wrap markers.
///
/// Only `Pr;Pc;1;@;@;Sflag;0;2;@;BBBB` is accepted, with `Sflag` one of
/// `@`, `A`, `H`, or `I`. The fixed fields restore normal intensity, no underline,
/// blink, or inverse, and ASCII in G0/G1 with G0 selected. Colors, italic, and
/// strikethrough are preserved, as are saved cursors, margins, cursor visibility,
/// and other modes. Dumps restore full attributes separately through SGR and SCS.
///
/// Malformed or unsupported reports, including payloads exceeding 128 bytes,
/// are ignored as a whole. A valid report is applied on ESC or C1 ST; the
/// backslash in `ESC \` completes normal escape parsing. CAN, SUB, and other
/// C1 controls cancel an unfinished report.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct CursorPresentation {
    pub row: u16,
    pub col: u16,
    pub origin_mode: bool,
    pub pending_wrap: bool,
}

impl Default for CursorPresentation {
    fn default() -> Self {
        Self {
            row: 1,
            col: 1,
            origin_mode: false,
            pending_wrap: false,
        }
    }
}

/// A decoded Operating System Command (OSC) operation.
#[derive(Debug, PartialEq, Clone)]
pub enum OscOp {
    ShellIntegration(ShellMarker),
}

/// An OSC 133 marker delimiting shell prompt, command-input, and output phases.
/// [`crate::observer::ShellTracker`] interprets these markers into shell events.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum ShellMarker {
    /// A (FTCS_PROMPT): prompt rendering begins.
    PromptStart,
    /// B (FTCS_COMMAND_START): editable command input begins.
    CommandStart,
    /// C (FTCS_COMMAND_EXECUTED): input is submitted, output begins.
    CommandExecuted,
    /// D (FTCS_COMMAND_FINISHED): execution ends, or input is cancelled.
    CommandFinished { exit_status: Option<u8> },
}

/// A standard ANSI mode selected by [`Function::Sm`] or [`Function::Rm`].
#[derive(Debug, Copy, Clone, PartialEq)]
#[repr(u16)]
pub enum AnsiMode {
    Insert = 4,   // IRM
    NewLine = 20, // LNM
}

/// A Cursor Tabulation Control operation for horizontal tab stops.
#[derive(Debug, Clone, PartialEq)]
pub enum CtcOp {
    Set,
    ClearCurrentColumn,
    ClearAll,
}

/// A DEC-private mode, including xterm extensions, selected by
/// [`Function::Decset`] or [`Function::Decrst`].
#[derive(Debug, Copy, Clone, PartialEq)]
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

/// The region selected by Erase in Display ([`Function::Ed`]).
#[derive(Debug, Clone, PartialEq)]
pub enum EdScope {
    /// From the cursor through the end of the visible display, inclusive.
    Below,
    /// From the start of the visible display through the cursor, inclusive.
    Above,
    /// The entire visible display.
    All,
    /// Move the occupied primary-screen prefix into scrollback, then erase the
    /// entire display. On the alternate screen, erase the display.
    ScrollAll,
    /// Retained scrollback lines.
    SavedLines,
}

/// The region selected by Erase in Line ([`Function::El`]).
#[derive(Debug, Clone, PartialEq)]
pub enum ElScope {
    /// From the cursor through the end of the row, inclusive.
    ToRight,
    /// From the start of the row through the cursor, inclusive.
    ToLeft,
    All,
}

/// A decoded Select Graphic Rendition operation affecting subsequent text.
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

/// The horizontal tab stops selected by Tabulation Clear ([`Function::Tbc`]).
#[derive(Debug, Clone, PartialEq)]
pub enum TbcScope {
    CurrentColumn,
    All,
}

/// A decoded Xterm Window Operations request.
#[derive(Debug, Clone, PartialEq)]
pub enum XtwinopsOp {
    /// Request a text-area size of `(columns, rows)` in character cells.
    /// The tuple order differs from the wire format, which puts rows first.
    /// [`crate::Terminal`] exposes this to observers but does not resize itself.
    Resize(u16, u16),
}

const SGR_OPS_INLINE_CAP: usize = 4;

/// An ordered collection of [`SgrOp`] values.
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

    /// The decoded graphics-rendition operations, in execution order.
    pub fn as_slice(&self) -> &[SgrOp] {
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

const MODES_INLINE_CAP: usize = 4;

/// An ordered collection of [`AnsiMode`] values.
#[derive(Debug, Clone, PartialEq)]
pub struct AnsiModes(AnsiModesStorage);

#[derive(Debug, Clone, PartialEq)]
enum AnsiModesStorage {
    Inline {
        modes: [AnsiMode; MODES_INLINE_CAP],
        len: u8,
    },
    Heap(Vec<AnsiMode>),
}

impl AnsiModes {
    pub(crate) fn new() -> Self {
        Self(AnsiModesStorage::Inline {
            modes: [AnsiMode::Insert; MODES_INLINE_CAP],
            len: 0,
        })
    }

    pub(crate) fn one(mode: AnsiMode) -> Self {
        let mut modes = Self::new();
        modes.push(mode);
        modes
    }

    pub(crate) fn collect<I: IntoIterator<Item = AnsiMode>>(iter: I) -> Self {
        let mut v = Self::new();

        for m in iter {
            v.push(m);
        }

        v
    }

    /// The decoded ANSI modes, in execution order.
    pub fn as_slice(&self) -> &[AnsiMode] {
        match &self.0 {
            AnsiModesStorage::Inline { modes, len } => &modes[..*len as usize],
            AnsiModesStorage::Heap(v) => v.as_slice(),
        }
    }

    pub(crate) fn push(&mut self, m: AnsiMode) {
        match &mut self.0 {
            AnsiModesStorage::Inline { modes, len } => {
                if (*len as usize) < MODES_INLINE_CAP {
                    modes[*len as usize] = m;
                    *len += 1;
                } else {
                    let mut v = Vec::with_capacity(MODES_INLINE_CAP * 2);
                    v.extend_from_slice(modes);
                    v.push(m);
                    self.0 = AnsiModesStorage::Heap(v);
                }
            }
            AnsiModesStorage::Heap(v) => v.push(m),
        }
    }
}

impl From<Vec<AnsiMode>> for AnsiModes {
    fn from(v: Vec<AnsiMode>) -> Self {
        if v.len() <= MODES_INLINE_CAP {
            Self::collect(v)
        } else {
            Self(AnsiModesStorage::Heap(v))
        }
    }
}

impl From<&[AnsiMode]> for AnsiModes {
    fn from(v: &[AnsiMode]) -> Self {
        Self::collect(v.iter().copied())
    }
}

/// An ordered collection of [`DecMode`] values.
#[derive(Debug, Clone, PartialEq)]
pub struct DecModes(DecModesStorage);

#[derive(Debug, Clone, PartialEq)]
enum DecModesStorage {
    Inline {
        modes: [DecMode; MODES_INLINE_CAP],
        len: u8,
    },
    Heap(Vec<DecMode>),
}

impl DecModes {
    pub(crate) fn new() -> Self {
        Self(DecModesStorage::Inline {
            modes: [DecMode::CursorKeys; MODES_INLINE_CAP],
            len: 0,
        })
    }

    pub(crate) fn one(mode: DecMode) -> Self {
        let mut modes = Self::new();
        modes.push(mode);
        modes
    }

    pub(crate) fn collect<I: IntoIterator<Item = DecMode>>(iter: I) -> Self {
        let mut v = Self::new();

        for m in iter {
            v.push(m);
        }

        v
    }

    /// The decoded DEC modes, in execution order.
    pub fn as_slice(&self) -> &[DecMode] {
        match &self.0 {
            DecModesStorage::Inline { modes, len } => &modes[..*len as usize],
            DecModesStorage::Heap(v) => v.as_slice(),
        }
    }

    pub(crate) fn push(&mut self, m: DecMode) {
        match &mut self.0 {
            DecModesStorage::Inline { modes, len } => {
                if (*len as usize) < MODES_INLINE_CAP {
                    modes[*len as usize] = m;
                    *len += 1;
                } else {
                    let mut v = Vec::with_capacity(MODES_INLINE_CAP * 2);
                    v.extend_from_slice(modes);
                    v.push(m);
                    self.0 = DecModesStorage::Heap(v);
                }
            }
            DecModesStorage::Heap(v) => v.push(m),
        }
    }
}

impl From<Vec<DecMode>> for DecModes {
    fn from(v: Vec<DecMode>) -> Self {
        if v.len() <= MODES_INLINE_CAP {
            Self::collect(v)
        } else {
            Self(DecModesStorage::Heap(v))
        }
    }
}

impl From<&[DecMode]> for DecModes {
    fn from(v: &[DecMode]) -> Self {
        Self::collect(v.iter().copied())
    }
}

impl Parser {
    pub fn new() -> Self {
        Default::default()
    }

    /// Lazily parses a fragment into terminal functions, borrowing print text
    /// directly from the input without allocation or copying. Emitted functions
    /// borrow only the input, so the parser can be reused or dropped while they
    /// remain alive. Use [`Function::into_owned`] to outlive the input as well.
    ///
    /// Printable text is emitted in nonempty runs ending at C0/C1 controls, DEL,
    /// or the fragment boundary, with no fixed size limit. DEL (U+007F) is ignored
    /// in Ground and excluded from print runs. Unfinished control sequences
    /// continue in the next fragment; printable text is never buffered across
    /// fragments. Run boundaries may change in future versions.
    ///
    /// Exhaust the iterator to consume the whole fragment. Dropping it early
    /// leaves the unconsumed suffix unprocessed, including any trailing controls.
    ///
    /// ```
    /// use avt::{Parser, Terminal};
    ///
    /// let mut parser = Parser::new();
    /// let mut terminal = Terminal::new(80, 24)?;
    /// terminal.execute(parser.feed("hello\x1b[31m world"));
    /// # Ok::<(), avt::InvalidSize>(())
    /// ```
    pub fn feed<'a>(&mut self, mut input: &'a str) -> impl Iterator<Item = Function<'a>> {
        core::iter::from_fn(move || self.next_function(&mut input))
    }

    // Each call releases the parser borrow before Terminal invokes observers.
    #[inline(always)]
    pub(crate) fn next_function<'a>(&mut self, input: &mut &'a str) -> Option<Function<'a>> {
        if input.is_empty() {
            return None;
        }

        if self.state == ParserState::Ground {
            if let Some(text) = Self::take_print_run(input) {
                return Some(Function::Print(Cow::Borrowed(text)));
            }
        }

        self.next_control_function(input)
    }

    // Find the first control using u64 arithmetic to test eight bytes at once.
    // In valid UTF-8, stops are C0 bytes, DEL and C2 80..9F pairs (C1 controls).
    // Each byte's high bit marks a candidate. Loads may split characters,
    // but all stops begin a character, so returned offsets are UTF-8 boundaries.
    #[inline(always)]
    fn print_run_end(input: &str) -> usize {
        const ONES: u64 = 0x0101010101010101;
        const HIGHS: u64 = 0x8080808080808080;

        if input.len() < 8 {
            return input
                .find(|ch| ch < '\u{20}' || ('\u{7f}'..='\u{9f}').contains(&ch))
                .unwrap_or(input.len());
        }

        let bytes = input.as_bytes();

        // Short print runs are common even when the input fragment is large.
        // Probe bytes without decoding or changing the word-loop alignment.
        for offset in 0..2 {
            let byte = bytes[offset];

            if byte < 0x20 || byte == 0x7f || (byte == 0xc2 && bytes[offset + 1] < 0xa0) {
                return offset;
            }
        }

        let mut rest = bytes;

        while let Some(chunk) = rest.first_chunk::<8>() {
            let word = u64::from_le_bytes(*chunk);

            // Subtracting 0x20 flags C0; adding 1 flags DEL. !word excludes non-ASCII.
            let ascii = (word.wrapping_sub(ONES * 0x20) | word.wrapping_add(ONES)) & !word;

            // XOR makes C2 zero; subtracting 1 flags that lane. &word excludes ASCII.
            let c2 = (word ^ (ONES * 0xc2)).wrapping_sub(ONES) & word;

            // After C2, bit 5 distinguishes printable A0..BF from C1 80..9F.
            // Shift that bit onto C2's marker to clear printable pairs.
            // A final-byte C2 has no lookahead here and remains a candidate.
            let mask = (ascii | (c2 & !(word >> 6))) & HIGHS;

            if mask == 0 {
                rest = &rest[8..];
                continue;
            }

            // Little-endian puts the first candidate lowest; borrow-induced marks follow it.
            let offset = mask.trailing_zeros() as usize / 8;

            // A boundary C2 needs lookahead. Valid UTF-8 guarantees a following
            // continuation byte; below A0 means a C1 control.
            if rest[offset] != 0xc2 || rest[offset + 1] < 0xa0 {
                return input.len() - rest.len() + offset;
            }

            // Skip the printable C2 pair.
            rest = &rest[offset + 2..];
        }

        // Fewer than eight bytes remain, possibly starting inside a character.
        for (offset, &byte) in rest.iter().enumerate() {
            if byte < 0x20 || byte == 0x7f || (byte == 0xc2 && rest[offset + 1] < 0xa0) {
                return input.len() - rest.len() + offset;
            }
        }

        input.len()
    }

    #[inline(always)]
    fn take_print_run<'a>(input: &mut &'a str) -> Option<&'a str> {
        let end = Self::print_run_end(input);

        if end == 0 {
            return None;
        }

        let (text, rest) = input.split_at(end);
        *input = rest;

        Some(text)
    }

    // Parse controls until a function is emitted, including text following an
    // ignored sequence. State transitions stay inlined within this loop.
    #[inline(never)]
    fn next_control_function<'a>(&mut self, input: &mut &'a str) -> Option<Function<'a>> {
        let mut chars = input.chars();

        while let Some(ch) = chars.next() {
            if let Some(function) = self.advance(ch) {
                *input = chars.as_str();

                return Some(function);
            }

            if self.state == ParserState::Ground {
                *input = chars.as_str();

                if let Some(text) = Self::take_print_run(input) {
                    return Some(Function::Print(Cow::Borrowed(text)));
                }
            }
        }

        *input = chars.as_str();

        None
    }

    #[inline(always)]
    fn advance(&mut self, input: char) -> Option<Function<'static>> {
        use ParserState::*;

        // DCS framing is independent of the selected command's payload syntax.
        if self.state == DcsPassthrough {
            match input {
                '\u{1b}' => {
                    self.state = Escape;
                    self.clear();

                    return self.dcs.end(dcs::End::Escape);
                }

                '\u{9c}' => {
                    self.state = Ground;

                    return self.dcs.end(dcs::End::St);
                }

                '\u{18}' | '\u{1a}' | '\u{80}'..='\u{9f}' => {
                    self.dcs.end(dcs::End::Cancel);
                    // Process the cancelling control through the normal transitions below.
                }

                '\u{7f}' => return None,

                _ => {
                    self.dcs.put(input);

                    return None;
                }
            }
        }

        let input2 = if input >= '\u{a0}' { '\u{41}' } else { input };

        match (&self.state, input2) {
            (Ground, '\u{20}'..='\u{7e}') => {
                unreachable!("ground-state printing is handled in next_function");
            }

            (CsiParam, '\u{30}'..='\u{3b}') => {
                self.param(input);
            }

            (_, '\u{1b}') => {
                let exit_fn = self.exit_state();
                self.state = Escape;
                self.clear();

                return exit_fn;
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
                self.osc_buffer.clear();
            }

            (OscString, '\u{07}') => {
                // 0x07 is xterm non-ANSI variant of transition to ground
                let exit_fn = self.exit_state();
                self.state = Ground;

                return exit_fn;
            }

            (OscString, '\u{18}') | (OscString, '\u{1a}') => {
                self.state = Ground;
                self.osc_buffer.clear();
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

            (DcsEntry | DcsParam, '\u{30}'..='\u{39}' | '\u{3b}') => {
                self.state = DcsParam;
                self.dcs_param(input);
            }

            (DcsEntry, '\u{3c}'..='\u{3f}') => {
                self.state = DcsParam;
                self.collect(input);
            }

            (DcsEntry | DcsParam | DcsIntermediate, '\u{40}'..='\u{7e}') => {
                self.state = DcsPassthrough;

                self.dcs.hook(
                    &self.params[..=self.cur_param],
                    self.intermediate,
                    input,
                );
            }

            (DcsEntry | DcsParam, '\u{20}'..='\u{2f}') => {
                if self.intermediate.is_some() {
                    self.state = DcsIgnore;
                } else {
                    self.state = DcsIntermediate;
                    self.collect(input);
                }
            }

            (CsiIntermediate | EscapeIntermediate, '\u{20}'..='\u{2f}') => {
                self.collect(input);
            }

            (DcsEntry, '\u{3a}')
            | (DcsIntermediate, '\u{20}'..='\u{3f}')
            | (DcsParam, '\u{3a}')
            | (DcsParam, '\u{3c}'..='\u{3f}') => {
                self.state = DcsIgnore;
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
                let exit_fn = self.exit_state();
                self.state = Ground;

                return exit_fn;
            }

            (_, '\u{9d}') => {
                self.state = OscString;
                self.osc_buffer.clear();
            }

            (_, '\u{90}') => {
                self.state = DcsEntry;
                self.clear();
            }

            (_, '\u{9b}') => {
                self.state = CsiEntry;
                self.clear();
            }

            // DEL is ignored in all states except OscString.
            (Ground | Escape | EscapeIntermediate
            | CsiEntry | CsiParam | CsiIntermediate | CsiIgnore
            | DcsEntry | DcsParam | DcsIntermediate | DcsIgnore
            | SosPmApcString, '\u{7f}')

            // CsiIgnore: params and intermediates range ignored
            | (CsiIgnore, '\u{20}'..='\u{3f}')

            // C0 controls ignored in DCS entry/param/intermediate
            | (DcsEntry | DcsParam | DcsIntermediate, '\u{00}'..='\u{17}')
            | (DcsEntry | DcsParam | DcsIntermediate, '\u{19}')
            | (DcsEntry | DcsParam | DcsIntermediate, '\u{1c}'..='\u{1f}')

            // Unrecognized control strings are ignored.
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

            // DCS payload is handled above.
            (DcsPassthrough, _) => unreachable!(),
        }

        None
    }

    fn execute(&mut self, input: char) -> Option<Function<'static>> {
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

    fn esc_dispatch(&mut self, input: char) -> Option<Function<'static>> {
        use Function::*;

        match (self.intermediate, input) {
            (None, c) if ('@'..='_').contains(&c) => self.execute(((input as u8) + 0x40) as char),

            (None, '7') => Some(Decsc),

            (None, '8') => Some(Decrc),

            (None, 'c') => {
                self.state = ParserState::Ground;
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

    fn csi_dispatch(&mut self, input: char) -> Option<Function<'static>> {
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
                22 => Some(Ed(EdScope::ScrollAll)),
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

            (None, 'h') => Some(Sm(AnsiModes::collect(
                ps[..=self.cur_param].iter().filter_map(ansi_mode),
            ))),

            (None, 'l') => Some(Rm(AnsiModes::collect(
                ps[..=self.cur_param].iter().filter_map(ansi_mode),
            ))),

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

            (Some('?'), 'W') if self.cur_param == 0 && ps[0].parts() == [5] => Some(Decst8c),

            (Some('?'), 'h') => Some(Decset(DecModes::collect(
                ps[..=self.cur_param].iter().filter_map(dec_mode),
            ))),

            (Some('?'), 'l') => Some(Decrst(DecModes::collect(
                ps[..=self.cur_param].iter().filter_map(dec_mode),
            ))),

            _ => None,
        }
    }

    fn dcs_param(&mut self, input: char) {
        if input.is_ascii_digit()
            && u32::from(self.params[self.cur_param].as_u16()) * 10 + input as u32 - '0' as u32
                > u32::from(u16::MAX)
        {
            self.state = ParserState::DcsIgnore;
        } else {
            self.param(input);
        }
    }

    fn osc_put(&mut self, input: char) {
        if self.osc_buffer.len() + input.len_utf8() <= OSC_BUFFER_LIMIT {
            self.osc_buffer.push(input);
        }
    }

    fn exit_state(&mut self) -> Option<Function<'static>> {
        match self.state {
            ParserState::OscString => self.osc_dispatch(),
            _ => None,
        }
    }

    fn osc_dispatch(&mut self) -> Option<Function<'static>> {
        let operation = parse_osc(&self.osc_buffer);
        self.osc_buffer.clear();

        operation
    }

    pub(crate) fn dump(&self) -> String {
        use ParserState::*;

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

                let params = self.params[..=self.cur_param]
                    .iter()
                    .map(|param| param.to_string())
                    .collect::<Vec<_>>()
                    .join(";");

                let s = &format!("\u{90}{params}{intermediates}");
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

            DcsPassthrough => self.dcs.dump(&mut seq),

            DcsIgnore => {
                seq.push_str("\u{90}\u{3a}");
            }

            OscString => {
                seq.push('\u{9d}');
                seq.push_str(&self.osc_buffer);
            }

            SosPmApcString => {
                seq.push('\u{98}');
            }
        }

        seq
    }

    #[cfg(test)]
    pub(crate) fn assert_continuation_equivalent(&self, other: &Parser) {
        use ParserState::*;

        assert_eq!(self.state, other.state);
        assert_eq!(self.dcs, other.dcs);

        // Supported CSI intermediate sequences do not use collected parameters.
        if matches!(self.state, CsiParam | DcsParam | DcsIntermediate) {
            assert_eq!(self.cur_param, other.cur_param);

            assert_eq!(
                self.params[..=self.cur_param],
                other.params[..=other.cur_param]
            );
        }

        if self.state == EscapeIntermediate
            || self.state == CsiIntermediate
            || self.state == CsiParam
            || self.state == DcsIntermediate
            || self.state == DcsParam
        {
            assert_eq!(self.intermediate, other.intermediate);
        }

        if self.state == OscString {
            assert_eq!(self.osc_buffer, other.osc_buffer);
        }
    }
}

pub(crate) fn encode(funs: &[Function<'_>]) -> String {
    let mut seq = String::new();

    for fun in funs {
        encode_function(&mut seq, fun);
    }

    seq
}

// `base` is 30 for foreground or 40 for background.
fn encode_sgr_color_params(color: Color, base: u8) -> String {
    match color {
        Color::Indexed(c) if c < 8 => (base + c).to_string(),
        Color::Indexed(c) if c < 16 => (base + 52 + c).to_string(),
        Color::Indexed(c) => format!("{}:5:{}", base + 8, c),
        Color::RGB(c) => format!("{}:2:{}:{}:{}", base + 8, c.r, c.g, c.b),
    }
}

fn encode_function(seq: &mut String, fun: &Function<'_>) {
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

        Decrsps(presentation) => {
            let flags = char::from(
                0x40 | u8::from(presentation.origin_mode)
                    | (u8::from(presentation.pending_wrap) << 3),
            );

            seq.push_str(&format!(
                "\u{90}1$t{};{};1;@;@;{flags};0;2;@;BBBB\u{9c}",
                presentation.row, presentation.col,
            ));
        }

        Decrst(modes) => {
            let params = modes
                .as_slice()
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
                .as_slice()
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

        Decst8c => push_csi(seq, Some('?'), &["5".to_owned()], 'W'),

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
                ScrollAll => 22,
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

        Osc(osc) => match osc {
            OscOp::ShellIntegration(kind) => {
                let payload = match kind {
                    ShellMarker::PromptStart => "133;A",
                    ShellMarker::CommandStart => "133;B",
                    ShellMarker::CommandExecuted => "133;C",
                    ShellMarker::CommandFinished { .. } => "133;D",
                };

                seq.push('\u{9d}');
                seq.push_str(payload);

                if let ShellMarker::CommandFinished {
                    exit_status: Some(status),
                } = kind
                {
                    seq.push(';');
                    seq.push_str(&status.to_string());
                }

                seq.push('\u{9c}');
            }
        },

        Print(text) => seq.push_str(text),
        Rep(n) => push_csi(seq, None, &[n.to_string()], 'b'),
        Ri => push_esc(seq, None, 'M'),
        Ris => push_esc(seq, None, 'c'),

        Rm(modes) => {
            let params = modes
                .as_slice()
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
                        SetForegroundColor(color) => encode_sgr_color_params(*color, 30),
                        ResetForegroundColor => "39".to_owned(),
                        SetBackgroundColor(color) => encode_sgr_color_params(*color, 40),
                        ResetBackgroundColor => "49".to_owned(),
                    })
                    .collect::<Vec<_>>();

                push_csi(seq, None, &params, 'm');
            }
        }

        Si => seq.push('\u{0f}'),

        Sm(modes) => {
            let params = modes
                .as_slice()
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

fn parse_osc(payload: &str) -> Option<Function<'static>> {
    let mut parts = payload.splitn(2, ';');
    let num = parts.next()?;

    match num {
        "133" => {
            let rest = parts.next().unwrap_or("");
            let mut params = rest.split(';');
            let subtype = params.next()?;

            let kind = match subtype {
                "A" => ShellMarker::PromptStart,
                "B" => ShellMarker::CommandStart,
                "C" => ShellMarker::CommandExecuted,

                "D" => ShellMarker::CommandFinished {
                    // Missing, malformed, and out-of-range statuses are unknown.
                    exit_status: params
                        .next()
                        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                        .and_then(|s| s.parse().ok()),
                },

                _ => return None,
            };

            Some(Function::Osc(OscOp::ShellIntegration(kind)))
        }

        _ => None,
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

struct SgrOpsDecoder<'a> {
    ps: &'a [Param],
}

impl Iterator for SgrOpsDecoder<'_> {
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
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
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
    use super::AnsiModes;
    use super::CtcOp;
    use super::CursorPresentation;
    use super::DecMode;
    use super::DecModes;
    use super::EdScope;
    use super::ElScope;
    use super::Function;
    use super::Function::*;
    use super::Parser;
    use super::ParserState;
    use super::SgrOp::*;
    use super::SgrOps;
    use super::TbcScope;
    use super::XtwinopsOp;
    use crate::charset::Charset;
    use crate::color::Color;
    use alloc::borrow::Cow;
    use alloc::format;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;
    use proptest::prelude::*;

    fn parse(s: &str) -> Vec<Function<'_>> {
        let mut parser = Parser::new();

        parser.feed(s).collect()
    }

    fn print(ch: char) -> Function<'static> {
        Print(String::from(ch).into())
    }

    fn emit(parser: &mut Parser, input: &[char]) -> Vec<Function<'static>> {
        parser
            .feed(&input.iter().collect::<String>())
            .map(Function::into_owned)
            .collect()
    }

    fn feed(parser: &mut Parser, s: &str) {
        assert!(parser.feed(s).next().is_none());
    }

    fn coalesce<'a>(functions: impl IntoIterator<Item = Function<'a>>) -> Vec<Function<'a>> {
        let mut result = Vec::new();

        for function in functions {
            if let (Some(Print(previous)), Print(text)) = (result.last_mut(), &function) {
                previous.to_mut().push_str(text);
            } else {
                result.push(function);
            }
        }

        result
    }

    #[test]
    fn print_runs_borrow_input_independently_of_the_parser() {
        let first = String::from("hé\x1b[3");
        let second = String::from("1m界");

        let (first_functions, second_functions) = {
            let mut parser = Parser::new();
            let first_functions: Vec<_> = parser.feed(&first).collect();
            let second_functions: Vec<_> = parser.feed(&second).collect();

            (first_functions, second_functions)
        };

        let [Print(Cow::Borrowed(text))] = first_functions.as_slice() else {
            panic!("expected borrowed print run");
        };

        assert_eq!(*text, "hé");
        assert_eq!(text.as_ptr(), first.as_ptr());

        let [Sgr(ops), Print(Cow::Borrowed(text))] = second_functions.as_slice() else {
            panic!("expected completed SGR and borrowed print run");
        };

        assert_eq!(ops.as_slice(), [SetForegroundColor(Color::Indexed(1))]);
        assert_eq!(*text, "界");
        assert_eq!(text.as_ptr(), second[2..].as_ptr());
    }

    #[test]
    fn into_owned_moves_existing_allocations() {
        let text = String::from("owned print run");
        let address = text.as_ptr();

        let Print(Cow::Owned(text)) = Print(text.into()).into_owned() else {
            panic!("expected owned print run");
        };

        assert_eq!(text, "owned print run");
        assert_eq!(text.as_ptr(), address);

        let ops = SgrOps::from(vec![SetBoldIntensity; super::SGR_OPS_INLINE_CAP + 1]);
        let address = ops.as_slice().as_ptr();

        let Sgr(ops) = Sgr(ops).into_owned() else {
            panic!("expected SGR");
        };

        assert_eq!(ops.as_slice().as_ptr(), address);
        assert!(ops.as_slice().iter().all(|op| *op == SetBoldIntensity));
    }

    #[test]
    fn print_runs_stop_at_controls_and_preserve_unicode() {
        assert_eq!(
            parse("hé界\x7f🦀\r\nabc\0def\u{85}ghi\u{9b}31m\u{a0}日"),
            [
                Print("hé界".into()),
                Print("🦀".into()),
                Cr,
                Lf,
                Print("abc".into()),
                Print("def".into()),
                Nel,
                Print("ghi".into()),
                Sgr(sgr_ops([SetForegroundColor(Color::Indexed(1))])),
                Print("\u{a0}日".into()),
            ]
        );
    }

    #[test]
    fn long_print_runs_borrow_whole_printable_spans() {
        let text = "xé界🦀".repeat(8192);
        // Exercise resuming printing after an ignored control as well as C1.
        let input = format!("\0{text}\u{85}{text}");
        let functions = parse(&input);

        let [
            Print(Cow::Borrowed(first)),
            Nel,
            Print(Cow::Borrowed(second)),
        ] = functions.as_slice()
        else {
            panic!("expected two borrowed print runs separated by NEL");
        };

        assert_eq!(*first, text);
        assert_eq!(*second, text);
        assert_eq!(first.as_ptr(), input[1..].as_ptr());

        assert_eq!(
            second.as_ptr(),
            input[1 + text.len() + '\u{85}'.len_utf8()..].as_ptr()
        );
    }

    fn assert_print_run_matches_reference(input: &str) {
        let end = input
            .find(|ch| ch < '\u{20}' || ('\u{7f}'..='\u{9f}').contains(&ch))
            .unwrap_or(input.len());

        let mut rest = input;
        let text = Parser::take_print_run(&mut rest);
        assert_eq!(text, (end > 0).then_some(&input[..end]));
        assert_eq!(rest, &input[end..]);
    }

    #[test]
    fn print_runs_handle_word_boundaries_and_tails() {
        for offset in 0..16 {
            let prefix = "x".repeat(offset);

            for ch in ('\0'..='\u{ff}').chain(['界', '🦀']) {
                for suffix in ["", "é界🦀", "\u{a0}£©±µ¿", "abcdefghi"] {
                    let input = format!("{prefix}{ch}{suffix}");
                    assert_print_run_matches_reference(&input);
                    assert_print_run_matches_reference(&format!("{input}\u{85}tail"));
                }
            }
        }
    }

    #[test]
    fn dropping_feed_does_not_consume_the_remaining_input() {
        let mut parser = Parser::new();
        let _ = parser.feed("\x1b[31");
        assert_eq!(parser.state, ParserState::Ground);
        assert_eq!(parser.feed("ab\x1b[31").next(), Some(Print("ab".into())));
        assert_eq!(parser.state, ParserState::Ground);
        assert!(parser.feed("\x1b[31").next().is_none());
        assert_eq!(parser.state, ParserState::CsiParam);
    }

    #[test]
    fn yields_embedded_controls_before_finishing_csi() {
        let mut parser = Parser::new();
        assert_eq!(parser.feed("\x1b[3\n1mxy").next(), Some(Lf));
        assert_eq!(parser.state, ParserState::CsiParam);

        assert_eq!(
            parser.feed("1mxy").collect::<Vec<_>>(),
            [
                Sgr(sgr_ops([SetForegroundColor(Color::Indexed(1))])),
                Print("xy".into()),
            ]
        );

        assert_eq!(parser.state, ParserState::Ground);
    }

    fn sgr_ops<const N: usize>(ops: [super::SgrOp; N]) -> SgrOps {
        SgrOps::from(&ops[..])
    }

    fn ansi_modes<const N: usize>(modes: [AnsiMode; N]) -> AnsiModes {
        AnsiModes::from(&modes[..])
    }

    fn dec_modes<const N: usize>(modes: [DecMode; N]) -> DecModes {
        DecModes::from(&modes[..])
    }

    fn assert_dump_continuation(
        input: &str,
        state: ParserState,
        suffix: &str,
        expected: &[Function<'_>],
    ) {
        let mut original = Parser::new();
        feed(&mut original, input);
        assert_eq!(original.state, state);
        let mut restored = Parser::new();
        feed(&mut restored, &original.dump());
        original.assert_continuation_equivalent(&restored);

        for parser in [&mut original, &mut restored] {
            let output: Vec<_> = parser.feed(suffix).collect();
            assert_eq!(output, expected);
        }

        original.assert_continuation_equivalent(&restored);
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
            Just("\x1bP1$".chars().collect()),
            Just("\x1bP1$t1;4;1;@;@;I;0;2;@;BBBB".chars().collect()),
            Just("\x1bP1$t1;4;\x1b".chars().collect()),
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
    fn parse_cursor_information_reports() {
        for start in ["\x1bP", "\u{90}"] {
            for end in ["\x1b", "\x1b\\", "\u{9c}"] {
                for (flags, origin_mode, pending_wrap) in [
                    ('@', false, false),
                    ('A', true, false),
                    ('H', false, true),
                    ('I', true, true),
                ] {
                    assert_eq!(
                        parse(&format!("{start}1$t3;65535;1;@;@;{flags};0;2;@;BBBB{end}")),
                        [Decrsps(CursorPresentation {
                            row: 3,
                            col: u16::MAX,
                            origin_mode,
                            pending_wrap,
                        })]
                    );
                }
            }
        }
    }

    #[test]
    fn cursor_reports_reject_malformed_and_unsupported_fields() {
        for (index, invalid) in [
            (0, ""),
            (0, "+1"),
            (0, "-1"),
            (0, "65536"),
            (1, ""),
            (1, "65536"),
            (2, "0"),
            (2, "2"),
            (3, ""),
            (3, "A"),
            (3, "O"),
            (3, "P"),
            (3, "`@"),
            (3, "AA"),
            (4, "A"),
            (5, "B"),
            (5, "D"),
            (5, "P"),
            (5, "`@"),
            (6, "1"),
            (6, "2"),
            (7, "0"),
            (7, "4"),
            (8, "A"),
            (8, "B"),
            (8, "L"),
            (8, "P"),
            (9, "B"),
            (9, "BBBBB"),
            (9, "ABBB"),
            (9, "0BBB"),
            (9, "B0BB"),
            (9, "BB%5%5"),
            (9, "B%5BB"),
            (9, "BBB%"),
            (9, "BBBB;"),
            (9, "BBB日"),
            (9, "BB\x07B"),
        ] {
            let mut fields = ["1", "4", "1", "@", "@", "I", "0", "2", "@", "BBBB"];
            fields[index] = invalid;
            let input = format!("\x1bP1$t{}\x1b\\X", fields.join(";"));
            assert_eq!(parse(&input), [print('X')], "{input:?}");
        }

        for header in [
            "0$t", "2$t", "$t", "1;1$t", "?1$t", "1!$t", "1$$t", "65537$t", "1$u",
        ] {
            let input = format!("\x1bP{header}1;4;1;@;@;I;0;2;@;BBBB\x1b\\X");
            assert_eq!(parse(&input), [print('X')], "{input:?}");
        }

        assert!(parse("\x1bP1$t1;4;1;@;@;I;0;2;@\x1b\\").is_empty());
    }

    #[test]
    fn cursor_reports_finish_on_esc_before_escape_dispatch() {
        let report = "\x1bP1$t1;1;1;@;@;@;0;2;@;BBBB";
        let function = Decrsps(CursorPresentation::default());
        let mut parser = Parser::new();
        feed(&mut parser, report);
        assert_eq!(parser.state, ParserState::DcsPassthrough);
        assert_eq!(parser.feed("\x1b").next(), Some(function.clone()));
        assert_eq!(parser.state, ParserState::Escape);
        assert_eq!(parser.dcs, super::dcs::Handler::default());
        assert_eq!(parser.dump(), "\x1b");
        assert_eq!(parser.feed("\\").next(), None);
        assert_eq!(parser.state, ParserState::Ground);

        for suffix in [
            "\x1b\\",
            "\x1b\x18",
            "\x1b\x1a",
            "\x1b\x1b\\",
            "\x1b\u{9c}",
            "\x1b\x7f\\",
            "\x1b#\\",
        ] {
            assert_eq!(
                parse(&format!("{report}{suffix}X")),
                [function.clone(), print('X')]
            );
        }

        for (suffix, expected) in [
            ("\x1b[2;3H", Cup(2, 3)),
            ("\x1bM", Ri),
            ("\x1b\n\\", Lf),
            ("\x1b(0", Gzd4(Charset::Drawing)),
            ("\x1bc", Ris),
        ] {
            assert_eq!(
                parse(&format!("{report}{suffix}X")),
                [function.clone(), expected, print('X')]
            );
        }

        assert_eq!(
            parse(&format!("{report}\x1bP1$t2;3;1;@;@;@;0;2;@;BBBB\u{9c}")),
            [
                function,
                Decrsps(CursorPresentation {
                    row: 2,
                    col: 3,
                    ..CursorPresentation::default()
                }),
            ]
        );
    }

    #[test]
    fn cursor_reports_cancel_on_can_sub_and_other_c1_controls() {
        let report = "\x1bP1$t1;1;1;@;@;@;0;2;@;BBBB";

        for cancel in [0x18, 0x1a].into_iter().chain(0x80..=0x9f) {
            if cancel == 0x9c {
                continue;
            }

            let mut parser = Parser::new();
            feed(&mut parser, report);
            let mut expected = Parser::new();

            assert_eq!(
                parser
                    .feed(char::from(cancel).encode_utf8(&mut [0; 4]))
                    .next(),
                expected
                    .feed(char::from(cancel).encode_utf8(&mut [0; 4]))
                    .next()
            );

            parser.assert_continuation_equivalent(&expected);
            assert_eq!(parser.dcs, super::dcs::Handler::default());

            // The next recognized report starts with fresh handler state.
            let suffix = format!("{report}\u{9c}X");
            let output: Vec<_> = parser.feed(&suffix).collect();

            assert_eq!(output, [Decrsps(CursorPresentation::default()), print('X')]);
        }

        for suffix in [
            "\u{90}0xignored",
            "\u{90}1$",
            "\u{9d}ignored",
            "\u{98}ignored",
            "\u{9e}ignored",
            "\u{9f}ignored",
        ] {
            let mut parser = Parser::new();
            feed(&mut parser, &format!("{report}{suffix}"));
            assert_eq!(parser.dcs, super::dcs::Handler::default());
            assert_eq!(parser.feed("\u{9c}").next(), None);
        }

        for (suffix, expected) in [("\u{9b}2;3H", Cup(2, 3)), ("\u{84}", Lf)] {
            assert_eq!(parse(&format!("{report}{suffix}X")), [expected, print('X')]);
        }
    }

    #[test]
    fn cursor_report_capture_is_bounded_without_truncation() {
        let payload = "1;1;1;@;@;@;0;2;@;BBBB";

        for extra in [0, 1, 1024] {
            let zeros = "0".repeat(128 - payload.len() + extra);
            let mut parser = Parser::new();
            feed(&mut parser, &format!("\x1bP1$t{zeros}{payload}"));
            let dumped = parser.dump();
            assert!(dumped.len() <= "\u{90}1$t".len() + 128);
            let mut restored = Parser::new();
            feed(&mut restored, &dumped);
            parser.assert_continuation_equivalent(&restored);

            let expected = (extra == 0).then_some(Decrsps(CursorPresentation::default()));

            assert_eq!(parser.feed("\u{9c}").next(), expected);
            assert_eq!(restored.feed("\u{9c}").next(), expected);
            assert_eq!(parser.feed("X").next(), Some(print('X')));
        }
    }

    #[test]
    fn dcs_payload_controls_and_rejections_preserve_framing() {
        let report = "\x1bP1$t1;1;1;@;@;@;0;2;@;BBBB";

        for extra in ["\x07", "\n", "日", "extra"] {
            for end in ["\x1b\\", "\u{9c}"] {
                let mut parser = Parser::new();
                feed(&mut parser, &format!("{report}{extra}"));
                assert_eq!(parser.state, ParserState::DcsPassthrough);
                let mut restored = Parser::new();
                feed(&mut restored, &parser.dump());
                parser.assert_continuation_equivalent(&restored);

                for parser in [&mut parser, &mut restored] {
                    let suffix = format!("{end}X{report}{end}Y");
                    let output: Vec<_> = parser.feed(&suffix).collect();

                    assert_eq!(
                        output,
                        [
                            print('X'),
                            Decrsps(CursorPresentation::default()),
                            print('Y')
                        ]
                    );
                }
            }
        }

        // DEL is ignored by DCS framing rather than delivered to the command.
        assert_eq!(
            parse("\x1bP1$t1;1;1;@;@;@;0;2;@;BB\x7fBB\u{9c}"),
            [Decrsps(CursorPresentation::default())]
        );
    }

    #[test]
    fn unknown_dcs_commands_ignore_payload_and_resume() {
        let payload = "1;1;1;@;@;@;0;2;@;BBBB\x07\n日".repeat(128);

        for header in ["0;0;0q", "0p", "$q", "+q", "2$t", "1$u", "tmux;"] {
            for suffix in ["\x1b\\X", "\u{9c}X", "\x1b[2;3HX"] {
                assert_dump_continuation(
                    &format!("\x1bP{header}{payload}"),
                    ParserState::DcsPassthrough,
                    suffix,
                    &parse(suffix),
                );
            }
        }
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
    fn parse_decst8c_across_fragments() {
        for input in ["\x1b[?5W", "\u{9b}?5W", "\x1b[?05W"] {
            for (split, _) in input.char_indices() {
                let mut parser = Parser::new();
                feed(&mut parser, &input[..split]);

                assert_eq!(parser.feed(&input[split..]).collect::<Vec<_>>(), [Decst8c]);
            }
        }
    }

    #[test]
    fn parse_decst8c_rejects_unsupported_parameters() {
        for params in ["", "0", "4", "5;0", "5;", "5:0"] {
            assert_eq!(parse(&format!("\x1b[?{params}WX")), [print('X')]);
        }
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
        assert_eq!(parse("\x1b[22J"), [Ed(EdScope::ScrollAll)]);
        assert_eq!(parse("\u{9b}22J"), [Ed(EdScope::ScrollAll)]);
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
            [Sm(ansi_modes([AnsiMode::Insert, AnsiMode::NewLine]))]
        );
        assert_eq!(
            parse("\x1b[4;20l"),
            [Rm(ansi_modes([AnsiMode::Insert, AnsiMode::NewLine]))]
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
        assert_eq!(parse("\x1b[?7h"), [Decset(dec_modes([DecMode::AutoWrap]))]);
        assert_eq!(parse("\u{9b}?7h"), [Decset(dec_modes([DecMode::AutoWrap]))]);
        assert_eq!(
            parse("\x1b[?6;1047h"),
            [Decset(dec_modes([
                DecMode::Origin,
                DecMode::AltScreenBuffer
            ]))]
        );
        assert_eq!(
            parse("\x1b[?47h"),
            [Decset(dec_modes([DecMode::AltScreenBuffer]))]
        );
        assert_eq!(
            parse("\x1b[?1049h"),
            [Decset(dec_modes([DecMode::SaveCursorAltScreenBuffer]))]
        );
        assert_eq!(
            parse("\u{9b}?1049h"),
            [Decset(dec_modes([DecMode::SaveCursorAltScreenBuffer]))]
        );
        assert_eq!(parse("\x1b[?7l"), [Decrst(dec_modes([DecMode::AutoWrap]))]);
        assert_eq!(parse("\u{9b}?7l"), [Decrst(dec_modes([DecMode::AutoWrap]))]);
        assert_eq!(
            parse("\x1b[?47l"),
            [Decrst(dec_modes([DecMode::AltScreenBuffer]))]
        );
        assert_eq!(
            parse("\x1b[?6;1049l"),
            [Decrst(dec_modes([
                DecMode::Origin,
                DecMode::SaveCursorAltScreenBuffer,
            ]))]
        );
        assert_eq!(
            parse("\u{9b}?6;1049l"),
            [Decrst(dec_modes([
                DecMode::Origin,
                DecMode::SaveCursorAltScreenBuffer,
            ]))]
        );
    }

    #[test]
    fn parse_partial_and_interrupted_seq() {
        let mut parser = Parser::new();

        assert_eq!(parser.feed("\x1b").next(), None);
        assert_eq!(parser.feed("[").next(), None);
        assert_eq!(parser.feed("3").next(), None);
        assert_eq!(parser.feed(";").next(), None);
        assert_eq!(parser.feed("4").next(), None);
        assert_eq!(parser.feed("H").next(), Some(Cup(3, 4)));

        assert_eq!(parser.feed("\x1b").next(), None);
        assert_eq!(parser.feed("[").next(), None);
        assert_eq!(parser.feed("3").next(), None);
        assert_eq!(parser.feed("\x1b").next(), None);
        assert_eq!(parser.feed("M").next(), Some(Ri));

        feed(&mut parser, "\x1b");
        assert_eq!(parser.state, ParserState::Escape);
        assert_eq!(parser.feed("\u{18}").next(), None);
        assert_eq!(parser.state, ParserState::Ground);
        assert_eq!(parser.feed("A").next(), Some(print('A')));

        feed(&mut parser, "\x1b[12");
        assert_eq!(parser.state, ParserState::CsiParam);
        assert_eq!(parser.feed("\u{1a}").next(), None);
        assert_eq!(parser.state, ParserState::Ground);
        assert_eq!(parser.feed("B").next(), Some(print('B')));

        feed(&mut parser, "\x1b]title");
        assert_eq!(parser.state, ParserState::OscString);
        assert_eq!(parser.feed("\u{18}").next(), None);
        assert_eq!(parser.state, ParserState::Ground);
        assert_eq!(parser.feed("C").next(), Some(print('C')));

        feed(&mut parser, "\x1bP1;2");
        assert_eq!(parser.state, ParserState::DcsParam);
        assert_eq!(parser.feed("\u{1a}").next(), None);
        assert_eq!(parser.state, ParserState::Ground);
        assert_eq!(parser.feed("D").next(), Some(print('D')));
    }

    #[test]
    fn ignore_unsupported_seq() {
        assert_eq!(parse("\x1b[4q"), []);
        assert_eq!(parse("\x1b[9W"), []);
        assert_eq!(parse("\x1b[?9999h"), [Decset(dec_modes([]))]);
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
        assert_eq!(parse("\x1b]title\x07A"), [print('A')]);
        assert_eq!(parse("\x1b]title\x1b\\A"), [print('A')]);
        assert_eq!(parse("\u{9d}title\u{9c}A"), [print('A')]);
        assert_eq!(parse("\x1bPabc\u{9c}A"), [print('A')]);
        assert_eq!(parse("\x1bPabc\x1b\\A"), [print('A')]);
        assert_eq!(parse("\u{90}abc\u{9c}A"), [print('A')]);
        assert_eq!(parse("\x1bXabc\u{9c}A"), [print('A')]);
        assert_eq!(parse("\x1bXabc\x1b\\A"), [print('A')]);
        assert_eq!(parse("\x1b^abc\u{9c}A"), [print('A')]);
        assert_eq!(parse("\x1b^abc\x1b\\A"), [print('A')]);
        assert_eq!(parse("\x1b_abc\u{9c}A"), [print('A')]);
        assert_eq!(parse("\x1b_abc\x1b\\A"), [print('A')]);
        assert_eq!(parse("\u{98}abc\u{9c}A"), [print('A')]);
        assert_eq!(parse("\u{9e}abc\u{9c}A"), [print('A')]);
        assert_eq!(parse("\u{9f}abc\u{9c}A"), [print('A')]);
    }

    #[test]
    fn parse_osc_parameters_and_unsupported_commands() {
        use super::OscOp;
        use super::ShellMarker::*;

        // Extra parameters tail is ignored
        assert_eq!(
            parse("\x1b]133;A;aid=12345;cl=m\x07"),
            [Osc(OscOp::ShellIntegration(PromptStart))]
        );

        for (status, expected) in [
            ("0", Some(0)),
            ("1;extra=x", Some(1)),
            ("", None),
            ("256", None),
            ("-1", None),
            ("+0", None),
            ("oops", None),
        ] {
            assert_eq!(
                parse(&format!("\x1b]133;D;{status}\x07")),
                [Osc(OscOp::ShellIntegration(CommandFinished {
                    exit_status: expected
                }))]
            );
        }

        // Unknown subtypes produce no function
        assert_eq!(parse("\x1b]133;Z\x07"), []);
        assert_eq!(parse("\x1b]133\x07"), []);

        // Unsupported OSC commands are ignored.
        assert_eq!(parse("\x1b]0;title\x07"), []);
        assert_eq!(parse("\x1b]title\x07"), []);
    }

    #[test]
    fn all_shell_markers_accept_bel_and_st() {
        use super::ShellMarker::*;

        for (payload, marker) in [
            ("A", PromptStart),
            ("B", CommandStart),
            ("C", CommandExecuted),
            ("D", CommandFinished { exit_status: None }),
            (
                "D;255",
                CommandFinished {
                    exit_status: Some(255),
                },
            ),
        ] {
            for intro in ["\x1b]", "\u{9d}"] {
                for end in ["\x07", "\x1b\\", "\u{9c}"] {
                    assert_eq!(
                        parse(&format!("{intro}133;{payload}{end}")),
                        [Osc(super::OscOp::ShellIntegration(marker))]
                    );
                }
            }
        }
    }

    #[test]
    fn osc_payload_dispatches_on_esc_before_entering_escape() {
        let mut parser = Parser::new();

        feed(&mut parser, "\x1b]133;A");
        assert_eq!(parser.state, ParserState::OscString);

        // ESC should dispatch the OSC and then enter Escape
        let out = parser.feed("\x1b").next();

        assert_eq!(
            out,
            Some(Osc(super::OscOp::ShellIntegration(
                super::ShellMarker::PromptStart
            )))
        );

        assert_eq!(parser.state, ParserState::Escape);

        // Following \ ends the ST sequence with no further function emitted
        assert_eq!(parser.feed("\\").next(), None);
        assert_eq!(parser.state, ParserState::Ground);
    }

    #[test]
    fn osc_cancelled_by_can_or_sub_emits_nothing() {
        for cancel in ['\u{18}', '\u{1a}'] {
            let mut parser = Parser::new();
            feed(&mut parser, "\x1b]133;D;42");
            assert_eq!(parser.feed(cancel.encode_utf8(&mut [0; 4])).next(), None);
            assert_eq!(parser.state, ParserState::Ground);
            // The parser remains usable after cancellation.
            feed(&mut parser, "\x1b]133;B");

            assert_eq!(
                parser.feed("\x07").next(),
                Some(Osc(super::OscOp::ShellIntegration(
                    super::ShellMarker::CommandStart
                )))
            );
        }
    }

    #[test]
    fn parse_unicode() {
        assert_eq!(parse("日"), [print('日')]);
        assert_eq!(parse("\x1b[日A"), [print('A')]);
    }

    #[test]
    fn dump_restores_non_ground_state_continuations() {
        for (input, state, suffix, expected) in [
            ("\x1b", ParserState::Escape, "7X", vec![Decsc, print('X')]),
            (
                "\x1b[?5",
                ParserState::CsiParam,
                "WX",
                vec![Decst8c, print('X')],
            ),
            (
                "\x1b(",
                ParserState::EscapeIntermediate,
                "0X",
                vec![Gzd4(Charset::Drawing), print('X')],
            ),
            (
                "\x1b[",
                ParserState::CsiEntry,
                "3;4HX",
                vec![Cup(3, 4), print('X')],
            ),
            (
                "\x1b[ ",
                ParserState::CsiIntermediate,
                "qX",
                vec![print('X')],
            ),
            ("\x1b[:", ParserState::CsiIgnore, "mX", vec![print('X')]),
            (
                "\x1bP",
                ParserState::DcsEntry,
                "qignored\x1b\\X",
                vec![print('X')],
            ),
            (
                "\x1bP ",
                ParserState::DcsIntermediate,
                "qignored\x1b\\X",
                vec![print('X')],
            ),
            (
                "\x1bP1;2",
                ParserState::DcsParam,
                "qignored\x1b\\X",
                vec![print('X')],
            ),
            (
                "\x1bPz",
                ParserState::DcsPassthrough,
                "ignored\x1b\\X",
                vec![print('X')],
            ),
            (
                "\x1bP:",
                ParserState::DcsIgnore,
                "ignored\x1b\\X",
                vec![print('X')],
            ),
            (
                "\x1b]",
                ParserState::OscString,
                "133;A\x07X",
                vec![
                    Osc(super::OscOp::ShellIntegration(
                        super::ShellMarker::PromptStart,
                    )),
                    print('X'),
                ],
            ),
            (
                "\x1bX",
                ParserState::SosPmApcString,
                "ignored\x1b\\X",
                vec![print('X')],
            ),
        ] {
            assert_dump_continuation(input, state, suffix, &expected);
        }
    }

    #[test]
    fn dump_restores_empty_and_colon_separated_parameters() {
        assert_dump_continuation(
            "\x1b[;1;;38:2:1:2:3;",
            ParserState::CsiParam,
            "mX",
            &[
                Sgr(sgr_ops([
                    Reset,
                    SetBoldIntensity,
                    Reset,
                    SetForegroundColor(Color::rgb(1, 2, 3)),
                    Reset,
                ])),
                print('X'),
            ],
        );
    }

    #[test]
    fn dump_resumes_intermediates_with_prior_parameters() {
        for (prefix, suffix, expected) in [
            ("\x1b[1;2!", "pX", vec![Decstr, print('X')]),
            ("\x1bP1;2 ", "qignored\x1b\\X", vec![print('X')]),
        ] {
            let mut original = Parser::new();
            feed(&mut original, prefix);
            let mut restored = Parser::new();
            feed(&mut restored, &original.dump());
            original.assert_continuation_equivalent(&restored);

            for parser in [&mut original, &mut restored] {
                let output: Vec<_> = parser.feed(suffix).collect();
                assert_eq!(output, expected);
            }

            original.assert_continuation_equivalent(&restored);
        }
    }

    #[test]
    fn encode_functions_roundtrip() {
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
            Decrst(dec_modes([])),
            Decrst(dec_modes([
                DecMode::CursorKeys,
                DecMode::Origin,
                DecMode::AutoWrap,
                DecMode::TextCursorEnable,
                DecMode::AltScreenBuffer,
                DecMode::SaveCursor,
                DecMode::SaveCursorAltScreenBuffer,
            ])),
            Decsc,
            Decset(dec_modes([])),
            Decset(dec_modes([
                DecMode::CursorKeys,
                DecMode::Origin,
                DecMode::AutoWrap,
                DecMode::TextCursorEnable,
                DecMode::AltScreenBuffer,
                DecMode::SaveCursor,
                DecMode::SaveCursorAltScreenBuffer,
            ])),
            Decst8c,
            Decstbm(2, 5),
            Decstr,
            Dl(17),
            Ech(21),
            Ed(EdScope::Below),
            Ed(EdScope::Above),
            Ed(EdScope::All),
            Ed(EdScope::ScrollAll),
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
            Osc(super::OscOp::ShellIntegration(
                super::ShellMarker::PromptStart,
            )),
            Osc(super::OscOp::ShellIntegration(
                super::ShellMarker::CommandStart,
            )),
            Osc(super::OscOp::ShellIntegration(
                super::ShellMarker::CommandExecuted,
            )),
            Osc(super::OscOp::ShellIntegration(
                super::ShellMarker::CommandFinished { exit_status: None },
            )),
            Osc(super::OscOp::ShellIntegration(
                super::ShellMarker::CommandFinished {
                    exit_status: Some(255),
                },
            )),
            Print("A日".into()),
            Rep(11),
            Ri,
            Ris,
            Rm(ansi_modes([])),
            Rm(ansi_modes([AnsiMode::Insert, AnsiMode::NewLine])),
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
            Sm(ansi_modes([])),
            Sm(ansi_modes([AnsiMode::Insert, AnsiMode::NewLine])),
            So,
            Su(19),
            Tbc(TbcScope::CurrentColumn),
            Tbc(TbcScope::All),
            Vpa(12),
            Vpr(13),
            Xtwinops(XtwinopsOp::Resize(80, 24)),
        ];

        let owned: Vec<_> = {
            let input = super::encode(&functions);
            let mut parser = Parser::new();

            parser.feed(&input).map(Function::into_owned).collect()
        };

        assert_eq!(owned, functions);
    }

    proptest! {
        #[test]
        fn prop_print_runs_match_character_scanning(
            prefix in prop::collection::vec(
                prop_oneof![
                    prop::char::range('\u{20}', '\u{7e}'),
                    prop::char::range('\u{a0}', '\u{bf}'),
                    prop::char::range('\u{a0}', char::MAX),
                ],
                0..=128,
            ),
            suffix in prop::collection::vec(any::<char>(), 0..=128),
        ) {
            let input: String = prefix.into_iter().chain(suffix).collect();
            assert_print_run_matches_reference(&input);
        }

        #[test]
        fn prop_feed_is_independent_of_fragment_boundaries(
            input in prop::collection::vec(
                prop_oneof![3 => gen_parser_char(), 1 => any::<char>()],
                0..=256,
            ),
            chunk_len in 1..=32usize,
        ) {
            let mut whole = Parser::new();
            let expected = coalesce(emit(&mut whole, &input));
            let mut chunked = Parser::new();
            let mut functions = Vec::new();

            for chunk in input.chunks(chunk_len) {
                functions.extend(emit(&mut chunked, chunk));
            }

            prop_assert_eq!(coalesce(functions), expected);
            whole.assert_continuation_equivalent(&chunked);
        }

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
            let dump_output = parser2.feed(&dumped).collect::<Vec<_>>();

            prop_assert!(dump_output.is_empty());
            parser1.assert_continuation_equivalent(&parser2);

            let suffix_output1 = emit(&mut parser1, suffix);
            let suffix_output2 = emit(&mut parser2, suffix);

            prop_assert_eq!(suffix_output1, suffix_output2);
            parser1.assert_continuation_equivalent(&parser2);
        }

        #[test]
        fn prop_cursor_report_roundtrip_and_resume(
            presentation in crate::tests::gen_cursor_presentation(),
            split in any::<usize>(),
            start in prop::sample::select(vec!["\x1bP", "\u{90}"]),
            end in prop::sample::select(vec!["\x1b", "\x1b\\", "\u{9c}"]),
        ) {
            let function = Decrsps(presentation);

            let input: Vec<_> = super::encode(core::slice::from_ref(&function))
                .replace('\u{90}', start)
                .replace('\u{9c}', end)
                .chars()
                .collect();

            let split = split % (input.len() + 1);
            let mut original = Parser::new();
            let prefix_output = emit(&mut original, &input[..split]);
            let mut restored = Parser::new();
            feed(&mut restored, &original.dump());
            original.assert_continuation_equivalent(&restored);

            let suffix_output = emit(&mut original, &input[split..]);
            prop_assert_eq!(emit(&mut restored, &input[split..]), &suffix_output[..]);
            original.assert_continuation_equivalent(&restored);

            let output: Vec<_> = prefix_output.into_iter().chain(suffix_output).collect();
            prop_assert_eq!(output, [function]);
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
            prop_assert_ne!(parser.state, ParserState::Ground);

            let mut encoded = [0; 4];
            let cancel_output = parser.feed(cancel.encode_utf8(&mut encoded)).next();

            prop_assert_eq!(cancel_output, None);
            prop_assert_eq!(parser.state, ParserState::Ground);
            prop_assert_eq!(&parser.dcs, &super::dcs::Handler::default());

            let suffix_output = emit(&mut parser, &suffix);

            let expected: Vec<_> = (!suffix.is_empty())
                .then(|| Print(suffix.iter().collect::<String>().into()))
                .into_iter()
                .collect();

            prop_assert_eq!(suffix_output, expected);
            prop_assert_eq!(parser.state, ParserState::Ground);
        }
    }
}
