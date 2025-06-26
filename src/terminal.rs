mod cursor;
mod dirty_lines;

pub use self::cursor::Cursor;
use self::dirty_lines::DirtyLines;
use crate::buffer::{Buffer, EraseMode};
use crate::charset::Charset;
use crate::line::Line;
use crate::parser::{
    AnsiMode, CtcOp, DecMode, EdScope, ElScope, Function, SgrOp, TbcScope, XtwinopsOp,
};
use crate::pen::{Intensity, Pen};
use crate::tabs::Tabs;
use std::cmp::Ordering;
use std::mem;

#[derive(Debug)]
pub(crate) struct Terminal {
    pub cols: usize,
    pub rows: usize,
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

#[derive(Debug, PartialEq)]
enum BufferType {
    Primary,
    Alternate,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum CursorKeysMode {
    Normal,
    Application,
}

#[derive(Debug, PartialEq)]
pub struct SavedCtx {
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
                let width = line[i].width();

                if width == 0 {
                    assert!(i > 0);
                    assert!(line[i - 1].width() == 2);
                } else if width == 2 {
                    assert!(line[i + 1].width() == 0, "{:?}", line);
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
            0
        };

        if n > 0 {
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

            self.cursor.col = self.buffer.print((0, self.cursor.row), ch, self.pen);
        } else {
            let n = self
                .buffer
                .print((self.cursor.col - 1, self.cursor.row), ch, self.pen);

            if n == 0 {
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
            let char = self.buffer[(self.cursor.col - 1, self.cursor.row)].char();

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

    fn sm(&mut self, modes: Vec<AnsiMode>) {
        use AnsiMode::*;

        for mode in modes {
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

    fn rm(&mut self, modes: Vec<AnsiMode>) {
        use AnsiMode::*;

        for mode in modes {
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

    fn sgr(&mut self, ops: Vec<SgrOp>) {
        use SgrOp::*;

        for op in ops {
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

    fn decset(&mut self, modes: Vec<DecMode>) {
        use DecMode::*;

        for mode in modes {
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

    fn decrst(&mut self, modes: Vec<DecMode>) {
        use DecMode::*;

        for mode in modes {
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

    pub fn dump(&self) -> String {
        let (primary_ctx, alternate_ctx): (&SavedCtx, &SavedCtx) = match self.active_buffer_type {
            BufferType::Primary => (&self.saved_ctx, &self.alternate_saved_ctx),
            BufferType::Alternate => (&self.alternate_saved_ctx, &self.saved_ctx),
        };

        // 1. dump primary screen buffer

        let mut seq: String = self.primary_buffer().dump();

        // 2. setup tab stops

        if self.tabs != Tabs::new(self.cols) {
            // clear all tab stops
            seq.push_str("\u{9b}5W");

            // set each tab stop
            for t in &self.tabs {
                seq.push_str(&format!("\u{9b}{}`\u{1b}[W", t + 1));
            }
        }

        // 3. configure saved context for primary screen

        if !primary_ctx.is_default() {
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
                primary_ctx.cursor_row + 1,
                primary_ctx.cursor_col + 1
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
        }

        // prevent pen bleed into alt screen buffer
        seq.push_str("\u{1b}[m");

        // 4. dump alternate screen buffer

        // switch to alternate screen
        if self.active_buffer_type == BufferType::Alternate || !alternate_ctx.is_default() {
            seq.push_str("\u{9b}?1047h");
        }

        if self.active_buffer_type == BufferType::Alternate {
            // move cursor home
            seq.push_str("\u{9b}1;1H");

            // dump alternate buffer
            seq.push_str(&self.alternate_buffer().dump());
        }

        // 5. configure saved context for alternate screen

        if !alternate_ctx.is_default() {
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
                alternate_ctx.cursor_row + 1,
                alternate_ctx.cursor_col + 1
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
        }

        // 6. ensure the right buffer is active

        if self.active_buffer_type == BufferType::Primary && !alternate_ctx.is_default() {
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
        if self.top_margin > 0 || self.bottom_margin < self.rows - 1 {
            seq.push_str(&format!(
                "\u{9b}{};{}r",
                self.top_margin + 1,
                self.bottom_margin + 1
            ));
        }

        // 9. setup cursor

        let col = self.cursor.col;
        let mut row = self.cursor.row;

        if self.origin_mode {
            if row < self.top_margin || row > self.bottom_margin {
                // bring cursor outside scroll region by restoring saved cursor
                // and moving it to desired position via CSI A/B/C/D

                seq.push_str("\u{9b}u");

                match col.cmp(&self.saved_ctx.cursor_col) {
                    Ordering::Less => {
                        let n = self.saved_ctx.cursor_col - col;
                        seq.push_str(&format!("\u{9b}{n}D"));
                    }

                    Ordering::Greater => {
                        let n = col - self.saved_ctx.cursor_col;
                        seq.push_str(&format!("\u{9b}{n}C"));
                    }

                    Ordering::Equal => (),
                }

                match row.cmp(&self.saved_ctx.cursor_row) {
                    Ordering::Less => {
                        let n = self.saved_ctx.cursor_row - row;
                        seq.push_str(&format!("\u{9b}{n}A"));
                    }

                    Ordering::Greater => {
                        let n = row - self.saved_ctx.cursor_row;
                        seq.push_str(&format!("\u{9b}{n}B"));
                    }

                    Ordering::Equal => (),
                }
            } else {
                row -= self.top_margin;
                seq.push_str(&format!("\u{9b}{};{}H", row + 1, col + 1));
            }
        } else {
            seq.push_str(&format!("\u{9b}{};{}H", row + 1, col + 1));
        }

        if self.cursor.col >= self.cols {
            // move cursor past the right border by re-printing the character in
            // the last column
            let last_cell = self.buffer[(self.cols - 1, self.cursor.row)];
            let width = last_cell.width();

            if width == 1 {
                seq.push_str(&format!("{}{}", last_cell.pen().dump(), last_cell.char()));
            } else if width == 0 {
                let prev_cell = self.buffer[(self.cols - 2, self.cursor.row)];

                seq.push_str(&format!(
                    "\u{9b}D{}{}", // move cursor back
                    prev_cell.pen().dump(),
                    prev_cell.char()
                ));
            }
        }

        // configure pen
        seq.push_str(&self.pen.dump());

        if !self.cursor.visible {
            // hide cursor
            seq.push_str("\u{9b}?25l");
        }

        // Following 3 steps must happen after ALL prints as they alter print behaviour,
        // including the "move cursor past the right border one" above.

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

        // 14. setup cursor key mode

        if self.cursor_keys_mode == CursorKeysMode::Application {
            // enable new line mode
            seq.push_str("\u{9b}?1h");
        }

        seq
    }
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
    use super::Terminal;
    use crate::color::Color;
    use crate::parser::{DecMode, Function, SgrOp};
    use crate::pen::Intensity;
    use Function::*;
    use SgrOp::*;

    fn sgr(op: SgrOp) -> Function {
        Sgr(vec![op])
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

        term.execute(Sgr(vec![
            SetBoldIntensity,
            SetForegroundColor(Color::Indexed(1)),
            SetBackgroundColor(Color::Indexed(2)),
            SetBlink,
            ResetIntensity,
        ]));

        assert_eq!(term.pen.intensity, Intensity::Normal);
        assert!(term.pen.is_blink());
        assert_eq!(term.pen.foreground, Some(Color::Indexed(1)));
        assert_eq!(term.pen.background, Some(Color::Indexed(2)));
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
        term.execute(Decset(vec![AltScreenBuffer]));

        // save cursor
        term.execute(Decsc);

        assert_eq!(term.saved_ctx.cursor_col, 15);

        // resize to 10x5
        term.resize(10, 5);

        assert_eq!(term.saved_ctx.cursor_col, 9);
    }
}
