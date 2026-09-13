//! Preserve primary text across scrolling, reflow, and a screen clear.
use avt::Terminal;
use avt::Transcript;

fn main() {
    let mut terminal = Terminal::new(24, 2).unwrap();
    let mut transcript = Transcript::new();
    let mut lines = terminal.feed_with("one\r\na longer second line\r\nthree", &mut transcript);
    lines.extend(terminal.resize_with(12, 3, &mut transcript));
    lines.extend(terminal.feed_with("\x1b[H\x1b[2Jfour", &mut transcript));

    // Collect the remaining primary content when the stream ends.
    lines.extend(transcript.finish(&terminal));
    println!("{}", lines.join("\n"));
}
