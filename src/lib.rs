// The parser is based on Paul Williams' parser for ANSI-compatible video
// terminals: https://www.vt100.net/emu/dec_ansi_parser

use std::ops::Range;
use rgb::RGB8;
use serde::ser::{Serialize, Serializer, SerializeMap, SerializeTuple};


#[derive(Debug, Copy, Clone, PartialEq)]
pub enum State {
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

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Color {
    Indexed(u8),
    RGB(RGB8)
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Pen {
    pub foreground: Option<Color>,
    pub background: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub blink: bool,
    pub inverse: bool
}

#[derive(Debug, Copy, Clone)]
struct Cell(char, Pen);

#[derive(Debug)]
pub struct Segment(Vec<char>, Pen);

#[derive(Debug, PartialEq)]
enum Charset {
    G0,
    G1
}

#[derive(Debug, PartialEq)]
enum BufferType {
    Primary,
    Alternate
}

#[derive(Debug, PartialEq)]
struct SavedCtx {
    cursor_x: usize,
    cursor_y: usize,
    pen: Pen,
    origin_mode: bool,
    auto_wrap_mode: bool
}

type Line = Vec<Cell>;

#[derive(Debug)]
pub struct VT {
    // parser
    pub state: State,

    // interpreter
    params: Vec<u16>,
    intermediates: Vec<char>,

    // screen
    pub columns: usize,
    pub rows: usize,
    buffer: Vec<Line>,
    alternate_buffer: Vec<Line>,
    active_buffer_type: BufferType,
    cursor_x: usize,
    cursor_y: usize,
    cursor_visible: bool,
    pen: Pen,
    charset: Charset,
    tabs: Vec<usize>,
    insert_mode: bool,
    origin_mode: bool,
    auto_wrap_mode: bool,
    new_line_mode: bool,
    next_print_wraps: bool,
    top_margin: usize,
    bottom_margin: usize,
    saved_ctx: SavedCtx,
    alternate_saved_ctx: SavedCtx,
    affected_lines: Vec<bool>
}

const SPECIAL_GFX_CHARS: [char; 31] = [
    '♦', '▒', '␉', '␌', '␍', '␊', '°', '±', '␤', '␋',
    '┘', '┐', '┌', '└', '┼', '⎺', '⎻', '─', '⎼', '⎽',
    '├', '┤', '┴', '┬', '│', '≤', '≥', 'π', '≠', '£',
    '⋅'
];

impl Pen {
    fn new() -> Pen {
        Pen {
            foreground: None,
            background: None,
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            blink: false,
            inverse: false
        }
    }

    fn sgr_seq(&self) -> String {
        let mut s = "\x1b[0".to_owned();

        if let Some(c) = self.foreground {
            s.push_str(&format!(";{}", c.sgr_params(30)));
        }

        if let Some(c) = self.background {
            s.push_str(&format!(";{}", c.sgr_params(40)));
        }

        if self.bold {
            s.push_str(";1");
        }

        if self.italic {
            s.push_str(";3");
        }

        if self.underline {
            s.push_str(";4");
        }

        if self.blink {
            s.push_str(";5");
        }

        if self.inverse {
            s.push_str(";7");
        }

        if self.strikethrough {
            s.push_str(";9");
        }

        s.push('m');

        s
    }
}

impl Color {
    fn sgr_params(&self, base: u8) -> String {
        match self {
            Color::Indexed(c) if *c < 8 => {
                format!("{}", base + c)
            }

            Color::Indexed(c) if *c < 16 => {
                format!("{}", base + 52 + c)
            }

            Color::Indexed(c) => {
                format!("{};5;{}", base + 8, c)
            }

            Color::RGB(c) => {
                format!("{};2;{};{};{}", base + 8, c.r, c.g, c.b)
            }
        }
    }
}

impl Cell {
    fn blank() -> Cell {
        Cell(' ', Pen::new())
    }
}

impl Charset {
    fn translate(&self, input: char) -> char {
        if ('\x60'..'\x7f').contains(&input) {
            match self {
                Charset::G0 => input,
                Charset::G1 => SPECIAL_GFX_CHARS[(input as usize) - 0x60],
            }
        } else {
            input
        }
    }
}

impl SavedCtx {
    fn new() -> SavedCtx {
        SavedCtx {
            cursor_x: 0,
            cursor_y: 0,
            pen: Pen::new(),
            origin_mode: false,
            auto_wrap_mode: true
        }
    }
}

impl VT {
    pub fn new(columns: usize, rows: usize) -> Self {
        assert!(columns > 0);
        assert!(rows > 0);

        let buffer = VT::new_buffer(columns, rows);
        let alternate_buffer = buffer.clone();

        VT {
            state: State::Ground,
            params: Vec::new(),
            intermediates: Vec::new(),
            columns: columns,
            rows: rows,
            buffer: buffer,
            alternate_buffer: alternate_buffer,
            active_buffer_type: BufferType::Primary,
            tabs: VT::default_tabs(columns),
            cursor_x: 0,
            cursor_y: 0,
            cursor_visible: true,
            pen: Pen::new(),
            charset: Charset::G0,
            insert_mode: false,
            origin_mode: false,
            auto_wrap_mode: true,
            new_line_mode: false,
            next_print_wraps: false,
            top_margin: 0,
            bottom_margin: (rows - 1),
            saved_ctx: SavedCtx::new(),
            alternate_saved_ctx: SavedCtx::new(),
            affected_lines: vec![true; rows]
        }
    }

    fn new_buffer(columns: usize, rows: usize) -> Vec<Line> {
        vec![vec![Cell::blank(); columns]; rows]
    }

    fn blank_line(&self) -> Line {
        vec![self.blank_cell(); self.columns]
    }

    fn blank_cell(&self) -> Cell {
        Cell(' ', self.pen)
    }

    fn default_tabs(columns: usize) -> Vec<usize> {
        let mut tabs = vec![];

        for t in (8..columns).step_by(8) {
            tabs.push(t);
        }

        tabs
    }

    pub fn cursor(&self) -> Option<(usize, usize)> {
        if self.cursor_visible {
            Some((self.cursor_x, self.cursor_y))
        } else {
            None
        }
    }

    // parser

    pub fn feed_str(&mut self, s: &str) -> Vec<usize> {
        // reset affected lines vec
        for l in &mut self.affected_lines[..] {
            *l = false;
        }

        // feed parser with chars
        for c in s.chars() {
            self.feed(c);
        }

        // return affected line numbers
        self.affected_lines
        .iter()
        .enumerate()
        .filter_map(|(i, &affected)|
            if affected {
                Some(i)
            } else {
                None
            }
        )
        .collect()
    }

    pub fn feed(&mut self, input: char) {
        let input2 = if input >= '\u{a0}' { '\u{41}' } else { input };

        match (&self.state, input2) {
            // Anywhere
            (_, '\u{18}')
            | (_, '\u{1a}')
            | (_, '\u{80}'..='\u{8f}')
            | (_, '\u{91}'..='\u{97}')
            | (_, '\u{99}')
            | (_, '\u{9a}') => {
                self.state = State::Ground;
                self.execute(input);
            }

            (_, '\u{1b}') => {
                self.state = State::Escape;
                self.clear();
            }

            (_, '\u{90}') => {
                self.state = State::DcsEntry;
                self.clear();
            }

            (_, '\u{9b}') => {
                self.state = State::CsiEntry;
                self.clear();
            }

            (_, '\u{9c}') => {
                self.state = State::Ground;
            }

            (_, '\u{9d}') => {
                self.state = State::OscString;
            }

            (_, '\u{98}') | (_, '\u{9e}') | (_, '\u{9f}') => {
                self.state = State::SosPmApcString;
            }

            // Ground

            // C0 prime
            (State::Ground, '\u{00}'..='\u{17}')
            | (State::Ground, '\u{19}')
            | (State::Ground, '\u{1c}'..='\u{1f}') => {
                self.execute(input);
            }

            (State::Ground, '\u{20}'..='\u{7f}') => {
                // (State::Ground, '\u{a0}'..='\u{ff}') => {
                self.print(input);
            }

            // Escape

            // C0 prime
            (State::Escape, '\u{00}'..='\u{17}')
            | (State::Escape, '\u{19}')
            | (State::Escape, '\u{1c}'..='\u{1f}') => {
                self.execute(input);
            }

            (State::Escape, '\u{20}'..='\u{2f}') => {
                self.state = State::EscapeIntermediate;
                self.collect(input);
            }

            (State::Escape, '\u{30}'..='\u{4f}')
            | (State::Escape, '\u{51}'..='\u{57}')
            | (State::Escape, '\u{59}')
            | (State::Escape, '\u{5a}')
            | (State::Escape, '\u{5c}')
            | (State::Escape, '\u{60}'..='\u{7e}') => {
                self.state = State::Ground;
                self.esc_dispatch(input);
            }

            (State::Escape, '\u{50}') => {
                self.state = State::DcsEntry;
                self.clear();
            }

            (State::Escape, '\u{5b}') => {
                self.state = State::CsiEntry;
                self.clear();
            }

            (State::Escape, '\u{5d}') => {
                self.state = State::OscString;
            }

            (State::Escape, '\u{58}') | (State::Escape, '\u{5e}') | (State::Escape, '\u{5f}') => {
                self.state = State::SosPmApcString;
            }

            // EscapeIntermediate

            // C0 prime
            (State::EscapeIntermediate, '\u{00}'..='\u{17}')
            | (State::EscapeIntermediate, '\u{19}')
            | (State::EscapeIntermediate, '\u{1c}'..='\u{1f}') => {
                self.execute(input);
            }

            (State::EscapeIntermediate, '\u{20}'..='\u{2f}') => {
                self.collect(input);
            }

            (State::EscapeIntermediate, '\u{30}'..='\u{7e}') => {
                self.state = State::Ground;
                self.esc_dispatch(input);
            }

            // CsiEntry

            // C0 prime
            (State::CsiEntry, '\u{00}'..='\u{17}')
            | (State::CsiEntry, '\u{19}')
            | (State::CsiEntry, '\u{1c}'..='\u{1f}') => {
                self.execute(input);
            }

            (State::CsiEntry, '\u{20}'..='\u{2f}') => {
                self.state = State::CsiIntermediate;
                self.collect(input);
            }

            (State::CsiEntry, '\u{30}'..='\u{39}') | (State::CsiEntry, '\u{3b}') => {
                self.state = State::CsiParam;
                self.param(input);
            }

            (State::CsiEntry, '\u{3a}') => {
                self.state = State::CsiIgnore;
            }

            (State::CsiEntry, '\u{3c}'..='\u{3f}') => {
                self.state = State::CsiParam;
                self.collect(input);
            }

            (State::CsiEntry, '\u{40}'..='\u{7e}') => {
                self.state = State::Ground;
                self.csi_dispatch(input);
            }

            // CsiParam

            // C0 prime
            (State::CsiParam, '\u{00}'..='\u{17}')
            | (State::CsiParam, '\u{19}')
            | (State::CsiParam, '\u{1c}'..='\u{1f}') => {
                self.execute(input);
            }

            (State::CsiParam, '\u{20}'..='\u{2f}') => {
                self.state = State::CsiIntermediate;
                self.collect(input);
            }

            (State::CsiParam, '\u{30}'..='\u{39}') | (State::CsiParam, '\u{3b}') => {
                self.param(input);
            }

            (State::CsiParam, '\u{3a}') | (State::CsiParam, '\u{3c}'..='\u{3f}') => {
                self.state = State::CsiIgnore;
            }

            (State::CsiParam, '\u{40}'..='\u{7e}') => {
                self.state = State::Ground;
                self.csi_dispatch(input);
            }

            // CsiIntermediate

            // C0 prime
            (State::CsiIntermediate, '\u{00}'..='\u{17}')
            | (State::CsiIntermediate, '\u{19}')
            | (State::CsiIntermediate, '\u{1c}'..='\u{1f}') => {
                self.execute(input);
            }

            (State::CsiIntermediate, '\u{20}'..='\u{2f}') => {
                self.collect(input);
            }

            (State::CsiIntermediate, '\u{30}'..='\u{3f}') => {
                self.state = State::CsiIgnore;
            }

            (State::CsiIntermediate, '\u{40}'..='\u{7e}') => {
                self.state = State::Ground;
                self.csi_dispatch(input);
            }

            // CsiIgnore

            // C0 prime
            (State::CsiIgnore, '\u{00}'..='\u{17}')
            | (State::CsiIgnore, '\u{19}')
            | (State::CsiIgnore, '\u{1c}'..='\u{1f}') => {
                self.execute(input);
            }

            (State::CsiIgnore, '\u{40}'..='\u{7e}') => {
                self.state = State::Ground;
            }

            // DcsEntry
            (State::DcsEntry, '\u{20}'..='\u{2f}') => {
                self.state = State::DcsIntermediate;
                self.collect(input);
            }

            (State::DcsEntry, '\u{30}'..='\u{39}') | (State::DcsEntry, '\u{3b}') => {
                self.state = State::DcsParam;
                self.param(input);
            }

            (State::DcsEntry, '\u{3c}'..='\u{3f}') => {
                self.state = State::DcsParam;
                self.collect(input);
            }

            (State::DcsEntry, '\u{3a}') => {
                self.state = State::DcsIgnore;
            }

            (State::DcsEntry, '\u{40}'..='\u{7e}') => {
                self.state = State::DcsPassthrough;
            }

            // DcsParam
            (State::DcsParam, '\u{20}'..='\u{2f}') => {
                self.state = State::DcsIntermediate;
                self.collect(input);
            }

            (State::DcsParam, '\u{30}'..='\u{39}') | (State::DcsParam, '\u{3b}') => {
                self.param(input);
            }

            (State::DcsParam, '\u{3a}') | (State::DcsParam, '\u{3c}'..='\u{3f}') => {
                self.state = State::DcsIgnore;
            }

            (State::DcsParam, '\u{40}'..='\u{7e}') => {
                self.state = State::DcsPassthrough;
            }

            // DcsIntermediate
            (State::DcsIntermediate, '\u{20}'..='\u{2f}') => {
                self.collect(input);
            }

            (State::DcsIntermediate, '\u{30}'..='\u{3f}') => {
                self.state = State::DcsIgnore;
            }

            (State::DcsIntermediate, '\u{40}'..='\u{7e}') => {
                self.state = State::DcsPassthrough;
            }

            // DcsPassthrough

            // C0 prime
            (State::DcsPassthrough, '\u{00}'..='\u{17}')
            | (State::DcsPassthrough, '\u{19}')
            | (State::DcsPassthrough, '\u{1c}'..='\u{1f}') => {
                self.put(input);
            }

            (State::DcsPassthrough, '\u{20}'..='\u{7e}') => {
                self.put(input);
            }

            // OscString
            (State::OscString, '\u{07}') => {
                // 0x07 is xterm non-ANSI variant of transition to ground
                self.state = State::Ground;
            }

            (State::OscString, '\u{20}'..='\u{7f}') => {
                self.osc_put(input);
            }

            _ => ()
        }
    }

    // interpreter

    fn execute(&mut self, input: char) {
        match input {
            '\u{08}' => self.execute_bs(),
            '\u{09}' => self.execute_ht(),
            '\u{0a}' => self.execute_lf(),
            '\u{0b}' => self.execute_lf(),
            '\u{0c}' => self.execute_lf(),
            '\u{0d}' => self.execute_cr(),
            '\u{0e}' => self.execute_so(),
            '\u{0f}' => self.execute_si(),
            '\u{84}' => self.execute_lf(),
            '\u{85}' => self.execute_nel(),
            '\u{88}' => self.execute_hts(),
            '\u{8d}' => self.execute_ri(),
            _ => ()
        }
    }

    fn print(&mut self, mut input: char) {
        if self.auto_wrap_mode && self.next_print_wraps {
            self.do_move_cursor_to_col(0);

            let next_row = self.cursor_y + 1;

            if next_row == self.rows {
                self.scroll_up(1);
            } else {
                self.do_move_cursor_to_row(next_row);
            }
        }

        input = self.charset.translate(input);

        let cell = Cell(input, self.pen);
        let next_column = self.cursor_x + 1;

        if next_column >= self.columns {
            self.set_cell(self.columns - 1, self.cursor_y, cell);

            if self.auto_wrap_mode {
                self.do_move_cursor_to_col(self.columns);
                self.next_print_wraps = true;
            }
        } else {
            if self.insert_mode {
                self.buffer[self.cursor_y][self.cursor_x..].rotate_right(1);
            }

            self.set_cell(self.cursor_x, self.cursor_y, cell);
            self.do_move_cursor_to_col(next_column);
        }

        self.mark_affected_line(self.cursor_y);
    }

    fn collect(&mut self, input: char) {
        self.intermediates.push(input);
    }

    fn esc_dispatch(&mut self, input: char) {
        match (self.intermediates.get(0), input) {
            (None, c) if ('@'..='_').contains(&c) => {
                self.execute(((input as u8) + 0x40) as char)
            }

            (None, '7') => self.execute_sc(),
            (None, '8') => self.execute_rc(),
            (None, 'c') => self.execute_ris(),
            (Some('#'), '8') => self.execute_decaln(),
            (Some('('), '0') => self.execute_so(),
            (Some('('), _) => self.execute_si(),
            _ => ()
        }
    }

    fn param(&mut self, input: char) {
        if input == ';' {
            self.params.push(0);
        } else {
            let n = self.params.len() - 1;
            let p = &mut self.params[n];
            *p = (10 * (*p as u32) + (input as u32) - 0x30) as u16;
        }
    }

    fn csi_dispatch(&mut self, input: char) {
        match (self.intermediates.get(0), input) {
            (None, '@') => self.execute_ich(),
            (None, 'A') => self.execute_cuu(),
            (None, 'B') => self.execute_cud(),
            (None, 'C') => self.execute_cuf(),
            (None, 'D') => self.execute_cub(),
            (None, 'E') => self.execute_cnl(),
            (None, 'F') => self.execute_cpl(),
            (None, 'G') => self.execute_cha(),
            (None, 'H') => self.execute_cup(),
            (None, 'I') => self.execute_cht(),
            (None, 'J') => self.execute_ed(),
            (None, 'K') => self.execute_el(),
            (None, 'L') => self.execute_il(),
            (None, 'M') => self.execute_dl(),
            (None, 'P') => self.execute_dch(),
            (None, 'S') => self.execute_su(),
            (None, 'T') => self.execute_sd(),
            (None, 'W') => self.execute_ctc(),
            (None, 'X') => self.execute_ech(),
            (None, 'Z') => self.execute_cbt(),
            (None, '`') => self.execute_cha(),
            (None, 'a') => self.execute_cuf(),
            (None, 'b') => self.execute_rep(),
            (None, 'd') => self.execute_vpa(),
            (None, 'e') => self.execute_cuu(),
            (None, 'f') => self.execute_cup(),
            (None, 'g') => self.execute_tbc(),
            (None, 'h') => self.execute_sm(),
            (None, 'l') => self.execute_rm(),
            (None, 'm') => self.execute_sgr(),
            (None, 'r') => self.execute_decstbm(),
            (Some('!'), 'p') => self.execute_decstr(),
            (Some('?'), 'h') => self.execute_prv_sm(),
            (Some('?'), 'l') => self.execute_prv_rm(),
            _ => {}
        }
    }

    fn put(&self, _input: char) {}

    fn osc_put(&self, _input: char) {}

    fn clear(&mut self) {
        self.params.clear();
        self.params.push(0);
        self.intermediates.clear();
    }

    fn execute_sc(&mut self) {
        self.save_cursor();
    }

    fn execute_rc(&mut self) {
        self.restore_cursor();
    }

    fn execute_ris(&mut self) {
        self.hard_reset();
    }

    fn execute_decaln(&mut self) {
        for y in 0..self.rows {
            for x in 0..self.columns {
                self.set_cell(x, y, Cell('\u{45}', Pen::new()));
            }

            self.mark_affected_line(y);
        }
    }

    fn execute_bs(&mut self) {
        self.move_cursor_to_rel_col(-1);
    }

    fn execute_ht(&mut self) {
        self.move_cursor_to_next_tab(1);
    }

    fn execute_lf(&mut self) {
        self.move_cursor_down_with_scroll();

        if self.new_line_mode {
            self.do_move_cursor_to_col(0);
        }
    }

    fn execute_cr(&mut self) {
        self.do_move_cursor_to_col(0);
    }

    fn execute_so(&mut self) {
        self.charset = Charset::G1;
    }

    fn execute_si(&mut self) {
        self.charset = Charset::G0;
    }

    fn execute_nel(&mut self) {
        self.move_cursor_down_with_scroll();
        self.do_move_cursor_to_col(0);
    }

    fn execute_hts(&mut self) {
        self.set_tab();
    }

    fn execute_ri(&mut self) {
        if self.cursor_y == self.top_margin {
            self.scroll_down(1);
        } else if self.cursor_y > 0 {
            self.do_move_cursor_to_row(self.cursor_y - 1);
        }
    }

    fn execute_ich(&mut self) {
        let mut n = self.get_param(0, 1) as usize;
        n = n.min(self.columns - self.cursor_x);
        let tpl = self.blank_cell();
        let cells = &mut self.buffer[self.cursor_y][self.cursor_x..];
        cells.rotate_right(n);

        for cell in &mut cells[0..n] {
            *cell = tpl;
        }

        self.mark_affected_line(self.cursor_y);
    }

    fn execute_cuu(&mut self) {
        self.cursor_up(self.get_param(0, 1) as usize);
    }

    fn execute_cud(&mut self) {
        self.cursor_down(self.get_param(0, 1) as usize);
    }

    fn execute_cuf(&mut self) {
        self.move_cursor_to_rel_col(self.get_param(0, 1) as isize);
    }

    fn execute_cub(&mut self) {
        self.move_cursor_to_rel_col(-(self.get_param(0, 1) as isize));
    }

    fn execute_cnl(&mut self) {
        self.cursor_down(self.get_param(0, 1) as usize);
        self.do_move_cursor_to_col(0);
    }

    fn execute_cpl(&mut self) {
        self.cursor_up(self.get_param(0, 1) as usize);
        self.do_move_cursor_to_col(0);
    }

    fn execute_cha(&mut self) {
        self.move_cursor_to_col((self.get_param(0, 1) as usize) - 1);
    }

    fn execute_cup(&mut self) {
        self.move_cursor_to_col((self.get_param(1, 1) as usize) - 1);
        self.move_cursor_to_row((self.get_param(0, 1) as usize) - 1);
    }

    fn execute_cht(&mut self) {
        self.move_cursor_to_next_tab(self.get_param(0, 1) as usize);
    }

    fn execute_ed(&mut self) {
        match self.get_param(0, 0) {
            0 => {
                // clear to end of screen
                self.clear_line(self.cursor_x..self.columns);
                self.clear_lines((self.cursor_y + 1)..self.rows);
                self.mark_affected_lines(self.cursor_y..self.rows);
            }

            1 => {
                // clear to beginning of screen
                self.clear_line(0..(self.cursor_x + 1).min(self.columns));
                self.clear_lines(0..self.cursor_y);
                self.mark_affected_lines(0..(self.cursor_y + 1));
            }

            2 => {
                // clear whole screen
                self.clear_lines(0..self.rows);
                self.mark_affected_lines(0..self.rows);
            }

            _ => ()
        }
    }

    fn execute_el(&mut self) {
        match self.get_param(0, 0) {
            0 => {
                // clear to end of line
                self.clear_line(self.cursor_x..self.columns);
                self.mark_affected_line(self.cursor_y);
            }

            1 => {
                // clear to begining of line
                self.clear_line(0..(self.cursor_x + 1).min(self.columns));
                self.mark_affected_line(self.cursor_y);
            }

            2 => {
                // clear whole line
                self.clear_line(0..self.columns);
                self.mark_affected_line(self.cursor_y);
            }

            _ => ()
        }
    }

    fn execute_il(&mut self) {
        let mut n = self.get_param(0, 1) as usize;

        if self.cursor_y <= self.bottom_margin {
            n = n.min(self.bottom_margin + 1 - self.cursor_y);
            self.buffer[self.cursor_y..=self.bottom_margin].rotate_right(n);
            self.clear_lines(self.cursor_y..(self.cursor_y + n));
            self.mark_affected_lines(self.cursor_y..(self.bottom_margin + 1));
        } else {
            n = n.min(self.rows - self.cursor_y);
            self.buffer[self.cursor_y..].rotate_right(n);
            self.clear_lines(self.cursor_y..(self.cursor_y + n));
            self.mark_affected_lines(self.cursor_y..self.rows);
        }
    }

    fn execute_dl(&mut self) {
        let mut n = self.get_param(0, 1) as usize;

        if self.cursor_y <= self.bottom_margin {
            let end_index = self.bottom_margin + 1;
            n = n.min(end_index - self.cursor_y);
            self.buffer[self.cursor_y..end_index].rotate_left(n);
            self.clear_lines((end_index - n)..end_index);
            self.mark_affected_lines(self.cursor_y..end_index);
        } else {
            n = n.min(self.rows - self.cursor_y);
            self.buffer[self.cursor_y..self.rows].rotate_left(n);
            self.clear_lines((self.rows - n)..self.rows);
            self.mark_affected_lines(self.cursor_y..self.rows);
        }
    }

    fn execute_dch(&mut self) {
        if self.cursor_x >= self.columns {
            self.move_cursor_to_col(self.columns - 1);
        }

        let mut n = self.get_param(0, 1) as usize;
        n = n.min(self.columns - self.cursor_x);
        self.buffer[self.cursor_y][self.cursor_x..].rotate_left(n);
        self.clear_line((self.columns - n)..self.columns);
        self.mark_affected_line(self.cursor_y);
    }

    fn execute_su(&mut self) {
        self.scroll_up(self.get_param(0, 1) as usize);
    }

    fn execute_sd(&mut self) {
        self.scroll_down(self.get_param(0, 1) as usize);
    }

    fn execute_ctc(&mut self) {
        match self.get_param(0, 0) {
            0 => self.set_tab(),
            2 => self.clear_tab(),
            5 => self.clear_all_tabs(),
            _ => ()
        }
    }

    fn execute_ech(&mut self) {
        let mut n = self.get_param(0, 1) as usize;
        n = n.min(self.columns - self.cursor_x);
        self.clear_line(self.cursor_x..(self.cursor_x + n));
        self.mark_affected_line(self.cursor_y);
    }

    fn execute_rep(&mut self) {
        if self.cursor_x > 0 {
            let n = self.get_param(0, 1);
            let char = self.buffer[self.cursor_y][self.cursor_x - 1].0;

            for _n in 0..n {
                self.print(char);
            }
        }
    }

    fn execute_cbt(&mut self) {
        self.move_cursor_to_prev_tab(self.get_param(0, 1) as usize);
    }

    fn execute_vpa(&mut self) {
        self.move_cursor_to_row((self.get_param(0, 1) - 1) as usize);
    }

    fn execute_tbc(&mut self) {
        match self.get_param(0, 0) {
            0 => self.clear_tab(),
            3 => self.clear_all_tabs(),
            _ => ()
        }
    }

    fn execute_sm(&mut self) {
        for param in self.params.clone() {
            match param {
                4 => self.insert_mode = true,
                20 => self.new_line_mode = true,
                _ => ()
            }
        }
    }

    fn execute_rm(&mut self) {
        for param in self.params.clone() {
            match param {
                4 => self.insert_mode = false,
                20 => self.new_line_mode = false,
                _ => ()
            }
        }
    }

    fn execute_sgr(&mut self) {
        let mut ps = &self.params[..];

        while let Some(param) = ps.get(0) {
            match param {
                0 => {
                    self.pen = Pen::new();
                    ps = &ps[1..];
                }

                1 => {
                    self.pen.bold = true;
                    ps = &ps[1..];
                }

                3 => {
                    self.pen.italic = true;
                    ps = &ps[1..];
                }

                4 => {
                    self.pen.underline = true;
                    ps = &ps[1..];
                }

                5 => {
                    self.pen.blink = true;
                    ps = &ps[1..];
                }

                7 => {
                    self.pen.inverse = true;
                    ps = &ps[1..];
                }

                9 => {
                    ps = &ps[1..];
                    self.pen.strikethrough = true;
                }

                21 => {
                    self.pen.bold = false;
                    ps = &ps[1..];
                }

                22 => {
                    self.pen.bold = false;
                    ps = &ps[1..];
                }

                23 => {
                    self.pen.italic = false;
                    ps = &ps[1..];
                }

                24 => {
                    self.pen.underline = false;
                    ps = &ps[1..];
                }

                25 => {
                    self.pen.blink = false;
                    ps = &ps[1..];
                }

                27 => {
                    self.pen.inverse = false;
                    ps = &ps[1..];
                }

                param if *param >= 30 && *param <= 37 => {
                    self.pen.foreground = Some(Color::Indexed((param - 30) as u8));
                    ps = &ps[1..];
                }

                38 => {
                    match ps.get(1) {
                        None => {
                            ps = &ps[1..];
                        }

                        Some(2) => {
                            if let Some(b) = ps.get(4) {
                                let r = ps.get(2).unwrap();
                                let g = ps.get(3).unwrap();
                                self.pen.foreground = Some(Color::RGB(RGB8::new(*r as u8, *g as u8, *b as u8)));
                                ps = &ps[5..];
                            } else {
                                ps = &ps[2..];
                            }
                        }

                        Some(5) => {
                            if let Some(param) = ps.get(2) {
                                self.pen.foreground = Some(Color::Indexed(*param as u8));
                                ps = &ps[3..];
                            } else {
                                ps = &ps[2..];
                            }
                        }

                        Some(_) => {
                            ps = &ps[1..];
                        }
                    }
                }

                39 => {
                    self.pen.foreground = None;
                    ps = &ps[1..];
                }

                param if *param >= 40 && *param <= 47 => {
                    self.pen.background = Some(Color::Indexed((param - 40) as u8));
                    ps = &ps[1..];
                }

                48 => {
                    match ps.get(1) {
                        None => {
                            ps = &ps[1..];
                        }

                        Some(2) => {
                            if let Some(b) = ps.get(4) {
                                let r = ps.get(2).unwrap();
                                let g = ps.get(3).unwrap();
                                self.pen.background = Some(Color::RGB(RGB8::new(*r as u8, *g as u8, *b as u8)));
                                ps = &ps[5..];
                            } else {
                                ps = &ps[2..];
                            }
                        }

                        Some(5) => {
                            if let Some(param) = ps.get(2) {
                                self.pen.background = Some(Color::Indexed(*param as u8));
                                ps = &ps[3..];
                            } else {
                                ps = &ps[2..];
                            }
                        }

                        Some(_) => {
                            ps = &ps[1..];
                        }
                    }
                }

                49 => {
                    self.pen.background = None;
                    ps = &ps[1..];
                }

                param if *param >= 90 && *param <= 97 => {
                    self.pen.foreground = Some(Color::Indexed((param - 90 + 8) as u8));
                    ps = &ps[1..];
                }

                param if *param >= 100 && *param <= 107 => {
                    self.pen.background = Some(Color::Indexed((param - 100 + 8) as u8));
                    ps = &ps[1..];
                }

                _ => {
                    ps = &ps[1..];
                }
            }
        }
    }

    fn execute_prv_sm(&mut self) {
        for param in self.params.clone() {
            match param {
                6 => {
                    self.origin_mode = true;
                    self.move_cursor_home();
                },

                7 => self.auto_wrap_mode = true,
                25 => self.cursor_visible = true,
                47 => self.switch_to_alternate_buffer(),
                1047 => self.switch_to_alternate_buffer(),
                1048 => self.save_cursor(),

                1049 => {
                    self.save_cursor();
                    self.switch_to_alternate_buffer();
                },
                _ => ()
            }
        }
    }

    fn execute_prv_rm(&mut self) {
        for param in self.params.clone() {
            match param {
                6 =>  {
                    self.origin_mode = false;
                    self.move_cursor_home();
                },

                7 => self.auto_wrap_mode = false,
                25 => self.cursor_visible = false,
                47 => self.switch_to_primary_buffer(),
                1047 => self.switch_to_primary_buffer(),
                1048 => self.restore_cursor(),

                1049 =>  {
                    self.switch_to_primary_buffer();
                    self.restore_cursor();
                },

                _ => ()
            }
        }
    }

    fn execute_decstr(&mut self) {
        self.soft_reset();
    }

    fn execute_decstbm(&mut self) {
        let top = (self.get_param(0, 1) - 1) as usize;
        let bottom = (self.get_param(1, self.rows as u16) - 1) as usize;

        if top < bottom && bottom < self.rows {
            self.top_margin = top;
            self.bottom_margin = bottom;
        }

        self.move_cursor_home();
    }

    // screen

    fn set_cell(&mut self, x: usize, y: usize, cell: Cell) {
        self.buffer[y][x] = cell;
    }

    fn set_tab(&mut self) {
        if 0 < self.cursor_x && self.cursor_x < self.columns {
            match self.tabs.binary_search(&self.cursor_x) {
                Ok(_pos) => (),
                Err(pos) => self.tabs.insert(pos, self.cursor_x)
            }
        }
    }

    fn clear_tab(&mut self) {
        match self.tabs.binary_search(&self.cursor_x) {
            Ok(pos) => { self.tabs.remove(pos); },
            Err(_pos) => ()
        }
    }

    fn clear_all_tabs(&mut self) {
        self.tabs.clear();
    }

    fn clear_line(&mut self, range: Range<usize>) {
        let tpl = self.blank_cell();

        for cell in &mut self.buffer[self.cursor_y][range] {
            *cell = tpl;
        }
    }

    fn clear_lines(&mut self, range: Range<usize>) {
        let tpl = self.blank_line();

        for line in &mut self.buffer[range] {
            *line = tpl.clone();
        }
    }

    fn get_param(&self, n: usize, default: u16) -> u16 {
        let param = *self.params.get(n).unwrap_or(&0);

        if param == 0 { default } else { param }
    }

    fn actual_top_margin(&self) -> usize {
        if self.origin_mode {
            self.top_margin
        } else {
            0
        }
    }

    fn actual_bottom_margin(&self) -> usize {
        if self.origin_mode {
            self.bottom_margin
        } else {
            self.rows - 1
        }
    }

    // cursor

    fn save_cursor(&mut self) {
        self.saved_ctx.cursor_x = self.cursor_x.min(self.columns - 1);
        self.saved_ctx.cursor_y = self.cursor_y;
        self.saved_ctx.pen = self.pen;
        self.saved_ctx.origin_mode = self.origin_mode;
        self.saved_ctx.auto_wrap_mode = self.auto_wrap_mode;
    }

    fn restore_cursor(&mut self) {
        self.cursor_x = self.saved_ctx.cursor_x;
        self.cursor_y = self.saved_ctx.cursor_y;
        self.pen = self.saved_ctx.pen;
        self.origin_mode = self.saved_ctx.origin_mode;
        self.auto_wrap_mode = self.saved_ctx.auto_wrap_mode;
        self.next_print_wraps = false;
    }

    fn move_cursor_to_col(&mut self, col: usize) {
        if col >= self.columns {
            self.do_move_cursor_to_col(self.columns - 1);
        } else {
            self.do_move_cursor_to_col(col);
        }
    }

    fn do_move_cursor_to_col(&mut self, col: usize) {
        self.cursor_x = col;
        self.next_print_wraps = false;
    }

    fn move_cursor_to_row(&mut self, mut row: usize) {
        let top = self.actual_top_margin();
        let bottom = self.actual_bottom_margin();
        row = (top + row).max(top).min(bottom);
        self.do_move_cursor_to_row(row);
    }

    fn do_move_cursor_to_row(&mut self, row: usize) {
        self.cursor_x = self.cursor_x.min(self.columns - 1);
        self.cursor_y = row;
        self.next_print_wraps = false;
    }

    fn move_cursor_to_rel_col(&mut self, rel_col: isize) {
        let new_col = self.cursor_x as isize + rel_col;

        if new_col < 0 {
            self.do_move_cursor_to_col(0);
        } else if new_col as usize >= self.columns {
            self.do_move_cursor_to_col(self.columns - 1);
        } else {
            self.do_move_cursor_to_col(new_col as usize);
        }
    }

    fn move_cursor_home(&mut self) {
        self.do_move_cursor_to_col(0);
        self.do_move_cursor_to_row(self.actual_top_margin());
    }

    fn move_cursor_to_next_tab(&mut self, n: usize) {
        let last_col = self.columns - 1;

        let next_tab =
            *self.tabs
            .iter()
            .skip_while(|&&t| self.cursor_x >= t)
            .nth(n - 1)
            .unwrap_or(&last_col);

        self.move_cursor_to_col(next_tab);
    }

    fn move_cursor_to_prev_tab(&mut self, n: usize) {
        let first_col = 0;

        let prev_tab =
            *self.tabs
            .iter()
            .rev()
            .skip_while(|&&t| self.cursor_x <= t)
            .nth(n - 1)
            .unwrap_or(&first_col);

        self.move_cursor_to_col(prev_tab);
    }

    fn move_cursor_down_with_scroll(&mut self) {
        if self.cursor_y == self.bottom_margin {
            self.scroll_up(1);
        } else if self.cursor_y < self.rows - 1 {
            self.do_move_cursor_to_row(self.cursor_y + 1);
        }
    }

    fn cursor_down(&mut self, n: usize) {
        let new_y = if self.cursor_y > self.bottom_margin {
            (self.rows - 1).min(self.cursor_y + n)
        } else {
            self.bottom_margin.min(self.cursor_y + n)
        };

        self.do_move_cursor_to_row(new_y);
    }

    fn cursor_up(&mut self, n: usize) {
        let mut new_y = (self.cursor_y as isize) - (n as isize);

        new_y = if self.cursor_y < self.top_margin {
            new_y.max(0)
        } else {
            new_y.max(self.top_margin as isize)
        };

        self.do_move_cursor_to_row(new_y as usize);
    }

    // scrolling

    fn scroll_up(&mut self, mut n: usize) {
        let end_index = self.bottom_margin + 1;
        n = n.min(end_index - self.top_margin);
        self.buffer[self.top_margin..end_index].rotate_left(n);
        self.clear_lines((end_index - n)..end_index);
        self.mark_affected_lines(self.top_margin..end_index);
    }

    fn scroll_down(&mut self, mut n: usize) {
        let end_index = self.bottom_margin + 1;
        n = n.min(end_index - self.top_margin);
        self.buffer[self.top_margin..end_index].rotate_right(n);
        self.clear_lines(0..n);
        self.mark_affected_lines(0..end_index);
    }

    // buffer switching

    fn switch_to_alternate_buffer(&mut self) {
        if let BufferType::Primary = self.active_buffer_type {
            self.active_buffer_type = BufferType::Alternate;
            std::mem::swap(&mut self.saved_ctx, &mut self.alternate_saved_ctx);
            std::mem::swap(&mut self.buffer, &mut self.alternate_buffer);
            self.clear_lines(0..self.rows);
            self.mark_affected_lines(0..self.rows);
        }
    }

    fn switch_to_primary_buffer(&mut self) {
        if let BufferType::Alternate = self.active_buffer_type {
            self.active_buffer_type = BufferType::Primary;
            std::mem::swap(&mut self.saved_ctx, &mut self.alternate_saved_ctx);
            std::mem::swap(&mut self.buffer, &mut self.alternate_buffer);
            self.mark_affected_lines(0..self.rows);
        }
    }

    // resetting

    fn soft_reset(&mut self) {
        self.cursor_visible = true;
        self.top_margin = 0;
        self.bottom_margin = self.rows - 1;
        self.insert_mode = false;
        self.origin_mode = false;
        self.pen = Pen::new();
        self.saved_ctx = SavedCtx::new();
    }

    fn hard_reset(&mut self) {
        let buffer = VT::new_buffer(self.columns, self.rows);
        let alternate_buffer = buffer.clone();

        self.state = State::Ground;
        self.params = Vec::new();
        self.params.push(0);
        self.intermediates = Vec::new();
        self.buffer = buffer;
        self.alternate_buffer = alternate_buffer;
        self.active_buffer_type = BufferType::Primary;
        self.tabs = VT::default_tabs(self.columns);
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.cursor_visible = true;
        self.pen = Pen::new();
        self.charset = Charset::G0;
        self.insert_mode = false;
        self.origin_mode = false;
        self.auto_wrap_mode = true;
        self.new_line_mode = false;
        self.next_print_wraps = false;
        self.top_margin = 0;
        self.bottom_margin = self.rows - 1;
        self.saved_ctx = SavedCtx::new();
        self.alternate_saved_ctx = SavedCtx::new();
        self.affected_lines = vec![true; self.rows];
    }

    // full state dump

    pub fn dump(&self) -> String {
        let (primary_ctx, alternate_ctx): (&SavedCtx, &SavedCtx) = match self.active_buffer_type {
            BufferType::Primary => (&self.saved_ctx, &self.alternate_saved_ctx),
            BufferType::Alternate => (&self.alternate_saved_ctx, &self.saved_ctx)
        };

        // 1. dump primary screen buffer

        let mut seq: String = self.dump_buffer(BufferType::Primary);

        // 2. setup tab stops

        // clear all tab stops
        seq.push_str("\u{9b}5W");

        // set each tab stop
        for t in &self.tabs {
            seq.push_str(&format!("\u{9b}{}`\u{1b}[W", t + 1));
        }

        // 3. configure saved context for primary screen

        if !primary_ctx.auto_wrap_mode {
            // disable auto-wrap mode
            seq.push_str("\u{9b}?7l");
        }

        if primary_ctx.origin_mode {
            // enable origin mode
            seq.push_str("\u{9b}?6h");
        }

        // fix cursor in target position
        seq.push_str(&format!("\u{9b}{};{}H", primary_ctx.cursor_y + 1, primary_ctx.cursor_x + 1));

        // configure pen
        seq.push_str(&primary_ctx.pen.sgr_seq());

        // save cursor
        seq.push_str("\u{1b}7");

        if !primary_ctx.auto_wrap_mode {
            // re-enable auto-wrap mode
            seq.push_str("\u{9b}?7h");
        }

        if primary_ctx.origin_mode {
            // re-disable origin mode
            seq.push_str("\u{9b}?6l");
        }

        // 4. dump alternate screen buffer

        // switch to alternate screen
        seq.push_str("\u{9b}?1047h");

        if self.active_buffer_type == BufferType::Alternate {
            // move cursor home
            seq.push_str("\u{9b}1;1H");

            // dump alternate buffer
            seq.push_str(&self.dump_buffer(BufferType::Alternate));
        }

        // 5. configure saved context for alternate screen

        if !alternate_ctx.auto_wrap_mode {
            // disable auto-wrap mode
            seq.push_str("\u{9b}?7l");
        }

        if alternate_ctx.origin_mode {
            // enable origin mode
            seq.push_str("\u{9b}?6h");
        }

        // fix cursor in target position
        seq.push_str(&format!("\u{9b}{};{}H", alternate_ctx.cursor_y + 1, alternate_ctx.cursor_x + 1));

        // configure pen
        seq.push_str(&alternate_ctx.pen.sgr_seq());

        // save cursor
        seq.push_str("\u{1b}7");

        if !alternate_ctx.auto_wrap_mode {
            // re-enable auto-wrap mode
            seq.push_str("\u{9b}?7h");
        }

        if alternate_ctx.origin_mode {
            // re-disable origin mode
            seq.push_str("\u{9b}?6l");
        }

        // 6. ensure the right buffer is active

        if self.active_buffer_type == BufferType::Primary {
            // switch back to primary screen
            seq.push_str("\u{9b}?1047l");
        }

        // 7. setup origin mode

        if self.origin_mode {
            // enable origin mode
            // note: this resets cursor position - must be done before fixing cursor
            seq.push_str("\u{9b}?6h");
        }

        // 8. setup margins

        // note: this resets cursor position - must be done before fixing cursor
        seq.push_str(&format!("\u{9b}{};{}r", self.top_margin + 1, self.bottom_margin + 1));

        // 9. setup cursor

        let row = if self.origin_mode {
            self.cursor_y - self.top_margin + 1
        } else {
            self.cursor_y + 1
        };

        let column = self.cursor_x + 1;

        // fix cursor in target position
        seq.push_str(&format!("\u{9b}{};{}H", row, column));

        if self.cursor_x >= self.columns {
            // move cursor past right border by re-printing the character in
            // the last column
            let cell = self.buffer[self.cursor_y][self.columns - 1];
            seq.push_str(&format!("{}{}", cell.1.sgr_seq(), cell.0));
        }

        // configure pen
        seq.push_str(&self.pen.sgr_seq());

        if !self.cursor_visible {
            // hide cursor
            seq.push_str("\u{9b}?25l");
        }

        // Below 3 must happen after ALL prints as they alter print behaviour,
        // including the "move cursor past right border one" above.

        // 10. setup charset

        if self.charset == Charset::G1 {
            // switch to G1 charset
            seq.push('\u{0e}');
        }

        // 11. setup insert mode

        if self.insert_mode {
            // enable insert mode
            seq.push_str("\u{9b}4h");
        }

        // 12. setup auto-wrap mode

        if !self.auto_wrap_mode {
            // disable auto-wrap mode
            seq.push_str("\u{9b}?7l");
        }

        // 13. setup new line mode

        if self.new_line_mode {
            // enable new line mode
            seq.push_str("\u{9b}20h");
        }

        // 14. transition into current state

        match self.state {
            State::Ground => (),

            State::Escape =>
                seq.push('\u{1b}'),

            State::EscapeIntermediate => {
                let intermediates = self.intermediates.iter().collect::<String>();
                let s = format!("\u{1b}{}", intermediates);
                seq.push_str(&s);
            },

            State::CsiEntry =>
                seq.push('\u{9b}'),

            State::CsiParam => {
                let intermediates = self.intermediates.iter().collect::<String>();

                let params = self.params
                    .iter()
                    .map(|param| param.to_string())
                    .collect::<Vec<_>>()
                    .join(";");

                let s = &format!("\u{9b}{}{}", intermediates, params);
                seq.push_str(s);
            },

            State::CsiIntermediate => {
                let intermediates = self.intermediates.iter().collect::<String>();
                let s = &format!("\u{9b}{}", intermediates);
                seq.push_str(s);
            },

            State::CsiIgnore =>
                seq.push_str("\u{9b}\u{3a}"),

            State::DcsEntry =>
                seq.push('\u{90}'),

            State::DcsIntermediate => {
                let intermediates = self.intermediates.iter().collect::<String>();
                let s = &format!("\u{90}{}", intermediates);
                seq.push_str(s);
            },

            State::DcsParam => {
                let intermediates = self.intermediates.iter().collect::<String>();

                let params = self.params
                    .iter()
                    .map(|param| param.to_string())
                    .collect::<Vec<_>>()
                    .join(";");

                let s = &format!("\u{90}{}{}", intermediates, params);
                seq.push_str(s);
            },

            State::DcsPassthrough => {
                let intermediates = self.intermediates.iter().collect::<String>();
                let s = &format!("\u{90}{}\u{40}", intermediates);
                seq.push_str(s);
            }

            State::DcsIgnore =>
                seq.push_str("\u{90}\u{3a}"),

            State::OscString =>
                seq.push('\u{9d}'),

            State::SosPmApcString =>
                seq.push('\u{98}')
        }

        seq
    }

    fn dump_buffer(&self, buffer_type: BufferType) -> String {
        let buffer = if self.active_buffer_type == buffer_type {
            &self.buffer
        } else {
            &self.alternate_buffer
        };

        buffer
        .iter()
        .map(|line| VT::dump_line(&VT::chunk_cells(line)))
        .collect()
    }

    fn dump_line(segments: &[Segment]) -> String {
        segments
        .iter()
        .map(VT::dump_segment)
        .collect()
    }

    fn dump_segment(segment: &Segment) -> String {
        let mut s = segment.1.sgr_seq();
        let text = segment.0.iter().collect::<String>();
        s.push_str(&text);

        s
    }

    pub fn lines(&self) -> Vec<Vec<(char, Pen)>> {
        self.buffer
        .iter()
        .map(|cells| { cells.iter().map(|cell| { (cell.0, cell.1)}).collect() })
        .collect()
    }

    pub fn get_line(&self, l: usize) -> Vec<Segment> {
        VT::chunk_cells(&self.buffer[l])
    }

    pub fn get_lines(&self) -> Vec<Vec<Segment>> {
        self.buffer
        .iter()
        .map(VT::chunk_cells)
        .collect()
    }

    fn chunk_cells(cells: &Line) -> Vec<Segment> {
        if !cells.is_empty() {
            let mut part = Segment(vec![cells[0].0], cells[0].1);
            let mut parts = vec![];

            for cell in &cells[1..] {
                if cell.1 == part.1 {
                    part.0.push(cell.0);
                } else {
                    parts.push(part);
                    part = Segment(vec![cell.0], cell.1);
                }
            }

            parts.push(part);

            parts
        } else {
            vec![]
        }
    }

    // line change tracking

    fn mark_affected_lines(&mut self, range: Range<usize>) {
        for l in &mut self.affected_lines[range] {
            *l = true;
        }
    }

    fn mark_affected_line(&mut self, line: usize) {
        self.affected_lines[line] = true;
    }
}

impl Serialize for Segment {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut tup = serializer.serialize_tuple(2)?;
        let text: String = self.0.iter().collect();
        tup.serialize_element(&text)?;
        tup.serialize_element(&self.1)?;
        tup.end()
    }
}

impl Serialize for Pen {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut len = 0;

        if self.foreground.is_some() {
            len += 1;
        }

        if self.background.is_some() {
            len += 1;
        }

        if self.bold {
            len += 1;
        }

        if self.italic {
            len += 1;
        }

        if self.underline {
            len += 1;
        }

        if self.strikethrough {
            len += 1;
        }

        if self.blink {
            len += 1;
        }

        if self.inverse {
            len += 1;
        }

        let mut map = serializer.serialize_map(Some(len))?;

        if let Some(c) = self.foreground {
            map.serialize_entry("fg", &c)?;
        }

        if let Some(c) = self.background {
            map.serialize_entry("bg", &c)?;
        }

        if self.bold {
            map.serialize_entry("bold", &true)?;
        }

        if self.italic {
            map.serialize_entry("italic", &true)?;
        }

        if self.underline {
            map.serialize_entry("underline", &true)?;
        }

        if self.strikethrough {
            map.serialize_entry("strikethrough", &true)?;
        }

        if self.blink {
            map.serialize_entry("blink", &true)?;
        }

        if self.inverse {
            map.serialize_entry("inverse", &true)?;
        }

        map.end()
    }
}

impl Serialize for Color {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Color::Indexed(c) => {
                serializer.serialize_u8(*c)
            }

            Color::RGB(c) => {
                serializer.serialize_str(&format!("rgb({},{},{})", c.r, c.g, c.b))
            }
        }
    }
}

#[cfg(test)]
extern crate quickcheck;
#[cfg(test)]
#[macro_use(quickcheck)]
extern crate quickcheck_macros;

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use pretty_assertions::assert_eq;
    use quickcheck::{TestResult, quickcheck};
    use rgb::RGB8;
    use super::BufferType;
    use super::Cell;
    use super::Color;
    use super::Line;
    use super::State;
    use super::VT;

    #[quickcheck]
    fn qc_cursor_position(bytes: Vec<u8>) -> bool {
        let mut vt = VT::new(10, 4);

        for b in bytes.iter() {
            vt.feed((*b) as char);
        }

        vt.cursor_x <= 10 && vt.cursor_y < 4
    }

    #[quickcheck]
    fn qc_buffer_size(bytes: Vec<u8>) -> bool {
        let mut vt = VT::new(10, 4);

        for b in bytes.iter() {
            vt.feed((*b) as char);
        }

        vt.buffer.len() == 4 && vt.buffer.iter().all(|line| line.len() == 10)
    }

    #[quickcheck]
    fn qc_wrapping(y: u8, bytes: Vec<u8>) -> TestResult {
        if y >= 5 {
            return TestResult::discard()
        }

        let mut vt = VT::new(10, 5);

        vt.cursor_x = 9;
        vt.cursor_y = y as usize;

        for b in bytes.iter() {
            vt.feed((*b) as char);
        }

        TestResult::from_bool(!vt.next_print_wraps || vt.cursor_x == 10)
    }

    #[test]
    fn default_tabs() {
        assert_eq!(VT::default_tabs(1), vec![]);
        assert_eq!(VT::default_tabs(8), vec![]);
        assert_eq!(VT::default_tabs(9), vec![8]);
        assert_eq!(VT::default_tabs(16), vec![8]);
        assert_eq!(VT::default_tabs(17), vec![8, 16]);
    }

    // #[test]
    // fn failed() {
    //     let mut vt = VT::new(2, 2);
    //     let bytes: Vec<u8> = vec![32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 27, 91, 49, 74];
    //     let bytes: Vec<u8> = vec![32, 32, 27, 91, 63, 55, 108, 32];
    //     let bytes: Vec<u8> = fs::read("100303.txt").unwrap();

    //     for b in bytes {
    //         vt.feed(b as char);
    //     }
    // }

    #[test]
    fn get_param() {
        let mut vt = VT::new(1, 1);

        vt.feed_str("\x1b[;1;;23;456;");

        assert_eq!(vt.get_param(0, 999), 999);
        assert_eq!(vt.get_param(1, 999), 1);
        assert_eq!(vt.get_param(2, 999), 999);
        assert_eq!(vt.get_param(3, 999), 23);
        assert_eq!(vt.get_param(4, 999), 456);
        assert_eq!(vt.get_param(5, 999), 999);
    }

    #[test]
    fn execute_lf() {
        let mut vt = build_vt(3, 0, vec![
            "abc     ",
            "        "
        ]);

        vt.feed_str("\n");

        assert_eq!(vt.cursor_x, 3);
        assert_eq!(vt.cursor_y, 1);

        assert_eq!(dump_lines(&vt), vec![
            "abc     ",
            "        "
        ]);

        vt.feed_str("d\n");

        assert_eq!(vt.cursor_x, 4);
        assert_eq!(vt.cursor_y, 1);

        assert_eq!(dump_lines(&vt), vec![
            "   d    ",
            "        "
        ]);
    }

    #[test]
    fn execute_ich() {
        let mut vt = build_vt(3, 0, vec![
            "abcdefgh",
            "ijklmnop"
        ]);

        vt.feed_str("\x1b[@");

        assert_eq!(vt.cursor_x, 3);

        assert_eq!(dump_lines(&vt), vec![
            "abc defg",
            "ijklmnop"
        ]);

        vt.feed_str("\x1b[2@");

        assert_eq!(dump_lines(&vt), vec![
            "abc   de",
            "ijklmnop"
        ]);

        vt.feed_str("\x1b[10@");

        assert_eq!(dump_lines(&vt), vec![
            "abc     ",
            "ijklmnop"
        ]);

        let mut vt = build_vt(7, 0, vec![
            "abcdefgh",
            "ijklmnop"
        ]);

        vt.feed_str("\x1b[10@");

        assert_eq!(dump_lines(&vt), vec![
            "abcdefg ",
            "ijklmnop"
        ]);
    }

    #[test]
    fn execute_il() {
        let mut vt = build_vt(3, 1, vec![
            "abcdefgh",
            "ijklmnop",
            "qrstuwxy"
        ]);

        vt.feed_str("\x1b[L");

        assert_eq!(vt.cursor_x, 3);
        assert_eq!(vt.cursor_y, 1);

        assert_eq!(dump_lines(&vt), vec![
            "abcdefgh",
            "        ",
            "ijklmnop"
        ]);

        vt.cursor_y = 0;

        vt.feed_str("\x1b[2L");

        assert_eq!(dump_lines(&vt), vec![
            "        ",
            "        ",
            "abcdefgh"
        ]);

        vt.cursor_y = 1;

        vt.feed_str("\x1b[100L");

        assert_eq!(dump_lines(&vt), vec![
            "        ",
            "        ",
            "        "
        ]);
    }

    #[test]
    fn execute_dl() {
        let mut vt = build_vt(3, 1, vec![
            "abcdefgh",
            "ijklmnop",
            "qrstuwxy"
        ]);

        vt.feed_str("\x1b[M");

        assert_eq!(vt.cursor_x, 3);
        assert_eq!(vt.cursor_y, 1);

        assert_eq!(dump_lines(&vt), vec![
            "abcdefgh",
            "qrstuwxy",
            "        "
        ]);

        vt.cursor_y = 0;

        vt.feed_str("\x1b[5M");

        assert_eq!(dump_lines(&vt), vec![
            "        ",
            "        ",
            "        "
        ]);
    }

    #[test]
    fn execute_dch() {
        let mut vt = build_vt(3, 0, vec![
            "abcdefgh"
        ]);

        vt.feed_str("\x1b[P");

        assert_eq!(vt.cursor_x, 3);

        assert_eq!(dump_lines(&vt), vec![
            "abcefgh "
        ]);

        vt.feed_str("\x1b[2P");

        assert_eq!(dump_lines(&vt), vec![
            "abcgh   "
        ]);

        vt.feed_str("\x1b[10P");

        assert_eq!(dump_lines(&vt), vec![
            "abc     "
        ]);
    }

    #[test]
    fn execute_ech() {
        let mut vt = build_vt(3, 0, vec![
            "abcdefgh"
        ]);

        vt.feed_str("\x1b[X");

        assert_eq!(vt.cursor_x, 3);

        assert_eq!(dump_lines(&vt), vec![
            "abc efgh"
        ]);

        vt.feed_str("\x1b[2X");

        assert_eq!(dump_lines(&vt), vec![
            "abc  fgh"
        ]);

        vt.feed_str("\x1b[10X");

        assert_eq!(dump_lines(&vt), vec![
            "abc     "
        ]);
    }

    #[test]
    fn execute_cht() {
        let mut vt = build_vt(3, 0, vec![
            "abcdefghijklmnopqrstuwxyzabc"
        ]);

        vt.feed_str("\x1b[I");

        assert_eq!(vt.cursor_x, 8);

        vt.feed_str("\x1b[2I");

        assert_eq!(vt.cursor_x, 24);

        vt.feed_str("\x1b[I");

        assert_eq!(vt.cursor_x, 27);
    }

    #[test]
    fn execute_cbt() {
        let mut vt = build_vt(26, 0, vec![
            "abcdefghijklmnopqrstuwxyzabc"
        ]);

        vt.feed_str("\x1b[Z");

        assert_eq!(vt.cursor_x, 24);

        vt.feed_str("\x1b[2Z");

        assert_eq!(vt.cursor_x, 8);

        vt.feed_str("\x1b[Z");

        assert_eq!(vt.cursor_x, 0);
    }

    #[test]
    fn execute_sgr() {
        let mut vt = build_vt(0, 0, vec!["abcd"]);

        vt.feed_str("\x1b[1m");
        assert!(vt.pen.bold);

        vt.feed_str("\x1b[3m");
        assert!(vt.pen.italic);

        vt.feed_str("\x1b[4m");
        assert!(vt.pen.underline);

        vt.feed_str("\x1b[5m");
        assert!(vt.pen.blink);

        vt.feed_str("\x1b[7m");
        assert!(vt.pen.inverse);

        vt.feed_str("\x1b[9m");
        assert!(vt.pen.strikethrough);

        vt.feed_str("\x1b[32m");
        assert_eq!(vt.pen.foreground, Some(Color::Indexed(2)));

        vt.feed_str("\x1b[43m");
        assert_eq!(vt.pen.background, Some(Color::Indexed(3)));

        vt.feed_str("\x1b[93m");
        assert_eq!(vt.pen.foreground, Some(Color::Indexed(11)));

        vt.feed_str("\x1b[104m");
        assert_eq!(vt.pen.background, Some(Color::Indexed(12)));

        vt.feed_str("\x1b[39m");
        assert_eq!(vt.pen.foreground, None);

        vt.feed_str("\x1b[49m");
        assert_eq!(vt.pen.background, None);

        vt.feed_str("\x1b[1;38;5;88;48;5;99;5m");
        assert!(vt.pen.bold);
        assert!(vt.pen.blink);
        assert_eq!(vt.pen.foreground, Some(Color::Indexed(88)));
        assert_eq!(vt.pen.background, Some(Color::Indexed(99)));

        vt.feed_str("\x1b[1;38;2;1;101;201;48;2;2;102;202;5m");
        assert!(vt.pen.bold);
        assert!(vt.pen.blink);
        assert_eq!(vt.pen.foreground, Some(Color::RGB(RGB8::new(1, 101, 201))));
        assert_eq!(vt.pen.background, Some(Color::RGB(RGB8::new(2, 102, 202))));
    }

    #[test]
    fn dump_initial() {
        let vt1 = VT::new(10, 4);
        let mut vt2 = VT::new(10, 4);

        vt2.feed_str(&vt1.dump());

        assert_vts_eq(&vt1, &vt2);
    }

    #[test]
    fn dump_modified() {
        let mut vt1 = VT::new(10, 4);
        let mut vt2 = VT::new(10, 4);

        vt1.feed_str(&"hello\n\rworld\u{9b}5W\u{9b}7`\u{1b}[W\u{9b}?6h\u{9b}2;4r\u{9b}1;5H\x1b[1;31;41m\u{9b}?25l\u{0e}\u{9b}4h\u{9b}?7l\u{9b}20h\u{9b}\u{3a}");
        vt2.feed_str(&vt1.dump());

        assert_vts_eq(&vt1, &vt2);
    }

    #[test]
    fn dump_with_file() {
        if let Ok((w, h, input, step)) = setup_dump_with_file() {
            let mut vt1 = VT::new(w, h);

            let mut s = 0;

            for c in input.chars().take(1_000_000) {
                vt1.feed(c);

                if s == 0 {
                    let d = vt1.dump();
                    let mut vt2 = VT::new(w, h);

                    vt2.feed_str(&d);

                    assert_vts_eq(&vt1, &vt2);
                }

                s = (s + 1) % step;
            }
        }
    }

    fn setup_dump_with_file() -> Result<(usize, usize, String, usize), env::VarError> {
        let path = env::var("P")?;
        let input = fs::read_to_string(path).unwrap();
        let w: usize = env::var("W").unwrap().parse::<usize>().unwrap();
        let h: usize = env::var("H").unwrap().parse::<usize>().unwrap();
        let step: usize = env::var("S").unwrap_or("1".to_owned()).parse::<usize>().unwrap();

        Ok((w, h, input, step))
    }

    fn build_vt(cx: usize, cy: usize, lines: Vec<&str>) -> VT {
        let w = lines.get(0).unwrap().len();
        let h = lines.len();
        let mut vt = VT::new(w, h);

        for line in lines {
            vt.feed_str(line);
        }

        vt.cursor_x = cx;
        vt.cursor_y = cy;

        vt
    }

    fn dump_lines(vt: &VT) -> Vec<String> {
        vt.buffer
        .iter()
        .map(|cells| dump_line(cells))
        .collect()
    }

    fn dump_line(cells: &[Cell]) -> String {
        cells.iter().map(|cell| cell.0).collect()
    }

    fn assert_vts_eq(vt1: &VT, vt2: &VT) {
        assert_eq!(vt1.state, vt2.state);

        if vt1.state == State::CsiParam || vt1.state == State::DcsParam {
            assert_eq!(vt1.params, vt2.params);
        }

        if vt1.state == State::EscapeIntermediate || vt1.state == State::CsiIntermediate || vt1.state == State::CsiParam || vt1.state == State::DcsIntermediate || vt1.state == State::DcsParam {
            assert_eq!(vt1.intermediates, vt2.intermediates);
        }

        assert_eq!(vt1.active_buffer_type, vt2.active_buffer_type);
        assert_eq!(vt1.cursor_x, vt2.cursor_x);
        assert_eq!(vt1.cursor_y, vt2.cursor_y, "margins: {} {}", vt1.top_margin, vt2.bottom_margin);
        assert_eq!(vt1.cursor_visible, vt2.cursor_visible);
        assert_eq!(vt1.pen, vt2.pen);
        assert_eq!(vt1.charset, vt2.charset);
        assert_eq!(vt1.tabs, vt2.tabs);
        assert_eq!(vt1.insert_mode, vt2.insert_mode);
        assert_eq!(vt1.origin_mode, vt2.origin_mode);
        assert_eq!(vt1.auto_wrap_mode, vt2.auto_wrap_mode);
        assert_eq!(vt1.new_line_mode, vt2.new_line_mode);
        assert_eq!(vt1.next_print_wraps, vt2.next_print_wraps);
        assert_eq!(vt1.top_margin, vt2.top_margin);
        assert_eq!(vt1.bottom_margin, vt2.bottom_margin);
        assert_eq!(vt1.saved_ctx, vt2.saved_ctx);
        assert_eq!(vt1.alternate_saved_ctx, vt2.alternate_saved_ctx);

        match vt1.active_buffer_type {
            BufferType::Primary => {
                assert_eq!(buffer_as_string(&vt1.buffer), buffer_as_string(&vt2.buffer));
            }

            BufferType::Alternate => {
                // primary:
                assert_eq!(buffer_as_string(&vt1.alternate_buffer), buffer_as_string(&vt2.alternate_buffer));
                // alternate:
                assert_eq!(buffer_as_string(&vt1.buffer), buffer_as_string(&vt2.buffer));
            }
        }
    }

    fn buffer_as_string(buffer: &Vec<Line>) -> String {
        let mut s = "".to_owned();

        for line in buffer {
            for cell in line {
                s.push(cell.0);
            }

            s.push('\n');
        }

        s
    }
}
