// The parser is based on Paul Williams' parser for ANSI-compatible video
// terminals: https://www.vt100.net/emu/dec_ansi_parser

#[derive(Debug)]
enum State {
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

#[derive(Debug, Copy, Clone)]
enum Color {
    Indexed(u8),
    RGB(u8, u8, u8)
}

#[derive(Debug, Copy, Clone)]
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
enum Charset {
    G0,
    G1
}

#[derive(Debug)]
pub struct VT {
    // parser
    state: State,

    // interpreter
    params: Vec<Vec<char>>,
    intermediates: Vec<char>,

    // screen
    columns: usize,
    rows: usize,
    buffer: Vec<Vec<Cell>>,
    alt_buffer: Vec<Vec<Cell>>,
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

    saved_cursor_x: usize,
    saved_cursor_y: usize,
    saved_pen: Pen,
    saved_origin_mode: bool,
    saved_auto_wrap_mode: bool,
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

impl VT {
    pub fn new(columns: usize, rows: usize) -> Self {
        let buffer = VT::new_buffer(columns, rows);
        let alt_buffer = buffer.clone();

        VT {
            state: State::Ground,
            params: Vec::new(),
            intermediates: Vec::new(),
            columns: columns,
            rows: rows,
            buffer: buffer,
            alt_buffer: alt_buffer,
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
            saved_cursor_x: 0,
            saved_cursor_y: 0,
            saved_pen: Pen::new(),
            saved_origin_mode: false,
            saved_auto_wrap_mode: true,
        }
    }

    fn new_buffer(columns: usize, rows: usize) -> Vec<Vec<Cell>> {
        vec![vec![Cell::blank(); columns]; rows]
    }

    fn blank_line(&self) -> Vec<Cell> {
        vec![Cell(' ', self.pen); self.columns]
    }

    fn default_tabs(columns: usize) -> Vec<usize> {
        let mut tabs = vec![];

        for t in (8..columns).step_by(8) {
            tabs.push(t);
        }

        tabs
    }

    pub fn get_cursor_x(&self) -> usize {
        self.cursor_x
    }

    // parser

    pub fn feed_str(&mut self, s: &str) {
        for c in s.chars() {
            self.feed(c);
        }
    }

    pub fn feed(&mut self, input: char) {
        let input2 = if input >= '\u{a0}' { '\u{41}' } else { input };

        match (&self.state, input2) {
            // Anywhere
            (_, '\u{18}')
            | (_, '\u{1a}')
            | (_, '\u{80}'...'\u{8f}')
            | (_, '\u{91}'...'\u{97}')
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
            (State::Ground, '\u{00}'...'\u{17}')
            | (State::Ground, '\u{19}')
            | (State::Ground, '\u{1c}'...'\u{1f}') => {
                self.execute(input);
            }

            (State::Ground, '\u{20}'...'\u{7f}') => {
                // (State::Ground, '\u{a0}'...'\u{ff}') => {
                self.print(input);
            }

            // Escape

            // C0 prime
            (State::Escape, '\u{00}'...'\u{17}')
            | (State::Escape, '\u{19}')
            | (State::Escape, '\u{1c}'...'\u{1f}') => {
                self.execute(input);
            }

            (State::Escape, '\u{20}'...'\u{2f}') => {
                self.state = State::EscapeIntermediate;
                self.collect(input);
            }

            (State::Escape, '\u{30}'...'\u{4f}')
            | (State::Escape, '\u{51}'...'\u{57}')
            | (State::Escape, '\u{59}')
            | (State::Escape, '\u{5a}')
            | (State::Escape, '\u{5c}')
            | (State::Escape, '\u{60}'...'\u{7e}') => {
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
            (State::EscapeIntermediate, '\u{00}'...'\u{17}')
            | (State::EscapeIntermediate, '\u{19}')
            | (State::EscapeIntermediate, '\u{1c}'...'\u{1f}') => {
                self.execute(input);
            }

            (State::EscapeIntermediate, '\u{20}'...'\u{2f}') => {
                self.collect(input);
            }

            (State::EscapeIntermediate, '\u{30}'...'\u{7e}') => {
                self.state = State::Ground;
                self.esc_dispatch(input);
            }

            // CsiEntry

            // C0 prime
            (State::CsiEntry, '\u{00}'...'\u{17}')
            | (State::CsiEntry, '\u{19}')
            | (State::CsiEntry, '\u{1c}'...'\u{1f}') => {
                self.execute(input);
            }

            (State::CsiEntry, '\u{20}'...'\u{2f}') => {
                self.state = State::CsiIntermediate;
                self.collect(input);
            }

            (State::CsiEntry, '\u{30}'...'\u{39}') | (State::CsiEntry, '\u{3b}') => {
                self.state = State::CsiParam;
                self.param(input);
            }

            (State::CsiEntry, '\u{3a}') => {
                self.state = State::CsiIgnore;
            }

            (State::CsiEntry, '\u{3c}'...'\u{3f}') => {
                self.state = State::CsiParam;
                self.collect(input);
            }

            (State::CsiEntry, '\u{40}'...'\u{7e}') => {
                self.state = State::Ground;
                self.csi_dispatch(input);
            }

            // CsiParam

            // C0 prime
            (State::CsiParam, '\u{00}'...'\u{17}')
            | (State::CsiParam, '\u{19}')
            | (State::CsiParam, '\u{1c}'...'\u{1f}') => {
                self.execute(input);
            }

            (State::CsiParam, '\u{20}'...'\u{2f}') => {
                self.state = State::CsiIntermediate;
                self.collect(input);
            }

            (State::CsiParam, '\u{30}'...'\u{39}') | (State::CsiParam, '\u{3b}') => {
                self.param(input);
            }

            (State::CsiParam, '\u{3a}') | (State::CsiParam, '\u{3c}'...'\u{3f}') => {
                self.state = State::CsiIgnore;
            }

            (State::CsiParam, '\u{40}'...'\u{7e}') => {
                self.state = State::Ground;
                self.csi_dispatch(input);
            }

            // CsiIntermediate

            // C0 prime
            (State::CsiIntermediate, '\u{00}'...'\u{17}')
            | (State::CsiIntermediate, '\u{19}')
            | (State::CsiIntermediate, '\u{1c}'...'\u{1f}') => {
                self.execute(input);
            }

            (State::CsiIntermediate, '\u{20}'...'\u{2f}') => {
                self.collect(input);
            }

            (State::CsiIntermediate, '\u{30}'...'\u{3f}') => {
                self.state = State::CsiIgnore;
            }

            (State::CsiIntermediate, '\u{40}'...'\u{7e}') => {
                self.state = State::Ground;
                self.csi_dispatch(input);
            }

            // CsiIgnore

            // C0 prime
            (State::CsiIgnore, '\u{00}'...'\u{17}')
            | (State::CsiIgnore, '\u{19}')
            | (State::CsiIgnore, '\u{1c}'...'\u{1f}') => {
                self.execute(input);
            }

            (State::CsiIgnore, '\u{40}'...'\u{7e}') => {
                self.state = State::Ground;
            }

            // DcsEntry
            (State::DcsEntry, '\u{20}'...'\u{2f}') => {
                self.state = State::DcsIntermediate;
                self.collect(input);
            }

            (State::DcsEntry, '\u{30}'...'\u{39}') | (State::DcsEntry, '\u{3b}') => {
                self.state = State::DcsParam;
                self.param(input);
            }

            (State::DcsEntry, '\u{3c}'...'\u{3f}') => {
                self.state = State::DcsParam;
                self.collect(input);
            }

            (State::DcsEntry, '\u{3a}') => {
                self.state = State::DcsIgnore;
            }

            (State::DcsEntry, '\u{40}'...'\u{7e}') => {
                self.state = State::DcsPassthrough;
            }

            // DcsParam
            (State::DcsParam, '\u{20}'...'\u{2f}') => {
                self.state = State::DcsIntermediate;
                self.collect(input);
            }

            (State::DcsParam, '\u{30}'...'\u{39}') | (State::DcsParam, '\u{3b}') => {
                self.param(input);
            }

            (State::DcsParam, '\u{3a}') | (State::DcsParam, '\u{3c}'...'\u{3f}') => {
                self.state = State::DcsIgnore;
            }

            (State::DcsParam, '\u{40}'...'\u{7e}') => {
                self.state = State::DcsPassthrough;
            }

            // DcsIntermediate
            (State::DcsIntermediate, '\u{20}'...'\u{2f}') => {
                self.collect(input);
            }

            (State::DcsIntermediate, '\u{30}'...'\u{3f}') => {
                self.state = State::DcsIgnore;
            }

            (State::DcsIntermediate, '\u{40}'...'\u{7e}') => {
                self.state = State::DcsPassthrough;
            }

            // DcsPassthrough

            // C0 prime
            (State::DcsPassthrough, '\u{00}'...'\u{17}')
            | (State::DcsPassthrough, '\u{19}')
            | (State::DcsPassthrough, '\u{1c}'...'\u{1f}') => {
                self.put(input);
            }

            (State::DcsPassthrough, '\u{20}'...'\u{7e}') => {
                self.put(input);
            }

            // OscString
            (State::OscString, '\u{07}') => {
                // 0x07 is xterm non-ANSI variant of transition to ground
                self.state = State::Ground;
            }

            (State::OscString, '\u{20}'...'\u{7f}') => {
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

        if next_column == self.columns {
            self.set_cell(self.cursor_x, self.cursor_y, cell);

            if self.auto_wrap_mode {
                self.do_move_cursor_to_col(next_column);
                self.next_print_wraps = true;
            }
        } else {
            if self.insert_mode {
                &mut self.buffer[self.cursor_y][self.cursor_x..].rotate_right(1);
            }

            self.set_cell(self.cursor_x, self.cursor_y, cell);
            self.do_move_cursor_to_col(next_column);
        }
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
        if self.params.is_empty() {
            self.params.push(Vec::new());
        }

        if input == ';' {
            self.params.push(Vec::new());
        } else {
            let n = self.params.len() - 1;
            self.params[n].push(input);
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
        self.intermediates.clear();
    }

    fn execute_sc(&mut self) {
        self.saved_cursor_x = self.cursor_x.min(self.columns - 1);
        self.saved_cursor_y = self.cursor_y;
        self.saved_pen = self.pen;
        self.saved_origin_mode = self.origin_mode;
        self.saved_auto_wrap_mode = self.auto_wrap_mode;
    }

    fn execute_rc(&mut self) {
        self.cursor_x = self.saved_cursor_x;
        self.cursor_y = self.saved_cursor_y;
        self.pen = self.saved_pen;
        self.origin_mode = self.saved_origin_mode;
        self.auto_wrap_mode = self.saved_auto_wrap_mode;
        self.next_print_wraps = false;
    }

    fn execute_ris(&mut self) {
        let buffer = VT::new_buffer(self.columns, self.rows);
        let alt_buffer = buffer.clone();

        self.state = State::Ground;
        self.params = Vec::new();
        self.intermediates = Vec::new();
        self.buffer = buffer;
        self.alt_buffer = alt_buffer;
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
        self.saved_cursor_x = 0;
        self.saved_cursor_y = 0;
        self.saved_pen = Pen::new();
        self.saved_origin_mode = false;
        self.saved_auto_wrap_mode = true;
    }

    fn execute_decaln(&mut self) {
        for y in 0..self.rows {
            for x in 0..self.columns {
                self.set_cell(x, y, Cell('\u{45}', Pen::new()));
            }
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

        let cells = &mut self.buffer[self.cursor_y][self.cursor_x..];
        cells.rotate_right(n);

        for cell in &mut cells[0..n] {
            *cell = Cell(' ', self.pen);
        }
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

    fn execute_cnl(&mut self) {}
    fn execute_cpl(&mut self) {}
    fn execute_cha(&mut self) {}
    fn execute_cup(&mut self) {}

    fn execute_cht(&mut self) {
        self.move_cursor_to_next_tab(self.get_param(0, 1) as usize);
    }

    fn execute_ed(&mut self) {}
    fn execute_el(&mut self) {}
    fn execute_il(&mut self) {}
    fn execute_dl(&mut self) {}
    fn execute_dch(&mut self) {}
    fn execute_su(&mut self) {}
    fn execute_sd(&mut self) {}

    fn execute_ctc(&mut self) {
        match self.get_param(0, 0) {
            0 => self.set_tab(),
            2 => self.clear_tab(),
            5 => self.clear_all_tabs(),
            _ => ()
        }
    }

    fn execute_ech(&mut self) {}
    fn execute_cbt(&mut self) {}
    fn execute_vpa(&mut self) {}
    fn execute_tbc(&mut self) {}
    fn execute_sm(&mut self) {}
    fn execute_rm(&mut self) {}
    fn execute_sgr(&mut self) {}
    fn execute_decstr(&mut self) {}
    fn execute_decstbm(&mut self) {}

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

    fn get_param(&self, n: usize, default: u16) -> u16 {
        let param =
            self.params
            .iter()
            .nth(n)
            .map_or(0, |chars| {
                let mut number = 0;
                let mut mult = 1;

                for c in chars.iter().rev() {
                    let digit = (*c as u16) - 0x30;
                    number += digit * mult;
                    mult *= 10;
                }

                number
            });

        if param == 0 { default } else { param }
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

    fn move_cursor_to_next_tab(&mut self, n: usize) {
        let last_col = self.columns - 1;

        let next_tab =
            self.tabs
            .iter()
            .skip_while(|&&t| self.cursor_x >= t)
            .nth(n - 1)
            .unwrap_or(&last_col);

        self.move_cursor_to_col(*next_tab);
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
        let new_y = if self.cursor_y < self.top_margin {
            0.max(self.cursor_y - n)
        } else {
            self.top_margin.max(self.cursor_y - n)
        };

        self.do_move_cursor_to_row(new_y);
    }

    fn scroll_up(&mut self, n: usize) {
        let filler = self.blank_line();
        VT::scroll_up_lines(&mut self.buffer[self.top_margin..=self.bottom_margin], n, &filler);
    }

    fn scroll_up_lines(lines: &mut [Vec<Cell>], mut n: usize, filler: &Vec<Cell>) {
        n = n.min(lines.len());
        lines.rotate_left(n);
        let y = lines.len() - n;

        for line in &mut lines[y..] {
            *line = filler.clone();
        }
    }

    fn scroll_down(&mut self, n: usize) {
        let filler = self.blank_line();
        VT::scroll_down_lines(&mut self.buffer[self.top_margin..=self.bottom_margin], n, &filler);
    }

    fn scroll_down_lines(lines: &mut [Vec<Cell>], mut n: usize, filler: &Vec<Cell>) {
        n = n.min(lines.len());
        lines.rotate_right(n);

        for line in &mut lines[0..n] {
            *line = filler.clone();
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
    use quickcheck::{TestResult, quickcheck};

    #[quickcheck]
    fn cursor_position(bytes: Vec<u8>) -> bool {
        let mut vt = VT::new(10, 4);

        for b in bytes.iter() {
            vt.feed((*b) as char);
        }

        vt.cursor_x <= 10 && vt.cursor_y < 4
    }

    #[quickcheck]
    fn buffer_size(bytes: Vec<u8>) -> bool {
        let mut vt = VT::new(10, 4);

        for b in bytes.iter() {
            vt.feed((*b) as char);
        }

        vt.buffer.len() == 4 && vt.buffer.iter().all(|line| line.len() == 10)
    }

    #[quickcheck]
    fn wrapping(y: u8, bytes: Vec<u8>) -> TestResult {
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