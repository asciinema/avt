use crate::Anchor;
use crate::ansi::{
    AnsiMode, AnsiModes, CtcOp, CursorPresentation, DecMode, DecModes, EdScope, ElScope, Function,
    SgrOp, SgrOps, TbcScope,
};
use crate::buffer::{Buffer, EraseMode};
use crate::cell::{Cell, Occupancy};
use crate::charset::Charset;
use crate::line::Line;
use crate::pen::{Intensity, Pen};
use crate::tabs::Tabs;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::ops::Range;

/// Synchronous notifications emitted while the engine mutates screen state.
pub(crate) trait Effects {
    fn dirty(&mut self, rows: Range<usize>);
    fn resized(&mut self, cols: usize, rows: usize);
}

/// Terminal policy and live state. Chooses editing operations and reports their
/// effects; buffers and lines implement the corresponding storage mutations.
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
    primary_saved_cursor: Option<SavedCursorState>,
    alternate_saved_cursor: Option<SavedCursorState>,
}

/// Identifies the primary or alternate screen.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ScreenKind {
    Primary,
    Alternate,
}

/// A screen-relative coordinate value at a particular moment.
/// Negative rows address retained scrollback. `col` may equal the line width,
/// representing the boundary after its last cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub screen: ScreenKind,
    pub col: usize,
    pub row: isize,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Cursor {
    pub col: usize,
    pub row: usize,
    pub visible: bool,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            col: 0,
            row: 0,
            visible: true,
        }
    }
}

impl Cursor {
    pub fn position(self) -> Option<(usize, usize)> {
        if self.visible {
            Some((self.col, self.row))
        } else {
            None
        }
    }
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
    fn dump(&self, funs: &mut Vec<Function<'static>>, cols: usize) {
        funs.push(dump_cursor(
            self.cursor_col,
            self.cursor_row,
            cols,
            self.origin_mode,
        ));

        if !self.auto_wrap_mode {
            funs.push(Function::Decrst(DecModes::one(DecMode::AutoWrap)));
        }

        funs.push(to_sgr(&self.pen));
        funs.push(Function::Decsc);

        if !self.auto_wrap_mode {
            funs.push(Function::Decset(DecModes::one(DecMode::AutoWrap)));
        }
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
            primary_saved_cursor: None,
            alternate_saved_cursor: None,
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
    fn active_saved_cursor(&self) -> SavedCursorState {
        match self.active {
            ScreenKind::Primary => self.primary_saved_cursor,
            ScreenKind::Alternate => self.alternate_saved_cursor,
        }
        .unwrap_or_default()
    }

    #[inline(always)]
    fn active_saved_cursor_mut(&mut self) -> &mut Option<SavedCursorState> {
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

    pub fn execute_with(&mut self, function: &Function<'_>, observer: &mut impl Effects) {
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

            Decrsps(ref presentation) => self.decrsps(presentation),

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

            Print(ref text) => self.print_text(text, observer),

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

    #[inline]
    pub fn evict_alternate(&mut self) {
        // A batch can scroll the alternate buffer and switch away before cleanup.
        while self.alternate_buffer.pop_evicted().is_some() {}
    }

    #[inline]
    pub fn pop_evicted(&mut self) -> Option<Line> {
        self.primary_buffer.pop_evicted()
    }

    // cursor

    fn save_cursor(&mut self) {
        let state = SavedCursorState {
            cursor_col: self.cursor.col,
            cursor_row: self.cursor.row,
            pen: self.pen,
            origin_mode: self.origin_mode,
            auto_wrap_mode: self.auto_wrap_mode,
        };

        *self.active_saved_cursor_mut() = Some(state);
    }

    fn restore_cursor(&mut self) {
        let state = self.active_saved_cursor();

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

    fn move_cursor_down_with_scroll(&mut self, observer: &mut impl Effects) {
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

    fn scroll_up_in_region(&mut self, n: usize, observer: &mut impl Effects) {
        let range = self.top_margin..self.bottom_margin + 1;
        let pen = self.pen;
        self.active_buffer_mut().scroll_up(range.clone(), n, &pen);
        observer.dirty(range);
    }

    fn scroll_down_in_region(&mut self, n: usize, observer: &mut impl Effects) {
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

    fn switch_to_alternate_buffer(&mut self, observer: &mut impl Effects) {
        if let ScreenKind::Primary = self.active {
            self.primary_cursor = (self.cursor.col, self.cursor.row);
            self.active = ScreenKind::Alternate;
            self.alternate_buffer = Buffer::new(self.cols, self.rows, Some(0), self.pen);
            observer.dirty(0..self.rows);
        }
    }

    fn switch_to_primary_buffer(&mut self, observer: &mut impl Effects) {
        if let ScreenKind::Alternate = self.active {
            self.active = ScreenKind::Primary;
            observer.dirty(0..self.rows);
        }
    }

    // resizing

    pub fn resize_with(&mut self, cols: usize, rows: usize, observer: &mut impl Effects) {
        debug_assert!(cols > 0 && rows > 0, "terminal size must be at least 1x1");

        if (cols, rows) == self.size() {
            return;
        }

        match cols.cmp(&self.cols) {
            Ordering::Less => self.tabs.contract(cols),

            Ordering::Equal => {}

            Ordering::Greater => self.tabs.expand(self.cols, cols),
        }

        self.top_margin = 0;
        self.bottom_margin = rows - 1;
        let width_changed = cols != self.cols;
        self.cols = cols;
        self.rows = rows;

        observer.resized(cols, rows);
        let cursor = (self.cursor.col, self.cursor.row);
        let reflow = self.active == ScreenKind::Primary;

        let mut saved = self
            .primary_saved_cursor
            .map(|s| (s.cursor_col, s.cursor_row));

        (self.cursor.col, self.cursor.row) = if reflow {
            self.primary_buffer
                .resize(cols, rows, cursor, true, &mut saved)
        } else {
            self.alternate_buffer
                .resize(cols, rows, cursor, false, &mut None)
        };

        if self.active == ScreenKind::Alternate {
            self.primary_cursor =
                self.primary_buffer
                    .resize(cols, rows, self.primary_cursor, true, &mut saved);
        }

        if let (Some(state), Some((col, row))) = (&mut self.primary_saved_cursor, saved) {
            state.cursor_col = col;
            state.cursor_row = row;
        }

        if let Some(saved) = &mut self.alternate_saved_cursor {
            if width_changed || saved.cursor_row >= rows {
                saved.cursor_col = saved.cursor_col.min(cols - 1);
            }

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
        self.auto_wrap_mode = true;
        self.cursor_keys_mode = CursorKeysMode::Normal;
        self.pen = Pen::default();
        self.charsets = [Charset::Ascii, Charset::Ascii];
        self.active_charset = 0;
        *self.active_saved_cursor_mut() = None;
    }

    fn hard_reset(&mut self, observer: &mut impl Effects) {
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

        for saved in [&self.primary_saved_cursor, &self.alternate_saved_cursor]
            .into_iter()
            .flatten()
        {
            assert!(saved.cursor_col <= self.cols);
            assert!(saved.cursor_row < self.rows);
        }

        let buffers = std::iter::once(&self.primary_buffer)
            .chain((self.active == ScreenKind::Alternate).then_some(&self.alternate_buffer));

        for buffer in buffers {
            assert_eq!(buffer.view().len(), self.rows);
            assert!(buffer.lines().all(|line| line.len() == self.cols));
            assert!(!buffer.lines().last().unwrap().is_wrapped());

            for line in buffer.lines() {
                for i in 0..self.cols {
                    let occupancy = line[i].occupancy();

                    if occupancy == Occupancy::WideTail {
                        assert!(i > 0);
                        assert!(line[i - 1].occupancy() == Occupancy::WideHead);
                        assert_eq!(line[i].pen(), line[i - 1].pen());
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

    fn print_text(&mut self, text: &str, observer: &mut impl Effects) {
        if text.len() <= 1
            || self.insert_mode
            || self.charsets[self.active_charset] != Charset::Ascii
        {
            self.print_chars(text.chars(), observer);

            return;
        }

        let mut dirty = self.cursor.row..self.cursor.row;
        let mut chars = text.chars();

        loop {
            let text = chars.as_str();

            let Some(ch) = chars.next() else {
                break;
            };

            // A lone ASCII character cannot amortize the batch setup. In
            // particular, keep CJK interspersed with spaces on the char path.
            if matches!(ch, ' '..='~') && matches!(chars.as_str().as_bytes(), [b' '..=b'~', ..]) {
                let pos = (self.cursor.col, self.cursor.row);
                let pen = self.pen;

                let written = self
                    .active_buffer_mut()
                    .replace_ascii_prefix(pos, text, pen);

                if written > 0 {
                    chars = text[written..].chars();
                    self.advance_cursor_after_print(written);
                    dirty.end = self.cursor.row + 1;
                    continue;
                }
            }

            self.print_char(ch, &mut dirty);
        }

        if !dirty.is_empty() {
            observer.dirty(dirty);
        }
    }

    fn print_chars(&mut self, chars: impl Iterator<Item = char>, observer: &mut impl Effects) {
        // Include the starting row, whose wrap flag may change. Scrolling can
        // extend the range upward.
        let mut dirty = self.cursor.row..self.cursor.row;

        for ch in chars {
            self.print_char(ch, &mut dirty);
        }

        if !dirty.is_empty() {
            observer.dirty(dirty);
        }
    }

    #[inline(always)] // Avoid measurable per-character call overhead.
    fn print_char(&mut self, ch: char, dirty: &mut Range<usize>) {
        let ch = self.charsets[self.active_charset].translate(ch);

        match self.try_print(ch) {
            Some(width) => self.advance_cursor_after_print(width),

            None if self.auto_wrap_mode => self.wrap_and_print_at_line_start(ch, dirty),

            None => self.print_at_line_end(ch),
        }

        dirty.end = self.cursor.row + 1;
    }

    /// Attempts placement at the cursor without moving it. A rejected placement
    /// leaves all state untouched; wrapping is the caller's responsibility.
    #[inline]
    fn try_print(&mut self, ch: char) -> Option<usize> {
        if self.cursor.col >= self.cols {
            return None;
        }

        let pos = (self.cursor.col, self.cursor.row);
        let pen = self.pen;

        if self.insert_mode {
            self.active_buffer_mut().insert_char(pos, ch, pen)
        } else {
            self.active_buffer_mut().replace_char(pos, ch, pen)
        }
    }

    fn advance_cursor_after_print(&mut self, width: usize) {
        self.cursor.col += width;

        if self.cursor.col == self.cols && !self.auto_wrap_mode {
            self.cursor.col = self.cols - 1;
        }
    }

    #[inline]
    fn wrap_and_print_at_line_start(&mut self, ch: char, dirty: &mut Range<usize>) {
        let row = self.cursor.row;
        let pen = self.pen;

        if row == self.bottom_margin {
            let range = self.top_margin..self.bottom_margin + 1;

            self.active_buffer_mut().wrap_and_scroll_up(range, &pen);

            dirty.start = dirty.start.min(self.top_margin);
        } else if row < self.rows - 1 {
            self.active_buffer_mut().wrap_to_next_row(row);
            self.cursor.row += 1;
        }

        self.cursor.col = 0;

        if let Some(width) = self.try_print(ch) {
            self.cursor.col = width;
        }
    }

    fn print_at_line_end(&mut self, ch: char) {
        // With autowrap disabled, back up enough to fit the character and leave
        // the cursor at the final column.
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
            let printed = self.try_print(ch).is_some();

            if !printed && self.cursor.col > 0 {
                self.cursor.col -= 1;
                self.try_print(ch);
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

    fn lf(&mut self, observer: &mut impl Effects) {
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

    fn nel(&mut self, observer: &mut impl Effects) {
        self.move_cursor_down_with_scroll(observer);
        self.cursor.col = 0;
    }

    fn hts(&mut self) {
        self.set_tab();
    }

    fn ri(&mut self, observer: &mut impl Effects) {
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

    fn decrsps(&mut self, presentation: &CursorPresentation) {
        let row = usize::from(presentation.row);
        let col = usize::from(presentation.col);

        if row == 0 || row > self.rows || col == 0 || col > self.cols {
            return;
        }

        self.cursor.row = row - 1;

        self.cursor.col = if presentation.pending_wrap && col == self.cols {
            self.cols
        } else {
            col - 1
        };

        self.origin_mode = presentation.origin_mode;
        // The remaining accepted report fields are fixed defaults.
        self.pen.intensity = Intensity::Normal;
        self.pen.unset_underline();
        self.pen.unset_blink();
        self.pen.unset_inverse();
        self.charsets = [Charset::Ascii; 2];
        self.active_charset = 0;
    }

    fn ris(&mut self, observer: &mut impl Effects) {
        self.hard_reset(observer);
    }

    fn decaln(&mut self, observer: &mut impl Effects) {
        for row in 0..self.rows {
            for col in 0..self.cols {
                self.active_buffer_mut()
                    .replace_char((col, row), '\u{45}', Pen::default());
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

    fn ich(&mut self, n: u16, observer: &mut impl Effects) {
        if self.cursor.col == self.cols {
            self.cursor.col = self.cols - 1;
        }

        let n = as_usize(n, 1).min(self.cols - self.cursor.col);

        let pos = (self.cursor.col, self.cursor.row);
        let row = self.cursor.row;
        let pen = self.pen;

        self.active_buffer_mut().insert_blanks(pos, n, pen);

        observer.dirty(row..row + 1);
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

    fn ed(&mut self, scope: &EdScope, observer: &mut impl Effects) {
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

    fn el(&mut self, scope: &ElScope, observer: &mut impl Effects) {
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

    fn il(&mut self, n: u16, observer: &mut impl Effects) {
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

    fn dl(&mut self, n: u16, observer: &mut impl Effects) {
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

    fn dch(&mut self, n: u16, observer: &mut impl Effects) {
        if self.cursor.col >= self.cols {
            self.move_cursor_to_col(self.cols - 1);
        }

        let pos = (self.cursor.col, self.cursor.row);
        let pen = self.pen;

        self.active_buffer_mut().delete(pos, as_usize(n, 1), &pen);

        observer.dirty(self.cursor.row..self.cursor.row + 1);
    }

    fn su(&mut self, n: u16, observer: &mut impl Effects) {
        self.scroll_up_in_region(as_usize(n, 1), observer);
    }

    fn sd(&mut self, n: u16, observer: &mut impl Effects) {
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

    fn ech(&mut self, n: u16, observer: &mut impl Effects) {
        let n = as_usize(n, 1);

        self.erase_at_cursor(EraseMode::NextChars(n));

        observer.dirty(self.cursor.row..self.cursor.row + 1);
    }

    fn cbt(&mut self, n: u16) {
        self.move_cursor_to_prev_tab(as_usize(n, 1));
    }

    fn rep(&mut self, n: u16, observer: &mut impl Effects) {
        if self.cursor.col > 0 {
            let n = as_usize(n, 1);
            let row = self.cursor.row;
            let mut col = self.cursor.col - 1;

            while col > 0 && self.active_buffer()[(col, row)].occupancy() == Occupancy::WideTail {
                col -= 1;
            }

            let char = self.active_buffer()[(col, row)].char();
            self.print_chars(core::iter::repeat_n(char, n), observer);
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

    fn decset(&mut self, modes: &DecModes, observer: &mut impl Effects) {
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

    fn decrst(&mut self, modes: &DecModes, observer: &mut impl Effects) {
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

    pub fn dump(&self) -> Vec<Function<'static>> {
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

        if let Some(saved) = primary_ctx {
            saved.dump(&mut funs, self.cols);
        }

        if self.active == ScreenKind::Alternate {
            let (col, row) = self.primary_cursor;

            funs.push(dump_cursor(col, row, self.cols, false));
        }

        // Reset the pen before switching: the alternate buffer is created with it.
        funs.push(to_sgr(&Pen::default()));

        // 4. dump alternate screen buffer

        // switch to alternate screen
        if self.active == ScreenKind::Alternate || alternate_ctx.is_some() {
            funs.push(Function::Decset(DecModes::one(DecMode::AltScreenBuffer)));
        }

        if self.active == ScreenKind::Alternate {
            // move cursor home
            funs.push(Function::Cup(1, 1));

            // dump alternate buffer
            dump_buffer(&self.alternate_buffer, &mut funs);
        }

        // 5. configure the alternate saved cursor while its buffer is active

        if let Some(saved) = alternate_ctx {
            saved.dump(&mut funs, self.cols);
        }

        // 6. ensure the right buffer is active

        if self.active == ScreenKind::Primary && alternate_ctx.is_some() {
            // switch back to primary screen
            funs.push(Function::Decrst(DecModes::one(DecMode::AltScreenBuffer)));
        }

        // 7. setup margins

        // Setting margins moves the cursor home, so restore its position afterward.
        if self.top_margin > 0 || self.bottom_margin < self.rows - 1 {
            funs.push(Function::Decstbm(
                as_u16(self.top_margin + 1),
                as_u16(self.bottom_margin + 1),
            ));
        }

        // 8. restore the live cursor independently of margins and saved cursors

        funs.push(dump_cursor(
            self.cursor.col,
            self.cursor.row,
            self.cols,
            self.origin_mode,
        ));

        // configure pen
        funs.push(to_sgr(&self.pen));

        if !self.cursor.visible {
            // hide cursor
            funs.push(Function::Decrst(DecModes::one(DecMode::TextCursorEnable)));
        }

        // Restore charset and printing modes after painting both buffers.

        // 9. setup charset

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

        // 10. setup insert mode

        if self.insert_mode {
            // enable insert mode
            funs.push(Function::Sm(AnsiModes::one(AnsiMode::Insert)));
        }

        // 11. setup auto-wrap mode

        if !self.auto_wrap_mode {
            // disable auto-wrap mode
            funs.push(Function::Decrst(DecModes::one(DecMode::AutoWrap)));
        }

        // 12. setup new line mode

        if self.new_line_mode {
            // enable new line mode
            funs.push(Function::Sm(AnsiModes::one(AnsiMode::NewLine)));
        }

        // 13. setup cursor key mode

        if self.cursor_keys_mode == CursorKeysMode::Application {
            funs.push(Function::Decset(DecModes::one(DecMode::CursorKeys)));
        }

        funs
    }
}

fn dump_cursor(col: usize, row: usize, cols: usize, origin_mode: bool) -> Function<'static> {
    Function::Decrsps(CursorPresentation {
        row: as_u16(row + 1),
        col: as_u16(col.min(cols - 1) + 1),
        origin_mode,
        pending_wrap: col == cols,
    })
}

fn dump_buffer(buffer: &Buffer, funs: &mut Vec<Function<'static>>) {
    let cutoff = buffer.occupied_prefix_len(false);
    let last = buffer.rows - 1;
    let mut pen = Pen::default();

    for (i, line) in buffer.view().take(cutoff).enumerate() {
        // Wide tails share their head's pen, so grouping before filtering tails
        // in dump_cells preserves the same style boundaries.
        for cells in line.cells().chunk_by(|c1, c2| c1.pen() == c2.pen()) {
            if cells[0].pen() != &pen {
                if let Some(sgr) = to_sgr_diff(&pen, cells[0].pen()) {
                    funs.push(sgr);
                }

                pen = *cells[0].pen();
            }

            dump_cells(cells, funs);
        }

        if i < last && !line.is_wrapped() {
            funs.push(Function::Cr);
            funs.push(Function::Lf);
        }
    }
}

fn to_sgr_diff(from: &Pen, to: &Pen) -> Option<Function<'static>> {
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

fn dump_cells(cells: &[Cell], funs: &mut Vec<Function<'static>>) {
    let mut chars = cells
        .iter()
        .filter(|cell| cell.occupancy() != Occupancy::WideTail)
        .map(Cell::char)
        .peekable();

    let mut text = String::new();

    while let Some(ch) = chars.next() {
        let mut run_len = 1;

        while chars.next_if_eq(&ch).is_some() {
            run_len += 1;
        }

        if run_len > 5 {
            text.push(ch);
            funs.push(Function::Print(core::mem::take(&mut text).into()));
            funs.push(Function::Rep(as_u16(run_len - 1)));
        } else {
            for _ in 0..run_len {
                text.push(ch);
            }
        }
    }

    if !text.is_empty() {
        funs.push(Function::Print(text.into()));
    }
}

fn to_sgr(pen: &Pen) -> Function<'static> {
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
    use crate::ansi::{
        AnsiMode, AnsiModes, CursorPresentation, DecMode, DecModes, EdScope, ElScope, Function,
        SgrOp, SgrOps,
    };
    use crate::charset::Charset;
    use crate::color::Color;
    use crate::line::Line;
    use crate::observer::DirtyRows;
    use crate::pen::Intensity;
    use crate::pen::Pen;
    use Function::*;
    use SgrOp::*;
    use alloc::borrow::ToOwned as _;
    use alloc::format;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;
    use proptest::prelude::*;

    #[derive(Debug, Default, PartialEq, Eq)]
    struct PrintEffects {
        dirty: Vec<core::ops::Range<usize>>,
    }

    impl super::Effects for PrintEffects {
        fn dirty(&mut self, rows: core::ops::Range<usize>) {
            self.dirty.push(rows);
        }

        fn resized(&mut self, cols: usize, rows: usize) {
            panic!("unexpected resize to {cols}x{rows}");
        }
    }

    fn assert_printing_matches_chars(
        cols: usize,
        rows: usize,
        setup: &str,
        functions: &[Function<'_>],
    ) {
        let mut batched = Terminal::with_scrollback(cols, rows, Some(3)).unwrap();
        let mut reference = Terminal::with_scrollback(cols, rows, Some(3)).unwrap();
        batched.feed(setup);
        reference.feed(setup);
        let mut anchors = Vec::new();

        for screen in [ScreenKind::Primary, ScreenKind::Alternate] {
            for row in 0..rows {
                for col in [0, cols / 2, cols] {
                    let position = super::Position {
                        screen,
                        col,
                        row: row as isize,
                    };

                    anchors.push((
                        batched.anchor_at(position).unwrap(),
                        reference.anchor_at(position).unwrap(),
                    ));
                }
            }
        }

        for function in functions {
            let mut actual_effects = PrintEffects::default();
            let mut expected_effects = PrintEffects::default();
            batched.engine.execute_with(function, &mut actual_effects);

            match function {
                Print(text) => reference
                    .engine
                    .print_chars(text.chars(), &mut expected_effects),

                _ => reference
                    .engine
                    .execute_with(function, &mut expected_effects),
            }

            assert_eq!(actual_effects, expected_effects, "{function:?}");
            batched.engine.verify();

            batched
                .engine
                .assert_execution_equivalent(&reference.engine);

            for (actual, expected) in &anchors {
                assert_eq!(
                    batched.resolve(actual),
                    reference.resolve(expected),
                    "{function:?}, before eviction"
                );
            }

            // Enforce retention as a normal execute call would.
            batched.execute(core::iter::empty::<Function<'_>>());
            reference.execute(core::iter::empty::<Function<'_>>());

            batched
                .engine
                .assert_execution_equivalent(&reference.engine);

            for (actual, expected) in &anchors {
                assert_eq!(
                    batched.resolve(actual),
                    reference.resolve(expected),
                    "{function:?}, after eviction"
                );
            }
        }
    }

    #[test]
    fn text_printing_matches_chars_across_wraps() {
        assert_printing_matches_chars(
            4,
            2,
            "",
            &[Print("abcd".into()), Print("efghijklmnopqrstuvwxyz".into())],
        );
    }

    #[test]
    fn text_printing_matches_chars_on_one_column() {
        assert_printing_matches_chars(1, 1, "", &[Print("abcdef".into())]);
    }

    #[test]
    fn text_printing_matches_chars_on_alternate_screen() {
        assert_printing_matches_chars(4, 2, "\x1b[?1047h", &[Print("abcdefghijkl".into())]);
    }

    #[test]
    fn text_printing_matches_chars_in_scroll_region() {
        assert_printing_matches_chars(
            4,
            4,
            "top\r\nmid\r\nlow\r\nend\x1b[2;3r\x1b[3;4H",
            &[Print("abcdef".into())],
        );
    }

    #[test]
    fn text_printing_matches_chars_without_autowrap() {
        assert_printing_matches_chars(4, 2, "\x1b[?7l", &[Print("abcdef".into())]);
    }

    #[test]
    fn text_printing_matches_chars_across_fallbacks() {
        // Start on a wide tail, then encounter a wide head and mixed input.
        // Controls here are literal Print contents, not parser actions.
        assert_printing_matches_chars(
            20,
            2,
            "界語abcd\r\x1b[2G",
            &[Print("ABCé界 DE\0FG\x7fHI\u{301}".into())],
        );
    }

    #[test]
    fn text_printing_matches_chars_in_scalar_modes() {
        for setup in [
            "abcd\r\x1b[4h", // Insert into existing text.
            "\x1b(0",        // G0 drawing charset.
            "\x1b)0\x0e",    // G1 drawing charset selected with SO.
        ] {
            assert_printing_matches_chars(8, 2, setup, &[Print("lqk".into())]);
        }
    }

    #[test]
    fn empty_text_printing_matches_chars() {
        assert_printing_matches_chars(4, 2, "ab", &[Print("".into())]);
    }

    proptest! {
        #[test]
        fn prop_text_printing_matches_chars(
            cols in 1..20usize,
            rows in 1..8usize,
            functions in crate::tests::gen_functions(100),
        ) {
            assert_printing_matches_chars(cols, rows, "ab界c\r\n0123", &functions);
        }
    }

    fn print(ch: char) -> Function<'static> {
        Print(String::from(ch).into())
    }

    fn build_term(cols: usize, rows: usize, cx: usize, cy: usize, init: &str) -> Terminal {
        let mut term = Terminal::with_scrollback(cols, rows, None).unwrap();
        term.feed(init);
        term.execute(Cup((cy + 1) as u16, (cx + 1) as u16));

        term
    }

    #[test]
    fn decrsps_restores_presentation_independently_of_saves() {
        for alternate in [false, true] {
            for saved in [false, true] {
                let mut terminal = Terminal::new(4, 4).unwrap();
                terminal.feed("ABCDX");

                if saved {
                    terminal.feed("\x1b7");
                }

                terminal.feed("\x1b[?1047h\x1b[Hab界");

                if saved {
                    terminal.feed("\x1b7");
                }

                if !alternate {
                    terminal.feed("\x1b[?1047l");
                }

                let mut dirty = DirtyRows::new();

                terminal.feed_with(
                    "\x1b[2;3r\x1b[?7;25l\x1b[4;20h\x1b[?1h\x1b[31;42;1;3;4;5;7;9m\x1b(0\x1b)0\x0e",
                    &mut dirty,
                );

                let primary_save = terminal.engine.primary_saved_cursor;
                let alternate_save = terminal.engine.alternate_saved_cursor;
                let primary_cursor = terminal.engine.primary_cursor;
                let primary: Vec<_> = terminal.primary_screen().visible_lines().cloned().collect();
                let active: Vec<_> = terminal.screen().visible_lines().cloned().collect();

                let rows = terminal.feed_with("\x1bP1$t1;4;1;@;@;I;0;2;@;BBBB\x1b\\", &mut dirty);

                assert!(rows.is_empty());
                let engine = &terminal.engine;
                assert_eq!((engine.cursor.col, engine.cursor.row), (4, 0));
                assert!(engine.origin_mode);
                assert!(!engine.cursor.visible);
                assert!(!engine.auto_wrap_mode);
                assert!(engine.insert_mode);
                assert!(engine.new_line_mode);
                assert!(terminal.cursor_key_app_mode());
                assert_eq!((engine.top_margin, engine.bottom_margin), (1, 2));
                assert_eq!(engine.primary_saved_cursor, primary_save);
                assert_eq!(engine.alternate_saved_cursor, alternate_save);
                assert_eq!(engine.primary_cursor, primary_cursor);
                assert!(terminal.primary_screen().visible_lines().eq(primary.iter()));
                assert!(terminal.screen().visible_lines().eq(active.iter()));
                assert_eq!(engine.charsets, [Charset::Ascii; 2]);
                assert_eq!(engine.active_charset, 0);
                assert!(!engine.pen.is_bold());
                assert!(!engine.pen.is_faint());
                assert!(!engine.pen.is_underline());
                assert!(!engine.pen.is_blink());
                assert!(!engine.pen.is_inverse());
                assert!(engine.pen.is_italic());
                assert!(engine.pen.is_strikethrough());
                assert_eq!(engine.pen.foreground(), Some(Color::Indexed(1)));
                assert_eq!(engine.pen.background(), Some(Color::Indexed(2)));

                // Physical positioning also works below the origin margins.
                terminal.feed("\x1bP1$t4;2;1;@;@;A;0;2;@;BBBB\x1b\\");
                let engine = &terminal.engine;
                assert_eq!((engine.cursor.col, engine.cursor.row), (1, 3));
                assert!(engine.origin_mode);
                terminal.feed("q");
                assert_eq!(terminal.screen().line(3).unwrap().cells()[1].char(), 'q');
                terminal.engine.verify();
            }
        }
    }

    #[test]
    fn decrsps_ignores_out_of_screen_coordinates_atomically() {
        for (row, col) in [(0, 1), (1, 0), (5, 1), (1, 5), (u16::MAX, u16::MAX)] {
            let setup = "ab界\x1b7\x1b[?1047h\x1b[2;3r\x1b[1;4;5;7;31m\x1b[?6h\x1b(0q";
            let mut terminal = Terminal::new(4, 4).unwrap();
            let mut expected = Terminal::new(4, 4).unwrap();
            let mut dirty = DirtyRows::new();
            terminal.feed_with(setup, &mut dirty);
            expected.feed(setup);

            let rows = terminal.execute_with(
                Decrsps(CursorPresentation {
                    row,
                    col,
                    pending_wrap: true,
                    ..CursorPresentation::default()
                }),
                &mut dirty,
            );

            assert!(rows.is_empty());

            terminal
                .engine
                .assert_execution_equivalent(&expected.engine);
        }
    }

    #[test]
    fn decrsps_restores_pending_wrap_without_painting() {
        for alternate in [false, true] {
            for auto_wrap in [false, true] {
                for (cols, text, overwritten) in
                    [(1, "A", "X"), (4, "ABCD", "ABCX"), (4, "AB界", "AB X")]
                {
                    let mut terminal = Terminal::new(cols, 2).unwrap();
                    let mut dirty = DirtyRows::new();

                    if alternate {
                        terminal.feed("\x1b[?1047h");
                    }

                    terminal.feed_with([text, "\x1b[H"], &mut dirty);

                    if !auto_wrap {
                        terminal.feed("\x1b[?7l");
                    }

                    let before: Vec<_> = terminal.screen().visible_lines().cloned().collect();

                    let rows = terminal.execute_with(
                        Decrsps(CursorPresentation {
                            col: cols as u16,
                            pending_wrap: true,
                            ..CursorPresentation::default()
                        }),
                        &mut dirty,
                    );

                    assert!(rows.is_empty());
                    assert_eq!(terminal.cursor().position(), Some((cols, 0)));
                    assert!(terminal.screen().visible_lines().eq(before.iter()));
                    assert!(!terminal.screen().line(0).unwrap().is_wrapped());
                    terminal.feed("X");
                    assert_eq!(terminal.screen().line(0).unwrap().is_wrapped(), auto_wrap);

                    if auto_wrap {
                        assert_eq!(terminal.screen().line(0).unwrap().text(), text);
                        assert_eq!(terminal.screen().line(1).unwrap().cells()[0].char(), 'X');
                        assert_eq!(terminal.cursor().position(), Some((1, 1)));
                    } else {
                        assert_eq!(terminal.screen().line(0).unwrap().text(), overwritten);
                        assert_eq!(terminal.cursor().position(), Some((cols - 1, 0)));
                    }

                    terminal.engine.verify();
                }
            }
        }
    }

    #[test]
    fn decrsps_only_honors_pending_wrap_at_the_right_edge() {
        for col in [1, 3, 4] {
            for pending_wrap in [false, true] {
                let mut terminal = Terminal::new(4, 2).unwrap();
                terminal.feed("ABCD");

                terminal.execute(Decrsps(CursorPresentation {
                    col,
                    pending_wrap,
                    ..CursorPresentation::default()
                }));

                let wraps = pending_wrap && col == 4;
                let expected_col = if wraps { 4 } else { usize::from(col) - 1 };
                assert_eq!(terminal.cursor().position(), Some((expected_col, 0)));
                terminal.feed("X");
                assert_eq!(terminal.cursor().row, usize::from(wraps));
                assert_eq!(terminal.screen().line(0).unwrap().is_wrapped(), wraps);
                terminal.engine.verify();
            }
        }
    }

    #[test]
    fn resizing_resets_margins_and_preserves_origin() {
        for alternate in [false, true] {
            for origin in [false, true] {
                for (cols, rows) in [(4, 5), (8, 5), (6, 3), (6, 7), (4, 3), (8, 7)] {
                    let mut terminal = Terminal::new(6, 5).unwrap();

                    if alternate {
                        terminal.feed("\x1b[?1047h");
                    }

                    terminal.feed("\x1b[2;4r");

                    if origin {
                        terminal.feed("\x1b[?6h");
                    }

                    terminal.feed("\x1b[2;2HX");
                    let cursor = terminal.cursor();
                    terminal.resize(cols, rows);
                    assert_eq!(terminal.cursor(), cursor);

                    // Addressing and scrolling must cover the whole new screen.
                    terminal.execute(Cup(1, 1));
                    assert_eq!(terminal.cursor().position(), Some((0, 0)));
                    terminal.execute([print('T'), Cup(rows as u16, 1)]);
                    assert_eq!(terminal.cursor().position(), Some((0, rows - 1)));
                    terminal.execute([print('B'), Su(rows as u16)]);
                    assert_eq!(terminal.screen().text(), vec![""; rows]);

                    // New margins expose whether origin mode survived the resize.
                    terminal.execute(Decstbm(2, rows as u16));
                    assert_eq!(terminal.cursor().position(), Some((0, usize::from(origin))));
                    terminal.engine.verify();
                }
            }
        }
    }

    #[test]
    fn unchanged_resize_preserves_margins_and_pending_wrap() {
        for alternate in [false, true] {
            let mut terminal = Terminal::new(6, 5).unwrap();
            let mut dirty = DirtyRows::new();

            if alternate {
                terminal.feed("\x1b[?1047h");
            }

            terminal.feed_with("\x1b[2;4r\x1b[?6h\x1b[2;6HX", &mut dirty);
            assert_eq!(terminal.cursor().position(), Some((6, 2)));
            assert!(terminal.resize_with(6, 5, &mut dirty).is_empty());
            assert_eq!(terminal.cursor().position(), Some((6, 2)));
            terminal.execute(Cup(1, 1));
            assert_eq!(terminal.cursor().position(), Some((0, 1)));
            terminal.execute(Cup(5, 1));
            assert_eq!(terminal.cursor().position(), Some((0, 3)));
            terminal.engine.verify();
        }
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
    fn alternate_resize_clips_and_pads_rows_without_reflow() {
        for mode in [1047, 1049] {
            let mut terminal = Terminal::new(4, 3).unwrap();
            terminal.feed(format!("\x1b[?{mode}habcdef\r\ngh"));

            for (cols, expected, cursor_col) in [
                (6, ["abcd  ", "ef    ", "gh    "], 2),
                (2, ["ab", "ef", "gh"], 1),
                (4, ["ab  ", "ef  ", "gh  "], 1),
            ] {
                terminal.resize(cols, 3);
                terminal.engine.verify();

                let lines: Vec<_> = terminal.screen().visible_lines().map(Line::text).collect();

                assert_eq!(lines, expected);
                assert_eq!(wrapped(&terminal), [true, false, false]);
                assert_eq!(terminal.cursor().position(), Some((cursor_col, 2)));
                assert_eq!(terminal.screen().all_lines().count(), 3);
            }

            assert_eq!(terminal.screen().text(), ["ab  ef", "gh"]);
            terminal.feed("X");
            assert_eq!(terminal.screen().line(2).unwrap().text(), "gX  ");
        }
    }

    #[test]
    fn alternate_resize_keeps_cursor_visible_when_shrinking_height() {
        for (row, expected) in [(0, ["aa", "bb"]), (2, ["bb", "cc"])] {
            let mut terminal = Terminal::new(4, 3).unwrap();
            terminal.feed("\x1b[?1047haaaa\r\nbbbb\r\ncccc");
            terminal.feed(format!("\x1b[{};1H", row + 1));
            terminal.resize(2, 2);
            terminal.engine.verify();

            let lines: Vec<_> = terminal.screen().visible_lines().map(Line::text).collect();

            assert_eq!(lines, expected);
            assert_eq!(terminal.cursor().position(), Some((0, row.min(1))));
            assert_eq!(terminal.screen().all_lines().count(), 2);
            terminal.resize(4, 3);
            assert_eq!(terminal.screen().text(), [expected[0], expected[1], ""]);
        }
    }

    #[test]
    fn alternate_width_resize_clears_pending_wrap() {
        for cols in [1, 2, 6] {
            let mut terminal = Terminal::new(4, 2).unwrap();
            terminal.feed("\x1b[?1047habcd");
            terminal.resize(cols, 2);
            assert_eq!(terminal.cursor().position(), Some((4.min(cols - 1), 0)));
            terminal.feed("X");
            terminal.engine.verify();
            assert_eq!(terminal.cursor().row, 0);

            assert_eq!(
                terminal.screen().line(0).unwrap().cells()[4.min(cols - 1)].char(),
                'X'
            );

            assert!(!terminal.screen().line(0).unwrap().is_wrapped());
        }
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

        for (input, cols, expected) in [
            ("ABCD", 2, (2, 1)),
            ("ABCD", 6, (4, 0)),
            ("AB界", 3, (2, 1)),
        ] {
            let mut terminal = Terminal::with_scrollback(4, 1, None).unwrap();
            terminal.feed(input);
            terminal.feed("\x1b[?1047h");
            terminal.resize(cols, 3);
            assert_eq!(terminal.engine.primary_cursor, expected);
        }
    }

    #[test]
    fn resizing_a_wide_character_to_one_column_erases_it() {
        for cols in [0, 1] {
            for suffix in ["", "\x1b[1G", "\x1b[2G"] {
                let mut terminal = Terminal::new(2, 2).unwrap();
                terminal.feed("界");
                terminal.feed(suffix);
                terminal.resize(cols, 2);
                terminal.engine.verify();
                assert_eq!(terminal.size(), (1, 2));
                assert_eq!(terminal.screen().text(), ["", ""]);
                assert_eq!(terminal.cursor().position(), Some((0, 0)));
                terminal.feed("a");
                assert_eq!(terminal.screen().line(0).unwrap().text(), "a");
            }
        }
    }

    #[test]
    fn reflow_preserves_cursor_and_print_continuation() {
        for (input, cols, rows, cursor, expected) in [
            ("ABCD", 6, 5, (4, 0), "ABCDX"),
            ("ABCD", 3, 5, (1, 1), "ABC\nDX"),
            ("ABCD", 2, 5, (2, 1), "AB\nCD\nX"),
            ("ABCD", 4, 6, (4, 0), "ABCD\nX"),
            ("ABCDEFGH", 6, 5, (2, 1), "ABCDEF\nGHX"),
            ("ABCD界", 5, 5, (2, 1), "ABCD\n界X"),
            ("ABCD界EF", 7, 5, (1, 1), "ABCD界E\nFX"),
            ("ABCD界EF", 8, 5, (8, 0), "ABCD界EF\nX"),
            ("AB界", 6, 5, (4, 0), "AB界X"),
            ("AB界", 3, 5, (2, 1), "AB\n界X"),
            ("ABC", 6, 5, (3, 0), "ABCX"),
            ("ABC", 3, 5, (3, 0), "ABC\nX"),
            ("ABCD\x1b[4G", 2, 5, (1, 1), "AB\nCX"),
            ("AB界\x1b[3G", 3, 5, (0, 1), "AB\nX"),
            ("AB界\x1b[4G", 3, 5, (1, 1), "AB\n X"),
            ("ABCD界EF\x1b[1G", 6, 5, (4, 0), "ABCDX\nEF"),
            ("ABCD界EF\x1b[2G", 6, 5, (5, 0), "ABCD X\nEF"),
            ("A\x1b[4G", 2, 5, (1, 0), "AX"),
            ("A\x1b[4G", 6, 5, (3, 0), "A  X"),
            ("ABCD", 1, 6, (1, 3), "A\nB\nC\nD\nX"),
        ] {
            for saved in [false, true] {
                let mut terminal = Terminal::with_scrollback(4, 1, None).unwrap();
                terminal.feed(input);

                if saved {
                    terminal.feed("\x1b7\x1b[H");
                }

                terminal.resize(cols, rows);

                if saved {
                    terminal.feed("\x1b8");
                }

                assert_eq!(
                    terminal.cursor().position(),
                    Some(cursor),
                    "{input:?} -> {cols}"
                );

                if saved {
                    terminal.feed("\x1b[H");
                }

                let mut restored = Terminal::with_scrollback(cols, rows, None).unwrap();
                restored.feed(terminal.dump());

                for term in [&mut terminal, &mut restored] {
                    if saved {
                        term.feed("\x1b8");
                    }

                    term.feed("X");
                    term.engine.verify();

                    let actual = term
                        .screen()
                        .visible_lines()
                        .map(|line| line.text().trim_end().to_owned())
                        .collect::<Vec<_>>()
                        .join("\n");

                    assert_eq!(actual.trim_end(), expected, "{input:?} -> {cols}");
                }
            }
        }
    }

    #[test]
    fn repeated_reflow_keeps_cursor_after_text() {
        let mut terminal = Terminal::with_scrollback(4, 1, None).unwrap();
        terminal.feed("ABCD");

        for (cols, cursor) in [(2, (2, 1)), (6, (4, 0)), (3, (1, 1)), (4, (4, 0))] {
            terminal.resize(cols, cursor.1 + 1);
            assert_eq!(terminal.cursor().position(), Some(cursor));
        }

        terminal.feed("X");
        assert_eq!(terminal.screen().text(), ["ABCDX"]);

        let mut terminal = Terminal::with_scrollback(4, 1, None).unwrap();
        terminal.feed("AB界");

        // The padding inserted before the wide character remains a space.
        for (cols, cursor) in [(3, (2, 1)), (5, (5, 0)), (3, (2, 1)), (6, (5, 0))] {
            terminal.resize(cols, cursor.1 + 1);
            assert_eq!(terminal.cursor().position(), Some(cursor));
        }

        terminal.feed("X");
        assert_eq!(terminal.screen().text(), ["AB 界X"]);
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
    fn save_restore_preserves_pending_wrap_in_both_buffers() {
        for alternate in [false, true] {
            for (save, restore) in [
                ("\x1b7", "\x1b8"),
                ("\x1b[s", "\x1b[u"),
                ("\x1b[?1048h", "\x1b[?1048l"),
            ] {
                let mut terminal = Terminal::new(4, 3).unwrap();

                if alternate {
                    terminal.feed("\x1b[?1047h");
                }

                terminal.feed(["AB界", save, "\x1b[H", restore]);
                assert_eq!(terminal.cursor().position(), Some((4, 0)));
                terminal.feed("X");
                assert_eq!(text(&terminal), "AB界\nX|\n");
            }
        }
    }

    #[test]
    fn offscreen_saved_cursor_returns_home_with_saved_attributes() {
        for (saved_row, live_row, expected) in [(1, 3, "XCCC"), (3, 1, "XAAA")] {
            let mut terminal = Terminal::with_scrollback(4, 3, None).unwrap();
            terminal.feed("AAAA\r\nBBBB\r\nCCCC");
            terminal.feed(format!(
                "\x1b[{saved_row};4H\x1b[1m{}\x1b7\x1b[0m\x1b[{live_row};1H",
                if saved_row == 1 { "A" } else { "C" }
            ));
            terminal.feed(format!("\x1b[?7l\x1b[?6h\x1b[{live_row};1H"));
            terminal.resize(4, 1);
            terminal.feed("\x1b8");
            assert_eq!(terminal.cursor().position(), Some((0, 0)));
            assert_eq!(terminal.engine.pen.intensity, Intensity::Bold);
            assert!(terminal.engine.auto_wrap_mode);
            assert!(!terminal.engine.origin_mode);
            terminal.feed("X");
            assert_eq!(terminal.screen().line(0).unwrap().text(), expected);
        }
    }

    #[test]
    fn saved_primary_wrap_survives_alternate_and_repeated_resize() {
        for alternate in [false, true] {
            let mut terminal = Terminal::new(4, 1).unwrap();
            terminal.feed("ABCD");
            terminal.feed(if alternate { "\x1b[?1049h" } else { "\x1b7" });
            terminal.feed("\x1b[H");
            terminal.resize(6, 2);
            terminal.resize(4, 3);
            terminal.feed(if alternate { "\x1b[?1049l" } else { "\x1b8" });
            assert_eq!(terminal.cursor().position(), Some((4, 0)));
            terminal.feed("X");
            assert_eq!(text(&terminal), "ABCD\nX|\n");
        }
    }

    #[test]
    fn alternate_saved_wrap_follows_clipping_policy() {
        for (cols, expected) in [(4, (4, 0)), (2, (1, 0)), (6, (4, 0))] {
            let mut terminal = Terminal::new(4, 2).unwrap();
            terminal.feed("\x1b[?1047hABCD\x1b7\x1b[H");
            terminal.resize(cols, 3);
            terminal.feed("\x1b8");
            assert_eq!(terminal.cursor().position(), Some(expected));
            terminal.feed("X");
            assert_eq!(terminal.cursor().row, usize::from(cols == 4));
        }
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

    fn sgr(op: SgrOp) -> Function<'static> {
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

        term.execute([print('d'), Lf]);

        assert_eq!(term.cursor().position().unwrap(), (4, 1));
        assert_eq!(text(&term), "   d\n    |");
    }

    #[test]
    fn linefeed_and_reverse_index_respect_scrolling_region() {
        // LF at the bottom margin and RI at the top affect only the region;
        // LF at the screen bottom below it does not scroll.
        for (input, expected, row) in [
            ("\x1b[4;2H\n", ["AAAA", "CCCC", "DDDD", "    ", "EEEE"], 3),
            (
                "\x1b[2;2H\x1bM",
                ["AAAA", "    ", "BBBB", "CCCC", "EEEE"],
                1,
            ),
            ("\x1b[5;2H\n", ["AAAA", "BBBB", "CCCC", "DDDD", "EEEE"], 4),
        ] {
            let mut term = Terminal::new(4, 5).unwrap();
            term.feed("AAAA\r\nBBBB\r\nCCCC\r\nDDDD\r\nEEEE");
            term.feed(["\x1b[2;4r", input]);
            let lines: Vec<_> = term.screen().visible_lines().map(Line::text).collect();
            assert_eq!(lines, expected, "{input:?}");
            assert_eq!(term.cursor().position(), Some((1, row)), "{input:?}");
            assert_eq!(wrapped(&term), [false; 5], "{input:?}");
        }
    }

    #[test]
    fn execute_cr() {
        let mut term = Terminal::with_scrollback(4, 2, None).unwrap();

        term.feed("abc");
        term.execute([Cr, print('d')]);

        assert_eq!(term.cursor().position().unwrap(), (1, 0));
        assert_eq!(text(&term), "d|bc\n");
    }

    #[test]
    fn cr_and_cup_cancel_pending_wrap() {
        // CR and CUP cancel pending wrap, so the next character is printed
        // at the new cursor position without wrapping.
        for (input, expected, col) in [("\rX", "XBCD", 1), ("\x1b[1;2HX", "AXCD", 2)] {
            let mut term = Terminal::new(4, 2).unwrap();
            term.feed(["ABCD", input]);
            let lines: Vec<_> = term.screen().visible_lines().map(Line::text).collect();
            assert_eq!(lines, [expected, "    "], "{input:?}");
            assert_eq!(term.cursor().position(), Some((col, 0)), "{input:?}");
            assert_eq!(wrapped(&term), [false, false], "{input:?}");
        }
    }

    #[test]
    fn execute_nel() {
        let mut term = build_term(8, 2, 3, 0, "abc");

        term.execute(Nel);

        assert_eq!(term.cursor().position().unwrap(), (0, 1));
        assert_eq!(text(&term), "abc\n|");

        term.execute(print('d'));

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

        term.execute(print('a'));

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
    fn clearing_tab_stops_preserves_cursor_position() {
        // TBC clears tab stops without moving the cursor. HT then advances
        // to the next remaining stop, or the last column if none remains.
        let mut term = Terminal::new(20, 1).unwrap();
        term.feed("\x1b[1;9H\x1b[0g");
        assert_eq!(term.cursor().position(), Some((8, 0)));
        term.feed("\r\t");
        assert_eq!(term.cursor().position(), Some((16, 0)));

        // Keep both default stops present, with the cursor at neither one,
        // so clearing only the current stop cannot satisfy the TBC 3 case.
        let mut term = Terminal::new(20, 1).unwrap();
        term.feed("\x1b[1;6H\x1b[3g");
        assert_eq!(term.cursor().position(), Some((5, 0)));
        term.feed("\r\t");
        assert_eq!(term.cursor().position(), Some((19, 0)));
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

        term.execute([print('A'), Rep(0)]);

        assert_eq!(text(&term), "AA|\n");

        term.execute(Rep(3));

        assert_eq!(text(&term), "AAAAA|\n");

        term.execute([Cuf(5), Rep(0)]);

        assert_eq!(text(&term), "AAAAA      |\n");
    }

    #[test]
    fn execute_rep_after_wide_char() {
        let mut term = build_term(20, 2, 0, 0, "");

        term.execute([print('ハ'), Rep(3)]);

        assert_eq!(text(&term), "ハハハハ|\n");
    }

    #[test]
    fn execute_rep_after_moving_into_wide_char_tail() {
        let mut term = build_term(20, 2, 0, 0, "");

        term.execute([print('ハ'), Cub(1), Rep(3)]);

        assert_eq!(text(&term), " ハハハ|\n");
    }

    #[test]
    fn dump_uses_rep_for_wide_char_runs() {
        let mut term = Engine::new((20, 2), None);

        term.execute_with(&Print("ハハハハハハ".into()), &mut ());

        let funs = term.dump();

        assert_eq!(funs[..2], [Print("ハ".into()), Rep(5)]);
    }

    #[test]
    fn dump_preserves_styles_and_counts_wide_glyphs_for_rep() {
        let mut original = Terminal::new(48, 2).unwrap();
        original.feed("A界界界界界界 \x1b[31m界界界\x1b[32m界界界界界界Z");
        let funs = original.engine.dump();

        assert_eq!(
            funs[..9],
            [
                Print("A界".into()),
                Rep(5),
                Print(" ".into()),
                sgr(SetForegroundColor(Color::Indexed(1))),
                Print("界界界".into()),
                sgr(SetForegroundColor(Color::Indexed(2))),
                Print("界".into()),
                Rep(5),
                Print("Z".into()),
            ]
        );

        let mut restored = Terminal::new(48, 2).unwrap();
        restored.feed(original.dump());
        original.engine.assert_dump_equivalent(&restored.engine);
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

        // EL2 clears an unwrapped row with default erase attributes even
        // outside the vertical margins, without moving the cursor.
        for (position, expected, row) in [
            ("\x1b[3;2H", ["AAAA", "BBBB", "    ", "DDDD", "EEEE"], 2),
            ("\x1b[5;2H", ["AAAA", "BBBB", "CCCC", "DDDD", "    "], 4),
        ] {
            let mut term = Terminal::new(4, 5).unwrap();
            term.feed("AAAA\r\nBBBB\r\nCCCC\r\nDDDD\r\nEEEE");
            term.feed(["\x1b[2;4r", position, "\x1b[2K"]);
            let lines: Vec<_> = term.screen().visible_lines().map(Line::text).collect();
            assert_eq!(lines, expected, "{position:?}");
            assert_eq!(term.cursor().position(), Some((1, row)), "{position:?}");
        }
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

        // ECH covering both cells of a wide glyph replaces them with
        // single-width blanks without moving the cursor.
        let mut term = Terminal::new(4, 2).unwrap();
        term.feed("A界B\x1b[1;2H\x1b[2X");
        let lines: Vec<_> = term.screen().visible_lines().map(Line::text).collect();
        assert_eq!(lines, ["A  B", "    "]);
        assert_eq!(term.cursor().position(), Some((1, 0)));

        assert!(
            term.screen()
                .line(0)
                .unwrap()
                .cells()
                .iter()
                .all(|cell| cell.width() == 1)
        );

        // Erasing either half clears the whole wide glyph using the current
        // erase attributes, including the otherwise untouched half.
        let erase_pen = pen(|pen| {
            pen.foreground = Some(Color::Indexed(1));
            pen.background = Some(Color::Indexed(4));
            pen.intensity = Intensity::Bold;
        });

        for alternate in [false, true] {
            for column in [2, 3] {
                let mut term = Terminal::new(4, 2).unwrap();

                if alternate {
                    term.feed("\x1b[?1047h");
                }

                let mut dirty = DirtyRows::new();
                term.feed_with(format!("A界B\x1b[1;{column}H\x1b[1;31;44m"), &mut dirty);

                let rows = term.feed_with("\x1b[X", &mut dirty);
                assert!(rows.contains(&0));
                assert_eq!(term.cursor().position(), Some((column - 1, 0)));

                assert_eq!(
                    term.screen()
                        .line(0)
                        .unwrap()
                        .cells()
                        .iter()
                        .map(|cell| (cell.char(), cell.width(), *cell.pen()))
                        .collect::<Vec<_>>(),
                    [
                        ('A', 1, Pen::default()),
                        (' ', 1, erase_pen),
                        (' ', 1, erase_pen),
                        ('B', 1, Pen::default()),
                    ]
                );
            }
        }
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
    fn execute_sc_rc_restores_pen_and_modes() {
        fn assert_save_restore(save: Function<'_>, restore: Function<'_>) {
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

        // DECSC/DECRC restore cursor position and SGR attributes.
        let mut term = Terminal::new(6, 3).unwrap();
        term.feed("\x1b[2;3H\x1b[1;31m\x1b7\x1b[3;1H\x1b[0mY\x1b8");
        assert_eq!(term.cursor().position(), Some((2, 1)));
        term.feed("X");
        let lines: Vec<_> = term.screen().visible_lines().map(Line::text).collect();
        assert_eq!(lines, ["      ", "  X   ", "Y     "]);
        assert_eq!(term.cursor().position(), Some((3, 1)));
        let screen = term.screen();
        let pen = screen.line(1).unwrap().cells()[2].pen();
        assert!(pen.is_bold());
        assert_eq!(pen.foreground(), Some(Color::Indexed(1)));
        assert!(screen.line(2).unwrap().cells()[0].pen().is_default());
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
    fn autowrap_toggles_at_right_edge() {
        // Mode toggles preserve the cursor's pending-wrap state.
        // Enabling autowrap at the last column does not create pending wrap.
        for alternate in [false, true] {
            for (initial, toggle, input, expected, cursor, wraps) in [
                ("ABCD", "\x1b[?7l", "X", ["ABCX", "    "], (3, 0), false),
                (
                    "ABCD",
                    "\x1b[?7l\x1b[?7h",
                    "X",
                    ["ABCD", "X   "],
                    (1, 1),
                    true,
                ),
                (
                    "\x1b[?7lABCD",
                    "\x1b[?7h",
                    "XY",
                    ["ABCX", "Y   "],
                    (1, 1),
                    true,
                ),
            ] {
                let mut term = Terminal::new(4, 2).unwrap();

                if alternate {
                    term.feed("\x1b[?1047h");
                }

                term.feed(initial);
                let before = term.cursor();
                term.feed(toggle);
                assert_eq!(term.cursor(), before);
                assert_eq!(wrapped(&term), [false, false]);
                term.feed(input);
                let lines: Vec<_> = term.screen().visible_lines().map(Line::text).collect();
                assert_eq!(lines, expected);
                assert_eq!(term.cursor().position(), Some(cursor));
                assert_eq!(wrapped(&term), [wraps, false]);
            }
        }
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

            if alternate {
                assert_eq!(line.text().trim_end(), "ABCD");
                assert_eq!(screen.line(3).unwrap().text().trim_end(), "E");
                assert!(line.is_wrapped());
            } else {
                assert_eq!(line.text().trim_end(), "ABCDE");
                assert!(!line.is_wrapped());
            }

            let mut replayed = Terminal::new(8, 5).unwrap();
            replayed.feed(terminal.dump());
            terminal.engine.assert_dump_equivalent(&replayed.engine);
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
    fn insert_mode_shifts_by_character_width() {
        let mut term = Terminal::new(5, 2).unwrap();
        term.feed("hello\x1b[1;2H\x1b[4h界");
        assert_eq!(term.screen().line(0).unwrap().text(), "h界el");
        assert_eq!(term.cursor().position(), Some((3, 0)));
        term.engine.verify();
    }

    #[test]
    fn insert_mode_applies_at_the_wrapped_destination() {
        for (prefix, input, expected) in [
            ("abcd", "X", "Xabc"),
            ("abcd", "界", "界ab"),
            ("abc", "界", "界ab"),
        ] {
            let mut term = Terminal::new(4, 2).unwrap();
            term.feed("\x1b[2;1Habcd\x1b[H");
            term.feed(prefix);
            term.feed("\x1b[4h");
            term.feed(input);
            assert_eq!(term.screen().line(1).unwrap().text(), expected);
            term.engine.verify();
        }
    }

    #[test]
    fn insert_mode_without_autowrap_fits_at_the_right_edge() {
        let mut term = Terminal::new(4, 2).unwrap();
        term.feed("abcd\x1b[1;4H\x1b[?7l\x1b[4h界");
        assert_eq!(term.screen().line(0).unwrap().text(), "ab界");
        assert_eq!(term.cursor().position(), Some((3, 0)));
        assert_eq!(wrapped(&term), [false, false]);
        term.engine.verify();
    }

    #[test]
    fn one_column_insert_rejects_wide_input_without_cell_edits() {
        for autowrap in [false, true] {
            let mut term = Terminal::new(1, 2).unwrap();
            term.feed("xy\x1b[H\x1b[4h");
            let anchor = term.anchor_at_cursor();
            let position = term.resolve(&anchor);

            if !autowrap {
                term.feed("\x1b[?7l");
            }

            term.feed("界");
            assert_eq!(term.screen().line(0).unwrap().text(), "x");
            assert_eq!(term.screen().line(1).unwrap().text(), "y");
            assert_eq!(term.resolve(&anchor), position);
            assert_eq!(term.cursor().position(), Some((0, usize::from(autowrap))));
            term.engine.verify();
        }
    }

    #[test]
    fn rejected_insert_placement_preserves_cells_and_anchors() {
        let mut term = Terminal::new(2, 2).unwrap();
        term.feed("界\x1b[H");
        let head = term.anchor_at_cursor();
        term.feed("\x1b[C");
        let tail = term.anchor_at_cursor();
        term.feed("\x1b[4h");
        let before = term.screen().line(0).unwrap().clone();
        let cursor = term.cursor();
        let head_pos = term.resolve(&head);
        let tail_pos = term.resolve(&tail);
        assert_eq!(term.engine.try_print('界'), None);
        assert_eq!(term.screen().line(0).unwrap(), &before);
        assert_eq!(term.cursor(), cursor);
        assert_eq!(term.resolve(&head), head_pos);
        assert_eq!(term.resolve(&tail), tail_pos);
        term.engine.verify();
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
    fn wide_character_fitting_right_edge_wraps_on_next_print() {
        let mut term = Terminal::new(4, 2).unwrap();
        let mut dirty = DirtyRows::new();
        term.feed_with("AB", &mut dirty);
        let rows = term.feed_with("界", &mut dirty);
        assert!(rows.contains(&0));
        let lines: Vec<_> = term.screen().visible_lines().map(Line::text).collect();
        assert_eq!(lines, ["AB界", "    "]);
        assert_eq!(term.cursor().row, 0);
        assert_eq!(wrapped(&term), [false, false]);

        // Row 0's wrap flag changes, but only row 1's cells need repainting.
        let rows = term.feed_with("X", &mut dirty);
        assert!(rows.contains(&1));
        let lines: Vec<_> = term.screen().visible_lines().map(Line::text).collect();
        assert_eq!(lines, ["AB界", "X   "]);
        assert_eq!(term.cursor().position(), Some((1, 1)));
        assert_eq!(wrapped(&term), [true, false]);

        assert_eq!(
            term.screen()
                .line(0)
                .unwrap()
                .cells()
                .iter()
                .map(|cell| cell.width())
                .collect::<Vec<_>>(),
            [1, 1, 2, 0]
        );
    }

    #[test]
    fn print_wide_char_on_wide_tail_with_one_col_right_and_autowrap() {
        let mut term = Terminal::with_scrollback(4, 2, None).unwrap();

        term.feed("Aハz");
        term.execute([Cub(1), print('界')]);

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
        term.execute([Cub(1), print('界')]);

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
    fn wide_char_wrap_preserves_occupied_last_column() {
        let mut term = Terminal::with_scrollback(4, 2, None).unwrap();

        term.feed("abcz");
        term.execute([Cup(1, 4), print('界')]);

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

        assert_eq!(text(&term), "qrst\nuw\nAAAB\nBBBB|\n");

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

        // The saved primary cursor follows its insertion position through reflow.
        assert_eq!(text(&term), "bbb\nc\nddd|");
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
        let mut term = Terminal::new(4, 5).unwrap();

        // DECSTBM homes the cursor at the screen origin with origin mode off.
        // With origin mode on, CUP addresses rows relative to the top margin.
        term.feed("\x1b[4;4H\x1b[2;4r");
        assert_eq!(term.cursor().position(), Some((0, 0)));
        term.feed("\x1b[?6h");

        assert_eq!(term.cursor().position().unwrap(), (0, 1));

        term.feed("\x1b[2;3H");
        assert_eq!(term.cursor().position(), Some((2, 2)));
        term.feed("X");
        let lines: Vec<_> = term.screen().visible_lines().map(Line::text).collect();
        assert_eq!(lines, ["    ", "    ", "  X ", "    ", "    "]);
        assert_eq!(term.cursor().position(), Some((3, 2)));

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
            print('a'),
            print('b'),
            Decset(dec_modes([DecMode::AltScreenBuffer])),
            print('x'),
            print('y'),
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
        assert_eq!(term.alternate_saved_cursor.unwrap().cursor_col, 2);
        assert_eq!(term.alternate_saved_cursor.unwrap().cursor_row, 2);
        assert_eq!(term.pen.intensity, Intensity::Bold);

        term.execute_with(&Decstr, &mut ());

        assert_eq!(term.active_screen_kind(), ScreenKind::Alternate);
        assert_eq!(term.cursor().position().unwrap(), (2, 2));
        assert_eq!(term.active_screen().text(), ["  xy", "", ""]);
        assert!(term.cursor.visible);
        assert!(!term.insert_mode);
        assert!(!term.origin_mode);
        assert!(term.auto_wrap_mode);
        assert!(term.new_line_mode);
        assert!(!term.cursor_keys_app_mode());
        assert_eq!(term.top_margin, 0);
        assert_eq!(term.bottom_margin, 2);
        assert_eq!(term.charsets, [Charset::Ascii, Charset::Ascii]);
        assert_eq!(term.active_charset, 0);

        assert_eq!(term.alternate_saved_cursor, None);

        assert_eq!(term.pen, crate::pen::Pen::default());
    }

    #[test]
    fn decstr_reenables_autowrap_and_resets_cursor_keys() {
        let mut term = Terminal::new(4, 3).unwrap();
        term.feed("\x1b[?7l\x1b[?1habcd");
        assert_eq!(term.cursor().position(), Some((3, 0)));
        assert!(term.cursor_key_app_mode());
        term.feed("\x1b[!p");
        assert_eq!(term.cursor().position(), Some((3, 0)));
        assert!(!term.cursor_key_app_mode());
        term.feed("XY");
        assert_eq!(term.screen().line(0).unwrap().text(), "abcX");
        assert_eq!(term.screen().line(1).unwrap().text(), "Y   ");
        assert!(term.screen().line(0).unwrap().is_wrapped());
        assert_eq!(term.cursor().position(), Some((1, 1)));
    }

    #[test]
    fn decstr_preserves_pending_wrap_buffers_tabs_and_newline() {
        for alternate in [false, true] {
            let mut term = Terminal::with_scrollback(12, 3, None).unwrap();
            term.feed("old1\r\nold2\r\nold3\r\nold4");

            if alternate {
                term.feed("\x1b[?1047h");
            }

            // Replace default tab stops with one at column 4, enable LNM,
            // and leave the live cursor with a pending wrap on the first row.
            term.feed("\x1b[3g\x1b[4G\x1bH\x1b[20h\x1b[Habcdefghijkl");
            let active = term.active_screen_kind();
            let cursor = term.cursor();
            let visible: Vec<_> = term.screen().visible_lines().cloned().collect();
            let primary: Vec<_> = term.primary_screen().all_lines().cloned().collect();
            assert_eq!(cursor.position(), Some((12, 0)));
            assert_eq!(primary.len(), 4);
            term.feed("\x1b[!p");
            assert_eq!(term.size(), (12, 3));
            assert_eq!(term.active_screen_kind(), active);
            assert_eq!(term.cursor(), cursor);
            assert!(term.screen().visible_lines().eq(visible.iter()));
            assert!(term.primary_screen().all_lines().eq(primary.iter()));
            term.feed("Z");
            assert_eq!(term.cursor().position(), Some((1, 1)));
            assert_eq!(term.screen().line(0).unwrap().text(), "abcdefghijkl");
            assert!(term.screen().line(0).unwrap().is_wrapped());
            assert_eq!(term.screen().line(1).unwrap().cells()[0].char(), 'Z');
            term.feed("\r\tX\nY");
            assert_eq!(term.screen().line(1).unwrap().cells()[3].char(), 'X');
            assert_eq!(term.screen().line(2).unwrap().cells()[0].char(), 'Y');
            assert_eq!(term.cursor().position(), Some((1, 2)));
        }
    }

    #[test]
    fn execute_ris() {
        let mut term = Engine::new((4, 3), None);

        for function in [
            print('a'),
            print('b'),
            Cr,
            Lf,
            print('c'),
            print('d'),
            sgr(SetBoldIntensity),
            Decset(dec_modes([
                DecMode::CursorKeys,
                DecMode::SaveCursorAltScreenBuffer,
            ])),
            print('z'),
            print('z'),
            Decsc,
            Ris,
        ] {
            term.execute_with(&function, &mut ());
        }

        assert_eq!(term.active_screen_kind(), ScreenKind::Primary);
        assert_eq!(term.cursor().position().unwrap(), (0, 0));
        assert_eq!(term.active_screen().text(), ["", "", ""]);
        assert_eq!(term.pen, crate::pen::Pen::default());
        assert!(!term.cursor_keys_app_mode());
        assert!(term.primary_saved_cursor.is_none());
        assert!(term.alternate_saved_cursor.is_none());
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
}
