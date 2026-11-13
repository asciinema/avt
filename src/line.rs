use crate::anchor::{Anchor, Mark};
use crate::cell::{Cell, Occupancy, display_width};
use crate::pen::Pen;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::ops::{Index, IndexMut, Range, RangeFull};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum CharWidth {
    Single,
    Double,
}

impl CharWidth {
    fn as_usize(self) -> usize {
        match self {
            CharWidth::Single => 1,
            CharWidth::Double => 2,
        }
    }
}

/// One physical row. Cell edits maintain wide pairs and row-local anchors;
/// the buffer coordinates relationships and anchor transfers between rows.
pub struct Line {
    cells: Vec<Cell>,
    wrapped: bool,
    marks: Vec<Mark>,
}

// A cell followed during reflow; `after` distinguishes pending wrap from a
// cursor on the final cell. Unlike public anchors, this survives erased cells.
pub(crate) struct ReflowCursor {
    pub col: usize,
    pub after: bool,
}

// Slot 0 is live, slot 1 is saved; tracked only for the duration of a resize.
pub(crate) type ReflowCursors = [Option<ReflowCursor>; 2];

impl ReflowCursor {
    // An insertion position immediately after retained text follows its last
    // cell when the blank cell at that position is trimmed away.
    fn adjust_for_trim(&mut self, len: usize) {
        if !self.after && len > 0 && self.col == len {
            self.col -= 1;
            self.after = true;
        }
    }

    pub fn clamp(&mut self, width: usize) {
        if self.col >= width {
            self.col = width - 1;
            self.after = false;
        }
    }
}

// Frames and evicted-line copies are visual values, not tracking registrations.
impl Clone for Line {
    fn clone(&self) -> Self {
        Self {
            cells: self.cells.clone(),
            wrapped: self.wrapped,
            marks: Vec::new(),
        }
    }
}

impl PartialEq for Line {
    fn eq(&self, other: &Self) -> bool {
        self.cells == other.cells && self.wrapped == other.wrapped
    }
}

impl Line {
    pub(crate) fn blank(cols: usize, pen: Pen) -> Self {
        Line {
            cells: vec![Cell::blank(pen); cols],
            wrapped: false,
            marks: Vec::new(),
        }
    }

    pub(crate) fn reset(&mut self, pen: Pen) {
        self.marks.clear();
        self.cells.fill(Cell::blank(pen));
        self.wrapped = false;
    }

    pub(crate) fn clear(&mut self, range: Range<usize>, pen: &Pen) {
        if range.is_empty() || range.start == self.len() {
            return;
        }

        let width = self.len();

        let start =
            if range.start < width && self.cells[range.start].occupancy() == Occupancy::WideTail {
                range.start - 1
            } else {
                range.start
            };

        let end = if range.end < width && self.cells[range.end].occupancy() == Occupancy::WideTail {
            range.end + 1
        } else {
            range.end
        };

        self.marks
            .retain(|mark| mark.live() && !(start <= mark.col && (mark.col < end || end == width)));

        self.cells[start..end].fill(Cell::blank(*pen));
    }

    pub(crate) fn replace_char(&mut self, col: usize, ch: char, pen: Pen) -> Option<usize> {
        let mut buf = [0u8; 4];
        self.replace_cluster(col, ch.encode_utf8(&mut buf), pen)
    }

    /// Replaces cells with one grapheme cluster, returning its cell width.
    /// Rejection leaves the entire row untouched, including anchors and wrapping.
    pub(crate) fn replace_cluster(&mut self, col: usize, cluster: &str, pen: Pen) -> Option<usize> {
        let char_width = self.cluster_display_width(cluster);
        let width = char_width.as_usize();

        if width > self.len().saturating_sub(col) {
            return None;
        }

        // Repair wide pairs crossing either edge without invalidating anchors.
        if self.cells[col].occupancy() == Occupancy::WideTail {
            debug_assert!(col > 0);
            debug_assert_eq!(self.cells[col - 1].occupancy(), Occupancy::WideHead);

            self.cells[col - 1].set(" ", Occupancy::Single, pen);
        }

        if let Some(tail) = self.cells.get_mut(col + width) {
            if tail.occupancy() == Occupancy::WideTail {
                tail.set(" ", Occupancy::Single, pen);
            }
        }

        match char_width {
            CharWidth::Single => {
                self.cells[col].set(cluster, Occupancy::Single, pen);
            }

            CharWidth::Double => {
                self.cells[col].set(cluster, Occupancy::WideHead, pen);
                self.cells[col + 1].set(" ", Occupancy::WideTail, pen);
            }
        }

        Some(width)
    }

    /// Replaces a printable ASCII prefix, bounded by this row, repairing wide
    /// pairs crossing either edge. Returns bytes consumed; zero leaves all state
    /// untouched.
    #[inline]
    pub(crate) fn replace_ascii_prefix(&mut self, col: usize, text: &str, pen: Pen) -> usize {
        let Some(cells) = self.cells.get_mut(col..) else {
            return 0;
        };

        let mut written = 0;

        for (cell, byte) in cells.iter_mut().zip(text.bytes()) {
            if !(b' '..=b'~').contains(&byte) {
                break;
            }

            // Printable ASCII is a single UTF-8 byte.
            cell.set(
                core::str::from_utf8(&[byte]).unwrap(),
                Occupancy::Single,
                pen,
            );
            written += 1;
        }

        if written == 0 {
            return 0;
        }

        // Only pairs crossing the edges can have a surviving half. Inspect the
        // untouched neighbors after writing, preserving anchors and wrapping.
        if col > 0 && self.cells[col - 1].occupancy() == Occupancy::WideHead {
            self.cells[col - 1].set(" ", Occupancy::Single, pen);
        }

        if let Some(tail) = self.cells.get_mut(col + written) {
            if tail.occupancy() == Occupancy::WideTail {
                tail.set(" ", Occupancy::Single, pen);
            }
        }

        written
    }

    #[cfg(test)]
    pub(crate) fn insert_char(&mut self, col: usize, ch: char, pen: Pen) -> Option<usize> {
        let mut buf = [0u8; 4];
        self.insert_cluster(col, ch.encode_utf8(&mut buf), pen)
    }

    /// Inserts one grapheme cluster, returning its cell width. Rejection leaves
    /// the entire row untouched, including anchor registrations and wrapping.
    pub(crate) fn insert_cluster(&mut self, col: usize, cluster: &str, pen: Pen) -> Option<usize> {
        let char_width = self.cluster_display_width(cluster);
        let width = char_width.as_usize();

        if width > self.len().saturating_sub(col) {
            return None;
        }

        self.insert_blanks(col, width, pen);

        // Insertion repairs wide pairs and leaves a gap of single-width blanks.
        match char_width {
            CharWidth::Single => {
                self.cells[col].set(cluster, Occupancy::Single, pen);
            }

            CharWidth::Double => {
                self.cells[col].set(cluster, Occupancy::WideHead, pen);
                self.cells[col + 1].set(" ", Occupancy::WideTail, pen);
            }
        }

        Some(width)
    }

    /// Inserts blanks, clipping at the right edge. Anchors at the insertion
    /// column stay before the inserted cells; later anchors follow shifted cells.
    pub(crate) fn insert_blanks(&mut self, col: usize, n: usize, pen: Pen) {
        let n = n.min(self.len().saturating_sub(col));

        if n == 0 {
            return;
        }

        let width = self.len();

        self.marks.retain_mut(|mark| {
            if mark.col > col {
                mark.col += n;

                if mark.col >= width {
                    return false;
                }
            }

            mark.live()
        });

        let cur_cell = &mut self.cells[col];

        if cur_cell.occupancy() == Occupancy::WideTail {
            self.marks.retain(|mark| mark.col != col - 1);
            cur_cell.set(" ", Occupancy::Single, pen);
            self.cells[col - 1].set(" ", Occupancy::Single, pen);
        }

        self.cells[col..].rotate_right(n);

        let cur_cell = &mut self.cells[col];

        if cur_cell.occupancy() == Occupancy::WideTail {
            self.marks.retain(|mark| mark.col != width - 1);
            cur_cell.set(" ", Occupancy::Single, pen);

            self.cells
                .last_mut()
                .unwrap()
                .set(" ", Occupancy::Single, pen);
        }

        // Preserve anchors at the insertion boundary when filling the gap.
        self.cells[col..col + n].fill(Cell::blank(pen));
    }

    pub(crate) fn delete(&mut self, col: usize, n: usize, pen: &Pen) {
        if n == 0 {
            return;
        }

        self.marks.retain_mut(|mark| {
            if (col..col + n).contains(&mark.col) {
                return false;
            }

            if mark.col >= col + n {
                mark.col -= n;
            }

            mark.live()
        });

        if self.cells[col].occupancy() == Occupancy::WideTail {
            self.marks.retain(|mark| mark.col != col - 1);
            self.cells[col - 1].set(" ", Occupancy::Single, *pen);
        }

        self.cells[col..].rotate_left(n);

        let cur_cell = &mut self.cells[col];

        if cur_cell.occupancy() == Occupancy::WideTail {
            self.marks.retain(|mark| mark.col != col);
            cur_cell.set(" ", Occupancy::Single, *pen);
        }

        let fill_start = self.cells.len() - n;
        self.cells[fill_start..].fill(Cell::blank(*pen));
    }

    /// Reflows toward `len` columns; the current row must be shorter than `len`.
    /// Pads hard-ended rows, or consumes the successor for soft-wrapped rows.
    /// Preserves wide pairs and moves anchors and tracked positions with cells.
    /// Returns any unconsumed successor and its tracked positions; `None` means
    /// the successor was fully consumed.
    pub(crate) fn append_for_reflow(
        &mut self,
        mut other: Line,
        len: usize,
        cursor: &mut ReflowCursors,
        mut other_cursor: ReflowCursors,
    ) -> Option<(Line, ReflowCursors)> {
        let mut needed = len - self.len();

        if !self.wrapped {
            self.expand(len);

            return Some((other, other_cursor));
        }

        if !other.wrapped {
            other.trim();

            for c in other_cursor.iter_mut().flatten() {
                c.adjust_for_trim(other.len());
            }
        }

        if needed < other.len() {
            if other[needed].occupancy() == Occupancy::WideTail {
                needed -= 1;
                let pen = self.cells[self.len() - 1].pen();
                self.cells.push(Cell::new(" ", Occupancy::Single, *pen));
            }

            let rest_marks = other.split_marks(needed);

            for (cursor, other_cursor) in cursor.iter_mut().zip(&mut other_cursor) {
                if other_cursor.as_ref().is_some_and(|c| c.col < needed) {
                    *cursor = other_cursor.take().map(|mut c| {
                        c.col += self.len();

                        c
                    });
                } else if let Some(c) = other_cursor {
                    c.col -= needed;
                }
            }

            self.append_marks(&mut other, self.len());
            self.cells.extend(&other[0..needed]);
            let mut cells = other.cells;
            cells.rotate_left(needed);
            cells.truncate(cells.len() - needed);

            return Some((
                Line {
                    cells,
                    wrapped: other.wrapped,
                    marks: rest_marks,
                },
                other_cursor,
            ));
        }

        for (cursor, other_cursor) in cursor.iter_mut().zip(other_cursor) {
            if let Some(mut c) = other_cursor {
                c.col += self.len();
                *cursor = Some(c);
            }
        }

        self.append_marks(&mut other, self.len());
        self.cells.extend(&other[..]);

        if !other.wrapped {
            self.wrapped = false;
            self.expand(len);
        }

        None
    }

    /// Resizes a row without reflow, preserving its soft-wrap relationship.
    pub(crate) fn resize(&mut self, len: usize) {
        debug_assert!(len > 0);

        if len < self.len() {
            let pen = *self.cells[len - 1].pen();
            self.clear(len..self.len(), &pen);
            self.cells.truncate(len);
        } else {
            self.expand(len);
        }
    }

    pub(crate) fn expand(&mut self, len: usize) {
        let filler = core::iter::repeat_n(Cell::blank(Pen::default()), len - self.len());
        self.cells.extend(filler);
    }

    /// Splits a row at `len` columns during reflow, trimming default trailing
    /// cells on hard-ended rows and keeping wide pairs together. The prefix
    /// stays in `self`; any remainder carries its anchors and tracked positions.
    /// Returns `None` when no remainder is needed, even if trimming changed the
    /// row. Wide cells that cannot fit on a one-column row are erased.
    /// The target width must be positive and smaller than the current width.
    pub(crate) fn split_for_reflow(
        &mut self,
        mut len: usize,
        cursor: &mut ReflowCursors,
    ) -> Option<(Line, ReflowCursors)> {
        // A wide pair cannot fit on a one-column row, even after wrapping.
        if len == 1 && self.cells[0].occupancy() == Occupancy::WideHead {
            let pen = *self.cells[0].pen();
            self.clear(0..2, &pen);

            for c in cursor.iter_mut().flatten().filter(|c| c.col < 2) {
                c.after = false;
            }
        }

        if !self.wrapped {
            let trimmed_len = self.len() - self.trailers();

            for c in cursor.iter_mut().flatten() {
                c.adjust_for_trim(trimmed_len);
            }

            self.cells.truncate(len.max(trimmed_len));
        }

        if self.len() > len {
            let wide_char_boundary = self.cells[len].occupancy() == Occupancy::WideTail;

            if wide_char_boundary {
                len -= 1;
            }

            let rest = Line {
                cells: self.cells.split_off(len),
                wrapped: self.wrapped,
                marks: self.split_marks(len),
            };

            let rest_cursor = core::array::from_fn(|i| {
                let cursor = &mut cursor[i];

                if cursor.as_ref().is_some_and(|c| c.col >= len) {
                    cursor.take().map(|mut c| {
                        c.col -= len;

                        c
                    })
                } else {
                    None
                }
            });

            if wide_char_boundary {
                let pen = self.cells[self.cells.len() - 1].pen();
                self.cells.push(Cell::new(" ", Occupancy::Single, *pen));
            }

            self.wrapped = true;

            Some((rest, rest_cursor))
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub(crate) fn anchor_at(&mut self, col: usize) -> Option<Anchor> {
        if col > self.len() {
            return None;
        }

        self.marks.retain(Mark::live);
        let anchor = Anchor(Arc::new(()));

        self.marks.push(Mark {
            token: Arc::downgrade(&anchor.0),
            col,
        });

        Some(anchor)
    }

    pub(crate) fn has_anchor_at(&self, col: usize) -> bool {
        self.marks.iter().any(|mark| mark.col == col && mark.live())
    }

    /// Receives registrations already expressed in this row's coordinates.
    pub(crate) fn add_marks(&mut self, marks: Vec<Mark>) {
        self.marks.extend(marks);
    }

    pub(crate) fn anchor_column(&self, anchor: &Anchor) -> Option<usize> {
        self.marks
            .iter()
            .find(|mark| mark.matches(anchor))
            .map(|mark| mark.col)
    }

    pub(crate) fn split_marks(&mut self, col: usize) -> Vec<Mark> {
        let mut rest = Vec::new();

        self.marks.retain_mut(|mark| {
            if !mark.live() {
                return false;
            }

            if mark.col >= col {
                rest.push(Mark {
                    token: mark.token.clone(),
                    col: mark.col - col,
                });

                false
            } else {
                true
            }
        });

        rest
    }

    fn append_marks(&mut self, other: &mut Line, offset: usize) {
        self.marks.extend(
            other
                .marks
                .drain(..)
                .filter(|mark| mark.live())
                .map(|mut mark| {
                    mark.col += offset;

                    mark
                }),
        );
    }

    pub(crate) fn clip_marks(&mut self) {
        let width = self.len();
        self.marks.retain(|mark| mark.live() && mark.col <= width);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// Whether this row continues onto the next row through a soft wrap.
    pub fn is_wrapped(&self) -> bool {
        self.wrapped
    }

    pub(crate) fn set_wrapped(&mut self, wrapped: bool) {
        self.wrapped = wrapped;
    }

    pub fn chunks<'a>(
        &'a self,
        predicate: impl Fn(&Cell, &Cell) -> bool + 'a,
    ) -> impl Iterator<Item = Vec<Cell>> + 'a {
        let i = self
            .cells
            .iter()
            .filter(|c| c.occupancy() != Occupancy::WideTail);

        Chunks::new(i, predicate)
    }

    pub fn chars(&self) -> impl Iterator<Item = char> + '_ {
        self.cells
            .iter()
            .filter(|cell| cell.occupancy() != Occupancy::WideTail)
            .flat_map(|cell| cell.contents().chars())
    }

    pub fn text(&self) -> String {
        self.chars().collect()
    }

    fn trim(&mut self) {
        let trailers = self.trailers();

        if trailers > 0 {
            self.cells.truncate(self.len() - trailers);
        }
    }

    fn trailers(&self) -> usize {
        self.cells
            .iter()
            .rev()
            .take_while(|cell| cell.is_default())
            .count()
    }

    pub(crate) fn is_blank(&self) -> bool {
        self.cells.iter().all(|c| c.is_default())
    }

    fn cluster_display_width(&self, cluster: &str) -> CharWidth {
        if display_width(cluster) == 2 {
            CharWidth::Double
        } else {
            CharWidth::Single
        }
    }
}

impl core::fmt::Debug for Line {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut s = self.text();

        if self.wrapped {
            s.push('⏎');
        }

        write!(f, "{:?}", s)
    }
}

struct Chunks<'a, I, F>
where
    I: Iterator<Item = &'a Cell>,
    F: Fn(&Cell, &Cell) -> bool,
{
    iter: I,
    predicate: F,
    cells: Vec<Cell>,
}

impl<'a, I: Iterator<Item = &'a Cell>, F: Fn(&Cell, &Cell) -> bool> Chunks<'a, I, F> {
    fn new(iter: I, predicate: F) -> Self {
        Self {
            iter,
            predicate,
            cells: Vec::new(),
        }
    }
}

impl<'a, I: Iterator<Item = &'a Cell>, F: Fn(&Cell, &Cell) -> bool> Iterator for Chunks<'a, I, F> {
    type Item = Vec<Cell>;

    fn next(&mut self) -> Option<Self::Item> {
        for cell in self.iter.by_ref() {
            if self.cells.is_empty() {
                self.cells.push(*cell);
                continue;
            }

            if (self.predicate)(self.cells.last().unwrap(), cell) {
                let cells = core::mem::take(&mut self.cells);
                self.cells.push(*cell);
                return Some(cells);
            } else {
                self.cells.push(*cell);
            }
        }

        if self.cells.is_empty() {
            None
        } else {
            Some(core::mem::take(&mut self.cells))
        }
    }
}

impl Index<usize> for Line {
    type Output = Cell;

    fn index(&self, index: usize) -> &Self::Output {
        &self.cells[index]
    }
}

impl IndexMut<usize> for Line {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.cells[index]
    }
}

impl Index<Range<usize>> for Line {
    type Output = [Cell];

    fn index(&self, range: Range<usize>) -> &Self::Output {
        &self.cells[range]
    }
}

impl Index<RangeFull> for Line {
    type Output = [Cell];

    fn index(&self, range: RangeFull) -> &Self::Output {
        &self.cells[range]
    }
}

#[cfg(test)]
mod tests {
    use super::{Cell, Chunks, Line, Occupancy, Pen};
    use alloc::vec::Vec;

    fn chars(cells: &[Cell]) -> Vec<char> {
        cells.iter().map(|c| c.char()).collect()
    }

    #[test]
    fn ascii_prefix_repairs_wide_edges_preserving_metadata() {
        for (col, text, expected) in [
            (0, "XY", 2),
            (1, "X", 1),
            (1, "XY", 2), // Replace a complete wide pair.
            (2, "X", 1),
            (2, "XY", 2), // Split wide pairs at both edges.
            (0, "XYZ123!", 6),
            (3, "XYZ!", 3),
            (3, "X界", 1),
            (2, "X\0", 1),
            (3, "X\x7f", 1),
            (3, " ~", 2),
            (2, "", 0),
            (2, "\0", 0),
            (2, "\x7f", 0),
            (2, "é", 0),
            (1, "\0", 0),
            (6, "X", 0),
            (7, "X", 0),
        ] {
            let mut line = Line::blank(6, Pen::default());

            for (col, ch) in [(0, 'a'), (1, '界'), (3, '語'), (5, 'b')] {
                line.replace_char(col, ch, Pen::default());
            }

            line.set_wrapped(true);
            let anchors: Vec<_> = (0..=6).map(|col| line.anchor_at(col).unwrap()).collect();
            let expired = line.anchor_at(6).unwrap();
            drop(expired);
            let mut reference = line.clone();
            let mut pen = Pen::default();
            pen.set_italic();
            pen.background = Some(crate::Color::Indexed(2));

            assert_eq!(
                line.replace_ascii_prefix(col, text, pen),
                expected,
                "col={col}, text={text:?}"
            );

            for (offset, ch) in text.chars().take(expected).enumerate() {
                reference.replace_char(col + offset, ch, pen);
            }

            assert_eq!(line, reference, "col={col}, text={text:?}");
            assert_eq!(line.marks.len(), 8);
            assert_eq!(line.marks[7].col, 6);

            for (col, anchor) in anchors.iter().enumerate() {
                assert_eq!(line.anchor_column(anchor), Some(col));
            }
        }
    }

    #[test]
    fn rejected_print_preserves_all_row_state() {
        for print in [Line::replace_char, Line::insert_char] {
            for (cols, seed) in [(1, 'x'), (2, '界'), (4, '界')] {
                let mut line = Line::blank(cols, Pen::default());
                assert!(line.replace_char(0, seed, Pen::default()).is_some());
                line.set_wrapped(true);
                let anchor = line.anchor_at(0).unwrap();
                let expired = line.anchor_at(cols).unwrap();
                drop(expired);
                let before = line.clone();

                for col in [cols - 1, cols] {
                    assert_eq!(print(&mut line, col, '界', Pen::default()), None);
                    assert_eq!(line, before);
                    assert_eq!(line.anchor_column(&anchor), Some(0));
                    assert_eq!(line.marks.len(), 2);
                    assert_eq!(line.marks[1].col, cols);
                }
            }
        }
    }

    #[test]
    fn replacement_repairs_wide_edges_preserving_anchors() {
        for (col, ch, expected, widths, changed) in [
            (0, 'x', "x界語b", [1, 2, 0, 2, 0, 1], 0..1),
            (0, '新', "新 語b", [2, 0, 1, 2, 0, 1], 0..3),
            (1, 'x', "ax 語b", [1, 1, 1, 2, 0, 1], 1..3),
            (1, '新', "a新語b", [1, 2, 0, 2, 0, 1], 1..3),
            (2, 'x', "a x語b", [1, 1, 1, 2, 0, 1], 1..3),
            (2, '新', "a 新 b", [1, 1, 2, 0, 1, 1], 1..5),
        ] {
            let mut old_pen = Pen::default();
            old_pen.set_underline();
            let mut pen = Pen::default();
            pen.set_italic();
            let mut line = Line::blank(6, old_pen);
            let mut cursor = 0;

            for ch in "a界語b".chars() {
                cursor += line.replace_char(cursor, ch, old_pen).unwrap();
            }

            line.set_wrapped(true);
            let anchors: Vec<_> = (0..=6).map(|col| line.anchor_at(col).unwrap()).collect();
            assert_eq!(line.replace_char(col, ch, pen), Some(widths[col]));
            assert_eq!(line.text(), expected);
            assert!(line.is_wrapped());

            for (col, cell) in line.cells().iter().enumerate() {
                assert_eq!(usize::from(cell.width()), widths[col]);

                assert_eq!(
                    cell.pen(),
                    if changed.contains(&col) {
                        &pen
                    } else {
                        &old_pen
                    }
                );
            }

            for (col, anchor) in anchors.iter().enumerate() {
                assert_eq!(line.anchor_column(anchor), Some(col));
            }
        }
    }

    #[test]
    fn inserting_characters_repairs_wide_boundaries() {
        for (text, col, ch, width, expected, widths) in [
            ("ab界cd", 2, 'x', 1, "abx界c", [1, 1, 1, 2, 0, 1]),
            ("ab界cd", 3, 'x', 1, "ab x c", [1, 1, 1, 1, 1, 1]),
            ("ab界cd", 2, '語', 2, "ab語界", [1, 1, 2, 0, 2, 0]),
            ("ab界cd", 3, '語', 2, "ab 語 ", [1, 1, 1, 2, 0, 1]),
            ("abcd界", 0, 'x', 1, "xabcd ", [1, 1, 1, 1, 1, 1]),
            ("abcd界", 0, '語', 2, "語abcd", [2, 0, 1, 1, 1, 1]),
        ] {
            let pen = Pen::default();
            let mut line = Line::blank(6, pen);
            let mut cursor = 0;

            for ch in text.chars() {
                cursor += line.replace_char(cursor, ch, pen).unwrap();
            }

            assert_eq!(line.insert_char(col, ch, pen), Some(width));
            assert_eq!(line.text(), expected);

            assert_eq!(
                line.cells().iter().map(Cell::width).collect::<Vec<_>>(),
                widths
            );
        }
    }

    #[test]
    fn inserted_and_shifted_cells_keep_their_styles() {
        let mut old_pen = Pen::default();
        old_pen.set_underline();
        let mut pen = Pen::default();
        pen.set_italic();
        let mut line = Line::blank(7, old_pen);
        let mut cursor = 0;

        for ch in "a界bc".chars() {
            cursor += line.replace_char(cursor, ch, old_pen).unwrap();
        }

        assert_eq!(line.insert_char(1, '語', pen), Some(2));

        assert_eq!(
            line.cells(),
            &[
                Cell::new("a", Occupancy::Single, old_pen),
                Cell::new("語", Occupancy::WideHead, pen),
                Cell::new(" ", Occupancy::WideTail, pen),
                Cell::new("界", Occupancy::WideHead, old_pen),
                Cell::new(" ", Occupancy::WideTail, old_pen),
                Cell::new("b", Occupancy::Single, old_pen),
                Cell::new("c", Occupancy::Single, old_pen),
            ]
        );
    }

    #[test]
    fn inserting_blanks_repairs_wide_pairs_and_clips() {
        for (text, cols, col, count, expected) in [
            ("abcd", 4, 1, 2, "a  b"),
            ("ab界cd", 6, 1, 1, "a b界c"),
            ("ab界cd", 6, 3, 1, "ab   c"),
            ("ab界", 4, 0, 1, " ab "),
            ("ab界", 4, 0, 2, "  ab"),
            ("ab界", 4, 1, usize::MAX, "a   "),
        ] {
            let mut line = Line::blank(cols, Pen::default());
            let mut cursor = 0;

            for ch in text.chars() {
                cursor += line.replace_char(cursor, ch, Pen::default()).unwrap();
            }

            let mut pen = Pen::default();
            pen.set_italic();
            line.insert_blanks(col, count, pen);
            assert_eq!(line.text(), expected);

            for cell in &line[col..col + count.min(cols - col)] {
                assert_eq!(*cell, Cell::blank(pen));
            }
        }
    }

    #[test]
    fn empty_insertion_preserves_cells_and_registrations() {
        let mut line = Line::blank(2, Pen::default());
        line.replace_char(0, '界', Pen::default());
        line.set_wrapped(true);
        let anchor = line.anchor_at(0).unwrap();
        let expired = line.anchor_at(2).unwrap();
        drop(expired);
        let before = line.clone();

        for (col, count) in [(1, 0), (2, 1)] {
            line.insert_blanks(col, count, Pen::default());
            assert_eq!(line, before);
            assert_eq!(line.anchor_column(&anchor), Some(0));
            assert_eq!(line.marks.len(), 2);
        }
    }

    #[test]
    fn empty_clear_and_delete_preserve_wide_cells_and_anchors() {
        let mut line = Line::blank(2, Pen::default());
        line.replace_char(0, '界', Pen::default());
        line.set_wrapped(true);
        let anchor = line.anchor_at(0).unwrap();
        let before = line.clone();
        line.clear(1..1, &Pen::default());
        assert_eq!(line, before);
        assert_eq!(line.anchor_column(&anchor), Some(0));
        line.delete(1, 0, &Pen::default());
        assert_eq!(line, before);
        assert_eq!(line.anchor_column(&anchor), Some(0));
    }

    #[test]
    fn registering_an_anchor_releases_expired_marks() {
        let mut line = Line::blank(2, Pen::default());
        let old = line.anchor_at(0).unwrap();
        drop(old);
        assert!(!line.has_anchor_at(0));
        let live = line.anchor_at(1).unwrap();
        assert_eq!(line.marks.len(), 1);
        assert_eq!(line.anchor_column(&live), Some(1));
    }

    #[test]
    fn chunks() {
        let pen = Pen::default();

        let cells = [
            Cell::new("0", Occupancy::Single, pen),
            Cell::new("a", Occupancy::Single, pen),
            Cell::new("b", Occupancy::Single, pen),
            Cell::new("C", Occupancy::Single, pen),
            Cell::new("D", Occupancy::Single, pen),
            Cell::new("E", Occupancy::Single, pen),
            Cell::new("1", Occupancy::Single, pen),
            Cell::new("F", Occupancy::Single, pen),
            Cell::new("g", Occupancy::Single, pen),
        ];

        let chunks: Vec<Vec<Cell>> = Chunks::new(cells.iter(), |c1, c2| {
            c1.char().is_ascii_digit()
                || c2.char().is_ascii_digit()
                || (c1.char().is_lowercase() && c2.char().is_uppercase())
                || (c1.char().is_uppercase() && c2.char().is_lowercase())
        })
        .collect();

        assert_eq!(&chars(&chunks[0]), &['0']);
        assert_eq!(&chars(&chunks[1]), &['a', 'b']);
        assert_eq!(&chars(&chunks[2]), &['C', 'D', 'E']);
        assert_eq!(&chars(&chunks[3]), &['1']);
        assert_eq!(&chars(&chunks[4]), &['F']);
        assert_eq!(&chars(&chunks[5]), &['g']);
    }
}
