//! Extract submitted commands from OSC 133 shell integration markers.
use avt::Terminal;
use avt::{ShellEvent, ShellTracker};

fn main() {
    let mut terminal = Terminal::new(24, 4).unwrap();
    let mut shell = ShellTracker::new();

    // A starts a prompt, B starts input, C submits it, and D ends the command.
    for output in [
        "\x1b]133;A\x07$ \x1b]133;B\x07echo hello\x1b]133;C\x07\r\nhello\r\n\x1b]133;D;0\x07",
        "\x1b]133;A\x07$ \x1b]133;B\x07pwd\x1b]133;C\x07\r\n/home/demo\r\n\x1b]133;D;0\x07",
    ] {
        for event in terminal.feed_with(output, &mut shell) {
            if let ShellEvent::CommandSubmitted(command) = event {
                println!("{}", command.text.as_deref().unwrap_or("<unavailable>"));
            }
        }
    }
}
