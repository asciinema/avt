mod cursor;
mod dirty_lines;

pub use self::cursor::Cursor;
use self::dirty_lines::DirtyLines;
use crate::buffer::{Buffer, EraseMode};
use crate::cell::{Cell, Occupancy};
use crate::charset::Charset;
use crate::line::Line;
use crate::parser::{
    AnsiMode, AnsiModes, CtcOp, DecMode, DecModes, EdScope, ElScope, Function, SgrOp, SgrOps,
    TbcScope, XtwinopsOp,
};
use crate::pen::{Intensity, Pen};
use crate::tabs::Tabs;
use std::cmp::Ordering;
use std::mem;

#[derive(Debug)]
pub struct Terminal {
    cols: usize,
    rows: usize,
    buffer: Buffer,
    other_buffer: Buffer,
    active_buffer_type: BufferType,
    scrollback_limit: Option<usize>,
    cursor: Cursor,
    pen: Pen,
    charsets: [Charset; 2],
    active_charset: usize,
    tabs: Tabs,
    insert_mode: bool,
    origin_mode: bool,
    auto_wrap_mode: bool,
    new_line_mode: bool,
    cursor_keys_mode: CursorKeysMode,
    top_margin: usize,
    bottom_margin: usize,
    saved_ctx: SavedCtx,
    alternate_saved_ctx: SavedCtx,
    dirty_lines: DirtyLines,
    xtwinops: bool,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum BufferType {
    Primary,
    Alternate,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub(crate) enum CursorKeysMode {
    Normal,
    Application,
}

#[derive(Debug, PartialEq)]
pub(crate) struct SavedCtx {
    pub cursor_col: usize,
    pub cursor_row: usize,
    pub pen: Pen,
    pub origin_mode: bool,
    pub auto_wrap_mode: bool,
}

impl Default for SavedCtx {
    fn default() -> Self {
        SavedCtx {
            cursor_col: 0,
            cursor_row: 0,
            pen: Pen::default(),
            origin_mode: false,
            auto_wrap_mode: true,
        }
    }
}

impl SavedCtx {
    fn is_default(&self) -> bool {
        self.cursor_col == 0
            && self.cursor_row == 0
            && self.pen.is_default()
            && !self.origin_mode
            && self.auto_wrap_mode
    }
}

impl Terminal {
    pub fn new((cols, rows): (usize, usize), scrollback_limit: Option<usize>) -> Self {
        let primary_buffer = Buffer::new(cols, rows, scrollback_limit, None);
        let alternate_buffer = Buffer::new(cols, rows, Some(0), None);
        let dirty_lines = DirtyLines::new(rows);

        Terminal {
            cols,
            rows,
            buffer: primary_buffer,
            other_buffer: alternate_buffer,
            active_buffer_type: BufferType::Primary,
            scrollback_limit,
            tabs: Tabs::new(cols),
            cursor: Cursor::default(),
            pen: Pen::default(),
            charsets: [Charset::Ascii, Charset::Ascii],
            active_charset: 0,
            insert_mode: false,
            origin_mode: false,
            auto_wrap_mode: true,
            new_line_mode: false,
            cursor_keys_mode: CursorKeysMode::Normal,
            top_margin: 0,
            bottom_margin: (rows - 1),
            saved_ctx: SavedCtx::default(),
            alternate_saved_ctx: SavedCtx::default(),
            dirty_lines,
            xtwinops: false,
        }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    pub fn active_buffer_type(&self) -> BufferType {
        self.active_buffer_type
    }

    pub fn execute(&mut self, fun: Function) {
        use Function::*;

        match fun {
            Bs => {
                self.bs();
            }

            Cbt(n) => {
                self.cbt(n);
            }

            Cha(n) => {
                self.cha(n);
            }

            Cht(n) => {
                self.cht(n);
            }

            Cnl(n) => {
                self.cnl(n);
            }

            Cpl(n) => {
                self.cpl(n);
            }

            Cr => {
                self.cr();
            }

            Ctc(mode) => {
                self.ctc(mode);
            }

            Cub(n) => {
                self.cub(n);
            }

            Cud(n) => {
                self.cud(n);
            }

            Cuf(n) => {
                self.cuf(n);
            }

            Cup(row, col) => {
                self.cup(row, col);
            }

            Cuu(n) => {
                self.cuu(n);
            }

            Dch(n) => {
                self.dch(n);
            }

            Decaln => {
                self.decaln();
            }

            Decrc => {
                self.rc();
            }

            Decrst(modes) => {
                self.decrst(modes);
            }

            Decsc => {
                self.sc();
            }

            Decset(modes) => {
                self.decset(modes);
            }

            Decstbm(top, bottom) => {
                self.decstbm(top, bottom);
            }

            Decstr => {
                self.decstr();
            }

            Dl(n) => {
                self.dl(n);
            }

            Ech(n) => {
                self.ech(n);
            }

            Ed(mode) => {
                self.ed(mode);
            }

            El(mode) => {
                self.el(mode);
            }

            G1d4(charset) => {
                self.g1d4(charset);
            }

            Gzd4(charset) => {
                self.gzd4(charset);
            }

            Ht => {
                self.ht();
            }

            Hts => {
                self.hts();
            }

            Ich(n) => {
                self.ich(n);
            }

            Il(n) => {
                self.il(n);
            }

            Lf => {
                self.lf();
            }

            Nel => {
                self.nel();
            }

            Print(ch) => {
                self.print(ch);
            }

            Rep(n) => {
                self.rep(n);
            }

            Ri => {
                self.ri();
            }

            Ris => {
                self.ris();
            }

            Rm(modes) => {
                self.rm(modes);
            }

            Scorc => {
                self.rc();
            }

            Scosc => {
                self.sc();
            }

            Sd(n) => {
                self.sd(n);
            }

            Sgr(params) => {
                self.sgr(params);
            }

            Si => {
                self.si();
            }

            Sm(modes) => {
                self.sm(modes);
            }

            So => {
                self.so();
            }

            Su(n) => {
                self.su(n);
            }

            Tbc(mode) => {
                self.tbc(mode);
            }

            Vpa(n) => {
                self.vpa(n);
            }

            Vpr(n) => {
                self.vpr(n);
            }

            Xtwinops(op) => {
                self.xtwinops(op);
            }
        }
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    pub fn gc(&mut self) -> Box<dyn Iterator<Item = Line> + '_> {
        let lines = self.buffer.gc();

        if self.active_buffer_type == BufferType::Alternate {
            return Box::new(std::iter::empty());
        }

        match lines {
            Some(iter) => Box::new(iter),
            None => Box::new(std::iter::empty()),
        }
    }

    pub fn changes(&mut self) -> Vec<usize> {
        let changes = self.dirty_lines.to_vec();
        self.dirty_lines.clear();

        changes
    }

    // cursor

    fn save_cursor(&mut self) {
        self.saved_ctx.cursor_col = self.cursor.col.min(self.cols - 1);
        self.saved_ctx.cursor_row = self.cursor.row;
        self.saved_ctx.pen = self.pen;
        self.saved_ctx.origin_mode = self.origin_mode;
        self.saved_ctx.auto_wrap_mode = self.auto_wrap_mode;
    }

    fn restore_cursor(&mut self) {
        self.cursor.col = self.saved_ctx.cursor_col;
        self.cursor.row = self.saved_ctx.cursor_row;
        self.pen = self.saved_ctx.pen;
        self.origin_mode = self.saved_ctx.origin_mode;
        self.auto_wrap_mode = self.saved_ctx.auto_wrap_mode;
    }

    fn move_cursor_to_col(&mut self, col: usize) {
        if col >= self.cols {
            self.cursor.col = self.cols - 1;
        } else {
            self.cursor.col = col;
        }
    }

    fn move_cursor_to_row(&mut self, mut row: usize) {
        let top = self.actual_top_margin();
        let bottom = self.actual_bottom_margin();
        row = (top + row).max(top).min(bottom);
        self.do_move_cursor_to_row(row);
    }

    fn do_move_cursor_to_row(&mut self, row: usize) {
        self.cursor.col = self.cursor.col.min(self.cols - 1);
        self.cursor.row = row;
    }

    fn move_cursor_to_rel_col(&mut self, rel_col: isize) {
        let new_col = self.cursor.col as isize + rel_col;

        if new_col < 0 {
            self.cursor.col = 0;
        } else if new_col as usize >= self.cols {
            self.cursor.col = self.cols - 1;
        } else {
            self.cursor.col = new_col as usize;
        }
    }

    fn move_cursor_home(&mut self) {
        self.cursor.col = 0;
        self.do_move_cursor_to_row(self.actual_top_margin());
    }

    fn move_cursor_to_next_tab(&mut self, n: usize) {
        let next_tab = self.tabs.after(self.cursor.col, n).unwrap_or(self.cols - 1);
        self.move_cursor_to_col(next_tab);
    }

    fn move_cursor_to_prev_tab(&mut self, n: usize) {
        let prev_tab = self.tabs.before(self.cursor.col, n).unwrap_or(0);
        self.move_cursor_to_col(prev_tab);
    }

    fn move_cursor_down_with_scroll(&mut self) {
        if self.cursor.row == self.bottom_margin {
            self.scroll_up_in_region(1);
        } else if self.cursor.row < self.rows - 1 {
            self.do_move_cursor_to_row(self.cursor.row + 1);
        }
    }

    fn cursor_down(&mut self, n: usize) {
        let new_y = if self.cursor.row > self.bottom_margin {
            (self.rows - 1).min(self.cursor.row + n)
        } else {
            self.bottom_margin.min(self.cursor.row + n)
        };

        self.do_move_cursor_to_row(new_y);
    }

    fn cursor_up(&mut self, n: usize) {
        let mut new_y = (self.cursor.row as isize) - (n as isize);

        new_y = if self.cursor.row < self.top_margin {
            new_y.max(0)
        } else {
            new_y.max(self.top_margin as isize)
        };

        self.do_move_cursor_to_row(new_y as usize);
    }

    // margins

    fn actual_top_margin(&self) -> usize {
        match self.origin_mode {
            false => 0,
            true => self.top_margin,
        }
    }

    fn actual_bottom_margin(&self) -> usize {
        match self.origin_mode {
            false => self.rows - 1,
            true => self.bottom_margin,
        }
    }

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

    // tabs

    fn set_tab(&mut self) {
        if 0 < self.cursor.col && self.cursor.col < self.cols {
            self.tabs.set(self.cursor.col);
        }
    }

    fn clear_tab(&mut self) {
        self.tabs.unset(self.cursor.col);
    }

    fn clear_all_tabs(&mut self) {
        self.tabs.clear();
    }

    // buffer switching

    fn switch_to_alternate_buffer(&mut self) {
        if let BufferType::Primary = self.active_buffer_type {
            self.active_buffer_type = BufferType::Alternate;
            mem::swap(&mut self.saved_ctx, &mut self.alternate_saved_ctx);
            mem::swap(&mut self.buffer, &mut self.other_buffer);
            self.buffer = Buffer::new(self.cols, self.rows, Some(0), Some(&self.pen));
            self.dirty_lines.extend(0..self.rows);
        }
    }

    fn switch_to_primary_buffer(&mut self) {
        if let BufferType::Alternate = self.active_buffer_type {
            self.active_buffer_type = BufferType::Primary;
            mem::swap(&mut self.saved_ctx, &mut self.alternate_saved_ctx);
            mem::swap(&mut self.buffer, &mut self.other_buffer);
            self.dirty_lines.extend(0..self.rows);
        }
    }

    // resizing

    pub fn resize(&mut self, cols: usize, rows: usize) -> bool {
        let mut resized: bool = false;

        match cols.cmp(&self.cols) {
            std::cmp::Ordering::Less => {
                self.tabs.contract(cols);
                resized = true;
            }

            std::cmp::Ordering::Equal => {}

            std::cmp::Ordering::Greater => {
                self.tabs.expand(self.cols, cols);
                resized = true;
            }
        }

        match rows.cmp(&self.rows) {
            std::cmp::Ordering::Less => {
                self.top_margin = 0;
                self.bottom_margin = rows - 1;
                resized = true;
            }

            std::cmp::Ordering::Equal => {}

            std::cmp::Ordering::Greater => {
                self.top_margin = 0;
                self.bottom_margin = rows - 1;
                resized = true;
            }
        }

        self.cols = cols;
        self.rows = rows;
        self.reflow();

        resized
    }

    fn reflow(&mut self) {
        (self.cursor.col, self.cursor.row) =
            self.buffer
                .resize(self.cols, self.rows, (self.cursor.col, self.cursor.row));

        self.dirty_lines.resize(self.rows);
        self.dirty_lines.extend(0..self.rows);

        if self.saved_ctx.cursor_col >= self.cols {
            self.saved_ctx.cursor_col = self.cols - 1;
        }

        if self.saved_ctx.cursor_row >= self.rows {
            self.saved_ctx.cursor_row = self.rows - 1;
        }
    }

    // resetting

    fn soft_reset(&mut self) {
        self.cursor.visible = true;
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

        self.buffer = primary_buffer;
        self.other_buffer = alternate_buffer;
        self.active_buffer_type = BufferType::Primary;
        self.tabs = Tabs::new(self.cols);
        self.cursor = Cursor::default();
        self.pen = Pen::default();
        self.charsets = [Charset::Ascii, Charset::Ascii];
        self.active_charset = 0;
        self.insert_mode = false;
        self.origin_mode = false;
        self.auto_wrap_mode = true;
        self.new_line_mode = false;
        self.cursor_keys_mode = CursorKeysMode::Normal;
        self.top_margin = 0;
        self.bottom_margin = self.rows - 1;
        self.saved_ctx = SavedCtx::default();
        self.alternate_saved_ctx = SavedCtx::default();
        self.dirty_lines = DirtyLines::new(self.rows);
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

    pub fn view(&self) -> impl Iterator<Item = &Line> {
        self.buffer.view()
    }

    pub fn lines(&self) -> impl Iterator<Item = &Line> {
        self.buffer.lines()
    }

    pub fn line(&self, n: usize) -> &Line {
        &self.buffer[n]
    }

    pub fn text(&self) -> Vec<String> {
        self.primary_buffer().text()
    }

    pub fn cursor_keys_app_mode(&self) -> bool {
        self.cursor_keys_mode == CursorKeysMode::Application
    }

    #[cfg(test)]
    pub fn verify(&self) {
        assert!(self.cursor.row < self.rows);
        assert!(self.cursor.col <= self.cols);
        assert!(self.lines().all(|line| line.len() == self.cols));
        assert!(!self.lines().last().unwrap().wrapped);

        for line in self.lines() {
            for i in 0..self.cols {
                let occupancy = line[i].occupancy();

                if occupancy == Occupancy::WideTail {
                    assert!(i > 0);
                    assert!(line[i - 1].occupancy() == Occupancy::WideHead);
                } else if occupancy == Occupancy::WideHead {
                    assert!(line[i + 1].occupancy() == Occupancy::WideTail, "{:?}", line);
                }
            }
        }
    }

    #[cfg(test)]
    pub fn assert_eq(&self, other: &Terminal) {
        assert_eq!(self.active_buffer_type, other.active_buffer_type);
        assert_eq!(self.cursor, other.cursor);
        assert_eq!(self.pen, other.pen);
        assert_eq!(self.charsets, other.charsets);
        assert_eq!(self.active_charset, other.active_charset);
        assert_eq!(self.tabs, other.tabs);
        assert_eq!(self.insert_mode, other.insert_mode);
        assert_eq!(self.origin_mode, other.origin_mode);
        assert_eq!(self.auto_wrap_mode, other.auto_wrap_mode);
        assert_eq!(self.new_line_mode, other.new_line_mode);
        assert_eq!(self.cursor_keys_mode, other.cursor_keys_mode);
        assert_eq!(self.top_margin, other.top_margin);
        assert_eq!(self.bottom_margin, other.bottom_margin);
        assert_eq!(self.saved_ctx, other.saved_ctx);
        assert_eq!(self.alternate_saved_ctx, other.alternate_saved_ctx);

        assert_eq!(
            self.primary_buffer().view().collect::<Vec<_>>(),
            other.primary_buffer().view().collect::<Vec<_>>()
        );

        if self.active_buffer_type == BufferType::Alternate {
            assert_eq!(
                self.alternate_buffer().view().collect::<Vec<_>>(),
                other.alternate_buffer().view().collect::<Vec<_>>()
            );
        }
    }

    fn print(&mut self, mut ch: char) {
        ch = self.charsets[self.active_charset].translate(ch);

        let n = if self.cursor.col < self.cols {
            if self.insert_mode {
                self.buffer
                    .shift_right((self.cursor.col, self.cursor.row), 1, self.pen);
            }

            self.buffer
                .print((self.cursor.col, self.cursor.row), ch, self.pen)
        } else {
            None
        };

        if let Some(n) = n {
            self.cursor.col += n;

            if self.cursor.col == self.cols && !self.auto_wrap_mode {
                self.cursor.col = self.cols - 1;
            }
        } else if self.auto_wrap_mode {
            if self.cursor.row == self.bottom_margin {
                self.buffer.wrap(self.cursor.row);
                self.scroll_up_in_region(1);
            } else if self.cursor.row < self.rows - 1 {
                self.buffer.wrap(self.cursor.row);
                self.cursor.row += 1;
            }

            self.cursor.col = self
                .buffer
                .print((0, self.cursor.row), ch, self.pen)
                .unwrap();
        } else {
            let n = self
                .buffer
                .print((self.cursor.col - 1, self.cursor.row), ch, self.pen);

            if n.is_none() {
                self.buffer
                    .print((self.cursor.col - 2, self.cursor.row), ch, self.pen);
            }

            self.cursor.col = self.cols - 1;
        }

        self.dirty_lines.add(self.cursor.row);
    }

    fn bs(&mut self) {
        if self.cursor.col == self.cols {
            self.move_cursor_to_rel_col(-2);
        } else {
            self.move_cursor_to_rel_col(-1);
        }
    }

    fn ht(&mut self) {
        self.move_cursor_to_next_tab(1);
    }

    fn lf(&mut self) {
        self.move_cursor_down_with_scroll();

        if self.new_line_mode {
            self.cursor.col = 0;
        }
    }

    fn cr(&mut self) {
        self.cursor.col = 0;
    }

    fn so(&mut self) {
        self.active_charset = 1;
    }

    fn si(&mut self) {
        self.active_charset = 0;
    }

    fn nel(&mut self) {
        self.move_cursor_down_with_scroll();
        self.cursor.col = 0;
    }

    fn hts(&mut self) {
        self.set_tab();
    }

    fn ri(&mut self) {
        if self.cursor.row == self.top_margin {
            self.scroll_down_in_region(1);
        } else if self.cursor.row > 0 {
            self.move_cursor_to_row(self.cursor.row - 1);
        }
    }

    fn sc(&mut self) {
        self.save_cursor();
    }

    fn rc(&mut self) {
        self.restore_cursor();
    }

    fn ris(&mut self) {
        self.hard_reset();
    }

    fn decaln(&mut self) {
        for row in 0..self.rows {
            for col in 0..self.cols {
                self.buffer.print((col, row), '\u{45}', Pen::default());
            }

            self.dirty_lines.add(row);
        }
    }

    fn gzd4(&mut self, charset: Charset) {
        self.charsets[0] = charset;
    }

    fn g1d4(&mut self, charset: Charset) {
        self.charsets[1] = charset;
    }

    fn ich(&mut self, n: u16) {
        if self.cursor.col == self.cols {
            self.cursor.col = self.cols - 1;
        }

        let n = as_usize(n, 1).min(self.cols - self.cursor.col);

        self.buffer
            .shift_right((self.cursor.col, self.cursor.row), n, self.pen);

        for col in self.cursor.col..self.cursor.col + n {
            self.buffer.print((col, self.cursor.row), ' ', self.pen);
        }

        self.dirty_lines.add(self.cursor.row);
    }

    fn cuu(&mut self, n: u16) {
        self.cursor_up(as_usize(n, 1));
    }

    fn cud(&mut self, n: u16) {
        self.cursor_down(as_usize(n, 1));
    }

    fn cuf(&mut self, n: u16) {
        self.move_cursor_to_rel_col(as_usize(n, 1) as isize);
    }

    fn cub(&mut self, n: u16) {
        let mut rel_col = -(as_usize(n, 1) as isize);

        if self.cursor.col == self.cols {
            rel_col -= 1;
        }

        self.move_cursor_to_rel_col(rel_col);
    }

    fn cnl(&mut self, n: u16) {
        self.cursor_down(as_usize(n, 1));
        self.cursor.col = 0;
    }

    fn cpl(&mut self, n: u16) {
        self.cursor_up(as_usize(n, 1));
        self.cursor.col = 0;
    }

    fn cha(&mut self, n: u16) {
        self.move_cursor_to_col(as_usize(n, 1) - 1);
    }

    fn cup(&mut self, row: u16, col: u16) {
        self.move_cursor_to_col(as_usize(col, 1) - 1);
        self.move_cursor_to_row(as_usize(row, 1) - 1);
    }

    fn cht(&mut self, n: u16) {
        self.move_cursor_to_next_tab(as_usize(n, 1));
    }

    fn ed(&mut self, scope: EdScope) {
        match scope {
            EdScope::Below => {
                self.buffer.erase(
                    (self.cursor.col, self.cursor.row),
                    EraseMode::FromCursorToEndOfView,
                    &self.pen,
                );

                self.dirty_lines.extend(self.cursor.row..self.rows);
            }

            EdScope::Above => {
                self.buffer.erase(
                    (self.cursor.col, self.cursor.row),
                    EraseMode::FromStartOfViewToCursor,
                    &self.pen,
                );

                self.dirty_lines.extend(0..self.cursor.row + 1);
            }

            EdScope::All => {
                self.buffer.erase(
                    (self.cursor.col, self.cursor.row),
                    EraseMode::WholeView,
                    &self.pen,
                );

                self.dirty_lines.extend(0..self.rows);
            }

            _ => {}
        }
    }

    fn el(&mut self, scope: ElScope) {
        match scope {
            ElScope::ToRight => {
                self.buffer.erase(
                    (self.cursor.col, self.cursor.row),
                    EraseMode::FromCursorToEndOfLine,
                    &self.pen,
                );

                self.dirty_lines.add(self.cursor.row);
            }

            ElScope::ToLeft => {
                self.buffer.erase(
                    (self.cursor.col, self.cursor.row),
                    EraseMode::FromStartOfLineToCursor,
                    &self.pen,
                );

                self.dirty_lines.add(self.cursor.row);
            }

            ElScope::All => {
                self.buffer.erase(
                    (self.cursor.col, self.cursor.row),
                    EraseMode::WholeLine,
                    &self.pen,
                );

                self.dirty_lines.add(self.cursor.row);
            }
        }
    }

    fn il(&mut self, n: u16) {
        let range = if self.cursor.row <= self.bottom_margin {
            self.cursor.row..self.bottom_margin + 1
        } else {
            self.cursor.row..self.rows
        };

        self.buffer
            .scroll_down(range.clone(), as_usize(n, 1), &self.pen);

        self.dirty_lines.extend(range);
    }

    fn dl(&mut self, n: u16) {
        let range = if self.cursor.row <= self.bottom_margin {
            self.cursor.row..self.bottom_margin + 1
        } else {
            self.cursor.row..self.rows
        };

        self.buffer
            .scroll_up(range.clone(), as_usize(n, 1), &self.pen);

        self.dirty_lines.extend(range);
    }

    fn dch(&mut self, n: u16) {
        if self.cursor.col >= self.cols {
            self.move_cursor_to_col(self.cols - 1);
        }

        self.buffer.delete(
            (self.cursor.col, self.cursor.row),
            as_usize(n, 1),
            &self.pen,
        );

        self.dirty_lines.add(self.cursor.row);
    }

    fn su(&mut self, n: u16) {
        self.scroll_up_in_region(as_usize(n, 1));
    }

    fn sd(&mut self, n: u16) {
        self.scroll_down_in_region(as_usize(n, 1));
    }

    fn ctc(&mut self, op: CtcOp) {
        match op {
            CtcOp::Set => {
                self.set_tab();
            }

            CtcOp::ClearCurrentColumn => {
                self.clear_tab();
            }

            CtcOp::ClearAll => {
                self.clear_all_tabs();
            }
        }
    }

    fn ech(&mut self, n: u16) {
        let n = as_usize(n, 1);

        self.buffer.erase(
            (self.cursor.col, self.cursor.row),
            EraseMode::NextChars(n),
            &self.pen,
        );

        self.dirty_lines.add(self.cursor.row);
    }

    fn cbt(&mut self, n: u16) {
        self.move_cursor_to_prev_tab(as_usize(n, 1));
    }

    fn rep(&mut self, n: u16) {
        if self.cursor.col > 0 {
            let n = as_usize(n, 1);
            let row = self.cursor.row;
            let mut col = self.cursor.col - 1;

            while col > 0 && self.buffer[(col, row)].occupancy() == Occupancy::WideTail {
                col -= 1;
            }

            let char = self.buffer[(col, row)].char();

            for _n in 0..n {
                self.print(char);
            }
        }
    }

    fn vpa(&mut self, n: u16) {
        self.move_cursor_to_row(as_usize(n, 1) - 1);
    }

    fn vpr(&mut self, n: u16) {
        self.cursor_down(as_usize(n, 1));
    }

    fn tbc(&mut self, scope: TbcScope) {
        match scope {
            TbcScope::CurrentColumn => {
                self.clear_tab();
            }

            TbcScope::All => {
                self.clear_all_tabs();
            }
        }
    }

    fn sm(&mut self, modes: AnsiModes) {
        use AnsiMode::*;

        for &mode in modes.as_slice() {
            match mode {
                Insert => {
                    self.insert_mode = true;
                }

                NewLine => {
                    self.new_line_mode = true;
                }
            }
        }
    }

    fn rm(&mut self, modes: AnsiModes) {
        use AnsiMode::*;

        for &mode in modes.as_slice() {
            match mode {
                Insert => {
                    self.insert_mode = false;
                }

                NewLine => {
                    self.new_line_mode = false;
                }
            }
        }
    }

    fn sgr(&mut self, ops: SgrOps) {
        use SgrOp::*;

        for &op in ops.as_slice() {
            match op {
                Reset => {
                    self.pen = Pen::default();
                }

                SetBoldIntensity => {
                    self.pen.intensity = Intensity::Bold;
                }

                SetFaintIntensity => {
                    self.pen.intensity = Intensity::Faint;
                }

                SetItalic => {
                    self.pen.set_italic();
                }

                SetUnderline => {
                    self.pen.set_underline();
                }

                SetBlink => {
                    self.pen.set_blink();
                }

                SetInverse => {
                    self.pen.set_inverse();
                }

                SetStrikethrough => {
                    self.pen.set_strikethrough();
                }

                ResetIntensity => {
                    self.pen.intensity = Intensity::Normal;
                }

                ResetItalic => {
                    self.pen.unset_italic();
                }

                ResetUnderline => {
                    self.pen.unset_underline();
                }

                ResetBlink => {
                    self.pen.unset_blink();
                }

                ResetInverse => {
                    self.pen.unset_inverse();
                }

                ResetStrikethrough => {
                    self.pen.unset_strikethrough();
                }

                SetForegroundColor(color) => {
                    self.pen.foreground = Some(color);
                }

                ResetForegroundColor => {
                    self.pen.foreground = None;
                }

                SetBackgroundColor(color) => {
                    self.pen.background = Some(color);
                }

                ResetBackgroundColor => {
                    self.pen.background = None;
                }
            }
        }
    }

    fn decstbm(&mut self, top: u16, bottom: u16) {
        let top = as_usize(top, 1) - 1;
        let bottom = as_usize(bottom, self.rows) - 1;

        if top < bottom && bottom < self.rows {
            self.top_margin = top;
            self.bottom_margin = bottom;
        }

        self.move_cursor_home();
    }

    fn xtwinops(&mut self, op: XtwinopsOp) {
        if self.xtwinops {
            let XtwinopsOp::Resize(cols, rows) = op;
            let cols = as_usize(cols, self.cols);
            let rows = as_usize(rows, self.rows);

            self.resize(cols, rows);
        }
    }

    fn decstr(&mut self) {
        self.soft_reset();
    }

    fn decset(&mut self, modes: DecModes) {
        use DecMode::*;

        for &mode in modes.as_slice() {
            match mode {
                CursorKeys => {
                    self.cursor_keys_mode = CursorKeysMode::Application;
                }

                Origin => {
                    self.origin_mode = true;
                    self.move_cursor_home();
                }

                AutoWrap => {
                    self.auto_wrap_mode = true;
                }

                TextCursorEnable => {
                    self.cursor.visible = true;
                }

                AltScreenBuffer => {
                    self.switch_to_alternate_buffer();
                    self.reflow();
                }

                SaveCursor => {
                    self.save_cursor();
                }

                SaveCursorAltScreenBuffer => {
                    self.save_cursor();
                    self.switch_to_alternate_buffer();
                    self.reflow();
                }
            }
        }
    }

    fn decrst(&mut self, modes: DecModes) {
        use DecMode::*;

        for &mode in modes.as_slice() {
            match mode {
                CursorKeys => {
                    self.cursor_keys_mode = CursorKeysMode::Normal;
                }

                Origin => {
                    self.origin_mode = false;
                    self.move_cursor_home();
                }

                AutoWrap => {
                    self.auto_wrap_mode = false;
                }

                TextCursorEnable => {
                    self.cursor.visible = false;
                }

                AltScreenBuffer => {
                    self.switch_to_primary_buffer();
                    self.reflow();
                }

                SaveCursor => {
                    self.restore_cursor();
                }

                SaveCursorAltScreenBuffer => {
                    self.switch_to_primary_buffer();
                    self.restore_cursor();
                    self.reflow();
                }
            }
        }
    }

    pub fn dump(&self) -> Vec<Function> {
        let (primary_ctx, alternate_ctx): (&SavedCtx, &SavedCtx) = match self.active_buffer_type {
            BufferType::Primary => (&self.saved_ctx, &self.alternate_saved_ctx),
            BufferType::Alternate => (&self.alternate_saved_ctx, &self.saved_ctx),
        };

        // 1. dump primary screen buffer

        let mut funs = Vec::new();
        dump_buffer(self.primary_buffer(), &mut funs);

        // 2. setup tab stops

        if self.tabs != Tabs::new(self.cols) {
            // clear all tab stops
            funs.push(Function::Ctc(CtcOp::ClearAll));

            // set each tab stop
            for t in &self.tabs {
                funs.push(Function::Cha(as_u16(t + 1)));
                funs.push(Function::Ctc(CtcOp::Set));
            }
        }

        // 3. configure saved context for primary screen

        if !primary_ctx.is_default() {
            if !primary_ctx.auto_wrap_mode {
                // disable auto-wrap mode
                funs.push(Function::Decrst(DecModes::one(DecMode::AutoWrap)));
            }

            if primary_ctx.origin_mode {
                // enable origin mode
                funs.push(Function::Decset(DecModes::one(DecMode::Origin)));
            }

            // fix cursor in target position
            funs.push(Function::Cup(
                as_u16(primary_ctx.cursor_row + 1),
                as_u16(primary_ctx.cursor_col + 1),
            ));

            // configure pen
            funs.push(to_sgr(&primary_ctx.pen));

            // save cursor
            funs.push(Function::Decsc);

            if !primary_ctx.auto_wrap_mode {
                // re-enable auto-wrap mode
                funs.push(Function::Decset(DecModes::one(DecMode::AutoWrap)));
            }

            if primary_ctx.origin_mode {
                // re-disable origin mode
                funs.push(Function::Decrst(DecModes::one(DecMode::Origin)));
            }
        }

        // prevent pen bleed into alt screen buffer
        funs.push(to_sgr(&Pen::default()));

        // 4. dump alternate screen buffer

        // switch to alternate screen
        if self.active_buffer_type == BufferType::Alternate || !alternate_ctx.is_default() {
            funs.push(Function::Decset(DecModes::one(DecMode::AltScreenBuffer)));
        }

        if self.active_buffer_type == BufferType::Alternate {
            // move cursor home
            funs.push(Function::Cup(1, 1));

            // dump alternate buffer
            dump_buffer(self.alternate_buffer(), &mut funs);
        }

        // 5. configure saved context for alternate screen

        if !alternate_ctx.is_default() {
            if !alternate_ctx.auto_wrap_mode {
                // disable auto-wrap mode
                funs.push(Function::Decrst(DecModes::one(DecMode::AutoWrap)));
            }

            if alternate_ctx.origin_mode {
                // enable origin mode
                funs.push(Function::Decset(DecModes::one(DecMode::Origin)));
            }

            // fix cursor in target position
            funs.push(Function::Cup(
                as_u16(alternate_ctx.cursor_row + 1),
                as_u16(alternate_ctx.cursor_col + 1),
            ));

            // configure pen
            funs.push(to_sgr(&alternate_ctx.pen));

            // save cursor
            funs.push(Function::Decsc);

            if !alternate_ctx.auto_wrap_mode {
                // re-enable auto-wrap mode
                funs.push(Function::Decset(DecModes::one(DecMode::AutoWrap)));
            }

            if alternate_ctx.origin_mode {
                // re-disable origin mode
                funs.push(Function::Decrst(DecModes::one(DecMode::Origin)));
            }
        }

        // 6. ensure the right buffer is active

        if self.active_buffer_type == BufferType::Primary && !alternate_ctx.is_default() {
            // switch back to primary screen
            funs.push(Function::Decrst(DecModes::one(DecMode::AltScreenBuffer)));
        }

        // 7. setup origin mode

        if self.origin_mode {
            // enable origin mode
            // note: this resets cursor position - must be done before fixing cursor
            funs.push(Function::Decset(DecModes::one(DecMode::Origin)));
        }

        // 8. setup margins

        // note: this resets cursor position - must be done before fixing cursor
        if self.top_margin > 0 || self.bottom_margin < self.rows - 1 {
            funs.push(Function::Decstbm(
                as_u16(self.top_margin + 1),
                as_u16(self.bottom_margin + 1),
            ));
        }

        // 9. setup cursor

        let col = self.cursor.col;
        let mut row = self.cursor.row;

        if self.origin_mode {
            if row < self.top_margin || row > self.bottom_margin {
                // bring cursor outside scroll region by restoring saved cursor
                // and moving it to desired position via CSI A/B/C/D

                funs.push(Function::Scorc);

                match col.cmp(&self.saved_ctx.cursor_col) {
                    Ordering::Less => {
                        let n = self.saved_ctx.cursor_col - col;
                        funs.push(Function::Cub(as_u16(n)));
                    }

                    Ordering::Greater => {
                        let n = col - self.saved_ctx.cursor_col;
                        funs.push(Function::Cuf(as_u16(n)));
                    }

                    Ordering::Equal => (),
                }

                match row.cmp(&self.saved_ctx.cursor_row) {
                    Ordering::Less => {
                        let n = self.saved_ctx.cursor_row - row;
                        funs.push(Function::Cuu(as_u16(n)));
                    }

                    Ordering::Greater => {
                        let n = row - self.saved_ctx.cursor_row;
                        funs.push(Function::Cud(as_u16(n)));
                    }

                    Ordering::Equal => (),
                }
            } else {
                row -= self.top_margin;
                funs.push(Function::Cup(as_u16(row + 1), as_u16(col + 1)));
            }
        } else {
            funs.push(Function::Cup(as_u16(row + 1), as_u16(col + 1)));
        }

        if self.cursor.col >= self.cols {
            // move cursor past the right border by re-printing the character in
            // the last column
            let last_cell = self.buffer[(self.cols - 1, self.cursor.row)];
            let occupancy = last_cell.occupancy();

            if occupancy == Occupancy::Single {
                funs.push(to_sgr(last_cell.pen()));
                funs.push(Function::Print(last_cell.char()));
            } else if occupancy == Occupancy::WideTail {
                let prev_cell = self.buffer[(self.cols - 2, self.cursor.row)];

                funs.push(Function::Cub(1));
                funs.push(to_sgr(prev_cell.pen()));
                funs.push(Function::Print(prev_cell.char()));
            }
        }

        // configure pen
        funs.push(to_sgr(&self.pen));

        if !self.cursor.visible {
            // hide cursor
            funs.push(Function::Decrst(DecModes::one(DecMode::TextCursorEnable)));
        }

        // Following 3 steps must happen after ALL prints as they alter print behaviour,
        // including the "move cursor past the right border one" above.

        // 10. setup charset

        if self.charsets[0] == Charset::Drawing {
            // put drawing charset into G0 slot
            funs.push(Function::Gzd4(Charset::Drawing));
        }

        if self.charsets[1] == Charset::Drawing {
            // put drawing charset into G1 slot
            funs.push(Function::G1d4(Charset::Drawing));
        }

        if self.active_charset == 1 {
            // shift-out: point GL to G1 slot
            funs.push(Function::So);
        }

        // 11. setup insert mode

        if self.insert_mode {
            // enable insert mode
            funs.push(Function::Sm(AnsiModes::one(AnsiMode::Insert)));
        }

        // 12. setup auto-wrap mode

        if !self.auto_wrap_mode {
            // disable auto-wrap mode
            funs.push(Function::Decrst(DecModes::one(DecMode::AutoWrap)));
        }

        // 13. setup new line mode

        if self.new_line_mode {
            // enable new line mode
            funs.push(Function::Sm(AnsiModes::one(AnsiMode::NewLine)));
        }

        // 14. setup cursor key mode

        if self.cursor_keys_mode == CursorKeysMode::Application {
            funs.push(Function::Decset(DecModes::one(DecMode::CursorKeys)));
        }

        funs
    }
}

fn dump_buffer(buffer: &Buffer, funs: &mut Vec<Function>) {
    let mut cutoff = 0;
    let mut wrapped = false;

    for (i, line) in buffer.view().enumerate() {
        if wrapped || line.wrapped || !line.is_blank() {
            cutoff = i + 1;
        }

        wrapped = line.wrapped;
    }

    let last = buffer.rows - 1;
    let mut pen = Pen::default();

    for (i, line) in buffer.view().take(cutoff).enumerate() {
        for cells in line.chunks(|c1, c2| c1.pen() != c2.pen()) {
            if cells[0].pen() != &pen {
                if let Some(sgr) = to_sgr_diff(&pen, cells[0].pen()) {
                    funs.push(sgr);
                }

                pen = *cells[0].pen();
            }

            dump_cells(&cells, funs);
        }

        if i < last && !line.wrapped {
            funs.push(Function::Cr);
            funs.push(Function::Lf);
        }
    }
}

fn to_sgr_diff(from: &Pen, to: &Pen) -> Option<Function> {
    if from == to {
        return None;
    }

    let full = to_sgr_ops(to);
    let diff = to_sgr_diff_ops(from, to);
    let ops = if diff.len() <= full.len() { diff } else { full };

    Some(Function::Sgr(ops))
}

fn to_sgr_diff_ops(from: &Pen, to: &Pen) -> SgrOps {
    let mut ops = SgrOps::new();

    if from.intensity != to.intensity {
        match to.intensity {
            Intensity::Normal => ops.push(SgrOp::ResetIntensity),
            Intensity::Bold => ops.push(SgrOp::SetBoldIntensity),
            Intensity::Faint => ops.push(SgrOp::SetFaintIntensity),
        }
    }

    if from.foreground() != to.foreground() {
        match to.foreground() {
            Some(color) => ops.push(SgrOp::SetForegroundColor(color)),
            None => ops.push(SgrOp::ResetForegroundColor),
        }
    }

    if from.background() != to.background() {
        match to.background() {
            Some(color) => ops.push(SgrOp::SetBackgroundColor(color)),
            None => ops.push(SgrOp::ResetBackgroundColor),
        }
    }

    push_attr_diff(
        &mut ops,
        from.is_italic(),
        to.is_italic(),
        SgrOp::SetItalic,
        SgrOp::ResetItalic,
    );

    push_attr_diff(
        &mut ops,
        from.is_underline(),
        to.is_underline(),
        SgrOp::SetUnderline,
        SgrOp::ResetUnderline,
    );

    push_attr_diff(
        &mut ops,
        from.is_blink(),
        to.is_blink(),
        SgrOp::SetBlink,
        SgrOp::ResetBlink,
    );

    push_attr_diff(
        &mut ops,
        from.is_inverse(),
        to.is_inverse(),
        SgrOp::SetInverse,
        SgrOp::ResetInverse,
    );

    push_attr_diff(
        &mut ops,
        from.is_strikethrough(),
        to.is_strikethrough(),
        SgrOp::SetStrikethrough,
        SgrOp::ResetStrikethrough,
    );

    ops
}

fn push_attr_diff(ops: &mut SgrOps, from: bool, to: bool, set: SgrOp, reset: SgrOp) {
    if from != to {
        ops.push(if to { set } else { reset });
    }
}

fn dump_cells(cells: &[Cell], funs: &mut Vec<Function>) {
    let mut i = 0;

    while i < cells.len() {
        let ch = cells[i].char();
        let mut run_len = 1;

        while i + run_len < cells.len() && cells[i + run_len].char() == ch {
            run_len += 1;
        }

        if run_len > 5 {
            funs.push(Function::Print(ch));
            funs.push(Function::Rep(as_u16(run_len - 1)));
        } else {
            for _ in 0..run_len {
                funs.push(Function::Print(ch));
            }
        }

        i += run_len;
    }
}

fn to_sgr(pen: &Pen) -> Function {
    Function::Sgr(to_sgr_ops(pen))
}

fn to_sgr_ops(pen: &Pen) -> SgrOps {
    let mut ops = SgrOps::new();
    ops.push(SgrOp::Reset);

    if let Some(color) = pen.foreground() {
        ops.push(SgrOp::SetForegroundColor(color));
    }

    if let Some(color) = pen.background() {
        ops.push(SgrOp::SetBackgroundColor(color));
    }

    match pen.intensity {
        Intensity::Normal => {}
        Intensity::Bold => ops.push(SgrOp::SetBoldIntensity),
        Intensity::Faint => ops.push(SgrOp::SetFaintIntensity),
    }

    if pen.is_italic() {
        ops.push(SgrOp::SetItalic);
    }

    if pen.is_underline() {
        ops.push(SgrOp::SetUnderline);
    }

    if pen.is_blink() {
        ops.push(SgrOp::SetBlink);
    }

    if pen.is_inverse() {
        ops.push(SgrOp::SetInverse);
    }

    if pen.is_strikethrough() {
        ops.push(SgrOp::SetStrikethrough);
    }

    ops
}

fn as_u16(value: usize) -> u16 {
    value
        .try_into()
        .expect("terminal dump parameter exceeds u16 range")
}

fn as_usize(value: u16, default: usize) -> usize {
    if value == 0 {
        default
    } else {
        value as usize
    }
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new((80, 24), None)
    }
}

#[cfg(test)]
mod tests {
    use super::{BufferType, Occupancy, Terminal};
    use crate::charset::Charset;
    use crate::color::Color;
    use crate::line::Line;
    use crate::parser::{
        AnsiMode, AnsiModes, DecMode, DecModes, EdScope, ElScope, Function, SgrOp, SgrOps,
    };
    use crate::pen::Intensity;
    use crate::pen::Pen;
    use Function::*;
    use SgrOp::*;

    fn feed(term: &mut Terminal, input: &str) {
        for ch in input.chars() {
            let fun = match ch {
                '\n' => Lf,
                '\r' => Cr,
                _ => Print(ch),
            };

            term.execute(fun);
        }
    }

    fn build_term(cols: usize, rows: usize, cx: usize, cy: usize, init: &str) -> Terminal {
        let mut term = Terminal::new((cols, rows), None);
        feed(&mut term, init);
        term.execute(Cup((cy + 1) as u16, (cx + 1) as u16));

        term
    }

    fn text(term: &Terminal) -> String {
        let cursor = term.cursor();

        buffer_text(term.view(), cursor.col, cursor.row)
    }

    fn buffer_text<'a, I: Iterator<Item = &'a Line> + 'a>(
        mut view: I,
        cursor_col: usize,
        cursor_row: usize,
    ) -> String {
        let mut lines = Vec::new();
        lines.extend(view.by_ref().take(cursor_row).map(|l| l.text()));
        let cursor_line = view.next().unwrap();
        let mut offset = 0;
        let mut line = String::new();

        let mut cells = cursor_line
            .cells()
            .iter()
            .filter(|c| c.occupancy() != Occupancy::WideTail);

        for cell in cells.by_ref() {
            let width = cell.width() as usize;

            if offset + width <= cursor_col {
                line.push(cell.char());
                offset += width;
            } else {
                line.push('|');
                line.push(cell.char());
                offset += width;
                break;
            }
        }

        if offset == cursor_col {
            line.push('|');
        }

        line.extend(cells.map(|c| c.char()));
        lines.push(line);
        lines.extend(view.map(|l| l.text()));

        lines
            .into_iter()
            .map(|line| line.trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn wrapped(term: &Terminal) -> Vec<bool> {
        term.view().map(|l| l.wrapped).collect()
    }

    fn ansi_modes<const N: usize>(modes: [AnsiMode; N]) -> AnsiModes {
        AnsiModes::from(&modes[..])
    }

    fn dec_modes<const N: usize>(modes: [DecMode; N]) -> DecModes {
        DecModes::from(&modes[..])
    }

    fn sgr(op: SgrOp) -> Function {
        Sgr(SgrOps::from(vec![op]))
    }

    fn pen(f: impl FnOnce(&mut Pen)) -> Pen {
        let mut pen = Pen::default();
        f(&mut pen);
        pen
    }

    #[test]
    fn execute_sgr() {
        let mut term = Terminal::default();

        term.execute(sgr(SetBoldIntensity));

        assert!(term.pen.intensity == Intensity::Bold);

        term.execute(sgr(SetFaintIntensity));

        assert_eq!(term.pen.intensity, Intensity::Faint);

        term.execute(sgr(SetItalic));

        assert!(term.pen.is_italic());

        term.execute(sgr(SetUnderline));

        assert!(term.pen.is_underline());

        term.execute(sgr(SetBlink));

        assert!(term.pen.is_blink());

        term.execute(sgr(SetInverse));

        assert!(term.pen.is_inverse());

        term.execute(sgr(SetStrikethrough));

        assert!(term.pen.is_strikethrough());

        term.execute(sgr(SetForegroundColor(Color::Indexed(1))));

        assert_eq!(term.pen.foreground, Some(Color::Indexed(1)));

        term.execute(sgr(SetBackgroundColor(Color::Indexed(2))));

        assert_eq!(term.pen.background, Some(Color::Indexed(2)));

        term.execute(sgr(ResetForegroundColor));

        assert_eq!(term.pen.foreground, None);

        term.execute(sgr(ResetBackgroundColor));

        assert_eq!(term.pen.background, None);

        term.execute(Sgr(SgrOps::from(vec![
            SetBoldIntensity,
            SetForegroundColor(Color::Indexed(1)),
            SetBackgroundColor(Color::Indexed(2)),
            SetBlink,
            ResetIntensity,
        ])));

        assert_eq!(term.pen.intensity, Intensity::Normal);
        assert!(term.pen.is_blink());
        assert_eq!(term.pen.foreground, Some(Color::Indexed(1)));
        assert_eq!(term.pen.background, Some(Color::Indexed(2)));
    }

    #[test]
    fn execute_lf() {
        let mut term = build_term(8, 2, 3, 0, "abc");

        term.execute(Lf);

        assert_eq!(term.cursor(), (3, 1));
        assert_eq!(text(&term), "abc\n   |");

        term.execute(Print('d'));
        term.execute(Lf);

        assert_eq!(term.cursor(), (4, 1));
        assert_eq!(text(&term), "   d\n    |");
    }

    #[test]
    fn execute_cr() {
        let mut term = Terminal::new((4, 2), None);

        feed(&mut term, "abc");
        term.execute(Cr);
        term.execute(Print('d'));

        assert_eq!(term.cursor(), (1, 0));
        assert_eq!(text(&term), "d|bc\n");
    }

    #[test]
    fn execute_nel() {
        let mut term = build_term(8, 2, 3, 0, "abc");

        term.execute(Nel);

        assert_eq!(term.cursor(), (0, 1));
        assert_eq!(text(&term), "abc\n|");

        term.execute(Print('d'));

        assert_eq!(text(&term), "abc\nd|");
    }

    #[test]
    fn execute_bs() {
        let mut term = Terminal::new((4, 2), None);

        feed(&mut term, "a");
        term.execute(Bs);

        assert_eq!(text(&term), "|a\n");

        term.execute(Bs);

        assert_eq!(text(&term), "|a\n");

        feed(&mut term, "abcd");
        term.execute(Bs);

        assert_eq!(text(&term), "ab|cd\n");

        feed(&mut term, "cdef");
        term.execute(Bs);

        assert_eq!(text(&term), "abcd\ne|f");

        term.execute(Bs);

        assert_eq!(text(&term), "abcd\n|ef");

        term.execute(Bs);

        assert_eq!(text(&term), "abcd\n|ef");
    }

    #[test]
    fn execute_cup() {
        let mut term = Terminal::new((4, 2), None);

        feed(&mut term, "abc\r\ndef");
        term.execute(Cup(1, 1));

        assert_eq!(term.cursor(), (0, 0));

        term.execute(Cup(10, 10));

        assert_eq!(term.cursor(), (3, 1));
    }

    #[test]
    fn execute_cuu() {
        let mut term = Terminal::new((8, 4), None);

        feed(&mut term, "abcd\n\n\n");
        term.execute(Cuu(0));

        assert_eq!(term.cursor(), (4, 2));

        term.execute(Cuu(2));

        assert_eq!(term.cursor(), (4, 0));
    }

    #[test]
    fn execute_cpl() {
        let mut term = Terminal::new((8, 4), None);

        feed(&mut term, "abcd\r\n\r\n\r\nef");

        assert_eq!(term.cursor(), (2, 3));

        term.execute(Cpl(0));

        assert_eq!(term.cursor(), (0, 2));

        term.execute(Cpl(2));

        assert_eq!(term.cursor(), (0, 0));
    }

    #[test]
    fn execute_cnl() {
        let mut term = Terminal::new((4, 4), None);

        feed(&mut term, "ab");
        term.execute(Cnl(0));

        assert_eq!(term.cursor(), (0, 1));

        term.execute(Cnl(3));

        assert_eq!(term.cursor(), (0, 3));
    }

    #[test]
    fn execute_vpa() {
        let mut term = Terminal::new((4, 4), None);

        feed(&mut term, "\r\n\r\naaa\r\nbbb");
        term.execute(Vpa(0));

        assert_eq!(term.cursor(), (3, 0));

        term.execute(Vpa(10));

        assert_eq!(term.cursor(), (3, 3));
    }

    #[test]
    fn execute_cud() {
        let mut term = Terminal::new((8, 4), None);

        feed(&mut term, "abcd");
        term.execute(Cud(0));

        assert_eq!(text(&term), "abcd\n    |\n\n");

        term.execute(Cud(2));

        assert_eq!(text(&term), "abcd\n\n\n    |");
    }

    #[test]
    fn execute_cuf() {
        let mut term = Terminal::new((4, 1), None);

        term.execute(Cuf(2));

        assert_eq!(text(&term), "  |");

        term.execute(Cuf(2));

        assert_eq!(text(&term), "   |");

        term.execute(Print('a'));

        assert_eq!(text(&term), "   a|");

        term.execute(Cuf(5));

        assert_eq!(text(&term), "   |a");

        feed(&mut term, "ab");
        term.execute(Cuf(10));

        assert_eq!(text(&term), "b  |");
    }

    #[test]
    fn execute_cha() {
        let mut term = Terminal::new((8, 2), None);

        feed(&mut term, "abc");
        term.execute(Cha(0));

        assert_eq!(term.cursor(), (0, 0));

        term.execute(Cha(3));

        assert_eq!(term.cursor(), (2, 0));

        term.execute(Cha(20));

        assert_eq!(term.cursor(), (7, 0));
    }

    #[test]
    fn execute_cub() {
        let mut term = Terminal::new((8, 2), None);

        feed(&mut term, "abcd");
        term.execute(Cub(2));

        assert_eq!(text(&term), "ab|cd\n");

        feed(&mut term, "cdef");
        term.execute(Cub(2));

        assert_eq!(text(&term), "abcd|ef\n");

        term.execute(Cub(10));

        assert_eq!(text(&term), "|abcdef\n");

        let mut term = Terminal::new((4, 2), None);

        feed(&mut term, "abcd");
        term.execute(Cub(0));

        assert_eq!(text(&term), "ab|cd\n");
    }

    #[test]
    fn execute_ht() {
        let mut term = Terminal::new((20, 1), None);

        term.execute(Ht);
        assert_eq!(term.cursor(), (8, 0));

        term.execute(Ht);
        assert_eq!(term.cursor(), (16, 0));

        term.execute(Ht);
        assert_eq!(term.cursor(), (19, 0));
    }

    #[test]
    fn execute_hts() {
        let mut term = Terminal::new((20, 1), None);

        term.execute(Cuf(5));
        term.execute(Hts);

        assert_eq!(term.tabs, vec![5, 8, 16]);

        term.execute(Cup(1, 1));
        term.execute(Ht);

        assert_eq!(term.cursor(), (5, 0));
    }

    #[test]
    fn execute_cht() {
        let mut term = build_term(28, 1, 3, 0, "abcdefghijklmnopqrstuwxyzabc");

        term.execute(Cht(0));

        assert_eq!(term.cursor(), (8, 0));

        term.execute(Cht(2));

        assert_eq!(term.cursor(), (24, 0));

        term.execute(Cht(0));

        assert_eq!(term.cursor(), (27, 0));
    }

    #[test]
    fn execute_cbt() {
        let mut term = build_term(28, 1, 26, 0, "abcdefghijklmnopqrstuwxyzabc");

        term.execute(Cbt(0));

        assert_eq!(term.cursor(), (24, 0));

        term.execute(Cbt(2));

        assert_eq!(term.cursor(), (8, 0));

        term.execute(Cbt(0));

        assert_eq!(term.cursor(), (0, 0));
    }

    #[test]
    fn execute_rep() {
        let mut term = build_term(20, 2, 0, 0, "");

        term.execute(Rep(0));

        assert_eq!(text(&term), "|\n");

        term.execute(Print('A'));
        term.execute(Rep(0));

        assert_eq!(text(&term), "AA|\n");

        term.execute(Rep(3));

        assert_eq!(text(&term), "AAAAA|\n");

        term.execute(Cuf(5));
        term.execute(Rep(0));

        assert_eq!(text(&term), "AAAAA      |\n");
    }

    #[test]
    fn execute_rep_after_wide_char() {
        let mut term = build_term(20, 2, 0, 0, "");

        term.execute(Print('ハ'));
        term.execute(Rep(3));

        assert_eq!(text(&term), "ハハハハ|\n");
    }

    #[test]
    fn execute_rep_after_moving_into_wide_char_tail() {
        let mut term = build_term(20, 2, 0, 0, "");

        term.execute(Print('ハ'));
        term.execute(Cub(1));
        term.execute(Rep(3));

        assert_eq!(text(&term), " ハハハ|\n");
    }

    #[test]
    fn dump_uses_rep_for_wide_char_runs() {
        let mut term = build_term(20, 2, 0, 0, "");

        feed(&mut term, "ハハハハハハ");

        let funs = term.dump();

        assert_eq!(funs[..2], [Print('ハ'), Rep(5)]);
    }

    #[test]
    fn sgr_diff_prefers_targeted_ops_on_tie() {
        let from = pen(|pen| pen.intensity = Intensity::Bold);

        assert_eq!(
            super::to_sgr_diff(&from, &Pen::default()),
            Some(sgr(ResetIntensity))
        );
    }

    #[test]
    fn sgr_diff_uses_full_reset_when_shorter() {
        let from = pen(|pen| {
            pen.intensity = Intensity::Bold;
            pen.set_underline();
        });

        assert_eq!(super::to_sgr_diff(&from, &Pen::default()), Some(sgr(Reset)));
    }

    #[test]
    fn sgr_diff_uses_targeted_sets_for_first_styled_chunk() {
        let to = pen(|pen| {
            pen.intensity = Intensity::Bold;
            pen.foreground = Some(Color::Indexed(1));
        });

        assert_eq!(
            super::to_sgr_diff(&Pen::default(), &to),
            Some(Sgr(SgrOps::from(vec![
                SetBoldIntensity,
                SetForegroundColor(Color::Indexed(1))
            ])))
        );
    }

    #[test]
    fn execute_vpr() {
        let mut term = Terminal::new((4, 4), None);

        feed(&mut term, "ab");
        term.execute(Vpr(0));

        assert_eq!(term.cursor(), (2, 1));
        assert_eq!(text(&term), "ab\n  |\n\n");

        term.execute(Vpr(10));

        assert_eq!(term.cursor(), (2, 3));
        assert_eq!(text(&term), "ab\n\n\n  |");
    }

    #[test]
    fn execute_ich() {
        let mut term = build_term(8, 2, 3, 0, "abcdefghijklmn");

        term.execute(Ich(0));

        assert_eq!(text(&term), "abc| defg\nijklmn");
        assert_eq!(wrapped(&term), vec![true, false]);

        term.execute(Ich(2));

        assert_eq!(text(&term), "abc|   de\nijklmn");

        term.execute(Ich(10));

        assert_eq!(text(&term), "abc|\nijklmn");

        let mut term = build_term(8, 2, 7, 0, "abcdefghijklmn");

        term.execute(Ich(10));
        assert_eq!(text(&term), "abcdefg|\nijklmn");
    }

    #[test]
    fn execute_il() {
        let mut term = build_term(4, 4, 2, 1, "abcdefghij");

        term.execute(Il(0));

        assert_eq!(text(&term), "abcd\n  |\nefgh\nij");
        assert_eq!(wrapped(&term), vec![false, false, true, false]);

        term.execute(Cuu(1));
        term.execute(Il(0));

        assert_eq!(text(&term), "  |\nabcd\n\nefgh");
        assert_eq!(wrapped(&term), vec![false, false, false, false]);

        term.execute(Cud(3));
        term.execute(Il(100));

        assert_eq!(text(&term), "\nabcd\n\n  |");
    }

    #[test]
    fn execute_dl() {
        let mut term = Terminal::new((4, 4), None);

        feed(&mut term, "abcdefghijklmn");
        term.execute(Cuu(2));
        term.execute(Dl(0));

        assert_eq!(text(&term), "abcd\nij|kl\nmn\n");
        assert_eq!(wrapped(&term), vec![false, true, false, false]);

        let mut term = Terminal::new((4, 4), None);

        feed(&mut term, "abcdefghijklmn");
        term.execute(Decstbm(1, 3));
        term.execute(Cup(2, 1));
        term.execute(Dl(0));

        assert_eq!(text(&term), "abcd\n|ijkl\n\nmn");
        assert_eq!(wrapped(&term), vec![false, false, false, false]);

        let mut term = Terminal::new((4, 4), None);

        feed(&mut term, "abcdefghijklmn");
        term.execute(Decstbm(1, 2));
        term.execute(Cup(4, 1));
        term.execute(Dl(0));

        assert_eq!(text(&term), "abcd\nefgh\nijkl\n|");
        assert_eq!(wrapped(&term), vec![true, true, false, false]);
    }

    #[test]
    fn execute_el() {
        let mut term = build_term(4, 2, 2, 0, "abcd");

        term.execute(El(ElScope::ToRight));

        assert_eq!(text(&term), "ab|\n");

        let mut term = build_term(4, 2, 2, 0, "a");

        term.execute(El(ElScope::ToRight));

        assert_eq!(text(&term), "a |\n");

        let mut term = build_term(4, 2, 2, 0, "abcd");

        term.execute(El(ElScope::ToLeft));

        assert_eq!(text(&term), "  | d\n");

        let mut term = build_term(4, 2, 2, 0, "abcd");

        term.execute(El(ElScope::All));

        assert_eq!(text(&term), "  |\n");

        let mut term = Terminal::new((4, 3), None);

        feed(&mut term, "abcdefghij");
        term.execute(Cuu(1));
        term.execute(El(ElScope::ToRight));

        assert_eq!(text(&term), "abcd\nef|\nij");
        assert_eq!(wrapped(&term), vec![true, false, false]);

        let mut term = Terminal::new((4, 3), None);

        feed(&mut term, "abcdefghij");
        term.execute(Cuu(1));
        term.execute(El(ElScope::ToLeft));

        assert_eq!(text(&term), "abcd\n  | h\nij");
        assert_eq!(wrapped(&term), vec![true, true, false]);

        let mut term = Terminal::new((4, 3), None);

        feed(&mut term, "abcdefghij");
        term.execute(Cuu(1));
        term.execute(El(ElScope::All));

        assert_eq!(text(&term), "abcd\n  |\nij");
        assert_eq!(wrapped(&term), vec![true, false, false]);
    }

    #[test]
    fn execute_ed() {
        let mut term = build_term(4, 3, 1, 1, "abc\r\ndef\r\nghi");

        term.execute(Ed(EdScope::Below));

        assert_eq!(text(&term), "abc\nd|\n");

        let mut term = build_term(4, 3, 1, 1, "abc\r\n\r\nghi");

        term.execute(Ed(EdScope::Below));

        assert_eq!(text(&term), "abc\n |\n");

        let mut term = build_term(4, 3, 1, 1, "abc\r\ndef\r\nghi");

        term.execute(Ed(EdScope::Above));

        assert_eq!(text(&term), "\n | f\nghi");

        let mut term = build_term(4, 3, 1, 1, "abc\r\ndef\r\nghi");

        term.execute(Ed(EdScope::All));

        assert_eq!(text(&term), "\n |\n");

        let mut term = build_term(4, 3, 1, 1, "abcdefghij");

        term.execute(Ed(EdScope::Below));

        assert_eq!(text(&term), "abcd\ne|\n");
        assert_eq!(wrapped(&term), vec![true, false, false]);

        let mut term = build_term(4, 3, 1, 1, "abcdefghij");

        term.execute(Ed(EdScope::Above));

        assert_eq!(text(&term), "\n | gh\nij");
        assert_eq!(wrapped(&term), vec![false, true, false]);

        let mut term = build_term(4, 3, 1, 1, "abcdefghij");

        term.execute(Ed(EdScope::All));

        assert_eq!(text(&term), "\n |\n");
        assert_eq!(wrapped(&term), vec![false, false, false]);
    }

    #[test]
    fn execute_dch() {
        let mut term = build_term(8, 2, 3, 0, "abcdefghijkl");

        term.execute(Dch(0));

        assert_eq!(text(&term), "abc|efgh\nijkl");
        assert_eq!(wrapped(&term), vec![false, false]);

        term.execute(Dch(2));

        assert_eq!(text(&term), "abc|gh\nijkl");

        term.execute(Dch(10));

        assert_eq!(text(&term), "abc|\nijkl");

        term.execute(Cuf(10));
        term.execute(Dch(10));

        assert_eq!(text(&term), "abc    |\nijkl");
    }

    #[test]
    fn execute_ech() {
        let mut term = build_term(8, 2, 3, 0, "abcdefghijkl");

        term.execute(Ech(0));

        assert_eq!(text(&term), "abc| efgh\nijkl");
        assert_eq!(wrapped(&term), vec![true, false]);

        term.execute(Ech(2));

        assert_eq!(text(&term), "abc|  fgh\nijkl");
        assert_eq!(wrapped(&term), vec![true, false]);

        term.execute(Ech(10));

        assert_eq!(text(&term), "abc|\nijkl");
        assert_eq!(wrapped(&term), vec![false, false]);

        term.execute(Cuf(3));
        term.execute(Ech(0));

        assert_eq!(text(&term), "abc   |\nijkl");
    }

    #[test]
    fn execute_ri() {
        let mut term = build_term(8, 5, 0, 0, "abcd\r\nefgh\r\nijkl\r\nmnop\r\nqrst");

        term.execute(Ri);

        assert_eq!(text(&term), "|\nabcd\nefgh\nijkl\nmnop");

        term.execute(Decstbm(3, 4));
        term.execute(Cup(3, 1));
        term.execute(Ri);

        assert_eq!(text(&term), "\nabcd\n|\nefgh\nmnop");
    }

    #[test]
    fn execute_sc_rc() {
        let mut term = build_term(4, 3, 0, 0, "");

        feed(&mut term, "  \n");
        term.execute(Decsc);
        feed(&mut term, " \n");
        term.execute(Decrc);

        assert_eq!(term.cursor(), (2, 1));

        let mut term = build_term(4, 3, 0, 0, "");

        feed(&mut term, "  \n");
        term.execute(Scosc);
        feed(&mut term, " \n");
        term.execute(Scorc);

        assert_eq!(term.cursor(), (2, 1));
    }

    #[test]
    fn execute_sc_rc_restores_pen_and_modes() {
        fn assert_save_restore(save: Function, restore: Function) {
            let mut term = Terminal::new((4, 4), None);

            term.execute(Decrst(dec_modes([DecMode::AutoWrap])));
            term.execute(Decstbm(2, 4));
            term.execute(Decset(dec_modes([DecMode::Origin])));
            term.execute(Cup(2, 3));
            term.execute(sgr(SetBoldIntensity));
            term.execute(save);

            term.execute(Decset(dec_modes([DecMode::AutoWrap])));
            term.execute(Decrst(dec_modes([DecMode::Origin])));
            term.execute(Cup(4, 4));
            term.execute(sgr(Reset));
            term.execute(restore);

            assert_eq!(term.cursor(), (2, 2));
            assert_eq!(term.pen.intensity, Intensity::Bold);
            assert!(term.origin_mode);
            assert!(!term.auto_wrap_mode);
        }

        assert_save_restore(Decsc, Decrc);
        assert_save_restore(Scosc, Scorc);
    }

    #[test]
    fn auto_wrap_mode() {
        let mut term = Terminal::new((4, 4), None);

        term.execute(Decset(dec_modes([DecMode::AutoWrap])));
        feed(&mut term, "abcdef");

        assert_eq!(text(&term), "abcd\nef|\n\n");

        let mut term = Terminal::new((4, 4), None);

        term.execute(Decrst(dec_modes([DecMode::AutoWrap])));
        feed(&mut term, "abcdef");

        assert_eq!(text(&term), "abc|f\n\n\n");
    }

    #[test]
    fn insert_mode() {
        let mut term = Terminal::new((4, 4), None);

        feed(&mut term, "abcd");
        term.execute(Cub(2));
        term.execute(Sm(ansi_modes([AnsiMode::Insert])));
        feed(&mut term, "ef");

        assert_eq!(text(&term), "aef|b\n\n\n");

        feed(&mut term, "ghij");

        assert_eq!(text(&term), "aefg\nhij|\n\n");
    }

    #[test]
    fn print_at_the_end_of_the_screen() {
        let mut term = Terminal::new((4, 6), None);

        feed(&mut term, "xxxxxxxxxx");
        term.execute(Cup(50, 1));
        feed(&mut term, "yyy");
        term.execute(Cuf(50));
        feed(&mut term, "zzz");

        assert_eq!(text(&term), "xxxx\nxx\n\n\nyyyz\nzz|");

        let mut term = Terminal::new((4, 6), None);

        feed(&mut term, "\nxxxxxxxxxx");
        term.execute(Decstbm(2, 4));
        term.execute(Cup(1, 1));
        feed(&mut term, "yyy");
        term.execute(Cuf(50));
        feed(&mut term, "zzz");

        assert_eq!(text(&term), "yyyz\nzz|xx\nxxxx\nxx\n\n");

        let mut term = Terminal::new((4, 6), None);

        term.execute(Decstbm(0, 3));
        feed(&mut term, "xxxxxxxxxx");
        term.execute(Cup(50, 1));
        feed(&mut term, "yyy");
        term.execute(Cuf(50));
        feed(&mut term, "zzz");

        assert_eq!(text(&term), "xxxx\nxxxx\nxx\n\n\nzz|yz");
    }

    #[test]
    fn wide_chars() {
        let mut term = Terminal::new((20, 2), None);

        feed(&mut term, "ハローワールド");
        assert_eq!(text(&term), "ハローワールド|\n");

        term.execute(Cub(5));
        assert_eq!(term.cursor().col, 9);
        assert_eq!(text(&term), "ハローワ|ールド\n");
    }

    #[test]
    fn execute_su() {
        let mut term = Terminal::new((4, 6), None);
        feed(&mut term, "aa\r\nbb\r\ncc\r\ndd\r\nee\r\nff");
        term.execute(Su(2));
        assert_eq!(text(&term), "cc\ndd\nee\nff\n\n  |");

        let mut term = Terminal::new((4, 6), None);
        feed(&mut term, "aa\r\nbb\r\ncc\r\ndd\r\nee\r\nff");
        term.execute(sgr(SetBoldIntensity));
        term.execute(Su(2));
        assert_eq!(text(&term), "cc\ndd\nee\nff\n\n  |");
        assert!(term.view().last().unwrap()[0].pen().is_bold());

        let mut term = Terminal::new((4, 6), None);

        feed(&mut term, "aa\r\nbb\r\ncc\r\ndd\r\nee\r\nff");
        term.execute(Decstbm(2, 5));
        term.execute(Cup(1, 1));
        term.execute(Su(2));

        assert_eq!(text(&term), "|aa\ndd\nee\n\n\nff");

        let mut term = Terminal::new((4, 6), None);

        feed(&mut term, "aaaaaa\r\nbbbbbb\r\ncccccc");
        term.execute(Su(2));

        assert_eq!(text(&term), "bbbb\nbb\ncccc\ncc\n\n  |");
        assert_eq!(wrapped(&term), vec![true, false, true, false, false, false]);

        let mut term = Terminal::new((4, 6), None);

        feed(&mut term, "aaaaaa\r\nbbbbbb\r\ncccccc");
        term.execute(Decstbm(2, 5));
        term.execute(Cup(1, 1));
        term.execute(Su(2));

        assert_eq!(text(&term), "|aaaa\nbb\ncccc\n\n\ncc");

        assert_eq!(
            wrapped(&term),
            vec![false, false, false, false, false, false]
        );
    }

    #[test]
    fn execute_sd() {
        let mut term = Terminal::new((4, 6), None);

        feed(&mut term, "aa\r\nbb\r\ncc\r\ndd\r\nee\r\nff");
        term.execute(Sd(2));

        assert_eq!(text(&term), "\n\naa\nbb\ncc\ndd|");

        let mut term = Terminal::new((4, 6), None);

        feed(&mut term, "aa\r\nbb\r\ncc\r\ndd\r\nee\r\nff");
        term.execute(Decstbm(2, 5));
        term.execute(Cup(1, 1));
        term.execute(Sd(2));

        assert_eq!(text(&term), "|aa\n\n\nbb\ncc\nff");

        let mut term = Terminal::new((4, 6), None);

        feed(&mut term, "aaaaaa\r\nbbbbbb\r\ncccccc");
        term.execute(Sd(2));

        assert_eq!(text(&term), "\n\naaaa\naa\nbbbb\nbb|");
        assert_eq!(wrapped(&term), vec![false, false, true, false, true, false]);

        let mut term = Terminal::new((4, 6), None);

        feed(&mut term, "aaaaaa\r\nbbbbbb\r\ncccccc");
        term.execute(Decstbm(2, 5));
        term.execute(Cup(1, 1));
        term.execute(Sd(2));

        assert_eq!(text(&term), "|aaaa\n\n\naa\nbbbb\ncc");

        assert_eq!(
            wrapped(&term),
            vec![false, false, false, false, false, false]
        );
    }

    #[test]
    fn charsets() {
        let mut term = build_term(6, 7, 0, 0, "");

        feed(&mut term, "alpty\r\n");
        term.execute(Gzd4(Charset::Drawing));
        feed(&mut term, "alpty\r\n");
        term.execute(So);
        feed(&mut term, "alpty\r\n");
        term.execute(G1d4(Charset::Drawing));
        feed(&mut term, "alpty\r\n");
        term.execute(G1d4(Charset::Ascii));
        feed(&mut term, "alpty\r\n");
        term.execute(Gzd4(Charset::Ascii));
        term.execute(Si);
        feed(&mut term, "alpty");

        assert_eq!(text(&term), "alpty\n▒┌⎻├≤\nalpty\n▒┌⎻├≤\nalpty\nalpty|\n");
    }

    #[test]
    fn execute_decaln() {
        let mut term = Terminal::new((4, 2), None);

        feed(&mut term, "ab\r\nc");
        term.execute(Cup(2, 3));
        term.execute(Decaln);

        assert_eq!(term.cursor(), (2, 1));
        assert_eq!(text(&term), "EEEE\nEE|EE");
    }

    #[test]
    fn resize_wider() {
        let mut term = Terminal::new((6, 6), None);

        term.resize(7, 6);

        assert_eq!(text(&term), "|\n\n\n\n\n");
        assert!(!term.view().any(|l| l.wrapped));

        term.resize(15, 6);

        assert_eq!(text(&term), "|\n\n\n\n\n");
        assert!(!term.view().any(|l| l.wrapped));

        let mut term = Terminal::new((6, 6), None);

        feed(&mut term, "000000111111222222333333444444555");

        assert_eq!(text(&term), "000000\n111111\n222222\n333333\n444444\n555|");
        assert_eq!(wrapped(&term), vec![true, true, true, true, true, false]);

        term.resize(7, 6);

        assert_eq!(text(&term), "0000001\n1111122\n2222333\n3334444\n44555|\n");
        assert_eq!(wrapped(&term), vec![true, true, true, true, false, false]);

        term.resize(15, 6);

        assert_eq!(text(&term), "000000111111222\n222333333444444\n555|\n\n\n");
        assert_eq!(wrapped(&term), vec![true, true, false, false, false, false]);

        let mut term = Terminal::new((4, 3), None);

        feed(&mut term, "000011\r\n22");

        assert_eq!(text(&term), "0000\n11\n22|");
        assert_eq!(wrapped(&term), vec![true, false, false]);

        term.resize(8, 3);

        assert_eq!(text(&term), "000011\n22|\n");
        assert_eq!(wrapped(&term), vec![false, false, false]);
    }

    #[test]
    fn resize_narrower() {
        let mut term = Terminal::new((15, 6), None);

        term.resize(7, 6);

        assert_eq!(text(&term), "|\n\n\n\n\n");
        assert!(!term.view().any(|l| l.wrapped));

        term.resize(6, 6);

        assert_eq!(text(&term), "|\n\n\n\n\n");
        assert!(!term.view().any(|l| l.wrapped));

        let mut term = Terminal::new((8, 2), None);

        feed(&mut term, "\nabcdef");

        assert_eq!(wrapped(&term), vec![false, false]);

        term.resize(4, 2);

        assert_eq!(text(&term), "abcd\nef|");
        assert_eq!(wrapped(&term), vec![true, false]);

        let mut term = Terminal::new((15, 6), None);

        feed(&mut term, "000000111111222222333333444444555");

        assert_eq!(text(&term), "000000111111222\n222333333444444\n555|\n\n\n");
        assert_eq!(wrapped(&term), vec![true, true, false, false, false, false]);

        term.resize(7, 6);

        assert_eq!(text(&term), "2222333\n3334444\n44555|\n\n\n");
        assert_eq!(wrapped(&term), vec![true, true, false, false, false, false]);

        term.resize(6, 6);

        assert_eq!(text(&term), "333333\n444444\n555|\n\n\n");
        assert_eq!(wrapped(&term), vec![true, true, false, false, false, false]);
    }

    #[test]
    fn resize() {
        let mut term = Terminal::new((8, 4), None);
        feed(&mut term, "abcdefgh\r\nijklmnop\r\nqrstuw");
        term.execute(Cup(4, 1));
        feed(&mut term, "AAA");

        term.resize(8, 5);

        assert_eq!(text(&term), "abcdefgh\nijklmnop\nqrstuw\nAAA|\n");

        feed(&mut term, "BBBBB");

        assert_eq!(term.cursor(), (8, 3));

        term.resize(4, 5);

        assert_eq!(text(&term), "qrst\nuw\nAAAB\nBBB|B\n");

        feed(&mut term, "\rCCC");

        assert_eq!(text(&term), "qrst\nuw\nAAAB\nCCC|B\n");
        assert_eq!(wrapped(&term), vec![true, false, true, false, false]);

        term.resize(3, 5);

        assert_eq!(text(&term), "tuw\nAAA\nBCC\nC|B\n");

        term.resize(5, 5);

        assert_eq!(text(&term), "qrstu\nw\nAAABC\nCC|B\n");

        feed(&mut term, "DDD");
        term.resize(6, 5);

        assert_eq!(text(&term), "op\nqrstuw\nAAABCC\nCDDD|\n");
    }

    #[test]
    fn resize_taller() {
        let mut term = Terminal::new((6, 4), None);
        feed(&mut term, "AAA\n\rBBB\n\r");

        term.resize(6, 5);

        assert_eq!(text(&term), "AAA\nBBB\n|\n\n");
    }

    #[test]
    fn resize_shorter() {
        let mut term = Terminal::new((6, 6), None);

        feed(&mut term, "AAA\n\rBBB\n\rCCC\n\r");

        term.resize(6, 5);

        assert_eq!(text(&term), "AAA\nBBB\nCCC\n|\n");

        term.resize(6, 3);

        assert_eq!(text(&term), "BBB\nCCC\n|");

        term.resize(6, 2);

        assert_eq!(text(&term), "CCC\n|");
    }

    #[test]
    fn resize_vs_buffer_switching() {
        let mut term = Terminal::new((4, 4), None);

        feed(&mut term, "aaa\n\rbbb\n\rc\n\rddd");

        assert_eq!(term.cursor(), (3, 3));

        term.resize(4, 5);

        assert_eq!(text(&term), "aaa\nbbb\nc\nddd|\n");

        term.execute(Decset(dec_modes([DecMode::SaveCursorAltScreenBuffer])));

        assert_eq!(term.cursor(), (3, 3));

        term.resize(4, 2);

        assert_eq!(term.cursor(), (3, 1));

        term.resize(2, 3);
        term.resize(3, 3);

        term.execute(Decrst(dec_modes([DecMode::SaveCursorAltScreenBuffer])));

        assert_eq!(text(&term), "bbb\nc\ndd|d");
    }

    #[test]
    fn execute_new_line_mode() {
        let mut term = build_term(8, 2, 3, 0, "abc");

        term.execute(Sm(ansi_modes([AnsiMode::NewLine])));
        term.execute(Lf);

        assert_eq!(term.cursor(), (0, 1));

        term.execute(Rm(ansi_modes([AnsiMode::NewLine])));
        term.execute(Cup(1, 4));
        term.execute(Lf);

        assert_eq!(term.cursor(), (3, 1));
    }

    #[test]
    fn execute_cursor_keys_mode() {
        let mut term = Terminal::new((4, 2), None);

        assert!(!term.cursor_keys_app_mode());

        term.execute(Decset(dec_modes([DecMode::CursorKeys])));
        assert!(term.cursor_keys_app_mode());

        term.execute(Decrst(dec_modes([DecMode::CursorKeys])));
        assert!(!term.cursor_keys_app_mode());
    }

    #[test]
    fn execute_text_cursor_visibility_mode() {
        let mut term = Terminal::new((4, 2), None);

        assert!(term.cursor.visible);

        term.execute(Decrst(dec_modes([DecMode::TextCursorEnable])));
        assert!(!term.cursor.visible);

        term.execute(Decset(dec_modes([DecMode::TextCursorEnable])));
        assert!(term.cursor.visible);
    }

    #[test]
    fn execute_origin_mode() {
        let mut term = Terminal::new((4, 4), None);

        term.execute(Decstbm(2, 4));
        term.execute(Decset(dec_modes([DecMode::Origin])));

        assert_eq!(term.cursor(), (0, 1));

        term.execute(Cup(2, 1));
        assert_eq!(term.cursor(), (0, 2));

        term.execute(Decrst(dec_modes([DecMode::Origin])));
        assert_eq!(term.cursor(), (0, 0));
    }

    #[test]
    fn execute_alt_screen_buffer_mode() {
        let mut term = Terminal::new((4, 3), None);

        feed(&mut term, "ab\r\ncd");

        assert_eq!(term.active_buffer_type(), BufferType::Primary);
        assert_eq!(text(&term), "ab\ncd|\n");

        term.execute(Decset(dec_modes([DecMode::AltScreenBuffer])));

        assert_eq!(term.active_buffer_type(), BufferType::Alternate);
        assert_eq!(text(&term), "\n  |\n");

        feed(&mut term, "xy");
        assert_eq!(text(&term), "\n  xy|\n");

        term.execute(Decrst(dec_modes([DecMode::AltScreenBuffer])));

        assert_eq!(term.active_buffer_type(), BufferType::Primary);
        assert_eq!(text(&term), "ab\ncd  |\n");
    }

    #[test]
    fn execute_save_cursor_mode() {
        let mut term = Terminal::new((4, 4), None);

        term.execute(Decrst(dec_modes([DecMode::AutoWrap])));
        term.execute(Decstbm(2, 4));
        term.execute(Decset(dec_modes([DecMode::Origin])));
        term.execute(Cup(2, 3));
        term.execute(sgr(SetBoldIntensity));

        term.execute(Decset(dec_modes([DecMode::SaveCursor])));

        term.execute(Decset(dec_modes([DecMode::AutoWrap])));
        term.execute(Decrst(dec_modes([DecMode::Origin])));
        term.execute(Cup(4, 4));
        term.execute(sgr(Reset));

        term.execute(Decrst(dec_modes([DecMode::SaveCursor])));

        assert_eq!(term.cursor(), (2, 2));
        assert_eq!(term.pen.intensity, Intensity::Bold);
        assert!(term.origin_mode);
        assert!(!term.auto_wrap_mode);
    }

    #[test]
    fn execute_decstr() {
        let mut term = Terminal::new((4, 3), None);

        feed(&mut term, "ab");
        term.execute(Decset(dec_modes([DecMode::AltScreenBuffer])));
        feed(&mut term, "xy");
        term.execute(Decrst(dec_modes([DecMode::TextCursorEnable])));
        term.execute(Sm(ansi_modes([AnsiMode::Insert, AnsiMode::NewLine])));
        term.execute(Decrst(dec_modes([DecMode::AutoWrap])));
        term.execute(Decset(dec_modes([DecMode::CursorKeys])));
        term.execute(Decstbm(2, 3));
        term.execute(Decset(dec_modes([DecMode::Origin])));
        term.execute(Cup(2, 3));
        term.execute(sgr(SetBoldIntensity));
        term.execute(G1d4(Charset::Drawing));
        term.execute(So);
        term.execute(Decsc);

        assert_eq!(term.active_buffer_type(), BufferType::Alternate);
        assert_eq!(term.cursor(), (2, 2));
        assert_eq!(text(&term), "  xy\n\n  |");
        assert!(!term.cursor.visible);
        assert!(term.insert_mode);
        assert!(term.origin_mode);
        assert!(!term.auto_wrap_mode);
        assert!(term.new_line_mode);
        assert!(term.cursor_keys_app_mode());
        assert_eq!(term.top_margin, 1);
        assert_eq!(term.bottom_margin, 2);
        assert_eq!(term.charsets, [Charset::Ascii, Charset::Drawing]);
        assert_eq!(term.active_charset, 1);
        assert_eq!(term.saved_ctx.cursor_col, 2);
        assert_eq!(term.saved_ctx.cursor_row, 2);
        assert_eq!(term.pen.intensity, Intensity::Bold);

        term.execute(Decstr);

        assert_eq!(term.active_buffer_type(), BufferType::Alternate);
        assert_eq!(term.cursor(), (2, 2));
        assert_eq!(text(&term), "  xy\n\n  |");
        assert!(term.cursor.visible);
        assert!(!term.insert_mode);
        assert!(!term.origin_mode);
        assert!(!term.auto_wrap_mode);
        assert!(term.new_line_mode);
        assert!(term.cursor_keys_app_mode());
        assert_eq!(term.top_margin, 0);
        assert_eq!(term.bottom_margin, 2);
        assert_eq!(term.charsets, [Charset::Ascii, Charset::Ascii]);
        assert_eq!(term.active_charset, 0);
        assert_eq!(term.saved_ctx, super::SavedCtx::default());
        assert_eq!(term.pen, crate::pen::Pen::default());
    }

    #[test]
    fn execute_ris() {
        let mut term = Terminal::new((4, 3), None);

        feed(&mut term, "ab\r\ncd");
        term.execute(sgr(SetBoldIntensity));

        term.execute(Decset(dec_modes([
            DecMode::CursorKeys,
            DecMode::SaveCursorAltScreenBuffer,
        ])));

        feed(&mut term, "zz");

        term.execute(Ris);

        assert_eq!(term.active_buffer_type(), BufferType::Primary);
        assert_eq!(term.cursor(), (0, 0));
        assert_eq!(text(&term), "|\n\n");
        assert_eq!(term.pen, crate::pen::Pen::default());
        assert!(!term.cursor_keys_app_mode());
    }

    #[test]
    fn resize_vs_tabs() {
        let mut term = Terminal::new((6, 2), None);

        assert_eq!(term.tabs, vec![]);

        term.resize(10, 2);

        assert_eq!(term.tabs, vec![8]);

        term.resize(30, 2);

        assert_eq!(term.tabs, vec![8, 16, 24]);

        term.resize(20, 2);

        assert_eq!(term.tabs, vec![8, 16]);
    }

    #[test]
    fn resize_vs_saved_ctx() {
        use DecMode::*;

        let mut term = Terminal::new((20, 5), None);

        // move cursor forward by 15 cols
        term.execute(Cuf(15));

        assert_eq!(term.cursor.col, 15);

        // save cursor
        term.execute(Decsc);

        assert_eq!(term.saved_ctx.cursor_col, 15);

        // switch to alternate buffer
        term.execute(Decset(dec_modes([AltScreenBuffer])));

        // save cursor
        term.execute(Decsc);

        assert_eq!(term.saved_ctx.cursor_col, 15);

        // resize to 10x5
        term.resize(10, 5);

        assert_eq!(term.saved_ctx.cursor_col, 9);
    }
}
