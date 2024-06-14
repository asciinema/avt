mod cursor;
mod dirty_lines;
pub use self::cursor::Cursor;
use self::dirty_lines::DirtyLines;
use crate::buffer::{Buffer, EraseMode, NullScrollbackCollector, ScrollbackCollector};
use crate::cell::Cell;
use crate::charset::Charset;
use crate::color::Color;
use crate::dump::Dump;
use crate::line::Line;
use crate::parser::{Executor, Params};
use crate::pen::{Intensity, Pen};
use crate::saved_ctx::SavedCtx;
use crate::tabs::Tabs;
use rgb::RGB8;
use std::cmp::Ordering;

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
    next_print_wraps: bool,
    top_margin: usize,
    bottom_margin: usize,
    saved_ctx: SavedCtx,
    alternate_saved_ctx: SavedCtx,
    dirty_lines: DirtyLines,
    pub resizable: bool,
    resized: bool,
}

#[derive(Debug, PartialEq)]
enum BufferType {
    Primary,
    Alternate,
}

impl Terminal {
    pub fn new(
        (cols, rows): (usize, usize),
        scrollback_limit: Option<usize>,
        resizable: bool,
    ) -> Self {
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
            next_print_wraps: false,
            top_margin: 0,
            bottom_margin: (rows - 1),
            saved_ctx: SavedCtx::default(),
            alternate_saved_ctx: SavedCtx::default(),
            dirty_lines,
            resizable,
            resized: false,
        }
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    pub fn gc<C: ScrollbackCollector>(&mut self, sc: C) -> Result<(), C::Error> {
        match self.active_buffer_type {
            BufferType::Primary => self.buffer.gc(sc),

            BufferType::Alternate => {
                let _ = self.buffer.gc(NullScrollbackCollector);

                Ok(())
            }
        }
    }

    pub fn changes(&mut self) -> (Vec<usize>, bool) {
        let changes = (self.dirty_lines.to_vec(), self.resized);
        self.dirty_lines.clear();
        self.resized = false;

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
        self.cursor.col = col;
        self.next_print_wraps = false;
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
        self.next_print_wraps = false;
    }

    fn move_cursor_to_rel_col(&mut self, rel_col: isize) {
        let new_col = self.cursor.col as isize + rel_col;

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

    // resizing

    fn reflow(&mut self) {
        if self.cols != self.buffer.cols {
            self.next_print_wraps = false;
        }

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
        self.next_print_wraps = false;
        self.top_margin = 0;
        self.bottom_margin = self.rows - 1;
        self.saved_ctx = SavedCtx::default();
        self.alternate_saved_ctx = SavedCtx::default();
        self.dirty_lines = DirtyLines::new(self.rows);
        self.resized = false;
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

    pub fn view(&self) -> &[Line] {
        self.buffer.view()
    }

    pub fn lines(&self) -> &[Line] {
        self.buffer.lines()
    }

    pub fn line(&self, n: usize) -> &Line {
        &self.buffer[n]
    }

    pub fn text(&self) -> Vec<String> {
        self.primary_buffer().text()
    }

    #[cfg(test)]
    pub fn verify(&self) {
        assert!(self.cursor.row < self.rows);
        assert!(self.lines().iter().all(|line| line.len() == self.cols));
        assert!(!self.lines().last().unwrap().wrapped);

        assert!(
            !self.next_print_wraps && self.cursor.col < self.cols
                || self.next_print_wraps && self.cursor.col == self.cols
        );
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
        assert_eq!(self.next_print_wraps, other.next_print_wraps);
        assert_eq!(self.top_margin, other.top_margin);
        assert_eq!(self.bottom_margin, other.bottom_margin);
        assert_eq!(self.saved_ctx, other.saved_ctx);
        assert_eq!(self.alternate_saved_ctx, other.alternate_saved_ctx);
        assert_eq!(self.primary_buffer().view(), other.primary_buffer().view());

        if self.active_buffer_type == BufferType::Alternate {
            assert_eq!(
                self.alternate_buffer().view(),
                other.alternate_buffer().view()
            );
        }
    }
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new((80, 24), None, false)
    }
}

impl Executor for Terminal {
    fn print(&mut self, mut input: char) {
        input = self.charsets[self.active_charset].translate(input);
        let cell = Cell(input, self.pen);

        if self.auto_wrap_mode && self.next_print_wraps {
            self.do_move_cursor_to_col(0);

            if self.cursor.row == self.bottom_margin {
                self.buffer.wrap(self.cursor.row);
                self.scroll_up_in_region(1);
            } else if self.cursor.row < self.rows - 1 {
                self.buffer.wrap(self.cursor.row);
                self.do_move_cursor_to_row(self.cursor.row + 1);
            }
        }

        let next_col = self.cursor.col + 1;

        if next_col >= self.cols {
            self.buffer.print((self.cols - 1, self.cursor.row), cell);

            if self.auto_wrap_mode {
                self.do_move_cursor_to_col(self.cols);
                self.next_print_wraps = true;
            }
        } else {
            if self.insert_mode {
                self.buffer
                    .insert((self.cursor.col, self.cursor.row), 1, cell);
            } else {
                self.buffer.print((self.cursor.col, self.cursor.row), cell);
            }

            self.do_move_cursor_to_col(next_col);
        }

        self.dirty_lines.add(self.cursor.row);
    }

    fn bs(&mut self) {
        if self.next_print_wraps {
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
            self.do_move_cursor_to_col(0);
        }
    }

    fn cr(&mut self) {
        self.do_move_cursor_to_col(0);
    }

    fn so(&mut self) {
        self.active_charset = 1;
    }

    fn si(&mut self) {
        self.active_charset = 0;
    }

    fn nel(&mut self) {
        self.move_cursor_down_with_scroll();
        self.do_move_cursor_to_col(0);
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
                self.buffer
                    .print((col, row), Cell('\u{45}', Pen::default()));
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

    fn ich(&mut self, params: &Params) {
        self.buffer.insert(
            (self.cursor.col, self.cursor.row),
            params.get(0, 1),
            Cell::blank(self.pen),
        );

        self.dirty_lines.add(self.cursor.row);
    }

    fn cuu(&mut self, params: &Params) {
        self.cursor_up(params.get(0, 1));
    }

    fn cud(&mut self, params: &Params) {
        self.cursor_down(params.get(0, 1));
    }

    fn cuf(&mut self, params: &Params) {
        self.move_cursor_to_rel_col(params.get(0, 1) as isize);
    }

    fn cub(&mut self, params: &Params) {
        let mut rel_col = -(params.get(0, 1) as isize);

        if self.next_print_wraps {
            rel_col -= 1;
        }

        self.move_cursor_to_rel_col(rel_col);
    }

    fn cnl(&mut self, params: &Params) {
        self.cursor_down(params.get(0, 1));
        self.do_move_cursor_to_col(0);
    }

    fn cpl(&mut self, params: &Params) {
        self.cursor_up(params.get(0, 1));
        self.do_move_cursor_to_col(0);
    }

    fn cha(&mut self, params: &Params) {
        self.move_cursor_to_col(params.get(0, 1) - 1);
    }

    fn cup(&mut self, params: &Params) {
        self.move_cursor_to_col(params.get(1, 1) - 1);
        self.move_cursor_to_row(params.get(0, 1) - 1);
    }

    fn cht(&mut self, params: &Params) {
        self.move_cursor_to_next_tab(params.get(0, 1));
    }

    fn ed(&mut self, params: &Params) {
        use EraseMode::*;

        match params.get(0, 0) {
            0 => {
                self.buffer.erase(
                    (self.cursor.col, self.cursor.row),
                    FromCursorToEndOfView,
                    &self.pen,
                );

                self.dirty_lines.extend(self.cursor.row..self.rows);
            }

            1 => {
                self.buffer.erase(
                    (self.cursor.col, self.cursor.row),
                    FromStartOfViewToCursor,
                    &self.pen,
                );

                self.dirty_lines.extend(0..self.cursor.row + 1);
            }

            2 => {
                self.buffer
                    .erase((self.cursor.col, self.cursor.row), WholeView, &self.pen);

                self.dirty_lines.extend(0..self.rows);
            }

            _ => (),
        }
    }

    fn el(&mut self, params: &Params) {
        use EraseMode::*;

        match params.get(0, 0) {
            0 => {
                self.buffer.erase(
                    (self.cursor.col, self.cursor.row),
                    FromCursorToEndOfLine,
                    &self.pen,
                );

                self.dirty_lines.add(self.cursor.row);
            }

            1 => {
                self.buffer.erase(
                    (self.cursor.col, self.cursor.row),
                    FromStartOfLineToCursor,
                    &self.pen,
                );

                self.dirty_lines.add(self.cursor.row);
            }

            2 => {
                self.buffer
                    .erase((self.cursor.col, self.cursor.row), WholeLine, &self.pen);

                self.dirty_lines.add(self.cursor.row);
            }

            _ => (),
        }
    }

    fn il(&mut self, params: &Params) {
        let range = if self.cursor.row <= self.bottom_margin {
            self.cursor.row..self.bottom_margin + 1
        } else {
            self.cursor.row..self.rows
        };

        self.buffer
            .scroll_down(range.clone(), params.get(0, 1), &self.pen);

        self.dirty_lines.extend(range);
    }

    fn dl(&mut self, params: &Params) {
        let range = if self.cursor.row <= self.bottom_margin {
            self.cursor.row..self.bottom_margin + 1
        } else {
            self.cursor.row..self.rows
        };

        self.buffer
            .scroll_up(range.clone(), params.get(0, 1), &self.pen);

        self.dirty_lines.extend(range);
    }

    fn dch(&mut self, params: &Params) {
        if self.cursor.col >= self.cols {
            self.move_cursor_to_col(self.cols - 1);
        }

        self.buffer.delete(
            (self.cursor.col, self.cursor.row),
            params.get(0, 1),
            &self.pen,
        );

        self.dirty_lines.add(self.cursor.row);
    }

    fn su(&mut self, params: &Params) {
        self.scroll_up_in_region(params.get(0, 1));
    }

    fn sd(&mut self, params: &Params) {
        self.scroll_down_in_region(params.get(0, 1));
    }

    fn ctc(&mut self, params: &Params) {
        match params.get(0, 0) {
            0 => self.set_tab(),
            2 => self.clear_tab(),
            5 => self.clear_all_tabs(),
            _ => (),
        }
    }

    fn ech(&mut self, params: &Params) {
        let n = params.get(0, 1);

        self.buffer.erase(
            (self.cursor.col, self.cursor.row),
            EraseMode::NextChars(n),
            &self.pen,
        );

        self.dirty_lines.add(self.cursor.row);
    }

    fn cbt(&mut self, params: &Params) {
        self.move_cursor_to_prev_tab(params.get(0, 1));
    }

    fn rep(&mut self, params: &Params) {
        if self.cursor.col > 0 {
            let n = params.get(0, 1);
            let char = self.buffer[(self.cursor.col - 1, self.cursor.row)].0;

            for _n in 0..n {
                self.print(char);
            }
        }
    }

    fn vpa(&mut self, params: &Params) {
        self.move_cursor_to_row(params.get(0, 1) - 1);
    }

    fn vpr(&mut self, params: &Params) {
        self.cursor_down(params.get(0, 1));
    }

    fn tbc(&mut self, params: &Params) {
        match params.get(0, 0) {
            0 => self.clear_tab(),
            3 => self.clear_all_tabs(),
            _ => (),
        }
    }

    fn sm(&mut self, params: &Params) {
        for param in params.iter() {
            match param.as_slice() {
                [4] => self.insert_mode = true,
                [20] => self.new_line_mode = true,
                _ => (),
            }
        }
    }

    fn rm(&mut self, params: &Params) {
        for param in params.iter() {
            match param.as_slice() {
                [4] => self.insert_mode = false,
                [20] => self.new_line_mode = false,
                _ => (),
            }
        }
    }

    fn sgr(&mut self, params: &Params) {
        let mut ps = params.as_slice();

        while let Some(param) = ps.first() {
            match param.as_slice() {
                [0] => {
                    self.pen = Pen::default();
                    ps = &ps[1..];
                }

                [1] => {
                    self.pen.intensity = Intensity::Bold;
                    ps = &ps[1..];
                }

                [2] => {
                    self.pen.intensity = Intensity::Faint;
                    ps = &ps[1..];
                }

                [3] => {
                    self.pen.set_italic();
                    ps = &ps[1..];
                }

                [4] => {
                    self.pen.set_underline();
                    ps = &ps[1..];
                }

                [5] => {
                    self.pen.set_blink();
                    ps = &ps[1..];
                }

                [7] => {
                    self.pen.set_inverse();
                    ps = &ps[1..];
                }

                [9] => {
                    self.pen.set_strikethrough();
                    ps = &ps[1..];
                }

                [21] | [22] => {
                    self.pen.intensity = Intensity::Normal;
                    ps = &ps[1..];
                }

                [23] => {
                    self.pen.unset_italic();
                    ps = &ps[1..];
                }

                [24] => {
                    self.pen.unset_underline();
                    ps = &ps[1..];
                }

                [25] => {
                    self.pen.unset_blink();
                    ps = &ps[1..];
                }

                [27] => {
                    self.pen.unset_inverse();
                    ps = &ps[1..];
                }

                [param] if *param >= 30 && *param <= 37 => {
                    self.pen.foreground = Some(Color::Indexed((param - 30) as u8));
                    ps = &ps[1..];
                }

                [38, 2, r, g, b] => {
                    self.pen.foreground = Some(Color::RGB(RGB8::new(*r as u8, *g as u8, *b as u8)));
                    ps = &ps[1..];
                }

                [38, 5, idx] => {
                    self.pen.foreground = Some(Color::Indexed(*idx as u8));
                    ps = &ps[1..];
                }

                [38] => match ps.get(1).map(|p| p.as_slice()) {
                    None => {
                        ps = &ps[1..];
                    }

                    Some([2]) => {
                        if let Some(b) = ps.get(4) {
                            let r = ps.get(2).unwrap().as_slice()[0];
                            let g = ps.get(3).unwrap().as_slice()[0];
                            let b = b.as_slice()[0];

                            self.pen.foreground =
                                Some(Color::RGB(RGB8::new(r as u8, g as u8, b as u8)));

                            ps = &ps[5..];
                        } else {
                            ps = &ps[2..];
                        }
                    }

                    Some([5]) => {
                        if let Some(idx) = ps.get(2) {
                            let idx = idx.as_slice()[0];
                            self.pen.foreground = Some(Color::Indexed(idx as u8));
                            ps = &ps[3..];
                        } else {
                            ps = &ps[2..];
                        }
                    }

                    Some(_) => {
                        ps = &ps[1..];
                    }
                },

                [39] => {
                    self.pen.foreground = None;
                    ps = &ps[1..];
                }

                [param] if *param >= 40 && *param <= 47 => {
                    self.pen.background = Some(Color::Indexed((param - 40) as u8));
                    ps = &ps[1..];
                }

                [48, 2, r, g, b] => {
                    self.pen.background = Some(Color::RGB(RGB8::new(*r as u8, *g as u8, *b as u8)));
                    ps = &ps[1..];
                }

                [48, 5, idx] => {
                    self.pen.background = Some(Color::Indexed(*idx as u8));
                    ps = &ps[1..];
                }

                [48] => match ps.get(1).map(|p| p.as_slice()) {
                    None => {
                        ps = &ps[1..];
                    }

                    Some([2]) => {
                        if let Some(b) = ps.get(4) {
                            let r = ps.get(2).unwrap().as_slice()[0];
                            let g = ps.get(3).unwrap().as_slice()[0];
                            let b = b.as_slice()[0];

                            self.pen.background =
                                Some(Color::RGB(RGB8::new(r as u8, g as u8, b as u8)));

                            ps = &ps[5..];
                        } else {
                            ps = &ps[2..];
                        }
                    }

                    Some([5]) => {
                        if let Some(idx) = ps.get(2) {
                            let idx = idx.as_slice()[0];
                            self.pen.background = Some(Color::Indexed(idx as u8));
                            ps = &ps[3..];
                        } else {
                            ps = &ps[2..];
                        }
                    }

                    Some(_) => {
                        ps = &ps[1..];
                    }
                },

                [49] => {
                    self.pen.background = None;
                    ps = &ps[1..];
                }

                [param] if *param >= 90 && *param <= 97 => {
                    self.pen.foreground = Some(Color::Indexed((param - 90 + 8) as u8));
                    ps = &ps[1..];
                }

                [param] if *param >= 100 && *param <= 107 => {
                    self.pen.background = Some(Color::Indexed((param - 100 + 8) as u8));
                    ps = &ps[1..];
                }

                _ => {
                    ps = &ps[1..];
                }
            }
        }
    }

    fn decstbm(&mut self, params: &Params) {
        let top = params.get(0, 1) - 1;
        let bottom = params.get(1, self.rows) - 1;

        if top < bottom && bottom < self.rows {
            self.top_margin = top;
            self.bottom_margin = bottom;
        }

        self.move_cursor_home();
    }

    fn xtwinops(&mut self, params: &Params) {
        if self.resizable && params.get(0, 0) == 8 {
            let cols = params.get(2, self.cols);
            let rows = params.get(1, self.rows);

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

    fn decstr(&mut self) {
        self.soft_reset();
    }

    fn prv_sm(&mut self, params: &Params) {
        for param in params.iter() {
            match param.as_slice() {
                [6] => {
                    self.origin_mode = true;
                    self.move_cursor_home();
                }

                [7] => self.auto_wrap_mode = true,
                [25] => self.cursor.visible = true,

                [47] => {
                    self.switch_to_alternate_buffer();
                    self.reflow();
                }

                [1047] => {
                    self.switch_to_alternate_buffer();
                    self.reflow();
                }

                [1048] => self.save_cursor(),

                [1049] => {
                    self.save_cursor();
                    self.switch_to_alternate_buffer();
                    self.reflow();
                }
                _ => (),
            }
        }
    }

    fn prv_rm(&mut self, params: &Params) {
        for param in params.iter() {
            match param.as_slice() {
                [6] => {
                    self.origin_mode = false;
                    self.move_cursor_home();
                }

                [7] => self.auto_wrap_mode = false,
                [25] => self.cursor.visible = false,

                [47] => {
                    self.switch_to_primary_buffer();
                    self.reflow();
                }

                [1047] => {
                    self.switch_to_primary_buffer();
                    self.reflow();
                }

                [1048] => self.restore_cursor(),

                [1049] => {
                    self.switch_to_primary_buffer();
                    self.restore_cursor();
                    self.reflow();
                }

                _ => (),
            }
        }
    }
}

impl Dump for Terminal {
    fn dump(&self) -> String {
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
            // move cursor past right border by re-printing the character in
            // the last column
            let cell = self.buffer[(self.cols - 1, self.cursor.row)];
            seq.push_str(&format!("{}{}", cell.1.dump(), cell.0));
        }

        // configure pen
        seq.push_str(&self.pen.dump());

        if !self.cursor.visible {
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

        seq
    }
}

#[cfg(test)]
mod tests {
    use super::Terminal;
    use crate::parser::{Executor, Param, Params};
    use crate::terminal::{Color, Intensity};
    use rgb::RGB8;

    fn params<P: Into<Param> + Clone, T: AsRef<[P]>>(values: T) -> Params {
        let params: Vec<Param> = values.as_ref().iter().map(|p| (p.clone()).into()).collect();

        Params::from(params)
    }

    #[test]
    fn sgr() {
        let mut term = Terminal::default();

        term.sgr(&params([1]));

        assert!(term.pen.intensity == Intensity::Bold);

        term.sgr(&params([2]));

        assert_eq!(term.pen.intensity, Intensity::Faint);

        term.sgr(&params([3]));

        assert!(term.pen.is_italic());

        term.sgr(&params([4]));

        assert!(term.pen.is_underline());

        term.sgr(&params([5]));

        assert!(term.pen.is_blink());

        term.sgr(&params([7]));

        assert!(term.pen.is_inverse());

        term.sgr(&params([9]));

        assert!(term.pen.is_strikethrough());

        term.sgr(&params([32]));

        assert_eq!(term.pen.foreground, Some(Color::Indexed(2)));

        term.sgr(&params([43]));

        assert_eq!(term.pen.background, Some(Color::Indexed(3)));

        term.sgr(&params([93]));

        assert_eq!(term.pen.foreground, Some(Color::Indexed(11)));

        term.sgr(&params([104]));

        assert_eq!(term.pen.background, Some(Color::Indexed(12)));

        term.sgr(&params([39]));

        assert_eq!(term.pen.foreground, None);

        term.sgr(&params([49]));

        assert_eq!(term.pen.background, None);

        term.sgr(&params(vec![
            vec![1],
            vec![38, 5, 88],
            vec![48, 5, 99],
            vec![5],
        ]));

        assert_eq!(term.pen.intensity, Intensity::Bold);
        assert!(term.pen.is_blink());
        assert_eq!(term.pen.foreground, Some(Color::Indexed(88)));
        assert_eq!(term.pen.background, Some(Color::Indexed(99)));

        term.sgr(&params(vec![
            vec![38, 2, 101, 102, 103],
            vec![48, 2, 201, 202, 203],
        ]));

        assert_eq!(
            term.pen.foreground,
            Some(Color::RGB(RGB8::new(101, 102, 103)))
        );

        assert_eq!(
            term.pen.background,
            Some(Color::RGB(RGB8::new(201, 202, 203)))
        );

        term.sgr(&params([23, 24, 25, 27]));

        assert!(!term.pen.is_italic());
        assert!(!term.pen.is_underline());
        assert!(!term.pen.is_blink());
        assert!(!term.pen.is_inverse());
    }

    #[test]
    fn sgr_colon_colors() {
        let mut term = Terminal::default();

        term.sgr(&params([38, 5, 88, 48, 5, 99]));

        assert_eq!(term.pen.foreground, Some(Color::Indexed(88)));
        assert_eq!(term.pen.background, Some(Color::Indexed(99)));

        term.sgr(&params([38, 2, 101, 102, 103, 48, 2, 201, 202, 203]));

        assert_eq!(
            term.pen.foreground,
            Some(Color::RGB(RGB8::new(101, 102, 103)))
        );

        assert_eq!(
            term.pen.background,
            Some(Color::RGB(RGB8::new(201, 202, 203)))
        );
    }

    #[test]
    fn xtwinops_vs_tabs() {
        let mut term = Terminal::new((6, 2), None, true);

        assert_eq!(term.tabs, vec![]);

        term.xtwinops(&params([8, 0, 10]));

        assert_eq!(term.tabs, vec![8]);

        term.xtwinops(&params([8, 0, 30]));

        assert_eq!(term.tabs, vec![8, 16, 24]);

        term.xtwinops(&params([8, 0, 20]));

        assert_eq!(term.tabs, vec![8, 16]);
    }

    #[test]
    fn xtwinops_vs_saved_ctx() {
        let mut term = Terminal::new((20, 5), None, true);

        // move cursor to col 15
        term.cuf(&params([15]));

        assert_eq!(term.cursor.col, 15);

        // save cursor
        term.sc();

        assert_eq!(term.saved_ctx.cursor_col, 15);

        // switch to alternate buffer
        term.prv_sm(&params([47]));

        // save cursor
        term.sc();

        assert_eq!(term.saved_ctx.cursor_col, 15);

        // resize to 10x5
        term.xtwinops(&params([8, 0, 10]));

        assert_eq!(term.saved_ctx.cursor_col, 9);
    }
}
