use super::{Observation, Observer};
use crate::ansi::{Function, OscOp, ShellMarker};
use crate::{Anchor, Operation, ScreenKind, Terminal};
use alloc::borrow::ToOwned as _;
use alloc::string::String;
use alloc::vec::Vec;

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

struct CommandInput {
    anchor: Anchor,
    // Validated through the cursor after the last print. Further printing
    // preserves history, so only visible rows need rechecking afterward.
    // Operations other than Print, REP, and SGR invalidate both guarantees.
    validated: bool,
}

impl CommandInput {
    fn validate(&mut self, terminal: &Terminal) -> bool {
        if !self.anchor.is_registered() {
            return false;
        }

        if terminal.active_screen_kind() != ScreenKind::Primary {
            return true;
        }

        let screen = terminal.primary_screen();
        let cursor = terminal.cursor();

        let start_row = if let Some((col, row)) = screen.resolve(&self.anchor) {
            if (cursor.row, cursor.col) < (row, col) {
                return false;
            }

            row
        } else {
            // An offscreen boundary may return after a resize. Validate its
            // historical prefix once, rather than once per print fragment.
            if !self.validated {
                let history = screen.all_lines().count() - terminal.size().1;

                let hard_break = screen
                    .all_lines()
                    .take(history)
                    .skip_while(|line| line.anchor_column(&self.anchor).is_none())
                    .any(|line| !line.is_wrapped());

                if hard_break {
                    return false;
                }
            }

            0
        };

        let valid = !screen
            .visible_lines()
            .skip(start_row)
            .take(cursor.row - start_row)
            .any(|line| !line.is_wrapped());

        self.validated = valid;

        valid
    }
}

/// Tracks OSC 133 shell integration and reconstructs single-logical-line commands,
/// including soft wrapping and reflow while the input remains on the primary screen.
/// Hard multiline input is unknown.
/// Dead prompt registrations are pruned at the end of calls that start a prompt.
#[derive(Default)]
pub struct ShellTracker {
    next_prompt: u64,
    next_command: u64,
    prompt: Option<PromptId>,
    editing: bool,
    input: Option<CommandInput>,
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
            let (col, row) = terminal.screen().resolve(&marker.anchor)?;

            Some(VisiblePrompt {
                id: marker.id,
                row,
                col,
                outcome: marker.outcome,
            })
        })
    }

    fn validate_input(&mut self, terminal: &Terminal) {
        if self
            .input
            .as_mut()
            .is_some_and(|input| !input.validate(terminal))
        {
            self.input = None;
        }
    }

    fn set_outcome(&mut self, id: Option<PromptId>, outcome: PromptOutcome) {
        // Starting a prompt replaces the editing and running state, so only
        // the newest marker can receive an outcome.
        if let Some(marker) = self
            .markers
            .last_mut()
            .filter(|marker| Some(marker.id) == id)
        {
            marker.outcome = outcome;
        }
    }

    fn marker(&mut self, context: &mut Observation<'_>, marker: ShellMarker) {
        match marker {
            ShellMarker::PromptStart => {
                let id = PromptId(self.next_prompt);
                self.next_prompt += 1;
                self.prompt = Some(id);
                self.editing = true;
                self.input = None;
                self.running = None;
                let terminal = context.terminal();
                let cursor = terminal.cursor();
                let line = terminal.screen().line(cursor.row).unwrap();

                // Replace annotations at this position. An unmarked position cannot
                // have an old prompt, so avoid scanning the batch's growing history.
                if line.has_anchor_at(cursor.col) {
                    self.markers
                        .retain(|marker| line.anchor_column(&marker.anchor) != Some(cursor.col));
                }

                self.markers.push(PromptMarker {
                    id,
                    anchor: context.anchor_at_cursor(),
                    outcome: PromptOutcome::Editing,
                });

                self.events.push(ShellEvent::PromptStarted(id));
            }

            ShellMarker::CommandStart => {
                self.editing = true;

                self.input =
                    (context.terminal().active_screen_kind() == ScreenKind::Primary).then(|| {
                        CommandInput {
                            anchor: context.anchor_at_cursor(),
                            validated: false,
                        }
                    });
            }

            ShellMarker::CommandExecuted => {
                let id = CommandId(self.next_command);
                self.next_command += 1;

                let text = self
                    .input
                    .take()
                    .and_then(|input| command_text(context.terminal(), &input.anchor));

                let prompt = self.prompt.take();
                self.editing = false;
                self.running = Some((id, prompt));
                self.set_outcome(prompt, PromptOutcome::Running);

                self.events
                    .push(ShellEvent::CommandSubmitted(Command { id, prompt, text }));
            }

            ShellMarker::CommandFinished { exit_status } => {
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

    fn before(&mut self, context: &mut Observation<'_>, operation: &Operation<'_>) {
        if !matches!(
            operation,
            Operation::Function(Function::Print(_) | Function::Rep(_) | Function::Sgr(_))
        ) {
            if let Some(input) = &mut self.input {
                input.validated = false;
            }
        }

        let Operation::Function(function) = operation else {
            return;
        };

        if let Function::Osc(OscOp::ShellIntegration(marker)) = function {
            self.marker(context, *marker);
        } else if matches!(function, Function::Ris) {
            self.input = None;
            self.prompt = None;
            self.editing = false;
            self.running = None;
            self.markers.clear();
        } else if matches!(function, Function::Print(_) | Function::Rep(_))
            && self.input.as_ref().is_some_and(|input| !input.validated)
        {
            // Recheck after intervening operations, before writing can hide a
            // hard break or advance a cursor clamped before the input boundary.
            self.validate_input(context.terminal());
        }
    }

    fn after(&mut self, context: &mut Observation<'_>, operation: &Operation<'_>) {
        if matches!(
            operation,
            Operation::Function(Function::Print(_) | Function::Rep(_))
        ) {
            // Scrolling can break a wrap above the margins within a print run.
            // Reject it now even when submission follows without more printing.
            self.validate_input(context.terminal());
        }
    }

    fn end(&mut self, _: &mut Observation<'_>) -> Self::Output {
        // Prune after eviction only when adding prompts; ordinary feed calls need
        // not visit every retained marker. Dead anchors are already invisible.
        if self
            .events
            .iter()
            .any(|event| matches!(event, ShellEvent::PromptStarted(_)))
        {
            self.markers.retain(|marker| marker.anchor.is_registered());
        }

        core::mem::take(&mut self.events)
    }
}

fn command_text(terminal: &Terminal, anchor: &Anchor) -> Option<String> {
    if terminal.active_screen_kind() != ScreenKind::Primary {
        return None;
    }

    let (start_col, start_row) = terminal.primary_screen().resolve(anchor)?;
    let mut text = String::new();

    for (index, line) in terminal
        .primary_screen()
        .visible_lines()
        .skip(start_row)
        .enumerate()
    {
        let col = if index == 0 { start_col } else { 0 };
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
    use alloc::format;

    const PROMPT: &str = "\x1b]133;A\x07$ \x1b]133;B\x07";
    const SUBMIT: &str = "\x1b]133;C\x07";

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

        for command in ["echox", "echo ok"] {
            let mut terminal = Terminal::new(6, 5).unwrap();
            let mut shell = ShellTracker::new();

            let events =
                terminal.feed_with(["\x1b[2;4r\x1b[4;1H", PROMPT, command, SUBMIT], &mut shell);

            assert!(terminal.screen().line(2).unwrap().is_wrapped());
            assert_eq!(shell.visible_prompts(&terminal).next().unwrap().row, 2);

            assert!(events.iter().any(|event| matches!(
                event,
                ShellEvent::CommandSubmitted(submitted) if submitted.text.as_deref() == Some(command)
            )));
        }
    }

    #[test]
    fn hidden_primary_resize_restores_retained_input_and_prompt() {
        for limit in [Some(4), None] {
            let mut terminal = Terminal::with_scrollback(8, 2, limit).unwrap();
            let mut shell = ShellTracker::new();
            terminal.feed_with([PROMPT, "abcdefghijklmno"], &mut shell);
            assert_eq!(shell.visible_prompts(&terminal).count(), 0);
            terminal.feed_with("p\x1b[?1049h\x1b[Halternate", &mut shell);
            terminal.resize_with(8, 4, &mut shell);
            assert_eq!(shell.visible_prompts(&terminal).count(), 0);
            let events = terminal.feed_with(["\x1b[?1049l", SUBMIT], &mut shell);

            assert!(matches!(
                &events[0],
                ShellEvent::CommandSubmitted(command)
                    if command.text.as_deref() == Some("abcdefghijklmnop")
            ));

            assert_eq!(
                shell.visible_prompts(&terminal).collect::<Vec<_>>(),
                [VisiblePrompt {
                    id: PromptId(0),
                    row: 0,
                    col: 0,
                    outcome: PromptOutcome::Running,
                }]
            );
        }
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
    fn prompt_cleanup_retains_history_and_removes_erased_markers() {
        for limit in [Some(0), Some(1), None] {
            let mut terminal = Terminal::with_scrollback(8, 2, limit).unwrap();
            let mut shell = ShellTracker::new();
            terminal.feed_with([PROMPT, "one\x1b[H\x1b[2J", PROMPT], &mut shell);
            let expected = if limit == Some(0) { 1 } else { 2 };
            assert_eq!(shell.markers.len(), expected);
            assert_eq!(shell.visible_prompts(&terminal).count(), 1);

            terminal.feed_with(["\r\x1b[2K", PROMPT], &mut shell);
            assert_eq!(shell.markers.len(), expected);
            let prompts: Vec<_> = shell.visible_prompts(&terminal).collect();
            assert_eq!(prompts.len(), 1);
            assert_eq!(prompts[0].id, PromptId(2));
        }
    }

    #[test]
    fn evicted_prompts_stay_hidden_until_lazy_cleanup() {
        for limit in [Some(0), Some(1), None] {
            let mut terminal = Terminal::with_scrollback(8, 1, limit).unwrap();
            let mut shell = ShellTracker::new();
            terminal.feed_with(PROMPT, &mut shell);
            terminal.feed_with(["one", SUBMIT, "\r\n"], &mut shell);
            assert!(shell.visible_prompts(&terminal).next().is_none());
            terminal.feed_with(PROMPT, &mut shell);
            assert_eq!(shell.markers.len(), 1 + usize::from(limit != Some(0)));
            let prompts: Vec<_> = shell.visible_prompts(&terminal).collect();
            assert_eq!(prompts.len(), 1);
            assert_eq!(prompts[0].id, PromptId(1));
        }
    }

    #[test]
    fn batched_resizes_restore_command_input_and_prompt() {
        let mut terminal = Terminal::new(8, 3).unwrap();
        let mut shell = ShellTracker::new();
        terminal.feed_with([PROMPT, "abcdefghijklmno"], &mut shell);

        let events = terminal.execute_with(
            [
                Operation::Resize { cols: 8, rows: 1 },
                Operation::Resize { cols: 8, rows: 3 },
                Function::Osc(OscOp::ShellIntegration(ShellMarker::CommandExecuted)).into(),
            ],
            &mut shell,
        );

        assert!(matches!(
            &events[0],
            ShellEvent::CommandSubmitted(command)
                if command.text.as_deref() == Some("abcdefghijklmno")
        ));

        assert_eq!(shell.visible_prompts(&terminal).next().unwrap().row, 0);
    }

    #[test]
    fn restored_history_input_distinguishes_hard_and_soft_wraps() {
        for (text, expected) in [
            ("abcdefgh\x1b[31mijklmno", Some("abcdefghijklmno")),
            ("first\r\nsecond", None),
        ] {
            let mut terminal = Terminal::new(8, 1).unwrap();
            let mut shell = ShellTracker::new();
            terminal.feed_with(PROMPT, &mut shell);
            let mut parser = crate::Parser::new();
            let input = parser.feed(text).map(Operation::from);

            let actions = input.chain([
                Operation::Resize { cols: 8, rows: 3 },
                Function::Osc(OscOp::ShellIntegration(ShellMarker::CommandExecuted)).into(),
            ]);

            let events = terminal.execute_with(actions, &mut shell);

            assert!(matches!(
                &events[0],
                ShellEvent::CommandSubmitted(command) if command.text.as_deref() == expected
            ));
        }
    }

    #[test]
    fn fragmented_input_survives_styles_repetition_and_reflow() {
        for limit in [Some(0), None] {
            let mut terminal = Terminal::with_scrollback(8, 1, limit).unwrap();
            let mut shell = ShellTracker::new();
            terminal.feed_with(PROMPT, &mut shell);
            let text = "abcdefgh\x1b[31mijklmnop".repeat(8);
            terminal.feed_with(text.chars(), &mut shell);
            terminal.execute_with(Function::Rep(3), &mut shell);
            terminal.resize_with(16, 1, &mut shell);
            terminal.feed_with('!', &mut shell);
            terminal.resize_with(16, 12, &mut shell);
            let events = terminal.feed_with(SUBMIT, &mut shell);
            let expected = (limit.is_none()).then(|| "abcdefghijklmnop".repeat(8) + "ppp!");

            assert!(matches!(
                &events[0],
                ShellEvent::CommandSubmitted(command) if command.text == expected
            ));
        }
    }

    #[test]
    fn historical_hard_break_invalidates_previously_valid_input() {
        for alternate in ["", "\x1b[?1049halt\x1b[?1049l"] {
            let mut terminal = Terminal::with_scrollback(8, 1, None).unwrap();
            let mut shell = ShellTracker::new();
            terminal.feed_with([PROMPT, "abcdefghijklmnop"], &mut shell);
            terminal.feed_with(["\r\n", alternate, "new"], &mut shell);
            terminal.resize_with(8, 5, &mut shell);
            let events = terminal.feed_with(SUBMIT, &mut shell);

            assert!(matches!(
                &events[0],
                ShellEvent::CommandSubmitted(command) if command.text.is_none()
            ));
        }
    }

    #[test]
    fn edited_wrap_break_remains_invalid_after_scrolling() {
        let mut terminal = Terminal::with_scrollback(8, 3, None).unwrap();
        let mut shell = ShellTracker::new();
        terminal.feed_with([PROMPT, &"abcdefgh".repeat(6)], &mut shell);
        terminal.feed_with("\x1b[H\x1b[P\x1b[3Snew", &mut shell);
        terminal.resize_with(8, 12, &mut shell);
        let events = terminal.feed_with(SUBMIT, &mut shell);

        assert!(matches!(
            &events[0],
            ShellEvent::CommandSubmitted(command) if command.text.is_none()
        ));
    }

    #[test]
    fn printing_rechecks_wraps_above_scrolling_margins() {
        for text in [
            "abcdefghijklmnx",
            "abcdefghijklmnxy",
            "abcdefghijklmn\x1b[2b",
        ] {
            let input = format!("\x1b[2;4r{PROMPT}{text}{SUBMIT}");

            for fragmented in [false, true] {
                let mut terminal = Terminal::new(4, 4).unwrap();
                let mut shell = ShellTracker::new();

                let events = if fragmented {
                    terminal.feed_with(input.chars(), &mut shell)
                } else {
                    terminal.feed_with(&input, &mut shell)
                };

                assert!(
                    matches!(
                        &events[1],
                        ShellEvent::CommandSubmitted(command) if command.text.is_none()
                    ),
                    "text {text:?}, fragmented {fragmented}: {events:?}"
                );
            }
        }
    }

    #[test]
    fn newline_before_submission_preserves_command_text() {
        for text in ["echo", "echo ok"] {
            let mut terminal = Terminal::new(8, 3).unwrap();
            let mut shell = ShellTracker::new();
            let events = terminal.feed_with([PROMPT, text, "\r\n", SUBMIT], &mut shell);

            assert!(matches!(
                &events[1],
                ShellEvent::CommandSubmitted(command) if command.text.as_deref() == Some(text)
            ));
        }
    }

    #[test]
    fn del_preserves_command_tracking() {
        for text in ["echo\r\n\x7f", "echo\r\n\x7f\x7f", "echo\r\x7f\x1b[7G"] {
            let input = format!("{PROMPT}{text}{SUBMIT}");

            for fragmented in [false, true] {
                let mut terminal = Terminal::new(24, 4).unwrap();
                let mut shell = ShellTracker::new();

                let events = if fragmented {
                    terminal.feed_with(input.chars(), &mut shell)
                } else {
                    terminal.feed_with(&input, &mut shell)
                };

                assert!(
                    matches!(
                        &events[1],
                        ShellEvent::CommandSubmitted(command)
                            if command.text.as_deref() == Some("echo")
                    ),
                    "text {text:?}, fragmented {fragmented}: {events:?}"
                );
            }
        }
    }

    #[test]
    fn prompt_replacement_only_affects_the_active_buffer() {
        let mut terminal = Terminal::new(8, 2).unwrap();
        let mut shell = ShellTracker::new();
        terminal.feed_with(PROMPT, &mut shell);

        for (index, switch) in ["\x1b[?1047h", "\x1b[?1047l", "\x1b[?1047h"]
            .into_iter()
            .enumerate()
        {
            terminal.feed_with([switch, "\x1b[H", PROMPT], &mut shell);
            assert_eq!(shell.markers.len(), 2);
            let prompts: Vec<_> = shell.visible_prompts(&terminal).collect();
            assert_eq!(prompts.len(), 1);
            assert_eq!(prompts[0].id, PromptId(index as u64 + 1));
        }
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
