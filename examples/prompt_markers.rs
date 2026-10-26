//! Locate prompt annotations beside visible screen rows, including after reflow.
use avt::Terminal;
use avt::observer::ShellTracker;

fn main() {
    let mut terminal = Terminal::new(24, 6).unwrap();
    let mut shell = ShellTracker::new();

    for output in [
        "\x1b]133;A\x07$ \x1b]133;B\x07echo hello\x1b]133;C\x07",
        "\r\nhello\r\n\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07",
        "false\x1b]133;C\x07\r\n\x1b]133;D;1\x07",
    ] {
        terminal.feed_with(output, &mut shell);
        print_prompts(&terminal, &shell);
    }

    terminal.resize_with(8, 10, &mut shell);
    println!("After resize:");
    print_prompts(&terminal, &shell);
}

fn print_prompts(terminal: &Terminal, shell: &ShellTracker) {
    for prompt in shell.visible_prompts(terminal) {
        // Use the row to position an annotation and the outcome to choose a color.
        println!("row {}: {:?}", prompt.row, prompt.outcome);
    }
}
