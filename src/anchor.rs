use std::sync::{Arc, Weak};

use crate::ScreenKind;

/// A screen-relative coordinate value at a particular moment.
/// Negative rows address retained scrollback. `col` may equal the line width,
/// representing the boundary after its last cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub screen: ScreenKind,
    pub col: usize,
    pub row: isize,
}

/// An owned handle to a position tracked through editing, scrolling, and
/// reflow. Only the originating terminal can resolve it.
///
/// Inserting characters at the anchored column leaves the anchor before the
/// inserted characters. Inserting earlier in the line shifts its column.
///
/// Clones refer to the same position; dropping all handles releases
/// registration lazily during subsequent registration, structural editing,
/// reflow, or removal of the containing line.
// The shared allocation provides identity and lifetime tracking. Position
// information lives in the line-local registration.
#[derive(Clone, Debug)]
pub struct Anchor(pub(crate) Arc<()>);

impl Anchor {
    /// Whether a line still holds this registration.
    /// This includes removed lines during eviction callbacks, when the terminal
    /// can no longer resolve the anchor.
    pub(crate) fn is_registered(&self) -> bool {
        Arc::weak_count(&self.0) != 0
    }
}

/// An anchor registration stored on a line.
/// The containing line determines its row and buffer.
#[derive(Debug)]
pub(crate) struct Mark {
    pub token: Weak<()>,
    pub col: usize,
}

impl Mark {
    pub fn live(&self) -> bool {
        self.token.strong_count() != 0
    }

    pub fn matches(&self, anchor: &Anchor) -> bool {
        self.token.as_ptr() == Arc::as_ptr(&anchor.0)
    }
}

#[cfg(test)]
mod tests {
    use crate::{Anchor, Line, Observation, Observer, Operation, Position, ScreenKind, Terminal};
    use proptest::prelude::*;

    fn at(terminal: &mut Terminal, col: usize, row: isize) -> Anchor {
        terminal
            .anchor_at(Position {
                screen: terminal.active_screen_kind(),
                col,
                row,
            })
            .unwrap()
    }

    fn pos(terminal: &Terminal, anchor: &Anchor) -> Option<(usize, isize)> {
        terminal
            .resolve(anchor)
            .map(|position| (position.col, position.row))
    }

    #[test]
    fn anchors_survive_overwrites_and_track_character_edits() {
        let mut terminal = Terminal::new(12, 3).unwrap();
        terminal.feed("$ ");
        let anchor = terminal.anchor_at_cursor();
        terminal.feed("abc\r\x1b[2C\x1b[@"); // Insert exactly at the input boundary.
        assert_eq!(pos(&terminal, &anchor), Some((2, 0)));
        terminal.feed("\r\x1b[2@"); // Insert before it.
        assert_eq!(pos(&terminal, &anchor), Some((4, 0)));
        terminal.feed("\x1b[P"); // Delete before it.
        assert_eq!(pos(&terminal, &anchor), Some((3, 0)));
        terminal.feed("\x1b[3CX"); // Overwrite at it.
        assert_eq!(pos(&terminal, &anchor), Some((3, 0)));
        terminal.feed("\x1b[D\x1b[P"); // Delete its position.
        assert_eq!(pos(&terminal, &anchor), None);
    }

    #[test]
    fn line_insertion_deletion_and_scroll_regions_move_anchors() {
        let mut terminal = Terminal::new(8, 5).unwrap();
        terminal.feed("a\r\nb\r\nc\r\nd\r\ne");
        let anchor = at(&mut terminal, 0, 2);
        terminal.feed("\x1b[2;1H\x1b[L");
        assert_eq!(pos(&terminal, &anchor), Some((0, 3)));
        terminal.feed("\x1b[M");
        assert_eq!(pos(&terminal, &anchor), Some((0, 2)));
        terminal.feed("\x1b[2;4r\x1b[S");
        assert_eq!(pos(&terminal, &anchor), Some((0, 1)));
        terminal.feed("\x1b[T");
        assert_eq!(pos(&terminal, &anchor), Some((0, 2)));
        terminal.feed("\x1b[3;1H\x1b[M");
        assert_eq!(pos(&terminal, &anchor), None);
    }

    #[test]
    fn reflow_moves_anchors_with_wide_cells() {
        let mut terminal = Terminal::new(8, 6).unwrap();
        terminal.feed("ab界cdE");
        let wide = at(&mut terminal, 2, 0);
        let end = at(&mut terminal, 6, 0);
        terminal.resize(3, 8); // Keep the newly reflowed rows in the zero-scrollback view.
        assert_eq!(pos(&terminal, &wide), Some((0, 1)));
        assert_eq!(pos(&terminal, &end), Some((1, 2)));
        terminal.resize(8, 8);

        for (anchor, expected) in [(&wide, '界'), (&end, 'E')] {
            let position = terminal.resolve(anchor).unwrap();

            assert_eq!(
                terminal
                    .screen()
                    .line(position.row as usize)
                    .unwrap()
                    .cells()[position.col]
                    .char(),
                expected
            );
        }
    }

    #[test]
    fn one_column_reflow_erases_wide_cells_and_their_anchors() {
        let mut terminal = Terminal::new(4, 2).unwrap();
        terminal.feed("\x1b[41m界\x1b[0mab");
        let head = at(&mut terminal, 0, 0);
        let tail = at(&mut terminal, 1, 0);
        let narrow = at(&mut terminal, 2, 0);
        terminal.resize(1, 5);
        terminal.engine.verify();
        assert!(terminal.resolve(&head).is_none());
        assert!(terminal.resolve(&tail).is_none());
        assert_eq!(pos(&terminal, &narrow), Some((0, 2)));

        for line in terminal.screen().visible_lines().take(2) {
            let cell = &line.cells()[0];
            assert_eq!(cell.char(), ' ');
            assert_eq!(cell.width(), 1);
            assert_eq!(cell.pen().background(), Some(crate::Color::Indexed(1)));
        }

        terminal.resize(4, 2);
        terminal.engine.verify();
        assert_eq!(terminal.screen().text(), ["  ab", ""]);
        assert_eq!(pos(&terminal, &narrow), Some((2, 0)));
    }

    #[test]
    fn alternate_resize_invalidates_clipped_and_split_wide_anchors() {
        for cols in [0usize, 1, 3] {
            let width = cols.max(1);
            let mut terminal = Terminal::new(6, 2).unwrap();
            terminal.feed("\x1b[?1047h");
            terminal.feed("a".repeat(width - 1));
            terminal.feed("\x1b[41m界cd");
            let head = at(&mut terminal, width - 1, 0);
            let tail = at(&mut terminal, width, 0);
            let clipped = at(&mut terminal, width + 1, 0);
            let boundary = at(&mut terminal, 6, 0);
            let retained = at(&mut terminal, 0, 1);
            terminal.resize(cols, 2);
            terminal.engine.verify();

            for anchor in [&head, &tail, &clipped, &boundary] {
                assert!(terminal.resolve(anchor).is_none());
            }

            assert_eq!(pos(&terminal, &retained), Some((0, 1)));
            let cell = terminal.screen().line(0).unwrap().cells()[width - 1];
            assert_eq!(cell.char(), ' ');
            assert_eq!(cell.width(), 1);
            assert_eq!(cell.pen().background(), Some(crate::Color::Indexed(1)));
            terminal.resize(6, 2);
            assert_eq!(pos(&terminal, &retained), Some((0, 1)));

            assert_eq!(
                terminal.screen().line(0).unwrap().text().trim_end(),
                "a".repeat(width - 1)
            );

            assert_eq!(
                *terminal.screen().line(0).unwrap().cells()[width].pen(),
                crate::Pen::default()
            );
        }
    }

    #[test]
    fn cursor_end_boundary_moves_to_next_row_on_wrap() {
        let mut terminal = Terminal::new(4, 2).unwrap();
        terminal.feed("1234");
        let anchor = terminal.anchor_at_cursor();
        assert_eq!(pos(&terminal, &anchor), Some((4, 0)));
        terminal.feed("\x1b[X"); // Erasing beyond the last cell is a no-op.
        assert_eq!(pos(&terminal, &anchor), Some((4, 0)));
        terminal.feed("a");
        assert_eq!(pos(&terminal, &anchor), Some((0, 1)));
        terminal.feed("bcd!"); // Wrap again, evicting the row before the anchor.
        assert_eq!(pos(&terminal, &anchor), Some((0, 0)));
    }

    #[test]
    fn autowrap_scroll_relocates_cell_and_boundary_anchors() {
        for (top, bottom) in [(1, 5), (1, 4), (2, 4), (2, 5)] {
            for alternate in [false, true] {
                let mut terminal = Terminal::new(4, 5).unwrap();

                if alternate {
                    terminal.feed("\x1b[?1047h");
                }

                terminal.feed(format!("\x1b[{top};{bottom}r\x1b[{bottom};1HABCD"));
                let source = at(&mut terminal, 1, bottom - 1);
                let boundary = terminal.anchor_at_cursor();
                terminal.feed("E");
                assert_eq!(pos(&terminal, &source), Some((1, bottom - 2)));
                assert_eq!(pos(&terminal, &boundary), Some((0, bottom - 1)));
                terminal.resize(8, 5);
                let source = terminal.resolve(&source).unwrap();
                let boundary = terminal.resolve(&boundary).unwrap();
                assert_eq!(source.col, 1);

                if alternate {
                    assert_eq!(boundary.col, 0);
                    assert_eq!(source.row + 1, boundary.row);
                } else {
                    assert_eq!(boundary.col, 4);
                    assert_eq!(source.row, boundary.row);
                }

                assert_eq!(source.screen, terminal.active_screen_kind());
                assert_eq!(boundary.screen, source.screen);
            }
        }
    }

    #[test]
    fn one_row_wrap_relocates_boundary_anchors_before_callbacks() {
        struct WrapObserver {
            source: Anchor,
            boundary: Anchor,
            after: bool,
            evicted: bool,
        }

        impl Observer for WrapObserver {
            type Output = ();

            fn after(&mut self, context: &mut Observation<'_>, _: &Operation) {
                let terminal = context.terminal();
                assert_eq!(pos(terminal, &self.source), Some((0, -1)));
                assert_eq!(pos(terminal, &self.boundary), Some((0, 0)));
                assert!(terminal.screen().all_lines().next().unwrap().is_wrapped());
                assert!(!terminal.screen().line(0).unwrap().is_wrapped());
                assert_eq!(terminal.cursor().position(), Some((1, 0)));
                terminal.engine.verify();
                self.after = true;
            }

            fn evicted(&mut self, context: &mut Observation<'_>, line: &Line) {
                assert!(self.after);
                assert!(!self.evicted);
                assert!(line.is_wrapped());
                assert!(context.resolve(&self.source).is_none());
                assert!(self.source.is_registered());
                assert_eq!(pos(context.terminal(), &self.boundary), Some((0, 0)));
                self.evicted = true;
            }

            fn end(&mut self, _: &mut Observation<'_>) {}
        }

        for cols in [1, 4] {
            for (alternate, limit) in [
                (false, Some(0)),
                (false, Some(1)),
                (false, None),
                (true, Some(0)),
            ] {
                let mut terminal = Terminal::with_scrollback(cols, 1, limit).unwrap();

                if alternate {
                    terminal.feed("\x1b[?1047h");
                }

                terminal.feed("A".repeat(cols));

                let mut observer = WrapObserver {
                    source: at(&mut terminal, 0, 0),
                    boundary: terminal.anchor_at_cursor(),
                    after: false,
                    evicted: false,
                };

                terminal.feed_with("B", &mut observer);
                assert!(observer.after);
                assert_eq!(observer.evicted, !alternate && limit == Some(0));
                let retained = !alternate && limit != Some(0);

                assert_eq!(
                    pos(&terminal, &observer.source),
                    retained.then_some((0, -1))
                );

                assert_eq!(observer.source.is_registered(), retained);
                assert_eq!(pos(&terminal, &observer.boundary), Some((0, 0)));
            }
        }
    }

    #[test]
    fn erasure_invalidates_affected_anchors_including_wide_pairs() {
        let mut terminal = Terminal::new(8, 3).unwrap();
        terminal.feed("界abc");
        let head = at(&mut terminal, 0, 0);
        let tail = at(&mut terminal, 1, 0);
        let later = at(&mut terminal, 4, 0);
        terminal.feed("\x1b[1;2H\x1b[X");
        assert_eq!(pos(&terminal, &head), None);
        assert_eq!(pos(&terminal, &tail), None);
        assert_eq!(pos(&terminal, &later), Some((4, 0)));
        terminal.feed("\x1b[2K");
        assert_eq!(pos(&terminal, &later), None);
    }

    #[test]
    fn eviction_truncation_and_reset_invalidate_handles() {
        let mut terminal = Terminal::new(8, 3).unwrap();
        let first = terminal.anchor_at_cursor();
        let last = at(&mut terminal, 0, 2);
        terminal.resize(8, 2); // The bottom row is removed while the cursor stays at home.
        assert_eq!(pos(&terminal, &last), None);
        terminal.feed("a\r\nb\r\nc");
        assert_eq!(pos(&terminal, &first), None);
        let current = terminal.anchor_at_cursor();
        terminal.feed("\x1bc");
        assert_eq!(pos(&terminal, &current), None);
    }

    #[test]
    fn anchors_are_terminal_local_and_excluded_from_line_copies() {
        let mut terminal = Terminal::new(8, 3).unwrap();
        let anchor = terminal.anchor_at_cursor();
        let copy = terminal.screen().line(0).unwrap().clone();
        assert!(copy.marks.is_empty());
        assert!(copy == *terminal.screen().line(0).unwrap());
        let mut other = Terminal::new(8, 3).unwrap();
        other.feed(terminal.dump());
        assert!(other.resolve(&anchor).is_none());
        let clone = anchor.clone();
        drop(anchor);
        assert!(terminal.resolve(&clone).is_some());
        drop(clone);
        let live = terminal.anchor_at_cursor();
        assert_eq!(terminal.screen().line(0).unwrap().marks.len(), 1);
        assert!(terminal.resolve(&live).is_some());

        assert!(
            terminal
                .anchor_at(Position {
                    screen: ScreenKind::Primary,
                    col: 9,
                    row: 0
                })
                .is_none()
        );
    }

    proptest! {
        #[test]
        fn prop_reflow_preserves_the_cells_at_surviving_anchors(
            chars in prop::collection::vec(prop::sample::select(vec!['a', 'b', '界', '日']), 1..20),
            cols in 1..13usize,
        ) {
            let mut terminal = Terminal::new(8, 32).unwrap();
            terminal.feed(chars);

            let cells: Vec<_> = terminal
                .screen()
                .visible_lines()
                .enumerate()
                .flat_map(|(row, line)| {
                    line.cells()
                        .iter()
                        .enumerate()
                        .filter(|(_, cell)| cell.char() != ' ')
                        .map(move |(col, cell)| (col, row as isize, cell.char()))
                })
                .collect();

            let anchors: Vec<_> = cells
                .into_iter()
                .map(|(col, row, ch)| (at(&mut terminal, col, row), ch))
                .collect();

            for (width, rows) in [(cols, 96), (8, 192)] {
                // Grow on each resize to retain reflowed rows instead of evicting them.
                terminal.resize(width, rows);

                for (anchor, ch) in &anchors {
                    if cols == 1 && matches!(ch, '界' | '日') {
                        prop_assert!(terminal.resolve(anchor).is_none());
                    } else {
                        let position = terminal.resolve(anchor).unwrap();

                        let actual = terminal
                            .screen()
                            .line(position.row as usize)
                            .unwrap()
                            .cells()[position.col]
                            .char();

                        prop_assert_eq!(actual, *ch);
                    }
                }
            }
        }
    }
}
