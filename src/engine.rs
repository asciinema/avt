mod cursor;

pub use self::cursor::Cursor;
use crate::buffer::{Buffer, EraseMode};
use crate::cell::{Cell, Occupancy};
use crate::charset::Charset;
use crate::line::Line;
use crate::parser::{
    AnsiMode, AnsiModes, CtcOp, DecMode, DecModes, EdScope, ElScope, Function, SgrOp, SgrOps,
    TbcScope,
};
use crate::pen::{Intensity, Pen};
use crate::tabs::Tabs;
use crate::{Anchor, Observer, Position};
use std::cmp::Ordering;

#[derive(Debug)]
pub(crate) struct Engine {
    cols: usize,
    rows: usize,
    primary_buffer: Buffer,
    alternate_buffer: Buffer,
    active: ScreenKind,
    scrollback_limit: Option<usize>,
    cursor: Cursor,
    // Live primary position while alternate is active, separate from DECSC state.
    primary_cursor: (usize, usize),
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
    primary_saved_cursor: SavedCursorState,
    alternate_saved_cursor: SavedCursorState,
}

/// Identifies the primary or alternate screen.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ScreenKind {
    Primary,
    Alternate,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub(crate) enum CursorKeysMode {
    Normal,
    Application,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub(crate) struct SavedCursorState {
    pub cursor_col: usize,
    pub cursor_row: usize,
    pub pen: Pen,
    pub origin_mode: bool,
    pub auto_wrap_mode: bool,
}

impl Default for SavedCursorState {
    fn default() -> Self {
        SavedCursorState {
            cursor_col: 0,
            cursor_row: 0,
            pen: Pen::default(),
            origin_mode: false,
            auto_wrap_mode: true,
        }
    }
}

impl SavedCursorState {
    fn dump(&self, funs: &mut Vec<Function>) {
        if self.is_default() {
            return;
        }

        if !self.auto_wrap_mode {
            funs.push(Function::Decrst(DecModes::one(DecMode::AutoWrap)));
        }

        if self.origin_mode {
            funs.push(Function::Decset(DecModes::one(DecMode::Origin)));
        }

        funs.push(Function::Cup(
            as_u16(self.cursor_row + 1),
            as_u16(self.cursor_col + 1),
        ));

        funs.push(to_sgr(&self.pen));
        funs.push(Function::Decsc);

        if !self.auto_wrap_mode {
            funs.push(Function::Decset(DecModes::one(DecMode::AutoWrap)));
        }

        if self.origin_mode {
            funs.push(Function::Decrst(DecModes::one(DecMode::Origin)));
        }
    }

    fn is_default(&self) -> bool {
        self.cursor_col == 0
            && self.cursor_row == 0
            && self.pen.is_default()
            && !self.origin_mode
            && self.auto_wrap_mode
    }
}

/// Borrowed read-only view of a terminal screen, including visible rows and
/// any retained scrollback. Copy; methods take self and return data bound to
/// the underlying buffer lifetime.
#[derive(Debug, Clone, Copy)]
pub struct Screen<'a> {
    buffer: &'a Buffer,
}

impl<'a> Screen<'a> {
    pub fn visible_lines(self) -> impl ExactSizeIterator<Item = &'a Line> {
        self.buffer.view()
    }

    /// All lines: scrollback followed by the visible ones.
    pub fn all_lines(self) -> impl Iterator<Item = &'a Line> {
        self.buffer.lines()
    }

    pub fn line(self, n: usize) -> Option<&'a Line> {
        self.buffer.view().nth(n)
    }

    pub(crate) fn resolve(self, anchor: &Anchor) -> Option<(usize, usize)> {
        self.visible_lines()
            .enumerate()
            .find_map(|(row, line)| line.anchor_column(anchor).map(|col| (col, row)))
    }

    /// Logical text lines: scrollback included, soft-wrapped rows joined,
    /// trailing whitespace trimmed.
    pub fn text(self) -> Vec<String> {
        self.buffer.text()
    }
}

impl Engine {
    pub fn new((cols, rows): (usize, usize), scrollback_limit: Option<usize>) -> Self {
        assert!(cols > 0 && rows > 0, "terminal size must be at least 1x1");

        let primary_buffer = Buffer::new(cols, rows, scrollback_limit, Pen::default());
        let alternate_buffer = Buffer::new(cols, rows, Some(0), Pen::default());

        Engine {
            cols,
            rows,
            primary_buffer,
            alternate_buffer,
            active: ScreenKind::Primary,
            scrollback_limit,
            tabs: Tabs::new(cols),
            cursor: Cursor::default(),
            primary_cursor: (0, 0),
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
            primary_saved_cursor: SavedCursorState::default(),
            alternate_saved_cursor: SavedCursorState::default(),
        }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    pub fn active_screen_kind(&self) -> ScreenKind {
        self.active
    }

    pub fn anchor_at(&mut self, position: Position) -> Option<Anchor> {
        match position.screen {
            ScreenKind::Primary => self.primary_buffer.anchor_at(position.col, position.row),
            ScreenKind::Alternate => self.alternate_buffer.anchor_at(position.col, position.row),
        }
    }

    pub fn resolve(&self, anchor: &Anchor) -> Option<Position> {
        for (screen, contents) in [
            (ScreenKind::Primary, &self.primary_buffer),
            (ScreenKind::Alternate, &self.alternate_buffer),
        ] {
            if let Some((col, row)) = contents.resolve(anchor) {
                return Some(Position { screen, col, row });
            }
        }

        None
    }

    #[inline(always)]
    fn active_buffer(&self) -> &Buffer {
        match self.active {
            ScreenKind::Primary => &self.primary_buffer,
            ScreenKind::Alternate => &self.alternate_buffer,
        }
    }

    #[inline(always)]
    fn active_buffer_mut(&mut self) -> &mut Buffer {
        match self.active {
            ScreenKind::Primary => &mut self.primary_buffer,
            ScreenKind::Alternate => &mut self.alternate_buffer,
        }
    }

    #[inline(always)]
    fn active_saved_cursor(&self) -> &SavedCursorState {
        match self.active {
            ScreenKind::Primary => &self.primary_saved_cursor,
            ScreenKind::Alternate => &self.alternate_saved_cursor,
        }
    }

    #[inline(always)]
    fn active_saved_cursor_mut(&mut self) -> &mut SavedCursorState {
        match self.active {
            ScreenKind::Primary => &mut self.primary_saved_cursor,
            ScreenKind::Alternate => &mut self.alternate_saved_cursor,
        }
    }

    fn erase_at_cursor(&mut self, mode: EraseMode) {
        let pos = (self.cursor.col, self.cursor.row);
        let pen = self.pen;
        self.active_buffer_mut().erase(pos, mode, &pen);
    }

    pub fn execute_with(&mut self, function: &Function, observer: &mut impl Observer) {
        use Function::*;

        match *function {
            Bs => self.bs(),

            Cbt(n) => self.cbt(n),

            Cha(n) => self.cha(n),

            Cht(n) => self.cht(n),

            Cnl(n) => self.cnl(n),

            Cpl(n) => self.cpl(n),

            Cr => self.cr(),

            Ctc(ref mode) => self.ctc(mode),

            Cub(n) => self.cub(n),

            Cud(n) => self.cud(n),

            Cuf(n) => self.cuf(n),

            Cup(row, col) => self.cup(row, col),

            Cuu(n) => self.cuu(n),

            Dch(n) => self.dch(n, observer),

            Decaln => self.decaln(observer),

            Decrc => self.decrc(),

            Decrst(ref modes) => self.decrst(modes, observer),

            Decsc => self.decsc(),

            Decset(ref modes) => self.decset(modes, observer),

            Decstbm(top, bottom) => self.decstbm(top, bottom),

            Decstr => self.decstr(),

            Dl(n) => self.dl(n, observer),

            Ech(n) => self.ech(n, observer),

            Ed(ref mode) => self.ed(mode, observer),

            El(ref mode) => self.el(mode, observer),

            G1d4(charset) => self.g1d4(charset),

            Gzd4(charset) => self.gzd4(charset),

            Ht => self.ht(),

            Hts => self.hts(),

            Ich(n) => self.ich(n, observer),

            Il(n) => self.il(n, observer),

            Lf => self.lf(observer),

            Nel => self.nel(observer),

            Print(ch) => self.print(ch, observer),

            Rep(n) => self.rep(n, observer),

            Ri => self.ri(observer),

            Ris => self.ris(observer),

            Rm(ref modes) => self.rm(modes),

            Scorc => self.scorc(),

            Scosc => self.scosc(),

            Sd(n) => self.sd(n, observer),

            Sgr(ref params) => self.sgr(params),

            Si => self.si(),

            Sm(ref modes) => self.sm(modes),

            So => self.so(),

            Su(n) => self.su(n, observer),

            Tbc(ref mode) => self.tbc(mode),

            Vpa(n) => self.vpa(n),

            Vpr(n) => self.vpr(n),

            Osc(_) | Xtwinops(_) => (),
        }
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    pub fn pop_evicted(&mut self) -> Option<Line> {
        if self.active == ScreenKind::Alternate {
            while self.alternate_buffer.pop_evicted().is_some() {}
        }

        self.primary_buffer.pop_evicted()
    }

    // cursor

    fn save_cursor(&mut self) {
        let state = SavedCursorState {
            cursor_col: self.cursor.col.min(self.cols - 1),
            cursor_row: self.cursor.row,
            pen: self.pen,
            origin_mode: self.origin_mode,
            auto_wrap_mode: self.auto_wrap_mode,
        };

        *self.active_saved_cursor_mut() = state;
    }

    fn restore_cursor(&mut self) {
        let state = *self.active_saved_cursor();

        self.cursor.col = state.cursor_col;
        self.cursor.row = state.cursor_row;
        self.pen = state.pen;
        self.origin_mode = state.origin_mode;
        self.auto_wrap_mode = state.auto_wrap_mode;
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

    fn move_cursor_down_with_scroll(&mut self, observer: &mut impl Observer) {
        if self.cursor.row == self.bottom_margin {
            self.scroll_up_in_region(1, observer);
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

    fn scroll_up_in_region(&mut self, n: usize, observer: &mut impl Observer) {
        let range = self.top_margin..self.bottom_margin + 1;
        let pen = self.pen;
        self.active_buffer_mut().scroll_up(range.clone(), n, &pen);
        observer.dirty(range);
    }

    fn scroll_down_in_region(&mut self, n: usize, observer: &mut impl Observer) {
        let range = self.top_margin..self.bottom_margin + 1;
        let pen = self.pen;
        self.active_buffer_mut().scroll_down(range.clone(), n, &pen);
        observer.dirty(range);
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

    fn switch_to_alternate_buffer(&mut self, observer: &mut impl Observer) {
        if let ScreenKind::Primary = self.active {
            self.primary_cursor = (self.cursor.col, self.cursor.row);
            self.active = ScreenKind::Alternate;
            self.alternate_buffer = Buffer::new(self.cols, self.rows, Some(0), self.pen);
            observer.dirty(0..self.rows);
        }
    }

    fn switch_to_primary_buffer(&mut self, observer: &mut impl Observer) {
        if let ScreenKind::Alternate = self.active {
            self.active = ScreenKind::Primary;
            observer.dirty(0..self.rows);
        }
    }

    // resizing

    pub fn resize_with(&mut self, cols: usize, rows: usize, observer: &mut impl Observer) {
        debug_assert!(cols > 0 && rows > 0, "terminal size must be at least 1x1");

        if (cols, rows) == self.size() {
            return;
        }

        match cols.cmp(&self.cols) {
            Ordering::Less => self.tabs.contract(cols),

            Ordering::Equal => {}

            Ordering::Greater => self.tabs.expand(self.cols, cols),
        }

        if rows != self.rows {
            self.top_margin = 0;
            self.bottom_margin = rows - 1;
        }

        self.cols = cols;
        self.rows = rows;

        observer.resized(cols, rows);
        let cursor = (self.cursor.col, self.cursor.row);

        (self.cursor.col, self.cursor.row) = self.active_buffer_mut().resize(cols, rows, cursor);

        if self.active == ScreenKind::Alternate {
            self.primary_cursor = self.primary_buffer.resize(cols, rows, self.primary_cursor);
        }

        for saved in [
            &mut self.primary_saved_cursor,
            &mut self.alternate_saved_cursor,
        ] {
            saved.cursor_col = saved.cursor_col.min(cols - 1);
            saved.cursor_row = saved.cursor_row.min(rows - 1);
        }

        observer.dirty(0..rows);
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
        *self.active_saved_cursor_mut() = SavedCursorState::default();
    }

    fn hard_reset(&mut self, observer: &mut impl Observer) {
        *self = Self::new(self.size(), self.scrollback_limit);
        observer.dirty(0..self.rows);
    }

    pub fn active_screen(&self) -> Screen<'_> {
        Screen {
            buffer: self.active_buffer(),
        }
    }

    pub fn primary_screen(&self) -> Screen<'_> {
        Screen {
            buffer: &self.primary_buffer,
        }
    }

    pub fn cursor_keys_app_mode(&self) -> bool {
        self.cursor_keys_mode == CursorKeysMode::Application
    }

    #[cfg(test)]
    pub(crate) fn verify(&self) {
        assert!(self.cursor.row < self.rows);
        assert!(self.cursor.col <= self.cols);

        if self.active == ScreenKind::Alternate {
            assert!(self.primary_cursor.0 <= self.cols);
            assert!(self.primary_cursor.1 < self.rows);
        }

        for saved in [&self.primary_saved_cursor, &self.alternate_saved_cursor] {
            assert!(saved.cursor_col < self.cols);
            assert!(saved.cursor_row < self.rows);
        }

        let buffers = std::iter::once(&self.primary_buffer)
            .chain((self.active == ScreenKind::Alternate).then_some(&self.alternate_buffer));

        for buffer in buffers {
            assert_eq!(buffer.view().len(), self.rows);
            assert!(buffer.lines().all(|line| line.len() == self.cols));
            assert!(!buffer.lines().last().unwrap().wrapped);

            for line in buffer.lines() {
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
    }

    #[cfg(test)]
    pub(crate) fn assert_dump_equivalent(&self, other: &Engine) {
        assert_eq!(self.size(), other.size());
        assert_eq!(self.active, other.active);
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
        assert_eq!(self.primary_saved_cursor, other.primary_saved_cursor);
        assert_eq!(self.alternate_saved_cursor, other.alternate_saved_cursor);

        assert_eq!(
            self.primary_buffer.view().collect::<Vec<_>>(),
            other.primary_buffer.view().collect::<Vec<_>>()
        );

        if self.active == ScreenKind::Alternate {
            assert_eq!(self.primary_cursor, other.primary_cursor);

            assert_eq!(
                self.alternate_buffer.view().collect::<Vec<_>>(),
                other.alternate_buffer.view().collect::<Vec<_>>()
            );
        }
    }

    #[cfg(test)]
    pub(crate) fn assert_execution_equivalent(&self, other: &Engine) {
        self.assert_dump_equivalent(other);
        assert_eq!(self.scrollback_limit, other.scrollback_limit);

        self.primary_buffer
            .assert_execution_equivalent(&other.primary_buffer);

        self.alternate_buffer
            .assert_execution_equivalent(&other.alternate_buffer);
    }

    fn print(&mut self, mut ch: char, observer: &mut impl Observer) {
        ch = self.charsets[self.active_charset].translate(ch);

        match self.try_print(ch) {
            Some(width) => self.advance_cursor_after_print(width),

            None if self.auto_wrap_mode => self.wrap_and_print_at_line_start(ch, observer),

            None => self.print_at_line_end(ch),
        }

        observer.dirty(self.cursor.row..self.cursor.row + 1);
    }

    fn try_print(&mut self, ch: char) -> Option<usize> {
        if self.cursor.col >= self.cols {
            return None;
        }

        let pos = (self.cursor.col, self.cursor.row);
        let pen = self.pen;

        if self.insert_mode {
            self.active_buffer_mut().shift_right(pos, 1, pen);
        }

        self.active_buffer_mut().print(pos, ch, pen)
    }

    fn advance_cursor_after_print(&mut self, width: usize) {
        self.cursor.col += width;

        if self.cursor.col == self.cols && !self.auto_wrap_mode {
            self.cursor.col = self.cols - 1;
        }
    }

    fn wrap_and_print_at_line_start(&mut self, ch: char, observer: &mut impl Observer) {
        let row = self.cursor.row;
        let pen = self.pen;

        if row == self.bottom_margin {
            let range = self.top_margin..self.bottom_margin + 1;

            self.active_buffer_mut()
                .wrap_and_scroll_up(range.clone(), &pen);

            observer.dirty(range);
        } else if row < self.rows - 1 {
            self.active_buffer_mut().wrap_to_next_row(row);
            self.cursor.row += 1;
        }

        let pos = (0, self.cursor.row);

        if let Some(width) = self.active_buffer_mut().print(pos, ch, pen) {
            self.cursor.col = width;
        } else {
            self.cursor.col = 0;
        }
    }

    fn print_at_line_end(&mut self, ch: char) {
        let row = self.cursor.row;
        let pen = self.pen;

        if self.cursor.col > 0 {
            let col = self.cursor.col - 1;

            let printed = self
                .active_buffer_mut()
                .print((col, row), ch, pen)
                .is_some();

            if !printed && col > 0 {
                self.active_buffer_mut().print((col - 1, row), ch, pen);
            }
        }

        self.cursor.col = self.cols - 1;
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

    fn lf(&mut self, observer: &mut impl Observer) {
        self.move_cursor_down_with_scroll(observer);

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

    fn nel(&mut self, observer: &mut impl Observer) {
        self.move_cursor_down_with_scroll(observer);
        self.cursor.col = 0;
    }

    fn hts(&mut self) {
        self.set_tab();
    }

    fn ri(&mut self, observer: &mut impl Observer) {
        if self.cursor.row == self.top_margin {
            self.scroll_down_in_region(1, observer);
        } else if self.cursor.row > 0 {
            self.move_cursor_to_row(self.cursor.row - 1);
        }
    }

    fn decsc(&mut self) {
        self.save_cursor();
    }

    fn decrc(&mut self) {
        self.restore_cursor();
    }

    fn ris(&mut self, observer: &mut impl Observer) {
        self.hard_reset(observer);
    }

    fn decaln(&mut self, observer: &mut impl Observer) {
        for row in 0..self.rows {
            for col in 0..self.cols {
                self.active_buffer_mut()
                    .print((col, row), '\u{45}', Pen::default());
            }

            observer.dirty(row..row + 1);
        }
    }

    fn gzd4(&mut self, charset: Charset) {
        self.charsets[0] = charset;
    }

    fn g1d4(&mut self, charset: Charset) {
        self.charsets[1] = charset;
    }

    fn ich(&mut self, n: u16, observer: &mut impl Observer) {
        if self.cursor.col == self.cols {
            self.cursor.col = self.cols - 1;
        }

        let n = as_usize(n, 1).min(self.cols - self.cursor.col);

        let pos = (self.cursor.col, self.cursor.row);
        let row = self.cursor.row;
        let pen = self.pen;

        self.active_buffer_mut().shift_right(pos, n, pen);

        for col in pos.0..pos.0 + n {
            self.active_buffer_mut().print((col, row), ' ', pen);
        }

        observer.dirty(self.cursor.row..self.cursor.row + 1);
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

    fn ed(&mut self, scope: &EdScope, observer: &mut impl Observer) {
        match scope {
            EdScope::Below => {
                self.erase_at_cursor(EraseMode::FromCursorToEndOfView);

                observer.dirty(self.cursor.row..self.rows);
            }

            EdScope::Above => {
                self.erase_at_cursor(EraseMode::FromStartOfViewToCursor);

                observer.dirty(0..self.cursor.row + 1);
            }

            EdScope::All => {
                if self.active == ScreenKind::Primary {
                    self.primary_buffer.scroll_out(&self.pen);
                }

                self.erase_at_cursor(EraseMode::WholeView);

                observer.dirty(0..self.rows);
            }

            _ => {}
        }
    }

    fn el(&mut self, scope: &ElScope, observer: &mut impl Observer) {
        match scope {
            ElScope::ToRight => {
                self.erase_at_cursor(EraseMode::FromCursorToEndOfLine);

                observer.dirty(self.cursor.row..self.cursor.row + 1);
            }

            ElScope::ToLeft => {
                self.erase_at_cursor(EraseMode::FromStartOfLineToCursor);

                observer.dirty(self.cursor.row..self.cursor.row + 1);
            }

            ElScope::All => {
                self.erase_at_cursor(EraseMode::WholeLine);

                observer.dirty(self.cursor.row..self.cursor.row + 1);
            }
        }
    }

    fn il(&mut self, n: u16, observer: &mut impl Observer) {
        let range = if self.cursor.row <= self.bottom_margin {
            self.cursor.row..self.bottom_margin + 1
        } else {
            self.cursor.row..self.rows
        };

        let pen = self.pen;

        self.active_buffer_mut()
            .scroll_down(range.clone(), as_usize(n, 1), &pen);

        observer.dirty(range);
    }

    fn dl(&mut self, n: u16, observer: &mut impl Observer) {
        let range = if self.cursor.row <= self.bottom_margin {
            self.cursor.row..self.bottom_margin + 1
        } else {
            self.cursor.row..self.rows
        };

        let pen = self.pen;

        self.active_buffer_mut()
            .scroll_up(range.clone(), as_usize(n, 1), &pen);

        observer.dirty(range);
    }

    fn dch(&mut self, n: u16, observer: &mut impl Observer) {
        if self.cursor.col >= self.cols {
            self.move_cursor_to_col(self.cols - 1);
        }

        let pos = (self.cursor.col, self.cursor.row);
        let pen = self.pen;

        self.active_buffer_mut().delete(pos, as_usize(n, 1), &pen);

        observer.dirty(self.cursor.row..self.cursor.row + 1);
    }

    fn su(&mut self, n: u16, observer: &mut impl Observer) {
        self.scroll_up_in_region(as_usize(n, 1), observer);
    }

    fn sd(&mut self, n: u16, observer: &mut impl Observer) {
        self.scroll_down_in_region(as_usize(n, 1), observer);
    }

    fn ctc(&mut self, op: &CtcOp) {
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

    fn ech(&mut self, n: u16, observer: &mut impl Observer) {
        let n = as_usize(n, 1);

        self.erase_at_cursor(EraseMode::NextChars(n));

        observer.dirty(self.cursor.row..self.cursor.row + 1);
    }

    fn cbt(&mut self, n: u16) {
        self.move_cursor_to_prev_tab(as_usize(n, 1));
    }

    fn rep(&mut self, n: u16, observer: &mut impl Observer) {
        if self.cursor.col > 0 {
            let n = as_usize(n, 1);
            let row = self.cursor.row;
            let mut col = self.cursor.col - 1;

            while col > 0 && self.active_buffer()[(col, row)].occupancy() == Occupancy::WideTail {
                col -= 1;
            }

            let char = self.active_buffer()[(col, row)].char();

            for _n in 0..n {
                self.print(char, observer);
            }
        }
    }

    fn vpa(&mut self, n: u16) {
        self.move_cursor_to_row(as_usize(n, 1) - 1);
    }

    fn vpr(&mut self, n: u16) {
        self.cursor_down(as_usize(n, 1));
    }

    fn tbc(&mut self, scope: &TbcScope) {
        match scope {
            TbcScope::CurrentColumn => {
                self.clear_tab();
            }

            TbcScope::All => {
                self.clear_all_tabs();
            }
        }
    }

    fn sm(&mut self, modes: &AnsiModes) {
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

    fn rm(&mut self, modes: &AnsiModes) {
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

    fn sgr(&mut self, ops: &SgrOps) {
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

    fn scosc(&mut self) {
        self.save_cursor();
    }

    fn scorc(&mut self) {
        self.restore_cursor();
    }

    fn decstr(&mut self) {
        self.soft_reset();
    }

    fn decset(&mut self, modes: &DecModes, observer: &mut impl Observer) {
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
                    self.switch_to_alternate_buffer(observer);
                }

                SaveCursor => {
                    self.save_cursor();
                }

                SaveCursorAltScreenBuffer => {
                    self.save_cursor();
                    self.switch_to_alternate_buffer(observer);
                }
            }
        }
    }

    fn decrst(&mut self, modes: &DecModes, observer: &mut impl Observer) {
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
                    self.switch_to_primary_buffer(observer);
                }

                SaveCursor => {
                    self.restore_cursor();
                }

                SaveCursorAltScreenBuffer => {
                    self.switch_to_primary_buffer(observer);
                    self.restore_cursor();
                }
            }
        }
    }

    pub fn dump(&self) -> Vec<Function> {
        let primary_ctx = &self.primary_saved_cursor;
        let alternate_ctx = &self.alternate_saved_cursor;

        // 1. dump primary screen buffer

        let mut funs = Vec::new();
        dump_buffer(&self.primary_buffer, &mut funs);

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

        // 3. configure the primary saved cursor while its buffer is active

        primary_ctx.dump(&mut funs);

        if self.active == ScreenKind::Alternate {
            let (col, row) = self.primary_cursor;

            funs.push(Function::Cup(as_u16(row + 1), as_u16(col + 1)));
            dump_pending_wrap(&self.primary_buffer, col, row, self.cols, &mut funs);
        }

        // Reset the pen before switching: the alternate buffer is created with it.
        funs.push(to_sgr(&Pen::default()));

        // 4. dump alternate screen buffer

        // switch to alternate screen
        if self.active == ScreenKind::Alternate || !alternate_ctx.is_default() {
            funs.push(Function::Decset(DecModes::one(DecMode::AltScreenBuffer)));
        }

        if self.active == ScreenKind::Alternate {
            // move cursor home
            funs.push(Function::Cup(1, 1));

            // dump alternate buffer
            dump_buffer(&self.alternate_buffer, &mut funs);
        }

        // 5. configure the alternate saved cursor while its buffer is active

        alternate_ctx.dump(&mut funs);

        // 6. ensure the right buffer is active

        if self.active == ScreenKind::Primary && !alternate_ctx.is_default() {
            // switch back to primary screen
            funs.push(Function::Decrst(DecModes::one(DecMode::AltScreenBuffer)));
        }

        // 7. setup origin mode

        if self.origin_mode {
            // Enable origin mode before restoring the cursor: this moves it home.
            funs.push(Function::Decset(DecModes::one(DecMode::Origin)));
        }

        // 8. setup margins

        // Setting margins moves the cursor home, so restore its position afterward.
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
                // CUP is constrained by origin mode. Restore the saved cursor,
                // then use relative movement to reach a position outside the margins.

                funs.push(Function::Scorc);

                match col.cmp(&self.active_saved_cursor().cursor_col) {
                    Ordering::Less => {
                        let n = self.active_saved_cursor().cursor_col - col;
                        funs.push(Function::Cub(as_u16(n)));
                    }

                    Ordering::Greater => {
                        let n = col - self.active_saved_cursor().cursor_col;
                        funs.push(Function::Cuf(as_u16(n)));
                    }

                    Ordering::Equal => (),
                }

                match row.cmp(&self.active_saved_cursor().cursor_row) {
                    Ordering::Less => {
                        let n = self.active_saved_cursor().cursor_row - row;
                        funs.push(Function::Cuu(as_u16(n)));
                    }

                    Ordering::Greater => {
                        let n = row - self.active_saved_cursor().cursor_row;
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

        dump_pending_wrap(
            self.active_buffer(),
            self.cursor.col,
            self.cursor.row,
            self.cols,
            &mut funs,
        );

        // configure pen
        funs.push(to_sgr(&self.pen));

        if !self.cursor.visible {
            // hide cursor
            funs.push(Function::Decrst(DecModes::one(DecMode::TextCursorEnable)));
        }

        // Restore charset, insert mode, and auto-wrap only after all printing,
        // including the character reprinted above to restore pending wrap.

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

fn dump_pending_wrap(
    buffer: &Buffer,
    col: usize,
    row: usize,
    cols: usize,
    funs: &mut Vec<Function>,
) {
    if col < cols {
        return;
    }

    // Restore pending wrap by reprinting the character at the right edge.
    let last_cell = buffer[(cols - 1, row)];
    let occupancy = last_cell.occupancy();

    if occupancy == Occupancy::Single {
        funs.push(to_sgr(last_cell.pen()));
        funs.push(Function::Print(last_cell.char()));
    } else if occupancy == Occupancy::WideTail {
        let prev_cell = buffer[(cols - 2, row)];

        funs.push(Function::Cub(1));
        funs.push(to_sgr(prev_cell.pen()));
        funs.push(Function::Print(prev_cell.char()));
    }
}

fn dump_buffer(buffer: &Buffer, funs: &mut Vec<Function>) {
    let cutoff = buffer.occupied_prefix_len(false);
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
    if value == 0 { default } else { value as usize }
}

#[cfg(test)]
mod tests {
    use super::{Engine, ScreenKind};
    use crate::Terminal;
    use crate::charset::Charset;
    use crate::color::Color;
    use crate::line::Line;
    use crate::observer::DirtyRows;
    use crate::parser::{
        AnsiMode, AnsiModes, DecMode, DecModes, EdScope, ElScope, Function, SgrOp, SgrOps,
    };
    use crate::pen::Intensity;
    use crate::pen::Pen;
    use Function::*;
    use SgrOp::*;

    fn build_term(cols: usize, rows: usize, cx: usize, cy: usize, init: &str) -> Terminal {
        let mut term = Terminal::with_scrollback(cols, rows, None).unwrap();
        term.feed(init);
        term.execute(Cup((cy + 1) as u16, (cx + 1) as u16));

        term
    }

    #[test]
    fn primary_reflows_while_alternate_is_active() {
        let mut terminal = Terminal::new(4, 2).unwrap();
        terminal.feed(["abcd\x1b[H", "\x1b[?1047h"]);
        terminal.resize(2, 2);
        assert_eq!(terminal.size(), (2, 2));
        assert_eq!(terminal.primary_screen().line(0).unwrap().text(), "ab");
        assert_eq!(terminal.primary_screen().line(1).unwrap().text(), "cd");
        terminal.feed("\x1b[?1047l");
        assert_eq!(terminal.screen().line(0).unwrap().text(), "ab");
        assert_eq!(terminal.screen().line(1).unwrap().text(), "cd");
    }

    #[test]
    fn hidden_primary_resize_uses_its_live_cursor() {
        for mode in [1047, 1049] {
            let mut terminal = Terminal::new(4, 3).unwrap();
            terminal.feed("abcd\r\nefgh\r\nijkl\x1b7\x1b[H");
            terminal.feed(format!("\x1b[?{mode}h\x1b[3;1H"));
            terminal.resize(4, 1);
            terminal.resize(4, 3);
            assert_eq!(terminal.primary_screen().text(), ["abcd", "", ""]);
            terminal.feed(format!("\x1b[3;1H\x1b[?{mode}l"));
            assert_eq!(terminal.screen().text(), ["abcd", "", ""]);

            // 1047 carries the alternate cursor back; 1049 restores the saved one.
            assert_eq!(terminal.cursor().row, if mode == 1047 { 2 } else { 0 });
        }
    }

    #[test]
    fn resizing_a_wide_character_to_one_column_erases_it() {
        for cols in [0, 1] {
            let mut terminal = Terminal::new(2, 2).unwrap();
            terminal.feed("界");
            terminal.resize(cols, 2);
            terminal.engine.verify();
            assert_eq!(terminal.size(), (1, 2));
            assert_eq!(terminal.screen().text(), ["", ""]);
            terminal.feed("a");
            assert_eq!(terminal.screen().line(0).unwrap().text(), "a");
        }
    }

    #[test]
    fn alternate_saved_cursor_persists_across_sessions() {
        let mut terminal = Terminal::new(8, 4).unwrap();

        terminal.feed([
            "\x1b[?1047h",
            "\x1b[2;3H\x1b7",
            "\x1b[?1047l",
            "\x1b[?1047h",
            "\x1b8",
        ]);

        assert_eq!(terminal.cursor().position(), Some((2, 1)));
    }

    #[test]
    fn alternate_buffer_starts_blank_on_each_entry() {
        let mut terminal = Terminal::new(8, 2).unwrap();

        terminal.feed(["\x1b[?1047h", "hello", "\x1b[?1047l", "\x1b[41m\x1b[?1047h"]);

        assert_eq!(terminal.screen().line(0).unwrap().text().trim_end(), "");
        let pen = *terminal.screen().line(0).unwrap().cells()[0].pen();
        assert_eq!(pen.background(), Some(crate::Color::Indexed(1)));
    }

    #[test]
    fn reselecting_active_buffer_preserves_content_and_dirty_state() {
        for mode in [1047, 1049] {
            let mut terminal = Terminal::new(8, 2).unwrap();
            let mut dirty = DirtyRows::new();
            terminal.feed_with("", &mut dirty);
            let enter = format!("\x1b[?{mode}h");
            let leave = format!("\x1b[?{mode}l");
            assert_eq!(terminal.feed_with(&enter, &mut dirty), [0, 1]);
            terminal.feed_with("hello", &mut dirty);
            assert!(terminal.feed_with(&enter, &mut dirty).is_empty());

            assert_eq!(
                terminal.screen().line(0).unwrap().text().trim_end(),
                "hello"
            );

            if mode == 1049 {
                terminal.feed_with("\x1b[2;1H\x1b8", &mut dirty);
                assert_eq!(terminal.cursor().position(), Some((5, 0)));
            }

            assert_eq!(terminal.feed_with(&leave, &mut dirty), [0, 1]);
            terminal.feed_with("\x1b[2;1H", &mut dirty);
            assert!(terminal.feed_with(&leave, &mut dirty).is_empty());

            if mode == 1049 {
                assert_eq!(terminal.cursor().position(), Some((0, 0)));
            }
        }
    }

    fn text(term: &Terminal) -> String {
        let cursor = term.cursor();

        buffer_text(term.screen().visible_lines(), cursor.col, cursor.row)
    }

    #[test]
    fn ed2_archives_rows_and_anchors_preserving_cursor_and_margins() {
        let mut terminal = Terminal::with_scrollback(8, 6, None).unwrap();
        terminal.feed("one\r\n\r\nthree\x1b[2;4r\x1b[?6h\x1b[2;5H\x1b[?25l\x1b7");
        let cursor = terminal.cursor();

        let occupied = terminal
            .anchor_at(crate::Position {
                screen: crate::ScreenKind::Primary,
                col: 1,
                row: 2,
            })
            .unwrap();

        let blank = terminal
            .anchor_at(crate::Position {
                screen: crate::ScreenKind::Primary,
                col: 0,
                row: 5,
            })
            .unwrap();

        let addresses: Vec<_> = terminal
            .screen()
            .visible_lines()
            .take(3)
            .map(|line| line.cells().as_ptr())
            .collect();

        let mut dirty = DirtyRows::new();
        terminal.feed_with("", &mut dirty);

        assert_eq!(
            terminal.feed_with("\x1b[2J", &mut dirty),
            [0, 1, 2, 3, 4, 5]
        );

        assert_eq!(terminal.cursor(), cursor);
        assert_eq!(terminal.primary_screen().all_lines().count(), 9);

        assert_eq!(
            terminal
                .primary_screen()
                .all_lines()
                .take(3)
                .map(|line| line.cells().as_ptr())
                .collect::<Vec<_>>(),
            addresses
        );

        assert_eq!(
            terminal.screen().text(),
            ["one", "", "three", "", "", "", "", "", ""]
        );

        assert!(
            terminal
                .screen()
                .visible_lines()
                .all(|line| line.is_blank())
        );

        assert_eq!(terminal.resolve(&occupied).unwrap().row, -1);
        assert!(terminal.resolve(&blank).is_none());
        terminal.feed("\x1b[2J");
        assert_eq!(terminal.primary_screen().all_lines().count(), 9);
        terminal.feed("\x1b[H\x1b8");
        assert_eq!(terminal.cursor(), cursor);
        terminal.feed("\x1b[H");
        assert_eq!(terminal.cursor().row, 1); // Origin mode still uses the top margin.
        terminal.feed("\x1b[99B");
        assert_eq!(terminal.cursor().row, 3); // The bottom margin also survives.
    }

    #[test]
    fn ed2_counts_styled_blanks_but_not_default_spaces_as_occupied() {
        for style in ["0", "31", "41", "1", "2", "3", "4", "5", "7", "9"] {
            let mut terminal = Terminal::with_scrollback(6, 5, None).unwrap();
            terminal.feed(format!("one\x1b[4;1H\x1b[{style}m \x1b[0m\x1b[2J"));
            let preserved = if style == "0" { 1 } else { 4 };
            assert_eq!(terminal.primary_screen().all_lines().count(), 5 + preserved);

            assert!(
                terminal
                    .screen()
                    .visible_lines()
                    .all(|line| line.is_blank())
            );

            if style != "0" {
                let line = terminal.primary_screen().all_lines().nth(3).unwrap();
                assert!(!line.cells()[0].pen().is_default());
                assert_eq!(line.text(), "      ");
            }
        }
    }

    #[test]
    fn ed2_uses_erase_pen_and_skips_default_blank_screens() {
        let mut terminal = Terminal::with_scrollback(6, 3, None).unwrap();
        terminal.feed("\x1b[2J\x1b[2J\x1b[41m\x1b[2J");
        assert_eq!(terminal.primary_screen().all_lines().count(), 3);

        assert!(terminal.screen().visible_lines().all(|line| {
            line.cells()
                .iter()
                .all(|cell| cell.pen().background() == Some(crate::Color::Indexed(1)))
        }));

        terminal.feed("\x1b[0m\x1b[2J\x1b[2J");
        assert_eq!(terminal.primary_screen().all_lines().count(), 6);

        assert!(
            terminal
                .screen()
                .visible_lines()
                .all(|line| line.is_blank())
        );
    }

    #[test]
    fn ed2_preserves_pending_wrap() {
        let mut terminal = Terminal::with_scrollback(4, 2, None).unwrap();
        terminal.feed("abcd");
        let cursor = terminal.cursor();
        assert_eq!(cursor.col, 4);
        terminal.feed("\x1b[2J");
        assert_eq!(terminal.cursor(), cursor);
        terminal.feed("X");
        assert_eq!(terminal.cursor().position(), Some((1, 1)));
        assert_eq!(terminal.screen().line(0).unwrap().text(), "    ");
        assert_eq!(terminal.screen().line(1).unwrap().text(), "X   ");
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

        let mut cells = cursor_line.cells().iter().filter(|c| c.width() != 0);

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
        term.screen()
            .visible_lines()
            .map(Line::is_wrapped)
            .collect()
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
        let mut term = Engine::new((1, 1), Some(0));

        term.execute_with(&sgr(SetBoldIntensity), &mut ());

        assert!(term.pen.intensity == Intensity::Bold);

        term.execute_with(&sgr(SetFaintIntensity), &mut ());

        assert_eq!(term.pen.intensity, Intensity::Faint);

        term.execute_with(&sgr(SetItalic), &mut ());

        assert!(term.pen.is_italic());

        term.execute_with(&sgr(SetUnderline), &mut ());

        assert!(term.pen.is_underline());

        term.execute_with(&sgr(SetBlink), &mut ());

        assert!(term.pen.is_blink());

        term.execute_with(&sgr(SetInverse), &mut ());

        assert!(term.pen.is_inverse());

        term.execute_with(&sgr(SetStrikethrough), &mut ());

        assert!(term.pen.is_strikethrough());

        term.execute_with(&sgr(SetForegroundColor(Color::Indexed(1))), &mut ());

        assert_eq!(term.pen.foreground, Some(Color::Indexed(1)));

        term.execute_with(&sgr(SetBackgroundColor(Color::Indexed(2))), &mut ());

        assert_eq!(term.pen.background, Some(Color::Indexed(2)));

        term.execute_with(&sgr(ResetForegroundColor), &mut ());

        assert_eq!(term.pen.foreground, None);

        term.execute_with(&sgr(ResetBackgroundColor), &mut ());

        assert_eq!(term.pen.background, None);

        term.execute_with(
            &Sgr(SgrOps::from(vec![
                SetBoldIntensity,
                SetForegroundColor(Color::Indexed(1)),
                SetBackgroundColor(Color::Indexed(2)),
                SetBlink,
                ResetIntensity,
            ])),
            &mut (),
        );

        assert_eq!(term.pen.intensity, Intensity::Normal);
        assert!(term.pen.is_blink());
        assert_eq!(term.pen.foreground, Some(Color::Indexed(1)));
        assert_eq!(term.pen.background, Some(Color::Indexed(2)));
    }

    #[test]
    fn execute_lf() {
        let mut term = build_term(8, 2, 3, 0, "abc");

        term.execute(Lf);

        assert_eq!(term.cursor().position().unwrap(), (3, 1));
        assert_eq!(text(&term), "abc\n   |");

        term.execute([Print('d'), Lf]);

        assert_eq!(term.cursor().position().unwrap(), (4, 1));
        assert_eq!(text(&term), "   d\n    |");
    }

    #[test]
    fn execute_cr() {
        let mut term = Terminal::with_scrollback(4, 2, None).unwrap();

        term.feed("abc");
        term.execute([Cr, Print('d')]);

        assert_eq!(term.cursor().position().unwrap(), (1, 0));
        assert_eq!(text(&term), "d|bc\n");
    }

    #[test]
    fn execute_nel() {
        let mut term = build_term(8, 2, 3, 0, "abc");

        term.execute(Nel);

        assert_eq!(term.cursor().position().unwrap(), (0, 1));
        assert_eq!(text(&term), "abc\n|");

        term.execute(Print('d'));

        assert_eq!(text(&term), "abc\nd|");
    }

    #[test]
    fn execute_bs() {
        let mut term = Terminal::with_scrollback(4, 2, None).unwrap();

        term.feed("a");
        term.execute(Bs);

        assert_eq!(text(&term), "|a\n");

        term.execute(Bs);

        assert_eq!(text(&term), "|a\n");

        term.feed("abcd");
        term.execute(Bs);

        assert_eq!(text(&term), "ab|cd\n");

        term.feed("cdef");
        term.execute(Bs);

        assert_eq!(text(&term), "abcd\ne|f");

        term.execute(Bs);

        assert_eq!(text(&term), "abcd\n|ef");

        term.execute(Bs);

        assert_eq!(text(&term), "abcd\n|ef");
    }

    #[test]
    fn execute_cup() {
        let mut term = Terminal::with_scrollback(4, 2, None).unwrap();

        term.feed("abc\r\ndef");
        term.execute(Cup(1, 1));

        assert_eq!(term.cursor().position().unwrap(), (0, 0));

        term.execute(Cup(10, 10));

        assert_eq!(term.cursor().position().unwrap(), (3, 1));
    }

    #[test]
    fn execute_cuu() {
        let mut term = Terminal::with_scrollback(8, 4, None).unwrap();

        term.feed("abcd\n\n\n");
        term.execute(Cuu(0));

        assert_eq!(term.cursor().position().unwrap(), (4, 2));

        term.execute(Cuu(2));

        assert_eq!(term.cursor().position().unwrap(), (4, 0));
    }

    #[test]
    fn execute_cpl() {
        let mut term = Terminal::with_scrollback(8, 4, None).unwrap();

        term.feed("abcd\r\n\r\n\r\nef");

        assert_eq!(term.cursor().position().unwrap(), (2, 3));

        term.execute(Cpl(0));

        assert_eq!(term.cursor().position().unwrap(), (0, 2));

        term.execute(Cpl(2));

        assert_eq!(term.cursor().position().unwrap(), (0, 0));
    }

    #[test]
    fn execute_cnl() {
        let mut term = Terminal::with_scrollback(4, 4, None).unwrap();

        term.feed("ab");
        term.execute(Cnl(0));

        assert_eq!(term.cursor().position().unwrap(), (0, 1));

        term.execute(Cnl(3));

        assert_eq!(term.cursor().position().unwrap(), (0, 3));
    }

    #[test]
    fn execute_vpa() {
        let mut term = Terminal::with_scrollback(4, 4, None).unwrap();

        term.feed("\r\n\r\naaa\r\nbbb");
        term.execute(Vpa(0));

        assert_eq!(term.cursor().position().unwrap(), (3, 0));

        term.execute(Vpa(10));

        assert_eq!(term.cursor().position().unwrap(), (3, 3));
    }

    #[test]
    fn execute_cud() {
        let mut term = Terminal::with_scrollback(8, 4, None).unwrap();

        term.feed("abcd");
        term.execute(Cud(0));

        assert_eq!(text(&term), "abcd\n    |\n\n");

        term.execute(Cud(2));

        assert_eq!(text(&term), "abcd\n\n\n    |");
    }

    #[test]
    fn execute_cuf() {
        let mut term = Terminal::with_scrollback(4, 1, None).unwrap();

        term.execute(Cuf(2));

        assert_eq!(text(&term), "  |");

        term.execute(Cuf(2));

        assert_eq!(text(&term), "   |");

        term.execute(Print('a'));

        assert_eq!(text(&term), "   a|");

        term.execute(Cuf(5));

        assert_eq!(text(&term), "   |a");

        term.feed("ab");
        term.execute(Cuf(10));

        assert_eq!(text(&term), "b  |");
    }

    #[test]
    fn execute_cha() {
        let mut term = Terminal::with_scrollback(8, 2, None).unwrap();

        term.feed("abc");
        term.execute(Cha(0));

        assert_eq!(term.cursor().position().unwrap(), (0, 0));

        term.execute(Cha(3));

        assert_eq!(term.cursor().position().unwrap(), (2, 0));

        term.execute(Cha(20));

        assert_eq!(term.cursor().position().unwrap(), (7, 0));
    }

    #[test]
    fn execute_cub() {
        let mut term = Terminal::with_scrollback(8, 2, None).unwrap();

        term.feed("abcd");
        term.execute(Cub(2));

        assert_eq!(text(&term), "ab|cd\n");

        term.feed("cdef");
        term.execute(Cub(2));

        assert_eq!(text(&term), "abcd|ef\n");

        term.execute(Cub(10));

        assert_eq!(text(&term), "|abcdef\n");

        let mut term = Terminal::with_scrollback(4, 2, None).unwrap();

        term.feed("abcd");
        term.execute(Cub(0));

        assert_eq!(text(&term), "ab|cd\n");
    }

    #[test]
    fn execute_ht() {
        let mut term = Terminal::with_scrollback(20, 1, None).unwrap();

        term.execute(Ht);
        assert_eq!(term.cursor().position().unwrap(), (8, 0));

        term.execute(Ht);
        assert_eq!(term.cursor().position().unwrap(), (16, 0));

        term.execute(Ht);
        assert_eq!(term.cursor().position().unwrap(), (19, 0));
    }

    #[test]
    fn execute_hts() {
        let mut term = Engine::new((20, 1), None);

        term.execute_with(&Cuf(5), &mut ());
        term.execute_with(&Hts, &mut ());

        assert_eq!(term.tabs, vec![5, 8, 16]);

        term.execute_with(&Cup(1, 1), &mut ());
        term.execute_with(&Ht, &mut ());

        assert_eq!(term.cursor().position().unwrap(), (5, 0));
    }

    #[test]
    fn execute_cht() {
        let mut term = build_term(28, 1, 3, 0, "abcdefghijklmnopqrstuwxyzabc");

        term.execute(Cht(0));

        assert_eq!(term.cursor().position().unwrap(), (8, 0));

        term.execute(Cht(2));

        assert_eq!(term.cursor().position().unwrap(), (24, 0));

        term.execute(Cht(0));

        assert_eq!(term.cursor().position().unwrap(), (27, 0));
    }

    #[test]
    fn execute_cbt() {
        let mut term = build_term(28, 1, 26, 0, "abcdefghijklmnopqrstuwxyzabc");

        term.execute(Cbt(0));

        assert_eq!(term.cursor().position().unwrap(), (24, 0));

        term.execute(Cbt(2));

        assert_eq!(term.cursor().position().unwrap(), (8, 0));

        term.execute(Cbt(0));

        assert_eq!(term.cursor().position().unwrap(), (0, 0));
    }

    #[test]
    fn execute_rep() {
        let mut term = build_term(20, 2, 0, 0, "");

        term.execute(Rep(0));

        assert_eq!(text(&term), "|\n");

        term.execute([Print('A'), Rep(0)]);

        assert_eq!(text(&term), "AA|\n");

        term.execute(Rep(3));

        assert_eq!(text(&term), "AAAAA|\n");

        term.execute([Cuf(5), Rep(0)]);

        assert_eq!(text(&term), "AAAAA      |\n");
    }

    #[test]
    fn execute_rep_after_wide_char() {
        let mut term = build_term(20, 2, 0, 0, "");

        term.execute([Print('ハ'), Rep(3)]);

        assert_eq!(text(&term), "ハハハハ|\n");
    }

    #[test]
    fn execute_rep_after_moving_into_wide_char_tail() {
        let mut term = build_term(20, 2, 0, 0, "");

        term.execute([Print('ハ'), Cub(1), Rep(3)]);

        assert_eq!(text(&term), " ハハハ|\n");
    }

    #[test]
    fn dump_uses_rep_for_wide_char_runs() {
        let mut term = Engine::new((20, 2), None);

        for ch in "ハハハハハハ".chars() {
            term.execute_with(&Print(ch), &mut ());
        }

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
        let mut term = Terminal::with_scrollback(4, 4, None).unwrap();

        term.feed("ab");
        term.execute(Vpr(0));

        assert_eq!(term.cursor().position().unwrap(), (2, 1));
        assert_eq!(text(&term), "ab\n  |\n\n");

        term.execute(Vpr(10));

        assert_eq!(term.cursor().position().unwrap(), (2, 3));
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

        term.execute([Cuu(1), Il(0)]);

        assert_eq!(text(&term), "  |\nabcd\n\nefgh");
        assert_eq!(wrapped(&term), vec![false, false, false, false]);

        term.execute([Cud(3), Il(100)]);

        assert_eq!(text(&term), "\nabcd\n\n  |");
    }

    #[test]
    fn execute_dl() {
        let mut term = Terminal::with_scrollback(4, 4, None).unwrap();

        term.feed("abcdefghijklmn");
        term.execute([Cuu(2), Dl(0)]);

        assert_eq!(text(&term), "abcd\nij|kl\nmn\n");
        assert_eq!(wrapped(&term), vec![false, true, false, false]);

        let mut term = Terminal::with_scrollback(4, 4, None).unwrap();

        term.feed("abcdefghijklmn");
        term.execute([Decstbm(1, 2), Cup(4, 1), Dl(0)]);

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

        let mut term = Terminal::with_scrollback(4, 3, None).unwrap();

        term.feed("abcdefghij");
        term.execute([Cuu(1), El(ElScope::ToRight)]);

        assert_eq!(text(&term), "abcd\nef|\nij");
        assert_eq!(wrapped(&term), vec![true, false, false]);

        let mut term = Terminal::with_scrollback(4, 3, None).unwrap();

        term.feed("abcdefghij");
        term.execute([Cuu(1), El(ElScope::ToLeft)]);

        assert_eq!(text(&term), "abcd\n  | h\nij");
        assert_eq!(wrapped(&term), vec![true, true, false]);

        let mut term = Terminal::with_scrollback(4, 3, None).unwrap();

        term.feed("abcdefghij");
        term.execute([Cuu(1), El(ElScope::All)]);

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

        term.execute([Cuf(10), Dch(10)]);

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

        term.execute([Cuf(3), Ech(0)]);

        assert_eq!(text(&term), "abc   |\nijkl");
    }

    #[test]
    fn execute_ri() {
        let mut term = build_term(8, 5, 0, 0, "abcd\r\nefgh\r\nijkl\r\nmnop\r\nqrst");

        term.execute(Ri);

        assert_eq!(text(&term), "|\nabcd\nefgh\nijkl\nmnop");

        term.execute([Decstbm(3, 4), Cup(3, 1), Ri]);

        assert_eq!(text(&term), "\nabcd\n|\nefgh\nmnop");
    }

    #[test]
    fn execute_sc_rc() {
        let mut term = build_term(4, 3, 0, 0, "");

        term.feed("  \n");
        term.execute(Decsc);
        term.feed(" \n");
        term.execute(Decrc);

        assert_eq!(term.cursor().position().unwrap(), (2, 1));

        let mut term = build_term(4, 3, 0, 0, "");

        term.feed("  \n");
        term.execute(Scosc);
        term.feed(" \n");
        term.execute(Scorc);

        assert_eq!(term.cursor().position().unwrap(), (2, 1));
    }

    #[test]
    fn execute_sc_rc_restores_pen_and_modes() {
        fn assert_save_restore(save: Function, restore: Function) {
            let mut term = Engine::new((4, 4), None);

            for function in [
                Decrst(dec_modes([DecMode::AutoWrap])),
                Decstbm(2, 4),
                Decset(dec_modes([DecMode::Origin])),
                Cup(2, 3),
                sgr(SetBoldIntensity),
                save,
                Decset(dec_modes([DecMode::AutoWrap])),
                Decrst(dec_modes([DecMode::Origin])),
                Cup(4, 4),
                sgr(Reset),
                restore,
            ] {
                term.execute_with(&function, &mut ());
            }

            assert_eq!(term.cursor().position().unwrap(), (2, 2));
            assert_eq!(term.pen.intensity, Intensity::Bold);
            assert!(term.origin_mode);
            assert!(!term.auto_wrap_mode);
        }

        assert_save_restore(Decsc, Decrc);
        assert_save_restore(Scosc, Scorc);

        assert_save_restore(
            Decset(dec_modes([DecMode::SaveCursor])),
            Decrst(dec_modes([DecMode::SaveCursor])),
        );
    }

    #[test]
    fn auto_wrap_mode() {
        let mut term = Terminal::with_scrollback(4, 4, None).unwrap();

        term.execute(Decset(dec_modes([DecMode::AutoWrap])));
        term.feed("abcdef");

        assert_eq!(text(&term), "abcd\nef|\n\n");

        let mut term = Terminal::with_scrollback(4, 4, None).unwrap();

        term.execute(Decrst(dec_modes([DecMode::AutoWrap])));
        term.feed("abcdef");

        assert_eq!(text(&term), "abc|f\n\n\n");
    }

    #[test]
    fn autowrap_preserves_continuation_at_scrolling_margins() {
        for (top, bottom) in [(1, 5), (1, 4), (2, 4), (2, 5)] {
            for alternate in [false, true] {
                let limits: &[Option<usize>] = if top == 1 && !alternate {
                    &[Some(0), Some(2), None]
                } else {
                    &[Some(0)]
                };

                for &limit in limits {
                    let mut terminal = Terminal::with_scrollback(4, 5, limit).unwrap();

                    if alternate {
                        terminal.feed("\x1b[?1047h");
                    }

                    terminal.feed("1111\r\n2222\r\n3333\r\n4444\r\n5555");
                    terminal.execute([Decstbm(top, bottom), Cup(bottom, 1)]);
                    terminal.feed("ABCD");

                    assert!(
                        !terminal
                            .screen()
                            .line(bottom as usize - 1)
                            .unwrap()
                            .is_wrapped()
                    );

                    let before: Vec<_> = terminal.screen().visible_lines().cloned().collect();
                    let mut dirty = DirtyRows::new();
                    terminal.feed_with("", &mut dirty);
                    let rows = terminal.feed_with("E", &mut dirty);
                    let range = top as usize - 1..bottom as usize;
                    assert_eq!(rows, range.clone().collect::<Vec<_>>());
                    assert_eq!(terminal.cursor().position(), Some((1, range.end - 1)));

                    for (row, line) in terminal.screen().visible_lines().enumerate() {
                        let expected = if row == range.end - 1 {
                            "E   ".to_owned()
                        } else if range.contains(&row) {
                            before[row + 1].text()
                        } else {
                            before[row].text()
                        };

                        assert_eq!(line.text(), expected);
                        assert_eq!(line.is_wrapped(), row == range.end - 2);
                    }

                    assert!(terminal.screen().text().iter().any(|line| line == "ABCDE"));
                    terminal.engine.verify();
                }
            }
        }
    }

    #[test]
    fn restricted_margin_wrap_survives_dump_and_resize() {
        for alternate in [false, true] {
            let mut terminal = Terminal::new(4, 5).unwrap();

            if alternate {
                terminal.feed("\x1b[?1047h");
            }

            terminal.execute([Decstbm(2, 4), Cup(4, 1)]);
            terminal.feed("ABCDE");
            let mut restored = Terminal::new(4, 5).unwrap();
            restored.feed(terminal.dump());
            terminal.engine.assert_dump_equivalent(&restored.engine);
            terminal.resize(8, 5);
            restored.resize(8, 5);
            terminal.engine.assert_dump_equivalent(&restored.engine);
            let screen = terminal.screen();
            let line = screen.line(2).unwrap();
            assert_eq!(line.text().trim_end(), "ABCDE");
            assert!(!line.is_wrapped());
        }
    }

    #[test]
    fn autowrap_scroll_breaks_only_the_displaced_upper_connection() {
        let mut terminal = Terminal::new(4, 5).unwrap();
        terminal.feed("abcdefghijklmnopq");
        terminal.execute([Decstbm(2, 4), Cup(4, 1)]);
        terminal.feed("ABCD");
        terminal.execute(Rep(1));
        assert_eq!(wrapped(&terminal), [false, true, true, false, false]);
        assert_eq!(terminal.screen().text(), ["abcd", "ijklABCDD", "q"]);
        assert_eq!(terminal.cursor().position(), Some((1, 3)));
    }

    #[test]
    fn ordinary_scrolling_breaks_displaced_wrap_connections() {
        for (function, top, row) in [(Su(1), 2, 1), (Dl(1), 1, 2), (Lf, 2, 4), (Nel, 2, 4)] {
            let mut terminal = Terminal::new(4, 5).unwrap();
            terminal.feed("abcdefghijklmnopq");
            terminal.execute([Decstbm(top, 4), Cup(row, 1)]);
            terminal.execute(function);
            assert_eq!(wrapped(&terminal), [false, true, false, false, false]);
            assert_eq!(terminal.screen().text(), ["abcd", "ijklmnop", "", "q"]);
            assert_eq!(terminal.cursor().position(), Some((0, row as usize - 1)));
        }
    }

    #[test]
    fn wide_character_autowrap_preserves_continuation_at_bottom_margin() {
        for source in ["abc ", "abcd"] {
            let mut terminal = Terminal::new(4, 4).unwrap();
            terminal.execute([Decstbm(2, 3), Cup(3, 1)]);
            terminal.feed(source);
            terminal.execute(Cup(3, 4));
            terminal.feed("界");
            terminal.engine.verify();
            let expected = format!("{source}界");
            assert_eq!(wrapped(&terminal), [false, true, false, false]);
            assert_eq!(terminal.screen().text(), ["", &expected, ""]);
            assert_eq!(terminal.cursor().position(), Some((2, 2)));
            terminal.resize(8, 4);
            terminal.engine.verify();

            assert_eq!(
                terminal.screen().line(1).unwrap().text().trim_end(),
                expected
            );
        }
    }

    #[test]
    fn insert_mode() {
        let mut term = Terminal::with_scrollback(4, 4, None).unwrap();

        term.feed("abcd");
        term.execute([Cub(2), Sm(ansi_modes([AnsiMode::Insert]))]);
        term.feed("ef");

        assert_eq!(text(&term), "aef|b\n\n\n");

        term.feed("ghij");

        assert_eq!(text(&term), "aefg\nhij|\n\n");
    }

    #[test]
    fn print_at_the_end_of_the_screen() {
        let mut term = Terminal::with_scrollback(4, 6, None).unwrap();

        term.feed("xxxxxxxxxx");
        term.execute(Cup(50, 1));
        term.feed("yyy");
        term.execute(Cuf(50));
        term.feed("zzz");

        assert_eq!(text(&term), "xxxx\nxx\n\n\nyyyz\nzz|");

        let mut term = Terminal::with_scrollback(4, 6, None).unwrap();

        term.feed("\nxxxxxxxxxx");
        term.execute([Decstbm(2, 4), Cup(1, 1)]);
        term.feed("yyy");
        term.execute(Cuf(50));
        term.feed("zzz");

        assert_eq!(text(&term), "yyyz\nzz|xx\nxxxx\nxx\n\n");

        let mut term = Terminal::with_scrollback(4, 6, None).unwrap();

        term.execute(Decstbm(0, 3));
        term.feed("xxxxxxxxxx");
        term.execute(Cup(50, 1));
        term.feed("yyy");
        term.execute(Cuf(50));
        term.feed("zzz");

        assert_eq!(text(&term), "xxxx\nxxxx\nxx\n\n\nzz|yz");
    }

    #[test]
    fn wide_chars() {
        let mut term = Terminal::with_scrollback(20, 2, None).unwrap();

        term.feed("ハローワールド");
        assert_eq!(text(&term), "ハローワールド|\n");

        term.execute(Cub(5));
        assert_eq!(term.cursor().col, 9);
        assert_eq!(text(&term), "ハローワ|ールド\n");
    }

    #[test]
    fn print_wide_char_on_wide_tail_with_one_col_right_and_autowrap() {
        let mut term = Terminal::with_scrollback(4, 2, None).unwrap();

        term.feed("Aハz");
        term.execute([Cub(1), Print('界')]);

        assert_eq!(term.cursor().position().unwrap(), (4, 0));
        assert_eq!(text(&term), "A 界|\n");
        assert_eq!(wrapped(&term), vec![false, false]);

        let row0 = term.screen().line(0).unwrap();
        assert_eq!(row0.text(), "A 界");

        assert_eq!(
            row0.cells()
                .iter()
                .map(|cell| (cell.char(), cell.width()))
                .collect::<Vec<_>>(),
            vec![('A', 1), (' ', 1), ('界', 2), (' ', 0),]
        );
    }

    #[test]
    fn print_wide_char_on_wide_tail_with_one_col_right_and_no_autowrap() {
        let mut term = Terminal::with_scrollback(4, 2, None).unwrap();

        term.execute(Decrst(dec_modes([DecMode::AutoWrap])));
        term.feed("Aハz");
        term.execute([Cub(1), Print('界')]);

        assert_eq!(term.cursor().position().unwrap(), (3, 0));
        assert_eq!(text(&term), "A |界\n");
        assert_eq!(wrapped(&term), vec![false, false]);

        let row0 = term.screen().line(0).unwrap();
        assert_eq!(row0.text(), "A 界");

        assert_eq!(
            row0.cells()
                .iter()
                .map(|cell| (cell.char(), cell.width()))
                .collect::<Vec<_>>(),
            vec![('A', 1), (' ', 1), ('界', 2), (' ', 0),]
        );
    }

    #[test]
    fn print_wide_char_in_occupied_last_column_preserves_cell_before_wrap() {
        let mut term = Terminal::with_scrollback(4, 2, None).unwrap();

        term.feed("abcz");
        term.execute([Cup(1, 4), Print('界')]);

        assert_eq!(term.cursor().position().unwrap(), (2, 1));
        assert_eq!(text(&term), "abcz\n界|");
        assert_eq!(wrapped(&term), vec![true, false]);

        let row0 = term.screen().line(0).unwrap();
        let row1 = term.screen().line(1).unwrap();

        assert_eq!(row0.text(), "abcz");

        assert_eq!(
            row0.cells()
                .iter()
                .map(|cell| (cell.char(), cell.width()))
                .collect::<Vec<_>>(),
            vec![('a', 1), ('b', 1), ('c', 1), ('z', 1),]
        );

        assert_eq!(row1.text(), "界  ");

        assert_eq!(
            row1.cells()
                .iter()
                .map(|cell| (cell.char(), cell.width()))
                .collect::<Vec<_>>(),
            vec![('界', 2), (' ', 0), (' ', 1), (' ', 1),]
        );
    }

    #[test]
    fn execute_su() {
        let mut term = Terminal::with_scrollback(4, 6, None).unwrap();
        term.feed("aa\r\nbb\r\ncc\r\ndd\r\nee\r\nff");
        term.execute(Su(2));
        assert_eq!(text(&term), "cc\ndd\nee\nff\n\n  |");

        let mut term = Terminal::with_scrollback(4, 6, None).unwrap();
        term.feed("aa\r\nbb\r\ncc\r\ndd\r\nee\r\nff");
        term.execute([sgr(SetBoldIntensity), Su(2)]);
        assert_eq!(text(&term), "cc\ndd\nee\nff\n\n  |");

        assert!(term.screen().line(5).unwrap()[0].pen().is_bold());

        let mut term = Terminal::with_scrollback(4, 6, None).unwrap();

        term.feed("aa\r\nbb\r\ncc\r\ndd\r\nee\r\nff");
        term.execute([Decstbm(2, 5), Cup(1, 1), Su(2)]);

        assert_eq!(text(&term), "|aa\ndd\nee\n\n\nff");

        let mut term = Terminal::with_scrollback(4, 6, None).unwrap();

        term.feed("aaaaaa\r\nbbbbbb\r\ncccccc");
        term.execute(Su(2));

        assert_eq!(text(&term), "bbbb\nbb\ncccc\ncc\n\n  |");
        assert_eq!(wrapped(&term), vec![true, false, true, false, false, false]);
    }

    #[test]
    fn execute_sd() {
        let mut term = Terminal::with_scrollback(4, 6, None).unwrap();

        term.feed("aa\r\nbb\r\ncc\r\ndd\r\nee\r\nff");
        term.execute(Sd(2));

        assert_eq!(text(&term), "\n\naa\nbb\ncc\ndd|");

        let mut term = Terminal::with_scrollback(4, 6, None).unwrap();

        term.feed("aa\r\nbb\r\ncc\r\ndd\r\nee\r\nff");
        term.execute([Decstbm(2, 5), Cup(1, 1), Sd(2)]);

        assert_eq!(text(&term), "|aa\n\n\nbb\ncc\nff");

        let mut term = Terminal::with_scrollback(4, 6, None).unwrap();

        term.feed("aaaaaa\r\nbbbbbb\r\ncccccc");
        term.execute(Sd(2));

        assert_eq!(text(&term), "\n\naaaa\naa\nbbbb\nbb|");
        assert_eq!(wrapped(&term), vec![false, false, true, false, true, false]);

        let mut term = Terminal::with_scrollback(4, 6, None).unwrap();

        term.feed("aaaaaa\r\nbbbbbb\r\ncccccc");
        term.execute([Decstbm(2, 5), Cup(1, 1), Sd(2)]);

        assert_eq!(text(&term), "|aaaa\n\n\naa\nbbbb\ncc");

        assert_eq!(
            wrapped(&term),
            vec![false, false, false, false, false, false]
        );
    }

    #[test]
    fn charsets() {
        let mut term = build_term(6, 7, 0, 0, "");

        term.feed("alpty\r\n");
        term.execute(Gzd4(Charset::Drawing));
        term.feed("alpty\r\n");
        term.execute(So);
        term.feed("alpty\r\n");
        term.execute(G1d4(Charset::Drawing));
        term.feed("alpty\r\n");
        term.execute(G1d4(Charset::Ascii));
        term.feed("alpty\r\n");
        term.execute([Gzd4(Charset::Ascii), Si]);
        term.feed("alpty");

        assert_eq!(text(&term), "alpty\n▒┌⎻├≤\nalpty\n▒┌⎻├≤\nalpty\nalpty|\n");
    }

    #[test]
    fn execute_decaln() {
        let mut term = Terminal::with_scrollback(4, 2, None).unwrap();

        term.feed("ab\r\nc");
        term.execute([Cup(2, 3), Decaln]);

        assert_eq!(term.cursor().position().unwrap(), (2, 1));
        assert_eq!(text(&term), "EEEE\nEE|EE");
    }

    #[test]
    fn resize_wider() {
        let mut term = Terminal::with_scrollback(6, 6, None).unwrap();

        term.resize(7, 6);

        assert_eq!(text(&term), "|\n\n\n\n\n");
        assert!(!term.screen().visible_lines().any(Line::is_wrapped));

        term.resize(15, 6);

        assert_eq!(text(&term), "|\n\n\n\n\n");
        assert!(!term.screen().visible_lines().any(Line::is_wrapped));

        let mut term = Terminal::with_scrollback(6, 6, None).unwrap();

        term.feed("000000111111222222333333444444555");

        assert_eq!(text(&term), "000000\n111111\n222222\n333333\n444444\n555|");
        assert_eq!(wrapped(&term), vec![true, true, true, true, true, false]);

        term.resize(7, 6);

        assert_eq!(text(&term), "0000001\n1111122\n2222333\n3334444\n44555|\n");
        assert_eq!(wrapped(&term), vec![true, true, true, true, false, false]);

        term.resize(15, 6);

        assert_eq!(text(&term), "000000111111222\n222333333444444\n555|\n\n\n");
        assert_eq!(wrapped(&term), vec![true, true, false, false, false, false]);

        let mut term = Terminal::with_scrollback(4, 3, None).unwrap();

        term.feed("000011\r\n22");

        assert_eq!(text(&term), "0000\n11\n22|");
        assert_eq!(wrapped(&term), vec![true, false, false]);

        term.resize(8, 3);

        assert_eq!(text(&term), "000011\n22|\n");
        assert_eq!(wrapped(&term), vec![false, false, false]);
    }

    #[test]
    fn resize_narrower() {
        let mut term = Terminal::with_scrollback(15, 6, None).unwrap();

        term.resize(7, 6);

        assert_eq!(text(&term), "|\n\n\n\n\n");
        assert!(!term.screen().visible_lines().any(Line::is_wrapped));

        term.resize(6, 6);

        assert_eq!(text(&term), "|\n\n\n\n\n");
        assert!(!term.screen().visible_lines().any(Line::is_wrapped));

        let mut term = Terminal::with_scrollback(8, 2, None).unwrap();

        term.feed("\nabcdef");

        assert_eq!(wrapped(&term), vec![false, false]);

        term.resize(4, 2);

        assert_eq!(text(&term), "abcd\nef|");
        assert_eq!(wrapped(&term), vec![true, false]);

        let mut term = Terminal::with_scrollback(15, 6, None).unwrap();

        term.feed("000000111111222222333333444444555");

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
        let mut term = Terminal::with_scrollback(8, 4, None).unwrap();
        term.feed("abcdefgh\r\nijklmnop\r\nqrstuw");
        term.execute(Cup(4, 1));
        term.feed("AAA");

        term.resize(8, 5);

        assert_eq!(text(&term), "abcdefgh\nijklmnop\nqrstuw\nAAA|\n");

        term.feed("BBBBB");

        assert_eq!(term.cursor().position().unwrap(), (8, 3));

        term.resize(4, 5);

        assert_eq!(text(&term), "qrst\nuw\nAAAB\nBBB|B\n");

        term.feed("\rCCC");

        assert_eq!(text(&term), "qrst\nuw\nAAAB\nCCC|B\n");
        assert_eq!(wrapped(&term), vec![true, false, true, false, false]);

        term.resize(3, 5);

        assert_eq!(text(&term), "tuw\nAAA\nBCC\nC|B\n");

        term.resize(5, 5);

        assert_eq!(text(&term), "qrstu\nw\nAAABC\nCC|B\n");

        term.feed("DDD");
        term.resize(6, 5);

        assert_eq!(text(&term), "op\nqrstuw\nAAABCC\nCDDD|\n");
    }

    #[test]
    fn resize_taller() {
        let mut term = Terminal::with_scrollback(6, 4, None).unwrap();
        term.feed("AAA\n\rBBB\n\r");

        term.resize(6, 5);

        assert_eq!(text(&term), "AAA\nBBB\n|\n\n");
    }

    #[test]
    fn resize_shorter() {
        let mut term = Terminal::with_scrollback(6, 6, None).unwrap();

        term.feed("AAA\n\rBBB\n\rCCC\n\r");

        term.resize(6, 5);

        assert_eq!(text(&term), "AAA\nBBB\nCCC\n|\n");

        term.resize(6, 3);

        assert_eq!(text(&term), "BBB\nCCC\n|");

        term.resize(6, 2);

        assert_eq!(text(&term), "CCC\n|");
    }

    #[test]
    fn resize_vs_buffer_switching() {
        let mut term = Terminal::with_scrollback(4, 4, None).unwrap();

        term.feed("aaa\n\rbbb\n\rc\n\rddd");

        assert_eq!(term.cursor().position().unwrap(), (3, 3));

        term.resize(4, 5);

        assert_eq!(text(&term), "aaa\nbbb\nc\nddd|\n");

        term.execute(Decset(dec_modes([DecMode::SaveCursorAltScreenBuffer])));

        assert_eq!(term.cursor().position().unwrap(), (3, 3));

        term.resize(4, 2);

        assert_eq!(term.cursor().position().unwrap(), (3, 1));

        term.resize(2, 3);
        term.resize(3, 3);

        term.execute(Decrst(dec_modes([DecMode::SaveCursorAltScreenBuffer])));

        // The saved cursor was clamped by each intermediate resize.
        assert_eq!(text(&term), "bbb\nc|\nddd");
    }

    #[test]
    fn execute_new_line_mode() {
        let mut term = build_term(8, 2, 3, 0, "abc");

        term.execute([Sm(ansi_modes([AnsiMode::NewLine])), Lf]);

        assert_eq!(term.cursor().position().unwrap(), (0, 1));

        term.execute([Rm(ansi_modes([AnsiMode::NewLine])), Cup(1, 4), Lf]);

        assert_eq!(term.cursor().position().unwrap(), (3, 1));
    }

    #[test]
    fn execute_cursor_keys_mode() {
        let mut term = Terminal::with_scrollback(4, 2, None).unwrap();

        assert!(!term.cursor_key_app_mode());

        term.execute(Decset(dec_modes([DecMode::CursorKeys])));
        assert!(term.cursor_key_app_mode());

        term.execute(Decrst(dec_modes([DecMode::CursorKeys])));
        assert!(!term.cursor_key_app_mode());
    }

    #[test]
    fn execute_text_cursor_visibility_mode() {
        let mut term = Terminal::with_scrollback(4, 2, None).unwrap();

        assert!(term.cursor().visible);

        term.execute(Decrst(dec_modes([DecMode::TextCursorEnable])));
        assert!(!term.cursor().visible);

        term.execute(Decset(dec_modes([DecMode::TextCursorEnable])));
        assert!(term.cursor().visible);
    }

    #[test]
    fn execute_origin_mode() {
        let mut term = Terminal::with_scrollback(4, 4, None).unwrap();

        term.execute([Decstbm(2, 4), Decset(dec_modes([DecMode::Origin]))]);

        assert_eq!(term.cursor().position().unwrap(), (0, 1));

        term.execute(Cup(2, 1));
        assert_eq!(term.cursor().position().unwrap(), (0, 2));

        term.execute(Decrst(dec_modes([DecMode::Origin])));
        assert_eq!(term.cursor().position().unwrap(), (0, 0));
    }

    #[test]
    fn execute_alt_screen_buffer_mode() {
        let mut term = Terminal::with_scrollback(4, 3, None).unwrap();

        term.feed("ab\r\ncd");

        assert_eq!(term.active_screen_kind(), ScreenKind::Primary);
        assert_eq!(text(&term), "ab\ncd|\n");

        term.execute(Decset(dec_modes([DecMode::AltScreenBuffer])));

        assert_eq!(term.active_screen_kind(), ScreenKind::Alternate);
        assert_eq!(text(&term), "\n  |\n");

        term.feed("xy");
        assert_eq!(text(&term), "\n  xy|\n");

        term.execute(Decrst(dec_modes([DecMode::AltScreenBuffer])));

        assert_eq!(term.active_screen_kind(), ScreenKind::Primary);
        assert_eq!(text(&term), "ab\ncd  |\n");
    }

    #[test]
    fn execute_decstr() {
        let mut term = Engine::new((4, 3), None);

        for function in [
            Print('a'),
            Print('b'),
            Decset(dec_modes([DecMode::AltScreenBuffer])),
            Print('x'),
            Print('y'),
            Decrst(dec_modes([DecMode::TextCursorEnable])),
            Sm(ansi_modes([AnsiMode::Insert, AnsiMode::NewLine])),
            Decrst(dec_modes([DecMode::AutoWrap])),
            Decset(dec_modes([DecMode::CursorKeys])),
            Decstbm(2, 3),
            Decset(dec_modes([DecMode::Origin])),
            Cup(2, 3),
            sgr(SetBoldIntensity),
            G1d4(Charset::Drawing),
            So,
            Decsc,
        ] {
            term.execute_with(&function, &mut ());
        }

        assert_eq!(term.active_screen_kind(), ScreenKind::Alternate);
        assert_eq!((term.cursor.col, term.cursor.row), (2, 2));
        assert_eq!(term.active_screen().text(), ["  xy", "", ""]);
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
        assert_eq!(term.alternate_saved_cursor.cursor_col, 2);
        assert_eq!(term.alternate_saved_cursor.cursor_row, 2);
        assert_eq!(term.pen.intensity, Intensity::Bold);

        term.execute_with(&Decstr, &mut ());

        assert_eq!(term.active_screen_kind(), ScreenKind::Alternate);
        assert_eq!(term.cursor().position().unwrap(), (2, 2));
        assert_eq!(term.active_screen().text(), ["  xy", "", ""]);
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

        assert_eq!(
            term.alternate_saved_cursor,
            super::SavedCursorState::default()
        );

        assert_eq!(term.pen, crate::pen::Pen::default());
    }

    #[test]
    fn execute_ris() {
        let mut term = Engine::new((4, 3), None);

        for function in [
            Print('a'),
            Print('b'),
            Cr,
            Lf,
            Print('c'),
            Print('d'),
            sgr(SetBoldIntensity),
            Decset(dec_modes([
                DecMode::CursorKeys,
                DecMode::SaveCursorAltScreenBuffer,
            ])),
            Print('z'),
            Print('z'),
            Ris,
        ] {
            term.execute_with(&function, &mut ());
        }

        assert_eq!(term.active_screen_kind(), ScreenKind::Primary);
        assert_eq!(term.cursor().position().unwrap(), (0, 0));
        assert_eq!(term.active_screen().text(), ["", "", ""]);
        assert_eq!(term.pen, crate::pen::Pen::default());
        assert!(!term.cursor_keys_app_mode());
    }

    #[test]
    fn resize_vs_tabs() {
        let mut term = Engine::new((6, 2), None);

        assert_eq!(term.tabs, vec![]);

        term.resize_with(10, 2, &mut ());

        assert_eq!(term.tabs, vec![8]);

        term.resize_with(30, 2, &mut ());

        assert_eq!(term.tabs, vec![8, 16, 24]);

        term.resize_with(20, 2, &mut ());

        assert_eq!(term.tabs, vec![8, 16]);
    }

    #[test]
    fn resize_vs_saved_ctx() {
        use DecMode::*;

        let mut term = Engine::new((20, 5), None);

        term.execute_with(&Cuf(15), &mut ());

        assert_eq!(term.cursor.col, 15);

        term.execute_with(&Decsc, &mut ());

        assert_eq!(term.primary_saved_cursor.cursor_col, 15);

        term.execute_with(&Decset(dec_modes([AltScreenBuffer])), &mut ());

        term.execute_with(&Decsc, &mut ());

        assert_eq!(term.alternate_saved_cursor.cursor_col, 15);

        term.resize_with(10, 5, &mut ());

        assert_eq!(term.alternate_saved_cursor.cursor_col, 9);
    }
}
