// The parser is based on Paul Williams' parser for ANSI-compatible video
// terminals: https://www.vt100.net/emu/dec_ansi_parser

use crate::buffer::{Buffer, EraseMode};
use crate::cell::Cell;
use crate::charset::Charset;
use crate::color::Color;
use crate::dump::Dump;
use crate::line::Line;
use crate::pen::{Intensity, Pen};
use crate::saved_ctx::SavedCtx;
use crate::tabs::Tabs;
use rgb::RGB8;
use std::collections::HashSet;

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

#[derive(Debug, PartialEq)]
enum BufferType {
    Primary,
    Alternate,
}

#[derive(Debug)]
pub struct Vt {
    // parser
    pub state: State,

    // interpreter
    params: Vec<u16>,
    intermediates: Vec<char>,

    // screen
    pub cols: usize,
    pub rows: usize,
    buffer: Buffer,
    other_buffer: Buffer,
    active_buffer_type: BufferType,
    scrollback_limit: Option<usize>,
    cursor_x: usize,
    cursor_y: usize,
    cursor_visible: bool,
    pen: Pen,
    charsets: [Charset; 2],
    active_charset: usize,
    tabs: Tabs,
    insert_mode: bool,
    origin_mode: bool,
    auto_wrap_mode: bool,
    new_line_mode: bool,
    next_print_wraps: bool,
    top_margin: usize,
    bottom_margin: usize,
    saved_ctx: SavedCtx,
    alternate_saved_ctx: SavedCtx,
    dirty_lines: HashSet<usize>,
    pub resizable: bool,
    resized: bool,
}

impl Vt {
    pub fn new(cols: usize, rows: usize) -> Self {
        Vt::with_scrollback_limit(cols, rows, None)
    }

    pub fn with_scrollback_limit(
        cols: usize,
        rows: usize,
        scrollback_limit: Option<usize>,
    ) -> Self {
        assert!(cols > 0);
        assert!(rows > 0);

        let primary_buffer = Buffer::new(cols, rows, scrollback_limit, None);
        let alternate_buffer = Buffer::new(cols, rows, Some(0), None);
        let dirty_lines = HashSet::from_iter(0..rows);

        Vt {
            state: State::Ground,
            params: Vec::new(),
            intermediates: Vec::new(),
            cols,
            rows,
            buffer: primary_buffer,
            other_buffer: alternate_buffer,
            active_buffer_type: BufferType::Primary,
            scrollback_limit,
            tabs: Tabs::new(cols),
            cursor_x: 0,
            cursor_y: 0,
            cursor_visible: true,
            pen: Pen::default(),
            charsets: [Charset::Ascii, Charset::Ascii],
            active_charset: 0,
            insert_mode: false,
            origin_mode: false,
            auto_wrap_mode: true,
            new_line_mode: false,
            next_print_wraps: false,
            top_margin: 0,
            bottom_margin: (rows - 1),
            saved_ctx: SavedCtx::default(),
            alternate_saved_ctx: SavedCtx::default(),
            dirty_lines,
            resizable: false,
            resized: false,
        }
    }

    pub fn cursor(&self) -> Option<(usize, usize)> {
        if self.cursor_visible {
            Some((self.cursor_x, self.cursor_y))
        } else {
            None
        }
    }

    // parser

    pub fn feed_str(&mut self, s: &str) -> (Vec<usize>, bool) {
        // reset change tracking vars
        self.dirty_lines.clear();
        self.resized = false;

        // feed parser with chars
        for c in s.chars() {
            self.feed(c);
        }

        let dirty_lines = self.dirty_lines.iter().cloned().collect();

        (dirty_lines, self.resized)
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

            _ => (),
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
            _ => (),
        }
    }

    fn print(&mut self, mut input: char) {
        input = self.charsets[self.active_charset].translate(input);
        let cell = Cell(input, self.pen);

        if self.auto_wrap_mode && self.next_print_wraps {
            self.do_move_cursor_to_col(0);

            if self.cursor_y == self.bottom_margin {
                self.buffer.wrap(self.cursor_y);
                self.scroll_up_in_region(1);
            } else if self.cursor_y < self.rows - 1 {
                self.buffer.wrap(self.cursor_y);
                self.do_move_cursor_to_row(self.cursor_y + 1);
            }
        }

        let next_col = self.cursor_x + 1;

        if next_col >= self.cols {
            self.buffer.print((self.cols - 1, self.cursor_y), cell);

            if self.auto_wrap_mode {
                self.do_move_cursor_to_col(self.cols);
                self.next_print_wraps = true;
            }
        } else {
            if self.insert_mode {
                self.buffer.insert((self.cursor_x, self.cursor_y), 1, cell);
            } else {
                self.buffer.print((self.cursor_x, self.cursor_y), cell);
            }

            self.do_move_cursor_to_col(next_col);
        }

        self.dirty_lines.insert(self.cursor_y);
    }

    fn collect(&mut self, input: char) {
        self.intermediates.push(input);
    }

    fn esc_dispatch(&mut self, input: char) {
        match (self.intermediates.first(), input) {
            (None, c) if ('@'..='_').contains(&c) => self.execute(((input as u8) + 0x40) as char),

            (None, '7') => self.execute_sc(),
            (None, '8') => self.execute_rc(),
            (None, 'c') => self.execute_ris(),
            (Some('#'), '8') => self.execute_decaln(),
            (Some('('), '0') => self.execute_gzd4(Charset::Drawing),
            (Some('('), _) => self.execute_gzd4(Charset::Ascii),
            (Some(')'), '0') => self.execute_g1d4(Charset::Drawing),
            (Some(')'), _) => self.execute_g1d4(Charset::Ascii),
            _ => (),
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
        match (self.intermediates.first(), input) {
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
            (None, 'e') => self.execute_vpr(),
            (None, 'f') => self.execute_cup(),
            (None, 'g') => self.execute_tbc(),
            (None, 'h') => self.execute_sm(),
            (None, 'l') => self.execute_rm(),
            (None, 'm') => self.execute_sgr(),
            (None, 'r') => self.execute_decstbm(),
            (None, 's') => self.execute_sc(),
            (None, 't') => self.execute_xtwinops(),
            (None, 'u') => self.execute_rc(),
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
        for row in 0..self.rows {
            for col in 0..self.cols {
                self.buffer
                    .print((col, row), Cell('\u{45}', Pen::default()));
            }

            self.dirty_lines.insert(row);
        }
    }

    fn execute_gzd4(&mut self, charset: Charset) {
        self.charsets[0] = charset;
    }

    fn execute_g1d4(&mut self, charset: Charset) {
        self.charsets[1] = charset;
    }

    fn execute_bs(&mut self) {
        if self.next_print_wraps {
            self.move_cursor_to_rel_col(-2);
        } else {
            self.move_cursor_to_rel_col(-1);
        }
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
        self.active_charset = 1;
    }

    fn execute_si(&mut self) {
        self.active_charset = 0;
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
            self.scroll_down_in_region(1);
        } else if self.cursor_y > 0 {
            self.move_cursor_to_row(self.cursor_y - 1);
        }
    }

    fn execute_ich(&mut self) {
        self.buffer.insert(
            (self.cursor_x, self.cursor_y),
            self.get_param(0, 1) as usize,
            Cell::blank(self.pen),
        );

        self.dirty_lines.insert(self.cursor_y);
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
        let mut rel_col = -(self.get_param(0, 1) as isize);

        if self.next_print_wraps {
            rel_col -= 1;
        }

        self.move_cursor_to_rel_col(rel_col);
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
        use EraseMode::*;

        match self.get_param(0, 0) {
            0 => {
                self.buffer.erase(
                    (self.cursor_x, self.cursor_y),
                    FromCursorToEndOfView,
                    &self.pen,
                );

                self.dirty_lines.extend(self.cursor_y..self.rows);
            }

            1 => {
                self.buffer.erase(
                    (self.cursor_x, self.cursor_y),
                    FromStartOfViewToCursor,
                    &self.pen,
                );

                self.dirty_lines.extend(0..self.cursor_y + 1);
            }

            2 => {
                self.buffer
                    .erase((self.cursor_x, self.cursor_y), WholeView, &self.pen);

                self.dirty_lines.extend(0..self.rows);
            }

            _ => (),
        }
    }

    fn execute_el(&mut self) {
        use EraseMode::*;

        match self.get_param(0, 0) {
            0 => {
                self.buffer.erase(
                    (self.cursor_x, self.cursor_y),
                    FromCursorToEndOfLine,
                    &self.pen,
                );

                self.dirty_lines.insert(self.cursor_y);
            }

            1 => {
                self.buffer.erase(
                    (self.cursor_x, self.cursor_y),
                    FromStartOfLineToCursor,
                    &self.pen,
                );

                self.dirty_lines.insert(self.cursor_y);
            }

            2 => {
                self.buffer
                    .erase((self.cursor_x, self.cursor_y), WholeLine, &self.pen);

                self.dirty_lines.insert(self.cursor_y);
            }

            _ => (),
        }
    }

    fn execute_il(&mut self) {
        let range = if self.cursor_y <= self.bottom_margin {
            self.cursor_y..self.bottom_margin + 1
        } else {
            self.cursor_y..self.rows
        };

        self.buffer
            .scroll_down(range.clone(), self.get_param(0, 1) as usize, &self.pen);

        self.dirty_lines.extend(range);
    }

    fn execute_dl(&mut self) {
        let range = if self.cursor_y <= self.bottom_margin {
            self.cursor_y..self.bottom_margin + 1
        } else {
            self.cursor_y..self.rows
        };

        self.buffer
            .scroll_up(range.clone(), self.get_param(0, 1) as usize, &self.pen);

        self.dirty_lines.extend(range);
    }

    fn execute_dch(&mut self) {
        if self.cursor_x >= self.cols {
            self.move_cursor_to_col(self.cols - 1);
        }

        self.buffer.delete(
            (self.cursor_x, self.cursor_y),
            self.get_param(0, 1) as usize,
            &self.pen,
        );

        self.dirty_lines.insert(self.cursor_y);
    }

    fn execute_su(&mut self) {
        self.scroll_up_in_region(self.get_param(0, 1) as usize);
    }

    fn execute_sd(&mut self) {
        self.scroll_down_in_region(self.get_param(0, 1) as usize);
    }

    fn execute_ctc(&mut self) {
        match self.get_param(0, 0) {
            0 => self.set_tab(),
            2 => self.clear_tab(),
            5 => self.clear_all_tabs(),
            _ => (),
        }
    }

    fn execute_ech(&mut self) {
        let n = self.get_param(0, 1) as usize;

        self.buffer.erase(
            (self.cursor_x, self.cursor_y),
            EraseMode::NextChars(n),
            &self.pen,
        );

        self.dirty_lines.insert(self.cursor_y);
    }

    fn execute_rep(&mut self) {
        if self.cursor_x > 0 {
            let n = self.get_param(0, 1);
            let char = self.buffer[(self.cursor_x - 1, self.cursor_y)].0;

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

    fn execute_vpr(&mut self) {
        self.cursor_down(self.get_param(0, 1) as usize);
    }

    fn execute_tbc(&mut self) {
        match self.get_param(0, 0) {
            0 => self.clear_tab(),
            3 => self.clear_all_tabs(),
            _ => (),
        }
    }

    fn execute_sm(&mut self) {
        for param in &self.params {
            match param {
                4 => self.insert_mode = true,
                20 => self.new_line_mode = true,
                _ => (),
            }
        }
    }

    fn execute_rm(&mut self) {
        for param in &self.params {
            match param {
                4 => self.insert_mode = false,
                20 => self.new_line_mode = false,
                _ => (),
            }
        }
    }

    fn execute_sgr(&mut self) {
        let mut ps = &self.params[..];

        while let Some(param) = ps.first() {
            match param {
                0 => {
                    self.pen = Pen::default();
                    ps = &ps[1..];
                }

                1 => {
                    self.pen.intensity = Intensity::Bold;
                    ps = &ps[1..];
                }

                2 => {
                    self.pen.intensity = Intensity::Faint;
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

                21 | 22 => {
                    self.pen.intensity = Intensity::Normal;
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

                38 => match ps.get(1) {
                    None => {
                        ps = &ps[1..];
                    }

                    Some(2) => {
                        if let Some(b) = ps.get(4) {
                            let r = ps.get(2).unwrap();
                            let g = ps.get(3).unwrap();
                            self.pen.foreground =
                                Some(Color::RGB(RGB8::new(*r as u8, *g as u8, *b as u8)));
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
                },

                39 => {
                    self.pen.foreground = None;
                    ps = &ps[1..];
                }

                param if *param >= 40 && *param <= 47 => {
                    self.pen.background = Some(Color::Indexed((param - 40) as u8));
                    ps = &ps[1..];
                }

                48 => match ps.get(1) {
                    None => {
                        ps = &ps[1..];
                    }

                    Some(2) => {
                        if let Some(b) = ps.get(4) {
                            let r = ps.get(2).unwrap();
                            let g = ps.get(3).unwrap();
                            self.pen.background =
                                Some(Color::RGB(RGB8::new(*r as u8, *g as u8, *b as u8)));
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
                },

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
                }

                7 => self.auto_wrap_mode = true,
                25 => self.cursor_visible = true,

                47 => {
                    self.switch_to_alternate_buffer();
                    self.reflow();
                }

                1047 => {
                    self.switch_to_alternate_buffer();
                    self.reflow();
                }

                1048 => self.save_cursor(),

                1049 => {
                    self.save_cursor();
                    self.switch_to_alternate_buffer();
                    self.reflow();
                }
                _ => (),
            }
        }
    }

    fn execute_prv_rm(&mut self) {
        for param in self.params.clone() {
            match param {
                6 => {
                    self.origin_mode = false;
                    self.move_cursor_home();
                }

                7 => self.auto_wrap_mode = false,
                25 => self.cursor_visible = false,

                47 => {
                    self.switch_to_primary_buffer();
                    self.reflow();
                }

                1047 => {
                    self.switch_to_primary_buffer();
                    self.reflow();
                }

                1048 => self.restore_cursor(),

                1049 => {
                    self.switch_to_primary_buffer();
                    self.restore_cursor();
                    self.reflow();
                }

                _ => (),
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

    fn execute_xtwinops(&mut self) {
        if self.resizable && self.get_param(0, 0) == 8 {
            let cols = self.get_param(2, self.cols as u16) as usize;
            let rows = self.get_param(1, self.rows as u16) as usize;

            match cols.cmp(&self.cols) {
                std::cmp::Ordering::Less => {
                    self.tabs.contract(cols);
                    self.resized = true;
                }

                std::cmp::Ordering::Equal => (),

                std::cmp::Ordering::Greater => {
                    self.tabs.expand(self.cols, cols);
                    self.resized = true;
                }
            }

            match rows.cmp(&self.rows) {
                std::cmp::Ordering::Less => {
                    self.top_margin = 0;
                    self.bottom_margin = rows - 1;
                    self.resized = true;
                }

                std::cmp::Ordering::Equal => (),

                std::cmp::Ordering::Greater => {
                    self.top_margin = 0;
                    self.bottom_margin = rows - 1;
                    self.resized = true;
                }
            }

            self.cols = cols;
            self.rows = rows;
            self.reflow();
        }
    }

    // screen

    fn set_tab(&mut self) {
        if 0 < self.cursor_x && self.cursor_x < self.cols {
            self.tabs.set(self.cursor_x);
        }
    }

    fn clear_tab(&mut self) {
        self.tabs.unset(self.cursor_x);
    }

    fn clear_all_tabs(&mut self) {
        self.tabs.clear();
    }

    fn get_param(&self, n: usize, default: u16) -> u16 {
        let param = *self.params.get(n).unwrap_or(&0);

        if param == 0 {
            default
        } else {
            param
        }
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
        self.saved_ctx.cursor_x = self.cursor_x.min(self.cols - 1);
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
        if col >= self.cols {
            self.do_move_cursor_to_col(self.cols - 1);
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
        self.cursor_x = self.cursor_x.min(self.cols - 1);
        self.cursor_y = row;
        self.next_print_wraps = false;
    }

    fn move_cursor_to_rel_col(&mut self, rel_col: isize) {
        let new_col = self.cursor_x as isize + rel_col;

        if new_col < 0 {
            self.do_move_cursor_to_col(0);
        } else if new_col as usize >= self.cols {
            self.do_move_cursor_to_col(self.cols - 1);
        } else {
            self.do_move_cursor_to_col(new_col as usize);
        }
    }

    fn move_cursor_home(&mut self) {
        self.do_move_cursor_to_col(0);
        self.do_move_cursor_to_row(self.actual_top_margin());
    }

    fn move_cursor_to_next_tab(&mut self, n: usize) {
        let next_tab = self.tabs.after(self.cursor_x, n).unwrap_or(self.cols - 1);
        self.move_cursor_to_col(next_tab);
    }

    fn move_cursor_to_prev_tab(&mut self, n: usize) {
        let prev_tab = self.tabs.before(self.cursor_x, n).unwrap_or(0);
        self.move_cursor_to_col(prev_tab);
    }

    fn move_cursor_down_with_scroll(&mut self) {
        if self.cursor_y == self.bottom_margin {
            self.scroll_up_in_region(1);
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

    fn scroll_up_in_region(&mut self, n: usize) {
        let range = self.top_margin..self.bottom_margin + 1;
        self.buffer.scroll_up(range.clone(), n, &self.pen);
        self.dirty_lines.extend(range);
    }

    fn scroll_down_in_region(&mut self, n: usize) {
        let range = self.top_margin..self.bottom_margin + 1;
        self.buffer.scroll_down(range.clone(), n, &self.pen);
        self.dirty_lines.extend(range);
    }

    // buffer switching

    fn switch_to_alternate_buffer(&mut self) {
        if let BufferType::Primary = self.active_buffer_type {
            self.active_buffer_type = BufferType::Alternate;
            std::mem::swap(&mut self.saved_ctx, &mut self.alternate_saved_ctx);
            std::mem::swap(&mut self.buffer, &mut self.other_buffer);
            self.buffer = Buffer::new(self.cols, self.rows, Some(0), Some(&self.pen));
            self.dirty_lines.extend(0..self.rows);
        }
    }

    fn switch_to_primary_buffer(&mut self) {
        if let BufferType::Alternate = self.active_buffer_type {
            self.active_buffer_type = BufferType::Primary;
            std::mem::swap(&mut self.saved_ctx, &mut self.alternate_saved_ctx);
            std::mem::swap(&mut self.buffer, &mut self.other_buffer);
            self.dirty_lines.extend(0..self.rows);
        }
    }

    fn reflow(&mut self) {
        if self.cols != self.buffer.cols {
            self.next_print_wraps = false;
        }

        (self.cursor_x, self.cursor_y) =
            self.buffer
                .resize(self.cols, self.rows, (self.cursor_x, self.cursor_y));

        self.dirty_lines.retain(|r| r < &self.rows);
        self.dirty_lines.extend(0..self.rows);

        if self.saved_ctx.cursor_x >= self.cols {
            self.saved_ctx.cursor_x = self.cols - 1;
        }

        if self.saved_ctx.cursor_y >= self.rows {
            self.saved_ctx.cursor_y = self.rows - 1;
        }
    }

    // resetting

    fn soft_reset(&mut self) {
        self.cursor_visible = true;
        self.top_margin = 0;
        self.bottom_margin = self.rows - 1;
        self.insert_mode = false;
        self.origin_mode = false;
        self.pen = Pen::default();
        self.charsets = [Charset::Ascii, Charset::Ascii];
        self.active_charset = 0;
        self.saved_ctx = SavedCtx::default();
    }

    fn hard_reset(&mut self) {
        let primary_buffer = Buffer::new(self.cols, self.rows, self.scrollback_limit, None);
        let alternate_buffer = Buffer::new(self.cols, self.rows, Some(0), None);

        self.state = State::Ground;
        self.params = Vec::new();
        self.params.push(0);
        self.intermediates = Vec::new();
        self.buffer = primary_buffer;
        self.other_buffer = alternate_buffer;
        self.active_buffer_type = BufferType::Primary;
        self.tabs = Tabs::new(self.cols);
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.cursor_visible = true;
        self.pen = Pen::default();
        self.charsets = [Charset::Ascii, Charset::Ascii];
        self.active_charset = 0;
        self.insert_mode = false;
        self.origin_mode = false;
        self.auto_wrap_mode = true;
        self.new_line_mode = false;
        self.next_print_wraps = false;
        self.top_margin = 0;
        self.bottom_margin = self.rows - 1;
        self.saved_ctx = SavedCtx::default();
        self.alternate_saved_ctx = SavedCtx::default();
        self.dirty_lines = HashSet::from_iter(0..self.rows);
        self.resized = false;
    }

    // full state dump

    pub fn dump(&self) -> String {
        let (primary_ctx, alternate_ctx): (&SavedCtx, &SavedCtx) = match self.active_buffer_type {
            BufferType::Primary => (&self.saved_ctx, &self.alternate_saved_ctx),
            BufferType::Alternate => (&self.alternate_saved_ctx, &self.saved_ctx),
        };

        // 1. dump primary screen buffer

        // TODO don't include trailing empty lines
        let mut seq: String = self.primary_buffer().dump();

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
        seq.push_str(&format!(
            "\u{9b}{};{}H",
            primary_ctx.cursor_y + 1,
            primary_ctx.cursor_x + 1
        ));

        // configure pen
        seq.push_str(&primary_ctx.pen.dump());

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
            seq.push_str(&self.alternate_buffer().dump());
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
        seq.push_str(&format!(
            "\u{9b}{};{}H",
            alternate_ctx.cursor_y + 1,
            alternate_ctx.cursor_x + 1
        ));

        // configure pen
        seq.push_str(&alternate_ctx.pen.dump());

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
        seq.push_str(&format!(
            "\u{9b}{};{}r",
            self.top_margin + 1,
            self.bottom_margin + 1
        ));

        // 9. setup cursor

        let row = if self.origin_mode {
            self.cursor_y - self.top_margin + 1
        } else {
            self.cursor_y + 1
        };

        let column = self.cursor_x + 1;

        // fix cursor in target position
        seq.push_str(&format!("\u{9b}{row};{column}H"));

        if self.cursor_x >= self.cols {
            // move cursor past right border by re-printing the character in
            // the last column
            let cell = self.buffer[(self.cols - 1, self.cursor_y)];
            seq.push_str(&format!("{}{}", cell.1.dump(), cell.0));
        }

        // configure pen
        seq.push_str(&self.pen.dump());

        if !self.cursor_visible {
            // hide cursor
            seq.push_str("\u{9b}?25l");
        }

        // Below 3 must happen after ALL prints as they alter print behaviour,
        // including the "move cursor past right border one" above.

        // 10. setup charset

        if self.charsets[0] == Charset::Drawing {
            // put drawing charset into G0 slot
            seq.push_str("\u{1b}(0");
        }

        if self.charsets[1] == Charset::Drawing {
            // put drawing charset into G1 slot
            seq.push_str("\u{1b})0");
        }

        if self.active_charset == 1 {
            // shift-out: point GL to G1 slot
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

            State::Escape => seq.push('\u{1b}'),

            State::EscapeIntermediate => {
                let intermediates = self.intermediates.iter().collect::<String>();
                let s = format!("\u{1b}{intermediates}");
                seq.push_str(&s);
            }

            State::CsiEntry => seq.push('\u{9b}'),

            State::CsiParam => {
                let intermediates = self.intermediates.iter().collect::<String>();

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
                let intermediates = self.intermediates.iter().collect::<String>();
                let s = &format!("\u{9b}{intermediates}");
                seq.push_str(s);
            }

            State::CsiIgnore => seq.push_str("\u{9b}\u{3a}"),

            State::DcsEntry => seq.push('\u{90}'),

            State::DcsIntermediate => {
                let intermediates = self.intermediates.iter().collect::<String>();
                let s = &format!("\u{90}{intermediates}");
                seq.push_str(s);
            }

            State::DcsParam => {
                let intermediates = self.intermediates.iter().collect::<String>();

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
                let intermediates = self.intermediates.iter().collect::<String>();
                let s = &format!("\u{90}{intermediates}\u{40}");
                seq.push_str(s);
            }

            State::DcsIgnore => seq.push_str("\u{90}\u{3a}"),

            State::OscString => seq.push('\u{9d}'),

            State::SosPmApcString => seq.push('\u{98}'),
        }

        seq
    }

    fn primary_buffer(&self) -> &Buffer {
        if self.active_buffer_type == BufferType::Primary {
            &self.buffer
        } else {
            &self.other_buffer
        }
    }

    fn alternate_buffer(&self) -> &Buffer {
        if self.active_buffer_type == BufferType::Alternate {
            &self.buffer
        } else {
            &self.other_buffer
        }
    }

    pub fn lines(&self) -> &[Line] {
        self.buffer.view()
    }

    pub fn line(&self, n: usize) -> &Line {
        &self.buffer[n]
    }

    pub fn text(&self) -> Vec<String> {
        self.primary_buffer().text()
    }
}

#[cfg(test)]
mod tests {
    use super::Buffer;
    use super::BufferType;
    use super::Color;
    use super::Intensity;
    use super::State;
    use super::Vt;
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;
    use rgb::RGB8;
    use std::env;
    use std::fs;

    #[test]
    fn get_param() {
        let mut vt = Vt::new(1, 1);

        vt.feed_str("\x1b[;1;;23;456;");

        assert_eq!(vt.get_param(0, 999), 999);
        assert_eq!(vt.get_param(1, 999), 1);
        assert_eq!(vt.get_param(2, 999), 999);
        assert_eq!(vt.get_param(3, 999), 23);
        assert_eq!(vt.get_param(4, 999), 456);
        assert_eq!(vt.get_param(5, 999), 999);
    }

    #[test]
    fn auto_wrap_mode() {
        // auto wrap

        let mut vt = Vt::new(4, 4);
        vt.feed_str("\x1b[?7h");
        vt.feed_str("abcdef");
        assert_eq!(text(&vt), "abcd\nef|\n\n");

        // no auto wrap

        let mut vt = Vt::new(4, 4);
        vt.feed_str("\x1b[?7l");
        vt.feed_str("abcdef");
        assert_eq!(text(&vt), "abc|f\n\n\n");
    }

    #[test]
    fn print_at_the_end_of_the_screen() {
        // default margins, print at the bottom

        let mut vt = Vt::new(4, 6);

        let input = "xxxxxxxxxx\x1b[50;1Hyyy\x1b[50Czzz";
        vt.feed_str(input);

        assert_eq!(text(&vt), "xxxx\nxx\n\n\nyyyz\nzz|");

        // custom top margin, print above it

        let mut vt = Vt::new(4, 6);

        let input = "\nxxxxxxxxxx\x1b[2;4r\x1b[1;1Hyyy\x1b[50Czzz";

        vt.feed_str(input);

        assert_eq!(text(&vt), "yyyz\nzz|xx\nxxxx\nxx\n\n");

        // custom bottom margin, print below it

        let mut vt = Vt::new(4, 6);

        let input = "\x1b[;3rxxxxxxxxxx\x1b[50;1Hyyy\x1b[50Czzz";

        vt.feed_str(input);

        assert_eq!(text(&vt), "xxxx\nxxxx\nxx\n\n\nzz|yz");
    }

    #[test]
    fn execute_lf() {
        let mut vt = build_vt(8, 2, 3, 0, "abc");

        vt.feed_str("\n");

        assert_eq!(vt.cursor_x, 3);
        assert_eq!(vt.cursor_y, 1);
        assert_eq!(text(&vt), "abc\n   |");

        vt.feed_str("d\n");

        assert_eq!(vt.cursor_x, 4);
        assert_eq!(vt.cursor_y, 1);

        assert_eq!(text(&vt), "   d\n    |");
    }

    #[test]
    fn execute_ri() {
        let mut vt = build_vt(8, 5, 0, 0, "abcd\r\nefgh\r\nijkl\r\nmnop\r\nqrst");

        vt.feed_str("\x1bM"); // RI

        assert_eq!(text(&vt), "|\nabcd\nefgh\nijkl\nmnop");

        vt.feed_str("\x1b[3;4r"); // use smaller scroll region
        vt.feed_str("\x1b[3;1H"); // place cursor on top margin
        vt.feed_str("\x1bM"); // RI

        assert_eq!(text(&vt), "\nabcd\n|\nefgh\nmnop");
    }

    #[test]
    fn execute_su() {
        // short lines, default margins

        let mut vt = Vt::new(4, 6);
        vt.feed_str("aa\r\nbb\r\ncc\r\ndd\r\nee\r\nff");
        vt.feed_str("\x1b[2S");
        assert_eq!(text(&vt), "cc\ndd\nee\nff\n\n  |");

        // short lines, margins at 1 (top) and 4 (bottom)

        let mut vt = Vt::new(4, 6);
        vt.feed_str("aa\r\nbb\r\ncc\r\ndd\r\nee\r\nff");
        vt.feed_str("\x1b[2;5r");
        vt.feed_str("\x1b[1;1H");
        vt.feed_str("\x1b[2S");
        assert_eq!(text(&vt), "|aa\ndd\nee\n\n\nff");

        // wrapped lines, default margins

        let mut vt = Vt::new(4, 6);
        vt.feed_str("aaaaaa\r\nbbbbbb\r\ncccccc");
        vt.feed_str("\x1b[2S");
        assert_eq!(text(&vt), "bbbb\nbb\ncccc\ncc\n\n  |");
        assert!(vt.buffer[0].wrapped);
        assert!(!vt.buffer[1].wrapped);
        assert!(vt.buffer[2].wrapped);
        assert!(!vt.buffer[3].wrapped);
        assert!(!vt.buffer[4].wrapped);
        assert!(!vt.buffer[5].wrapped);

        // wrapped lines, margins at 1 (top) and 4 (bottom)

        let mut vt = Vt::new(4, 6);
        vt.feed_str("aaaaaa\r\nbbbbbb\r\ncccccc");
        vt.feed_str("\x1b[2;5r");
        vt.feed_str("\x1b[1;1H");
        vt.feed_str("\x1b[2S");
        assert_eq!(text(&vt), "|aaaa\nbb\ncccc\n\n\ncc");
        assert!(!vt.buffer[0].wrapped);
        assert!(!vt.buffer[1].wrapped);
        assert!(!vt.buffer[2].wrapped);
        assert!(!vt.buffer[3].wrapped);
        assert!(!vt.buffer[4].wrapped);
        assert!(!vt.buffer[5].wrapped);
    }

    #[test]
    fn execute_sd() {
        // short lines, default margins

        let mut vt = Vt::new(4, 6);
        vt.feed_str("aa\r\nbb\r\ncc\r\ndd\r\nee\r\nff");
        vt.feed_str("\x1b[2T");
        assert_eq!(text(&vt), "\n\naa\nbb\ncc\ndd|");

        // short lines, margins at 1 (top) and 4 (bottom)

        let mut vt = Vt::new(4, 6);
        vt.feed_str("aa\r\nbb\r\ncc\r\ndd\r\nee\r\nff");
        vt.feed_str("\x1b[2;5r");
        vt.feed_str("\x1b[1;1H");
        vt.feed_str("\x1b[2T");
        assert_eq!(text(&vt), "|aa\n\n\nbb\ncc\nff");

        // wrapped lines, default margins

        let mut vt = Vt::new(4, 6);
        vt.feed_str("aaaaaa\r\nbbbbbb\r\ncccccc");
        vt.feed_str("\x1b[2T");
        assert_eq!(text(&vt), "\n\naaaa\naa\nbbbb\nbb|");
        assert!(!vt.buffer[0].wrapped);
        assert!(!vt.buffer[1].wrapped);
        assert!(vt.buffer[2].wrapped);
        assert!(!vt.buffer[3].wrapped);
        assert!(vt.buffer[4].wrapped);
        assert!(!vt.buffer[5].wrapped);

        // wrapped lines, margins at 1 (top) and 4 (bottom)

        let mut vt = Vt::new(4, 6);
        vt.feed_str("aaaaaa\r\nbbbbbb\r\ncccccc");
        vt.feed_str("\x1b[2;5r");
        vt.feed_str("\x1b[1;1H");
        vt.feed_str("\x1b[2T");
        assert_eq!(text(&vt), "|aaaa\n\n\naa\nbbbb\ncc");
        assert!(!vt.buffer[0].wrapped);
        assert!(!vt.buffer[1].wrapped);
        assert!(!vt.buffer[2].wrapped);
        assert!(!vt.buffer[3].wrapped);
        assert!(!vt.buffer[4].wrapped);
        assert!(!vt.buffer[5].wrapped);
    }

    #[test]
    fn execute_bs() {
        let mut vt = Vt::new(4, 2);
        vt.feed_str("a");

        vt.feed_str("\x08");
        assert_eq!(text(&vt), "|a\n");

        vt.feed_str("\x08");
        assert_eq!(text(&vt), "|a\n");

        vt.feed_str("abcd");

        vt.feed_str("\x08");
        assert_eq!(text(&vt), "ab|cd\n");

        vt.feed_str("cdef");

        vt.feed_str("\x08");
        assert_eq!(text(&vt), "abcd\ne|f");

        vt.feed_str("\x08");
        assert_eq!(text(&vt), "abcd\n|ef");

        vt.feed_str("\x08");
        assert_eq!(text(&vt), "abcd\n|ef");
    }

    #[test]
    fn execute_cup() {
        let mut vt = Vt::new(4, 2);
        vt.feed_str("abc\r\ndef");

        vt.feed_str("\x1b[1;1;H");
        assert_eq!(vt.cursor_x, 0);
        assert_eq!(vt.cursor_y, 0);

        vt.feed_str("\x1b[10;10;H");
        assert_eq!(vt.cursor_x, 3);
        assert_eq!(vt.cursor_y, 1);
    }

    #[test]
    fn execute_cuu() {
        let mut vt = Vt::new(8, 4);
        vt.feed_str("abcd\n\n\n");

        vt.feed_str("\x1b[A");
        assert_eq!(vt.cursor_x, 4);
        assert_eq!(vt.cursor_y, 2);

        vt.feed_str("\x1b[2A");
        assert_eq!(vt.cursor_x, 4);
        assert_eq!(vt.cursor_y, 0);
    }

    #[test]
    fn execute_cpl() {
        let mut vt = Vt::new(8, 4);

        vt.feed_str("abcd\r\n\r\n\r\nef");
        assert_eq!(vt.cursor_x, 2);
        assert_eq!(vt.cursor_y, 3);

        vt.feed_str("\x1b[F");
        assert_eq!(vt.cursor_x, 0);
        assert_eq!(vt.cursor_y, 2);

        vt.feed_str("\x1b[2F");
        assert_eq!(vt.cursor_x, 0);
        assert_eq!(vt.cursor_y, 0);
    }

    #[test]
    fn execute_cnl() {
        let mut vt = Vt::new(4, 4);
        vt.feed_str("ab");

        vt.feed_str("\x1b[E");
        assert_eq!(vt.cursor_x, 0);
        assert_eq!(vt.cursor_y, 1);

        vt.feed_str("\x1b[3E");
        assert_eq!(vt.cursor_x, 0);
        assert_eq!(vt.cursor_y, 3);
    }

    #[test]
    fn execute_vpa() {
        let mut vt = Vt::new(4, 4);
        vt.feed_str("\r\n\r\naaa\r\nbbb");

        vt.feed_str("\x1b[d");
        assert_eq!(vt.cursor_x, 3);
        assert_eq!(vt.cursor_y, 0);

        vt.feed_str("\x1b[10d");
        assert_eq!(vt.cursor_x, 3);
        assert_eq!(vt.cursor_y, 3);
    }

    #[test]
    fn execute_cud() {
        let mut vt = Vt::new(8, 4);
        vt.feed_str("abcd");

        vt.feed_str("\x1b[B");
        assert_eq!(text(&vt), "abcd\n    |\n\n");

        vt.feed_str("\x1b[2B");
        assert_eq!(text(&vt), "abcd\n\n\n    |");
    }

    #[test]
    fn execute_cuf() {
        let mut vt = Vt::new(4, 1);

        vt.feed_str("\x1b[2C");
        assert_eq!(text(&vt), "  |");

        vt.feed_str("\x1b[2C");
        assert_eq!(text(&vt), "   |");

        vt.feed_str("a");
        assert_eq!(text(&vt), "   a|");
        assert!(vt.next_print_wraps);

        vt.feed_str("\x1b[5C");
        assert_eq!(text(&vt), "   |a");
        assert!(!vt.next_print_wraps);

        vt.feed_str("ab");
        vt.feed_str("\x1b[10C");
        assert_eq!(text(&vt), "b  |");
    }

    #[test]
    fn execute_cub() {
        let mut vt = Vt::new(8, 2);

        vt.feed_str("abcd");
        vt.feed_str("\x1b[2D");
        assert_eq!(text(&vt), "ab|cd\n");

        vt.feed_str("cdef");
        vt.feed_str("\x1b[2D");
        assert_eq!(text(&vt), "abcd|ef\n");

        vt.feed_str("\x1b[10D");
        assert_eq!(text(&vt), "|abcdef\n");

        let mut vt = Vt::new(4, 2);

        vt.feed_str("abcd");
        vt.feed_str("\x1b[D");
        assert_eq!(text(&vt), "ab|cd\n");
    }

    #[test]
    fn execute_ich() {
        let mut vt = build_vt(8, 2, 3, 0, "abcdefghijklmn");

        vt.feed_str("\x1b[@");
        assert_eq!(vt.cursor_x, 3);
        assert_eq!(text(&vt), "abc| defg\nijklmn");
        assert!(vt.buffer[0].wrapped);
        assert!(!vt.buffer[1].wrapped);

        vt.feed_str("\x1b[2@");
        assert_eq!(text(&vt), "abc|   de\nijklmn");

        vt.feed_str("\x1b[10@");
        assert_eq!(text(&vt), "abc|\nijklmn");

        let mut vt = build_vt(8, 2, 7, 0, "abcdefghijklmn");

        vt.feed_str("\x1b[10@");
        assert_eq!(text(&vt), "abcdefg|\nijklmn");
    }

    #[test]
    fn execute_il() {
        let mut vt = build_vt(4, 4, 2, 1, "abcdefghij");

        vt.feed_str("\x1b[L");
        assert_eq!(text(&vt), "abcd\n  |\nefgh\nij");
        assert!(!vt.buffer[0].wrapped);
        assert!(!vt.buffer[1].wrapped);
        assert!(vt.buffer[2].wrapped);
        assert!(!vt.buffer[3].wrapped);

        vt.feed_str("\x1b[A");
        vt.feed_str("\x1b[L");
        assert_eq!(text(&vt), "  |\nabcd\n\nefgh");
        assert!(!vt.buffer[0].wrapped);
        assert!(!vt.buffer[1].wrapped);
        assert!(!vt.buffer[2].wrapped);
        assert!(!vt.buffer[3].wrapped);

        vt.feed_str("\x1b[3B");
        vt.feed_str("\x1b[100L");
        assert_eq!(text(&vt), "\nabcd\n\n  |");
    }

    #[test]
    fn execute_dl() {
        let mut vt = Vt::new(4, 4);
        vt.feed_str("abcdefghijklmn");

        vt.feed_str("\x1b[2A");
        vt.feed_str("\x1b[M");
        assert_eq!(text(&vt), "abcd\nij|kl\nmn\n");
        assert!(!vt.buffer[0].wrapped);
        assert!(vt.buffer[1].wrapped);
        assert!(!vt.buffer[2].wrapped);
        assert!(!vt.buffer[3].wrapped);

        // cursor above bottom margin

        let mut vt = Vt::new(4, 4);
        vt.feed_str("abcdefghijklmn");

        vt.feed_str("\x1b[1;3r");
        vt.feed_str("\x1b[2;1H");
        vt.feed_str("\x1b[M");
        assert_eq!(text(&vt), "abcd\n|ijkl\n\nmn");
        assert!(!vt.buffer[0].wrapped);
        assert!(!vt.buffer[1].wrapped);
        assert!(!vt.buffer[2].wrapped);
        assert!(!vt.buffer[3].wrapped);

        // cursor below bottom margin

        let mut vt = Vt::new(4, 4);
        vt.feed_str("abcdefghijklmn");

        vt.feed_str("\x1b[1;2r");
        vt.feed_str("\x1b[4;1H");
        vt.feed_str("\x1b[M");
        assert_eq!(text(&vt), "abcd\nefgh\nijkl\n|");
        assert!(vt.buffer[0].wrapped);
        assert!(vt.buffer[1].wrapped);
        assert!(!vt.buffer[2].wrapped);
        assert!(!vt.buffer[3].wrapped);
    }

    #[test]
    fn execute_el() {
        // short lines

        // a) clear to the end of the line

        let mut vt = build_vt(4, 2, 2, 0, "abcd");
        vt.feed_str("\x1b[0K");
        assert_eq!(text(&vt), "ab|\n");

        let mut vt = build_vt(4, 2, 2, 0, "a");
        vt.feed_str("\x1b[0K");
        assert_eq!(text(&vt), "a |\n");

        // b) clear to the beginning of the line

        let mut vt = build_vt(4, 2, 2, 0, "abcd");
        vt.feed_str("\x1b[1K");
        assert_eq!(text(&vt), "  | d\n");

        // c) clear the whole line

        let mut vt = build_vt(4, 2, 2, 0, "abcd");
        vt.feed_str("\x1b[2K");
        assert_eq!(text(&vt), "  |\n");

        // wrapped lines

        // a) clear to the end of the line

        let mut vt = Vt::new(4, 3);
        vt.feed_str("abcdefghij\x1b[A");
        vt.feed_str("\x1b[0K");
        assert_eq!(text(&vt), "abcd\nef|\nij");
        assert!(vt.buffer[0].wrapped);
        assert!(!vt.buffer[1].wrapped);
        assert!(!vt.buffer[2].wrapped);

        // b) clear to the beginning of the line

        let mut vt = Vt::new(4, 3);
        vt.feed_str("abcdefghij\x1b[A");
        vt.feed_str("\x1b[1K");
        assert_eq!(text(&vt), "abcd\n  | h\nij");
        assert!(vt.buffer[0].wrapped);
        assert!(vt.buffer[1].wrapped);
        assert!(!vt.buffer[2].wrapped);

        // c) clear the whole line

        let mut vt = Vt::new(4, 3);
        vt.feed_str("abcdefghij\x1b[A");
        vt.feed_str("\x1b[2K");
        assert_eq!(text(&vt), "abcd\n  |\nij");
        assert!(vt.buffer[0].wrapped);
        assert!(!vt.buffer[1].wrapped);
        assert!(!vt.buffer[2].wrapped);
    }

    #[test]
    fn execute_ed() {
        // short lines

        // a) clear to the end of the screen

        let mut vt = build_vt(4, 3, 1, 1, "abc\r\ndef\r\nghi");
        vt.feed_str("\x1b[0J");
        assert_eq!(text(&vt), "abc\nd|\n");

        let mut vt = build_vt(4, 3, 1, 1, "abc\r\n\r\nghi");
        vt.feed_str("\x1b[0J");
        assert_eq!(text(&vt), "abc\n |\n");

        // b) clear to the beginning of the screen

        let mut vt = build_vt(4, 3, 1, 1, "abc\r\ndef\r\nghi");
        vt.feed_str("\x1b[1J");
        assert_eq!(text(&vt), "\n | f\nghi");

        // c) clear the whole screen

        let mut vt = build_vt(4, 3, 1, 1, "abc\r\ndef\r\nghi");
        vt.feed_str("\x1b[2J");
        assert_eq!(text(&vt), "\n |\n");

        // wrapped lines

        // a) clear to the end of the screen

        let mut vt = build_vt(4, 3, 1, 1, "abcdefghij");
        vt.feed_str("\x1b[0J");
        assert_eq!(text(&vt), "abcd\ne|\n");
        assert!(vt.buffer[0].wrapped);
        assert!(!vt.buffer[1].wrapped);
        assert!(!vt.buffer[2].wrapped);

        // b) clear to the beginning of the screen

        let mut vt = build_vt(4, 3, 1, 1, "abcdefghij");
        vt.feed_str("\x1b[1J");
        assert_eq!(text(&vt), "\n | gh\nij");
        assert!(!vt.buffer[0].wrapped);
        assert!(vt.buffer[1].wrapped);
        assert!(!vt.buffer[2].wrapped);

        // c) clear the whole screen

        let mut vt = build_vt(4, 3, 1, 1, "abcdefghij");
        vt.feed_str("\x1b[2J");
        assert_eq!(text(&vt), "\n |\n");
        assert!(!vt.buffer[0].wrapped);
        assert!(!vt.buffer[1].wrapped);
        assert!(!vt.buffer[2].wrapped);
    }

    #[test]
    fn execute_dch() {
        let mut vt = build_vt(8, 2, 3, 0, "abcdefghijkl");

        vt.feed_str("\x1b[P");
        assert_eq!(text(&vt), "abc|efgh\nijkl");
        assert!(!vt.buffer[0].wrapped);
        assert!(!vt.buffer[1].wrapped);

        vt.feed_str("\x1b[2P");
        assert_eq!(text(&vt), "abc|gh\nijkl");

        vt.feed_str("\x1b[10P");
        assert_eq!(text(&vt), "abc|\nijkl");

        vt.feed_str("\x1b[10C");
        vt.feed_str("\x1b[10P");
        assert_eq!(text(&vt), "abc    |\nijkl");
    }

    #[test]
    fn execute_ech() {
        let mut vt = build_vt(8, 2, 3, 0, "abcdefghijkl");

        vt.feed_str("\x1b[X");

        assert_eq!(text(&vt), "abc| efgh\nijkl");
        assert!(vt.buffer[0].wrapped);
        assert!(!vt.buffer[1].wrapped);

        vt.feed_str("\x1b[2X");
        assert_eq!(text(&vt), "abc|  fgh\nijkl");
        assert!(vt.buffer[0].wrapped);
        assert!(!vt.buffer[1].wrapped);

        vt.feed_str("\x1b[10X");
        assert_eq!(text(&vt), "abc|\nijkl");
        assert!(!vt.buffer[0].wrapped);
        assert!(!vt.buffer[1].wrapped);

        vt.feed_str("\x1b[3C\x1b[X");
        assert_eq!(text(&vt), "abc   |\nijkl");
    }

    #[test]
    fn execute_cht() {
        let mut vt = build_vt(28, 1, 3, 0, "abcdefghijklmnopqrstuwxyzabc");

        vt.feed_str("\x1b[I");
        assert_eq!(vt.cursor_x, 8);

        vt.feed_str("\x1b[2I");
        assert_eq!(vt.cursor_x, 24);

        vt.feed_str("\x1b[I");
        assert_eq!(vt.cursor_x, 27);
    }

    #[test]
    fn execute_cbt() {
        let mut vt = build_vt(28, 1, 26, 0, "abcdefghijklmnopqrstuwxyzabc");

        vt.feed_str("\x1b[Z");
        assert_eq!(vt.cursor_x, 24);

        vt.feed_str("\x1b[2Z");
        assert_eq!(vt.cursor_x, 8);

        vt.feed_str("\x1b[Z");
        assert_eq!(vt.cursor_x, 0);
    }

    #[test]
    fn execute_sc_rc() {
        // DECSC/DECRC variant

        let mut vt = build_vt(4, 3, 0, 0, "");

        // move 2x right, 1 down
        vt.feed_str("  \n");

        // save cursor
        vt.feed_str("\x1b7");

        // move 1x right, 1x down
        vt.feed_str(" \n");

        // restore cursor
        vt.feed_str("\x1b8");

        assert_eq!(vt.cursor_x, 2);
        assert_eq!(vt.cursor_y, 1);

        // ansi.sys variant

        let mut vt = build_vt(4, 3, 0, 0, "");

        // move 2x right, 1 down
        vt.feed_str("  \n");

        // save cursor
        vt.feed_str("\x1b[s");

        // move 1x right, 1x down
        vt.feed_str(" \n");

        // restore cursor
        vt.feed_str("\x1b[u");

        assert_eq!(vt.cursor_x, 2);
        assert_eq!(vt.cursor_y, 1);
    }

    #[test]
    fn execute_rep() {
        let mut vt = build_vt(20, 2, 0, 0, "");

        vt.feed_str("\x1b[b"); // REP
        assert_eq!(text(&vt), "|\n");

        vt.feed_str("A");
        vt.feed_str("\x1b[b");
        assert_eq!(text(&vt), "AA|\n");

        vt.feed_str("\x1b[3b");
        assert_eq!(text(&vt), "AAAAA|\n");

        vt.feed_str("\x1b[5C"); // move 5 cols to the right
        vt.feed_str("\x1b[b");
        assert_eq!(text(&vt), "AAAAA      |\n");
    }

    #[test]
    fn execute_sgr() {
        let mut vt = build_vt(4, 1, 0, 0, "abcd");

        vt.feed_str("\x1b[1m");
        assert!(vt.pen.intensity == Intensity::Bold);

        vt.feed_str("\x1b[2m");
        assert_eq!(vt.pen.intensity, Intensity::Faint);

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
        assert_eq!(vt.pen.intensity, Intensity::Bold);
        assert!(vt.pen.blink);
        assert_eq!(vt.pen.foreground, Some(Color::Indexed(88)));
        assert_eq!(vt.pen.background, Some(Color::Indexed(99)));

        vt.feed_str("\x1b[1;38;2;1;101;201;48;2;2;102;202;5m");
        assert_eq!(vt.pen.intensity, Intensity::Bold);
        assert!(vt.pen.blink);
        assert_eq!(vt.pen.foreground, Some(Color::RGB(RGB8::new(1, 101, 201))));
        assert_eq!(vt.pen.background, Some(Color::RGB(RGB8::new(2, 102, 202))));
    }

    #[test]
    fn execute_xtwinops_wider() {
        let mut vt = Vt::new(6, 6);
        vt.resizable = true;

        vt.feed_str("\x1b[8;6;7t");
        assert_eq!(text(&vt), "|\n\n\n\n\n");
        assert!(!vt.lines().iter().any(|l| l.wrapped));

        vt.feed_str("\x1b[8;6;15t");
        assert_eq!(text(&vt), "|\n\n\n\n\n");
        assert!(!vt.lines().iter().any(|l| l.wrapped));

        let mut vt = Vt::new(6, 6);
        vt.resizable = true;

        vt.feed_str("000000111111222222333333444444555");
        assert_eq!(text(&vt), "000000\n111111\n222222\n333333\n444444\n555|");
        assert_eq!(wrapped(&vt), vec![true, true, true, true, true, false]);

        vt.feed_str("\x1b[8;6;7t");
        assert_eq!(text(&vt), "0000001\n1111122\n2222333\n3334444\n44555|\n");
        assert_eq!(wrapped(&vt), vec![true, true, true, true, false, false]);

        vt.feed_str("\x1b[8;6;15t");
        assert_eq!(text(&vt), "000000111111222\n222333333444444\n555|\n\n\n");
        assert_eq!(wrapped(&vt), vec![true, true, false, false, false, false]);

        let mut vt = Vt::new(4, 3);
        vt.resizable = true;

        vt.feed_str("000011\r\n22");
        assert_eq!(text(&vt), "0000\n11\n22|");
        assert_eq!(wrapped(&vt), vec![true, false, false]);

        vt.feed_str("\x1b[8;3;8t");
        assert_eq!(text(&vt), "000011\n22|\n");
        assert_eq!(wrapped(&vt), vec![false, false, false]);
    }

    #[test]
    fn execute_xtwinops_narrower() {
        let mut vt = Vt::new(15, 6);
        vt.resizable = true;

        vt.feed_str("\x1b[8;6;7t");
        assert_eq!(text(&vt), "|\n\n\n\n\n");
        assert!(!vt.lines().iter().any(|l| l.wrapped));

        vt.feed_str("\x1b[8;6;6t");
        assert_eq!(text(&vt), "|\n\n\n\n\n");
        assert!(!vt.lines().iter().any(|l| l.wrapped));

        let mut vt = Vt::new(8, 2);
        vt.resizable = true;

        vt.feed_str("\nabcdef");
        assert_eq!(wrapped(&vt), vec![false, false]);
        vt.feed_str("\x1b[8;;4t");
        assert_eq!(text(&vt), "abcd\nef|");
        assert_eq!(wrapped(&vt), vec![true, false]);

        let mut vt = Vt::new(15, 6);
        vt.resizable = true;

        vt.feed_str("000000111111222222333333444444555");
        assert_eq!(text(&vt), "000000111111222\n222333333444444\n555|\n\n\n");
        assert_eq!(wrapped(&vt), vec![true, true, false, false, false, false]);

        vt.feed_str("\x1b[8;6;7t");
        assert_eq!(text(&vt), "2222333\n3334444\n44555|\n\n\n");
        assert_eq!(wrapped(&vt), vec![true, true, false, false, false, false]);

        vt.feed_str("\x1b[8;6;6t");
        assert_eq!(text(&vt), "333333\n444444\n555|\n\n\n");
        assert_eq!(wrapped(&vt), vec![true, true, false, false, false, false]);
    }

    #[test]
    fn execute_xtwinops() {
        let mut vt = build_vt(8, 4, 0, 3, "abcdefgh\r\nijklmnop\r\nqrstuw");
        vt.resizable = true;

        let (_, resized) = vt.feed_str("AAA");
        assert!(!resized);

        let (_, resized) = vt.feed_str("\x1b[8;5;t");
        assert!(resized);
        assert_eq!(text(&vt), "abcdefgh\nijklmnop\nqrstuw\nAAA|\n");

        vt.feed_str("BBBBB");
        assert_eq!(vt.cursor_x, 8);
        assert_eq!(vt.next_print_wraps, true);

        let (_, resized) = vt.feed_str("\x1b[8;;4t");
        assert!(resized);
        assert_eq!(text(&vt), "qrst\nuw\nAAAB\nBBB|B\n");
        assert_eq!(vt.next_print_wraps, false);

        vt.feed_str("\rCCC");
        assert_eq!(text(&vt), "qrst\nuw\nAAAB\nCCC|B\n");
        assert_eq!(wrapped(&vt), vec![true, false, true, false, false]);

        vt.feed_str("\x1b[8;;3t");
        assert_eq!(text(&vt), "tuw\nAAA\nBCC\nC|B\n");

        vt.feed_str("\x1b[8;;5t");
        assert_eq!(text(&vt), "qrstu\nw\nAAABC\nCC|B\n");

        vt.feed_str("DDD");
        assert_eq!(vt.next_print_wraps, true);

        vt.feed_str("\x1b[8;;6t");
        assert_eq!(text(&vt), "op\nqrstuw\nAAABCC\nCDDD|\n");
    }

    #[test]
    fn execute_xtwinops_noop() {
        let mut vt = Vt::new(8, 4);
        let (_, resized) = vt.feed_str("\x1b[8;;t");
        assert!(!resized);
    }

    #[test]
    fn execute_xtwinops_taller() {
        let mut vt = Vt::new(6, 4);
        vt.resizable = true;

        vt.feed_str("AAA\n\rBBB\n\r");

        let (_, resized) = vt.feed_str("\x1b[8;5;;t");
        assert!(resized);
        assert_eq!(text(&vt), "AAA\nBBB\n|\n\n");
    }

    #[test]
    fn execute_xtwinops_shorter() {
        let mut vt = Vt::new(6, 6);
        vt.resizable = true;

        vt.feed_str("AAA\n\rBBB\n\rCCC\n\r");

        let (_, resized) = vt.feed_str("\x1b[8;5;;t");
        assert!(resized);
        assert_eq!(text(&vt), "AAA\nBBB\nCCC\n|\n");

        let (_, resized) = vt.feed_str("\x1b[8;3;;t");
        assert!(resized);
        assert_eq!(text(&vt), "BBB\nCCC\n|");

        let (_, resized) = vt.feed_str("\x1b[8;2;;t");
        assert!(resized);
        assert_eq!(text(&vt), "CCC\n|");
    }

    #[test]
    fn execute_xtwinops_vs_tabs() {
        let mut vt = Vt::new(6, 2);
        vt.resizable = true;
        assert_eq!(vt.tabs, vec![]);

        vt.feed_str("\x1b[8;;10;t");
        assert_eq!(vt.tabs, vec![8]);

        vt.feed_str("\x1b[8;;30;t");
        assert_eq!(vt.tabs, vec![8, 16, 24]);

        vt.feed_str("\x1b[8;;20;t");
        assert_eq!(vt.tabs, vec![8, 16]);
    }

    #[test]
    fn execute_xtwinops_vs_saved_ctx() {
        let mut vt = Vt::new(20, 5);
        vt.resizable = true;

        // move cursor to col 15
        vt.feed_str("xxxxxxxxxxxxxxx");
        assert_eq!(vt.cursor_x, 15);

        // save cursor
        vt.feed_str("\x1b7");
        assert_eq!(vt.saved_ctx.cursor_x, 15);

        // switch to alternate buffer
        vt.feed_str("\x1b[?47h");

        // save cursor
        vt.feed_str("\x1b7");
        assert_eq!(vt.saved_ctx.cursor_x, 15);

        // resize to 10x5
        vt.feed_str("\x1b[8;;10;t");
        assert_eq!(vt.saved_ctx.cursor_x, 9);
    }

    #[test]
    fn execute_xtwinops_vs_buffer_switching() {
        let mut vt = Vt::new(4, 4);
        vt.resizable = true;

        // fill primary buffer
        vt.feed_str("aaa\n\rbbb\n\rc\n\rddd");
        assert_eq!(vt.cursor_x, 3);

        // resize to 4x5
        vt.feed_str("\x1b[8;5;4;t");
        assert_eq!(text(&vt), "aaa\nbbb\nc\nddd|\n");

        // switch to alternate buffer
        vt.feed_str("\x1b[?1049h");
        assert_eq!((vt.cursor_x, vt.cursor_y), (3, 3));

        // resize to 4x2
        vt.feed_str("\x1b[8;2;4t");
        assert_eq!((vt.cursor_x, vt.cursor_y), (3, 1));

        // resize to 2x3, we'll check later if primary buffer preserved more columns
        vt.feed_str("\x1b[8;3;2t");

        // resize to 3x3
        vt.feed_str("\x1b[8;3;3t");

        // switch back to primary buffer
        vt.feed_str("\x1b[?1049l");
        assert_eq!(text(&vt), "bbb\nc\ndd|d");
    }

    #[test]
    fn dump_initial() {
        let vt1 = Vt::new(10, 4);
        let mut vt2 = Vt::new(10, 4);

        vt2.feed_str(&vt1.dump());

        assert_vts_eq(&vt1, &vt2);
    }

    #[test]
    fn dump_modified() {
        let mut vt1 = Vt::new(10, 4);
        let mut vt2 = Vt::new(10, 4);

        vt1.feed_str("hello\n\rworld\u{9b}5W\u{9b}7`\u{1b}[W\u{9b}?6h");
        vt1.feed_str("\u{9b}2;4r\u{9b}1;5H\x1b[1;31;41m\u{9b}?25l\u{9b}4h");
        vt1.feed_str("\u{9b}?7l\u{9b}20h\u{9b}\u{3a}\x1b(0\x1b)0\u{0e}");

        vt2.feed_str(&vt1.dump());

        assert_vts_eq(&vt1, &vt2);
    }

    #[test]
    fn dump_with_file() {
        if let Ok((w, h, input, step)) = setup_dump_with_file() {
            let mut vt1 = Vt::new(w, h);

            let mut s = 0;

            for c in input.chars().take(1_000_000) {
                vt1.feed(c);

                if s == 0 {
                    let d = vt1.dump();
                    let mut vt2 = Vt::new(w, h);

                    vt2.feed_str(&d);

                    assert_vts_eq(&vt1, &vt2);
                }

                s = (s + 1) % step;
            }
        }
    }

    #[test]
    fn charsets() {
        let mut vt = build_vt(6, 7, 0, 0, "");

        // GL points to G0, G0 is set to ascii
        vt.feed_str("alpty\r\n");

        // GL points to G0, G0 is set to drawing
        vt.feed_str("\x1b(0alpty\r\n");

        // GL points to G1, G1 is still set to ascii
        vt.feed_str("\u{0e}alpty\r\n");

        // GL points to G1, G1 is set to drawing
        vt.feed_str("\x1b)0alpty\r\n");

        // GL points to G1, G1 is set back to ascii
        vt.feed_str("\x1b)Balpty\r\n");

        // GL points to G0, G0 is set back to ascii
        vt.feed_str("\x1b(B\u{0f}alpty");

        assert_eq!(text(&vt), "alpty\n▒┌⎻├≤\nalpty\n▒┌⎻├≤\nalpty\nalpty|\n");
    }

    fn gen_input(max_len: usize) -> impl Strategy<Value = Vec<char>> {
        prop::collection::vec(
            prop_oneof![gen_ctl_seq(), gen_esc_seq(), gen_csi_seq(), gen_text()],
            1..=max_len,
        )
        .prop_map(flatten)
    }

    fn gen_ctl_seq() -> impl Strategy<Value = Vec<char>> {
        let ctl_chars = vec![0x00..0x18, 0x19..0x1a, 0x1c..0x20];

        prop::sample::select(flatten(ctl_chars)).prop_map(|v: u8| vec![v as char])
    }

    fn gen_esc_seq() -> impl Strategy<Value = Vec<char>> {
        (
            prop::collection::vec(gen_esc_intermediate(), 0..=2),
            gen_esc_finalizer(),
        )
            .prop_map(|(inters, fin)| flatten(vec![vec!['\x1b'], inters, vec![fin]]))
    }

    fn gen_csi_seq() -> impl Strategy<Value = Vec<char>> {
        prop_oneof![
            gen_csi_sgr_seq(),
            gen_csi_sm_seq(),
            gen_csi_rm_seq(),
            gen_csi_any_seq(),
        ]
    }

    fn gen_text() -> impl Strategy<Value = Vec<char>> {
        prop::collection::vec(gen_char(), 1..10)
    }

    fn gen_esc_intermediate() -> impl Strategy<Value = char> {
        (0x20..0x30u8).prop_map(|v| v as char)
    }

    fn gen_esc_finalizer() -> impl Strategy<Value = char> {
        let finalizers = vec![
            0x30..0x50,
            0x51..0x58,
            0x59..0x5a,
            0x5a..0x5b,
            0x5c..0x5d,
            0x60..0x7f,
        ];

        prop::sample::select(flatten(finalizers)).prop_map(|v: u8| v as char)
    }

    fn gen_csi_sgr_seq() -> impl Strategy<Value = Vec<char>> {
        gen_csi_params().prop_map(|params| flatten(vec![vec!['\x1b', '['], params, vec!['m']]))
    }

    fn gen_csi_sm_seq() -> impl Strategy<Value = Vec<char>> {
        (gen_csi_intermediate(), gen_csi_sm_rm_param()).prop_map(|(inters, params)| {
            flatten(vec![vec!['\x1b', '['], inters, params, vec!['h']])
        })
    }

    fn gen_csi_rm_seq() -> impl Strategy<Value = Vec<char>> {
        (gen_csi_intermediate(), gen_csi_sm_rm_param()).prop_map(|(inters, params)| {
            flatten(vec![vec!['\x1b', '['], inters, params, vec!['l']])
        })
    }

    fn gen_csi_any_seq() -> impl Strategy<Value = Vec<char>> {
        (gen_csi_params(), gen_csi_finalizer())
            .prop_map(|(params, fin)| flatten(vec![vec!['\x1b', '['], params, vec![fin]]))
    }

    fn gen_csi_intermediate() -> impl Strategy<Value = Vec<char>> {
        prop::collection::vec(prop::sample::select(vec!['?', '!']), 0..=1)
    }

    fn gen_csi_params() -> impl Strategy<Value = Vec<char>> {
        prop::collection::vec(
            prop_oneof![
                gen_csi_param(),
                gen_csi_param(),
                prop::sample::select(vec![';'])
            ],
            0..=5,
        )
    }

    fn gen_csi_param() -> impl Strategy<Value = char> {
        (0x30..0x3au8).prop_map(|v| v as char)
    }

    fn gen_csi_sm_rm_param() -> impl Strategy<Value = Vec<char>> {
        let modes = vec![4, 6, 7, 20, 25, 47, 1047, 1048, 1049];

        prop_oneof![
            prop::sample::select(modes).prop_map(|n| n.to_string().chars().collect()),
            prop::collection::vec(gen_csi_param(), 1..=4)
        ]
    }

    fn gen_csi_finalizer() -> impl Strategy<Value = char> {
        (0x40..0x7fu8).prop_map(|v| v as char)
    }

    fn gen_char() -> impl Strategy<Value = char> {
        prop_oneof![
            gen_ascii_char(),
            gen_ascii_char(),
            gen_ascii_char(),
            gen_ascii_char(),
            gen_ascii_char(),
            (0x80..=0xd7ffu32).prop_map(|v| char::from_u32(v).unwrap()),
            (0xf900..=0xffffu32).prop_map(|v| char::from_u32(v).unwrap())
        ]
    }

    fn gen_ascii_char() -> impl Strategy<Value = char> {
        (0x20..=0x7fu8).prop_map(|v| v as char)
    }

    fn flatten<T, I: IntoIterator<Item = T>>(seqs: Vec<I>) -> Vec<T> {
        seqs.into_iter().flatten().collect()
    }

    proptest! {
        #[test]
        fn prop_sanity_checks_infinite_scrollback(input in gen_input(25)) {
            let mut vt = Vt::new(10, 5);
            vt.resizable = true;

            vt.feed_str(&(input.into_iter().collect::<String>()));

            assert!(!vt.next_print_wraps && vt.cursor_x < vt.cols || vt.next_print_wraps && vt.cursor_x == vt.cols);
            assert!(vt.cursor_y < vt.rows);
            assert!(vt.buffer.len() >= vt.rows);
            assert!(vt.lines().iter().all(|line| line.len() == vt.cols));
            assert!(!vt.lines().last().unwrap().wrapped);
        }

        #[test]
        fn prop_sanity_checks_no_scrollback(input in gen_input(25)) {
            let mut vt = Vt::with_scrollback_limit(10, 5, Some(0));
            vt.resizable = true;

            vt.feed_str(&(input.into_iter().collect::<String>()));

            assert!(!vt.next_print_wraps && vt.cursor_x < vt.cols || vt.next_print_wraps && vt.cursor_x == vt.cols);
            assert!(vt.cursor_y < vt.rows);
            assert!(vt.buffer.len() == vt.rows);
            assert!(vt.lines().iter().all(|line| line.len() == vt.cols));
            assert!(!vt.lines().last().unwrap().wrapped);
        }

        #[test]
        fn prop_sanity_checks_fixed_scrollback(input in gen_input(25)) {
            let scrollback_limit = 3;
            let mut vt = Vt::with_scrollback_limit(10, 5, Some(scrollback_limit));
            vt.resizable = true;

            vt.feed_str(&(input.into_iter().collect::<String>()));

            assert!(!vt.next_print_wraps && vt.cursor_x < vt.cols || vt.next_print_wraps && vt.cursor_x == vt.cols);
            assert!(vt.cursor_y < vt.rows);
            assert!(vt.buffer.len() >= vt.rows && vt.buffer.len() <= vt.rows + scrollback_limit);
            assert!(vt.lines().iter().all(|line| line.len() == vt.cols));
            assert!(!vt.lines().last().unwrap().wrapped);
        }

        #[test]
        fn prop_resizing(new_cols in 2..15usize, new_rows in 2..8usize, input1 in gen_input(25), input2 in gen_input(25)) {
            let mut vt = Vt::new(10, 5);
            vt.resizable = true;

            vt.feed_str(&(input1.into_iter().collect::<String>()));
            vt.feed_str(&format!("\x1b[8;{};{}t", new_rows, new_cols));
            vt.feed_str(&(input2.into_iter().collect::<String>()));

            assert!(!vt.next_print_wraps && vt.cursor_x < vt.cols || vt.next_print_wraps && vt.cursor_x == vt.cols);
            assert!(vt.cursor_y < vt.rows);
            assert!(vt.buffer.len() >= vt.rows);
            assert!(vt.lines().iter().all(|line| line.len() == vt.cols));
            assert!(!vt.lines().last().unwrap().wrapped);
        }

        #[test]
        fn prop_dump(input in gen_input(25)) {
            let mut vt1 = Vt::new(10, 5);
            let mut vt2 = Vt::new(vt1.cols, vt1.rows);

            vt1.feed_str(&(input.into_iter().collect::<String>()));
            vt2.feed_str(&vt1.dump());

            assert_vts_eq(&vt1, &vt2);
        }
    }

    fn setup_dump_with_file() -> Result<(usize, usize, String, usize), env::VarError> {
        let path = env::var("P")?;
        let input = fs::read_to_string(path).unwrap();
        let w: usize = env::var("W").unwrap().parse::<usize>().unwrap();
        let h: usize = env::var("H").unwrap().parse::<usize>().unwrap();
        let step: usize = env::var("S")
            .unwrap_or("1".to_owned())
            .parse::<usize>()
            .unwrap();

        Ok((w, h, input, step))
    }

    fn build_vt(cols: usize, rows: usize, cx: usize, cy: usize, init: &str) -> Vt {
        let mut vt = Vt::new(cols, rows);
        vt.feed_str(init);
        vt.feed_str(&format!("\u{9b}{};{}H", cy + 1, cx + 1));

        vt
    }

    fn assert_vts_eq(vt1: &Vt, vt2: &Vt) {
        assert_eq!(vt1.state, vt2.state);

        if vt1.state == State::CsiParam || vt1.state == State::DcsParam {
            assert_eq!(vt1.params, vt2.params);
        }

        if vt1.state == State::EscapeIntermediate
            || vt1.state == State::CsiIntermediate
            || vt1.state == State::CsiParam
            || vt1.state == State::DcsIntermediate
            || vt1.state == State::DcsParam
        {
            assert_eq!(vt1.intermediates, vt2.intermediates);
        }

        assert_eq!(vt1.active_buffer_type, vt2.active_buffer_type);
        assert_eq!(vt1.cursor_x, vt2.cursor_x);
        assert_eq!(
            vt1.cursor_y, vt2.cursor_y,
            "margins: {} {}",
            vt1.top_margin, vt2.bottom_margin
        );
        assert_eq!(vt1.cursor_visible, vt2.cursor_visible);
        assert_eq!(vt1.pen, vt2.pen);
        assert_eq!(vt1.charsets, vt2.charsets);
        assert_eq!(vt1.active_charset, vt2.active_charset);
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
        assert_eq!(primary_buffer_text(vt1), primary_buffer_text(vt2));
        assert_eq!(wrapped(vt1), wrapped(vt2));

        if vt1.active_buffer_type == BufferType::Alternate {
            assert_eq!(alternate_buffer_text(vt1), alternate_buffer_text(vt2));
        }
    }

    fn text(vt: &Vt) -> String {
        buffer_text(&vt.buffer, vt.cursor_x, vt.cursor_y)
    }

    fn primary_buffer_text(vt: &Vt) -> String {
        buffer_text(vt.primary_buffer(), vt.cursor_x, vt.cursor_y)
    }

    fn alternate_buffer_text(vt: &Vt) -> String {
        buffer_text(vt.alternate_buffer(), vt.cursor_x, vt.cursor_y)
    }

    fn buffer_text(buffer: &Buffer, cursor_x: usize, cursor_y: usize) -> String {
        let view = buffer.view();
        let mut lines = Vec::new();
        lines.extend(view[0..cursor_y].iter().map(|l| l.text()));
        let cursor_line = &view[cursor_y];
        let left = cursor_line.chars().take(cursor_x);
        let right = cursor_line.chars().skip(cursor_x);
        let mut line = String::from_iter(left);
        line.push('|');
        line.extend(right);
        lines.push(line);
        lines.extend(view[cursor_y + 1..].iter().map(|l| l.text()));

        lines
            .into_iter()
            .map(|line| line.trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn wrapped(vt: &Vt) -> Vec<bool> {
        vt.lines().iter().map(|l| l.wrapped).collect()
    }
}
