// The parser is based on Paul Williams' parser for ANSI-compatible video
// terminals: https://www.vt100.net/emu/dec_ansi_parser

use std::ops::Range;
use serde::ser::{Serialize, Serializer, SerializeMap, SerializeTuple};


#[derive(Debug, Copy, Clone)]
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
enum Color {
    Indexed(u8),
    RGB(u8, u8, u8)
}

#[derive(Debug, Copy, Clone, PartialEq)]
struct Pen {
    foreground: Option<Color>,
    background: Option<Color>,
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    blink: bool,
    inverse: bool
}

#[derive(Debug, Copy, Clone)]
struct Cell(char, Pen);

#[derive(Debug)]
pub struct Segment(Vec<char>, Pen);

#[derive(Debug)]
enum Charset {
    G0,
    G1
}

#[derive(Debug)]
enum BufferType {
    Primary,
    Alternate
}

#[derive(Debug)]
struct SavedCtx {
    cursor_x: usize,
    cursor_y: usize,
    pen: Pen,
    origin_mode: bool,
    auto_wrap_mode: bool
}

#[derive(Debug)]
pub struct VT {
    // parser
    pub state: State,

    // interpreter
    params: Vec<u16>,
    intermediates: Vec<char>,

    // screen
    columns: usize,
    rows: usize,
    buffer: Vec<Vec<Cell>>,
    alternate_buffer: Vec<Vec<Cell>>,
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
}

impl Cell {
    fn blank() -> Cell {
        Cell(' ', Pen::new())
    }
}

impl Charset {
    fn translate(&self, input: char) -> char {
        if input >= '\x60' && input < '\x7f' {
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

    fn new_buffer(columns: usize, rows: usize) -> Vec<Vec<Cell>> {
        vec![vec![Cell::blank(); columns]; rows]
    }

    fn blank_line(&self) -> Vec<Cell> {
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

    pub fn get_cursor(&self) -> Option<(usize, usize)> {
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
                &mut self.buffer[self.cursor_y][self.cursor_x..].rotate_right(1);
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
            (None, c) if '@' <= c && c <= '_' => {
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
            *p = (10 * *p) + (input as u16) - 0x30;
        }
    }

    fn csi_dispatch(&mut self, input: char) {
        match input {
            '@' => self.execute_ich(),
            'A' => self.execute_cuu(),
            'B' => self.execute_cud(),
            'C' => self.execute_cuf(),
            'D' => self.execute_cub(),
            'E' => self.execute_cnl(),
            'F' => self.execute_cpl(),
            'G' => self.execute_cha(),
            'H' => self.execute_cup(),
            'I' => self.execute_cht(),
            'J' => self.execute_ed(),
            'K' => self.execute_el(),
            'L' => self.execute_il(),
            'M' => self.execute_dl(),
            'P' => self.execute_dch(),
            'S' => self.execute_su(),
            'T' => self.execute_sd(),
            'W' => self.execute_ctc(),
            'X' => self.execute_ech(),
            'Z' => self.execute_cbt(),
            '`' => self.execute_cha(),
            'a' => self.execute_cuf(),
            'd' => self.execute_vpa(),
            'e' => self.execute_cuu(),
            'f' => self.execute_cup(),
            'g' => self.execute_tbc(),
            'h' => self.execute_sm(),
            'l' => self.execute_rm(),
            'm' => self.execute_sgr(),
            'p' => self.execute_decstr(),
            'r' => self.execute_decstbm(),
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
            &mut self.buffer[self.cursor_y..=self.bottom_margin].rotate_right(n);
            self.clear_lines(self.cursor_y..(self.cursor_y + n));
            self.mark_affected_lines(self.cursor_y..(self.bottom_margin + 1));
        } else {
            n = n.min(self.rows - self.cursor_y);
            &mut self.buffer[self.cursor_y..].rotate_right(n);
            self.clear_lines(self.cursor_y..(self.cursor_y + n));
            self.mark_affected_lines(self.cursor_y..self.rows);
        }
    }

    fn execute_dl(&mut self) {
        let mut n = self.get_param(0, 1) as usize;

        if self.cursor_y <= self.bottom_margin {
            let end_index = self.bottom_margin + 1;
            n = n.min(end_index - self.cursor_y);
            &mut self.buffer[self.cursor_y..end_index].rotate_left(n);
            self.clear_lines((end_index - n)..end_index);
            self.mark_affected_lines(self.cursor_y..end_index);
        } else {
            n = n.min(self.rows - self.cursor_y);
            &mut self.buffer[self.cursor_y..self.rows].rotate_left(n);
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
        &mut self.buffer[self.cursor_y][self.cursor_x..].rotate_left(n);
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
            match (self.intermediates.get(0), param) {
                (None, 4) => self.insert_mode = true,
                (None, 20) => self.new_line_mode = true,

                (Some('?'), 6) => {
                    self.origin_mode = true;
                    self.move_cursor_home();
                },

                (Some('?'), 7) => self.auto_wrap_mode = true,
                (Some('?'), 25) => self.cursor_visible = true,
                (Some('?'), 47) => self.switch_to_alternate_buffer(),
                (Some('?'), 1047) => self.switch_to_alternate_buffer(),
                (Some('?'), 1048) => self.save_cursor(),

                (Some('?'), 1049) => {
                    self.save_cursor();
                    self.switch_to_alternate_buffer();
                },
                _ => ()
            }
        }
    }

    fn execute_rm(&mut self) {
        for param in self.params.clone() {
            match (self.intermediates.get(0), param) {
                (None, 4) => self.insert_mode = false,
                (None, 20) => self.new_line_mode = false,

                (Some('?'), 6) =>  {
                    self.origin_mode = false;
                    self.move_cursor_home();
                },

                (Some('?'), 7) => self.auto_wrap_mode = false,
                (Some('?'), 25) => self.cursor_visible = false,
                (Some('?'), 47) => self.switch_to_primary_buffer(),
                (Some('?'), 1047) => self.switch_to_primary_buffer(),
                (Some('?'), 1048) => self.restore_cursor(),

                (Some('?'), 1049) =>  {
                    self.switch_to_primary_buffer();
                    self.restore_cursor();
                },

                _ => ()
            }
        }
    }

    fn execute_sgr(&mut self) {
        let mut ps = &self.params[..];

        while ps.len() > 0 {
            match ps.get(0).unwrap() {
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
                                self.pen.foreground = Some(Color::RGB(*r as u8, *g as u8, *b as u8));
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
                                self.pen.background = Some(Color::RGB(*r as u8, *g as u8, *b as u8));
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

    fn execute_decstr(&mut self) {
        if let Some('!') = self.intermediates.get(0) {
            self.soft_reset();
        }
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
        &mut self.buffer[self.top_margin..end_index].rotate_left(n);
        self.clear_lines((end_index - n)..end_index);
        self.mark_affected_lines(self.top_margin..end_index);
    }

    fn scroll_down(&mut self, mut n: usize) {
        let end_index = self.bottom_margin + 1;
        n = n.min(end_index - self.top_margin);
        &mut self.buffer[self.top_margin..end_index].rotate_right(n);
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

    pub fn get_line(&self, l: usize) -> Vec<Segment> {
        VT::chunk_cells(&self.buffer[l])
    }

    pub fn get_lines(&self) -> Vec<Vec<Segment>> {
        self.buffer
        .iter()
        .map(|line| VT::chunk_cells(line))
        .collect()
    }

    fn chunk_cells(cells: &Vec<Cell>) -> Vec<Segment> {
        if cells.len() > 0 {
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

        if let Some(_) = self.foreground {
            len += 1;
        }

        if let Some(_) = self.background {
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

            Color::RGB(r, g, b) => {
                serializer.serialize_str(&format!("rgb({},{},{})", r, g, b))
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
    use super::VT;
    use super::Cell;
    use super::Color;
    use quickcheck::{TestResult, quickcheck};

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
        assert_eq!(vt.pen.foreground, Some(Color::RGB(1, 101, 201)));
        assert_eq!(vt.pen.background, Some(Color::RGB(2, 102, 202)));
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
}