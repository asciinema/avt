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
    use crate::{Anchor, Position, PromptOutcome, ScreenKind, ShellTracker, Terminal};
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
    fn inactive_buffer_anchors_survive_until_that_buffer_is_replaced() {
        let mut terminal = Terminal::new(12, 5).unwrap();
        terminal.feed("\x1b]133;A\x07$ abcdefgh");
        let mut shell = ShellTracker::new();
        terminal.feed_with("\r\n\x1b]133;A\x07$ ", &mut shell);
        let primary = terminal.anchor_at_cursor();
        terminal.feed_with("\x1b[?1049h", &mut shell);

        assert_eq!(
            terminal.resolve(&primary).unwrap().screen,
            ScreenKind::Primary
        );

        assert_eq!(shell.visible_prompts(&terminal).count(), 0);
        let alternate = terminal.anchor_at_cursor();
        terminal.resize_with(6, 6, &mut shell);
        terminal.feed_with("\x1b[?1049l", &mut shell);
        assert_eq!(shell.visible_prompts(&terminal).next().unwrap().row, 2);
        assert!(terminal.resolve(&primary).is_some());
        terminal.feed_with("\x1b[?1049h", &mut shell);
        assert!(terminal.resolve(&alternate).is_none());
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

    #[test]
    fn outcomes_remain_associated_with_their_original_prompt() {
        let mut terminal = Terminal::new(20, 5).unwrap();
        let mut shell = ShellTracker::new();

        terminal.feed_with(
            "\x1b]133;A\x07$ one\x1b]133;C\x07\r\n\x1b]133;D;7\x07\x1b]133;A\x07$ two",
            &mut shell,
        );

        let prompts: Vec<_> = shell.visible_prompts(&terminal).collect();

        assert_eq!(
            prompts[0].outcome,
            PromptOutcome::Finished {
                exit_status: Some(7)
            }
        );

        assert_eq!(prompts[1].outcome, PromptOutcome::Editing);
        terminal.feed_with("\x1b]133;D\x07", &mut shell);

        assert_eq!(
            shell.visible_prompts(&terminal).last().unwrap().outcome,
            PromptOutcome::Cancelled
        );

        terminal.feed_with("\x1b[H\x1b[2J", &mut shell);
        assert_eq!(shell.visible_prompts(&terminal).count(), 0);
    }

    proptest! {
        #[test]
        fn reflow_preserves_the_cells_at_surviving_anchors(
            chars in prop::collection::vec(prop::sample::select(vec!['a', 'b', '界', '日']), 1..20),
            cols in 2..13usize,
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

            for width in [cols, 8] {
                terminal.resize(width, 32);
                prop_assert!(anchors.iter().any(|(anchor, _)| terminal.resolve(anchor).is_some()));

                for (anchor, ch) in &anchors {
                    if let Some(position) = terminal.resolve(anchor) {
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
