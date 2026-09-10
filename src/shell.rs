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
            if let Some(start) = self
                .input
                .as_ref()
                .and_then(|anchor| context.resolve(anchor))
            {
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
