use crate::parser::{Function, Osc};
use crate::{Anchor, Observation, Observer, Operation, ScreenKind, ShellIntegration, Terminal};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PromptId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub id: CommandId,
    pub prompt: Option<PromptId>,
    /// Reconstructed screen text, not necessarily the shell's history entry.
    /// None means the input boundary or its contents could not be tracked.
    pub text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellEvent {
    PromptStarted(PromptId),
    CommandSubmitted(Command),
    CommandFinished {
        id: CommandId,
        exit_status: Option<u8>,
    },
    CommandCancelled {
        prompt: Option<PromptId>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptOutcome {
    Editing,
    Running,
    Finished { exit_status: Option<u8> },
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisiblePrompt {
    pub id: PromptId,
    pub row: usize,
    pub col: usize,
    pub outcome: PromptOutcome,
}

struct PromptMarker {
    id: PromptId,
    anchor: Anchor,
    outcome: PromptOutcome,
}

/// Tracks OSC 133 shell integration and reconstructs single-logical-line commands,
/// including soft wrapping and reflow while the input remains on the primary screen.
/// Hard multiline input is unknown.
#[derive(Default)]
pub struct ShellTracker {
    next_prompt: u64,
    next_command: u64,
    prompt: Option<PromptId>,
    editing: bool,
    input: Option<Anchor>,
    running: Option<(CommandId, Option<PromptId>)>,
    markers: Vec<PromptMarker>,
    events: Vec<ShellEvent>,
}

impl ShellTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn visible_prompts<'a>(
        &'a self,
        terminal: &'a Terminal,
    ) -> impl Iterator<Item = VisiblePrompt> + 'a {
        self.markers.iter().filter_map(|marker| {
            let position = terminal.resolve(&marker.anchor)?;
            let row = usize::try_from(position.row).ok()?;

            (position.screen == terminal.active_screen_kind() && row < terminal.size().1).then_some(
                VisiblePrompt {
                    id: marker.id,
                    row,
                    col: position.col,
                    outcome: marker.outcome,
                },
            )
        })
    }

    fn set_outcome(&mut self, id: Option<PromptId>, outcome: PromptOutcome) {
        if let Some(marker) = self.markers.iter_mut().find(|marker| Some(marker.id) == id) {
            marker.outcome = outcome;
        }
    }

    fn marker(&mut self, context: &mut Observation<'_>, marker: ShellIntegration) {
        match marker {
            ShellIntegration::PromptStart => {
                let id = PromptId(self.next_prompt);
                self.next_prompt += 1;
                self.prompt = Some(id);
                self.editing = true;
                self.input = None;
                self.running = None;
                let position = context.terminal().cursor_position();

                // Reclaim lost annotations once per prompt, not on every printed
                // character. Queries always resolve against current terminal state.
                self.markers.retain(|marker| {
                    context
                        .resolve(&marker.anchor)
                        .is_some_and(|existing| existing != position)
                });

                self.markers.push(PromptMarker {
                    id,
                    anchor: context.anchor_at_cursor(),
                    outcome: PromptOutcome::Editing,
                });

                self.events.push(ShellEvent::PromptStarted(id));
            }

            ShellIntegration::CommandStart => {
                self.editing = true;

                self.input = (context.terminal().active_screen_kind() == ScreenKind::Primary)
                    .then(|| context.anchor_at_cursor());
            }

            ShellIntegration::CommandExecuted => {
                let id = CommandId(self.next_command);
                self.next_command += 1;

                let text = self
                    .input
                    .take()
                    .and_then(|start| command_text(context.terminal(), &start));

                let prompt = self.prompt.take();
                self.editing = false;
                self.running = Some((id, prompt));
                self.set_outcome(prompt, PromptOutcome::Running);

                self.events
                    .push(ShellEvent::CommandSubmitted(Command { id, prompt, text }));
            }

            ShellIntegration::CommandFinished { exit_status } => {
                if let Some((id, prompt)) = self.running.take() {
                    self.set_outcome(prompt, PromptOutcome::Finished { exit_status });

                    self.events
                        .push(ShellEvent::CommandFinished { id, exit_status });
                } else if self.editing {
                    let prompt = self.prompt;
                    self.set_outcome(prompt, PromptOutcome::Cancelled);
                    self.events.push(ShellEvent::CommandCancelled { prompt });
                }

                self.editing = false;
                self.input = None;
                self.prompt = None;
            }
        }
    }
}

impl Observer for ShellTracker {
    type Output = Vec<ShellEvent>;

    fn before(&mut self, context: &mut Observation<'_>, operation: &Operation) {
        let Operation::Function(function) = operation else {
            return;
        };

        if let Function::Osc(Osc::ShellIntegration(marker)) = function {
            self.marker(context, *marker);
        } else if matches!(function, Function::Ris) {
            self.input = None;
            self.prompt = None;
            self.editing = false;
            self.running = None;
            self.markers.clear();
        } else if matches!(function, Function::Print(_) | Function::Rep(_)) {
            // Reject hard multiline input or writing before a known boundary.
            // For example, resize can clamp the cursor before a trailing anchor.
            let Some(anchor) = &self.input else {
                return;
            };

            let Some(start) = context.resolve(anchor) else {
                self.input = None;

                return;
            };

            let terminal = context.terminal();
            let cursor = terminal.cursor();

            if start.screen == terminal.active_screen_kind() && start.row >= 0 {
                let before_start = (cursor.row as isize, cursor.col) < (start.row, start.col);

                let hard_break = cursor.row as isize > start.row
                    && terminal
                        .screen()
                        .visible_lines()
                        .skip(start.row as usize)
                        .take(cursor.row - start.row as usize)
                        .any(|line| !line.is_wrapped());

                if before_start || hard_break {
                    self.input = None;
                }
            }
        }
    }

    fn end(&mut self, _: &mut Observation<'_>) -> Self::Output {
        std::mem::take(&mut self.events)
    }
}

fn command_text(terminal: &Terminal, anchor: &Anchor) -> Option<String> {
    let start = terminal.resolve(anchor)?;

    if start.screen != ScreenKind::Primary || terminal.active_screen_kind() != ScreenKind::Primary {
        return None;
    }

    let mut text = String::new();
    let start_row = usize::try_from(start.row).ok()?;

    for (index, line) in terminal
        .primary_screen()
        .visible_lines()
        .skip(start_row)
        .enumerate()
    {
        let col = if index == 0 { start.col } else { 0 };
        let cells = line.cells().get(col..)?;

        if cells.first().is_some_and(|cell| cell.width() == 0) {
            return None;
        }

        text.extend(
            cells
                .iter()
                .filter(|cell| cell.width() != 0)
                .map(|cell| cell.char()),
        );

        if !line.is_wrapped() {
            return Some(text.trim_end().to_owned());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{PROMPT, SUBMIT};

    #[test]
    fn captures_commands_and_status_before_same_feed_overwrite() {
        let input = format!(
            "{PROMPT}ecxo\x1b[2Dh\x1b[C{SUBMIT}\x1b[2J\x1b[Hresult\x1b]133;D;7\x07{PROMPT}cancel\x1b]133;D\x07"
        );

        let mut terminal = Terminal::new(24, 4).unwrap();
        let events = terminal.feed_with(input, ShellTracker::new());

        let command = events
            .iter()
            .find_map(|event| match event {
                ShellEvent::CommandSubmitted(command) => Some(command),
                _ => None,
            })
            .unwrap();

        assert_eq!(command.text.as_deref(), Some("echo"));
        assert_eq!(command.prompt, Some(PromptId(0)));

        assert!(events.contains(&ShellEvent::CommandFinished {
            id: command.id,
            exit_status: Some(7)
        }));

        assert!(events.contains(&ShellEvent::CommandCancelled {
            prompt: Some(PromptId(1))
        }));
    }

    #[test]
    fn wraps_and_scrolls_command_while_start_is_retained() {
        let mut terminal = Terminal::new(6, 3).unwrap();
        let mut shell = ShellTracker::new();

        let events = terminal.feed_with(["old\r\nold\r\n", PROMPT, "echo 界", SUBMIT], &mut shell);

        let texts: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                ShellEvent::CommandSubmitted(command) => Some(command.text.as_deref()),
                _ => None,
            })
            .collect();

        assert_eq!(texts, [Some("echo 界")]);
    }

    #[test]
    fn unavailable_input_is_explicit() {
        for input in [
            format!("missing B{SUBMIT}"),
            format!("{PROMPT}first\r\nsecond{SUBMIT}"),
            format!("{PROMPT}lost\x1b[2J{SUBMIT}"),
            format!("{PROMPT}draft\r\x1b[2Knew{SUBMIT}"),
            format!("{PROMPT}long enough to evict the start of input completely{SUBMIT}"),
        ] {
            let mut terminal = Terminal::new(8, 3).unwrap();
            let mut shell = ShellTracker::new();
            let events = terminal.feed_with(&input, &mut shell);

            let command = events
                .iter()
                .find_map(|event| match event {
                    ShellEvent::CommandSubmitted(command) => Some(command),
                    _ => None,
                })
                .unwrap();

            assert!(command.text.is_none(), "{input:?}");

            assert_eq!(
                command.prompt,
                input.starts_with(PROMPT).then_some(PromptId(0))
            );
        }
    }

    #[test]
    fn command_capture_survives_reflow_edits_and_scrolling() {
        let mut terminal = Terminal::new(24, 4).unwrap();
        let mut shell = ShellTracker::new();
        terminal.feed_with([PROMPT, "echo ok"], &mut shell);
        terminal.resize_with(4, 6, &mut shell);
        let events = terminal.feed_with(SUBMIT, &mut shell);

        assert!(matches!(
            &events[0],
            ShellEvent::CommandSubmitted(command) if command.text.as_deref() == Some("echo ok")
        ));

        for input in [
            format!("{PROMPT}echo\r\x1b[P{SUBMIT}"),
            format!("\x1b[2;4r\x1b[4;1H{PROMPT}echo{SUBMIT}"),
        ] {
            let mut terminal = Terminal::new(6, 5).unwrap();
            let mut shell = ShellTracker::new();
            let events = terminal.feed_with(&input, &mut shell);

            assert!(events.iter().any(|event| matches!(
                event,
                ShellEvent::CommandSubmitted(command) if command.text.as_deref() == Some("echo")
            )));
        }

        let mut terminal = Terminal::new(6, 5).unwrap();
        let mut shell = ShellTracker::new();

        let events = terminal.feed_with(
            ["\x1b[2;4r\x1b[4;1H", PROMPT, "echo ok", SUBMIT],
            &mut shell,
        );

        // Scrolling a region above the screen bottom breaks the logical wrap link.
        // Reconstruction must stay unknown rather than return a partial command.
        assert!(!terminal.screen().line(2).unwrap().is_wrapped());
        assert_eq!(shell.visible_prompts(&terminal).next().unwrap().row, 2);

        assert!(events.iter().any(
            |event| matches!(event, ShellEvent::CommandSubmitted(command) if command.text.is_none())
        ));
    }

    #[test]
    fn clamped_cursor_before_input_makes_command_unavailable() {
        let mut terminal = Terminal::new(8, 4).unwrap();
        let mut shell = ShellTracker::new();
        terminal.feed_with(PROMPT, &mut shell);
        terminal.resize_with(2, 4, &mut shell);
        let events = terminal.feed_with(["x", SUBMIT], &mut shell);

        assert!(
            matches!(&events[0], ShellEvent::CommandSubmitted(command) if command.text.is_none())
        );
    }

    #[test]
    fn unknown_exit_status_is_not_reported_as_success() {
        let mut terminal = Terminal::new(24, 4).unwrap();
        let mut shell = ShellTracker::new();

        let events = terminal.feed_with([PROMPT, "cmd", SUBMIT, "\x1b]133;D\x07"], &mut shell);

        assert!(matches!(
            events.last(),
            Some(ShellEvent::CommandFinished {
                exit_status: None,
                ..
            })
        ));
    }

    #[test]
    fn reset_discards_editing_and_running_state() {
        for input in [format!("{PROMPT}draft"), format!("{PROMPT}cmd{SUBMIT}")] {
            let mut terminal = Terminal::new(24, 4).unwrap();
            let mut shell = ShellTracker::new();
            terminal.feed_with(input, &mut shell);
            terminal.execute_with(Function::Ris, &mut shell);
            assert!(shell.visible_prompts(&terminal).next().is_none());
            assert!(terminal.feed_with("\x1b]133;D\x07", &mut shell).is_empty());
        }
    }

    #[test]
    fn missing_prompt_does_not_reuse_a_previous_commands_prompt() {
        let mut terminal = Terminal::new(24, 4).unwrap();
        let mut shell = ShellTracker::new();
        terminal.feed_with([PROMPT, "one", SUBMIT, "\x1b]133;D;0\x07"], &mut shell);
        let events = terminal.feed_with(["\r\n\x1b]133;B\x07two", SUBMIT], &mut shell);

        assert!(matches!(
            &events[0],
            ShellEvent::CommandSubmitted(command)
                if command.prompt.is_none() && command.text.as_deref() == Some("two")
        ));
    }

    #[test]
    fn new_prompt_replaces_annotation_at_same_position() {
        let mut terminal = Terminal::new(24, 4).unwrap();
        let mut shell = ShellTracker::new();

        terminal.feed_with(
            [PROMPT, "one", SUBMIT, "\x1b]133;D;0\x07\r", PROMPT],
            &mut shell,
        );

        let prompts: Vec<_> = shell.visible_prompts(&terminal).collect();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].id, PromptId(1));
        assert_eq!(prompts[0].outcome, PromptOutcome::Editing);
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
}
