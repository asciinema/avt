//! Associate submitted commands with application-provided prompt-start times.
use avt::Terminal;
use avt::{ShellEvent, ShellTracker};
use std::collections::HashMap;

fn main() {
    let timestamped_input = [
        (10.0, "\x1b]133;A\x07$ \x1b]133;B\x07"),
        (
            12.0,
            "echo hello\x1b]133;C\x07\r\nhello\r\n\x1b]133;D;0\x07",
        ),
        (15.0, "\x1b]133;A\x07$ \x1b]133;B\x07"),
        (16.0, "pwd\x1b]133;C\x07\r\n/home/demo\r\n\x1b]133;D;0\x07"),
    ];

    let mut terminal = Terminal::new(24, 5).unwrap();
    let mut shell = ShellTracker::new();
    let mut starts = HashMap::new();

    for (time, output) in timestamped_input {
        for event in terminal.feed_with(output, &mut shell) {
            match event {
                ShellEvent::PromptStarted(id) => {
                    // Save the prompt-start time until its command is submitted.
                    starts.insert(id, time);
                }

                ShellEvent::CommandSubmitted(command) => {
                    let start = command.prompt.and_then(|id| starts.remove(&id));

                    if let (Some(start), Some(label)) = (start, command.text) {
                        println!("{start:.2}s: {label}");
                    }
                }

                ShellEvent::CommandCancelled { prompt: Some(id) } => {
                    starts.remove(&id);
                }

                _ => {}
            }
        }
    }
}
