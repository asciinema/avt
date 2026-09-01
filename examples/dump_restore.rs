//! Initialize a second terminal from a state dump and continue processing input.
use avt::Terminal;

fn main() {
    let mut source = Terminal::new(24, 4).unwrap();
    source.feed("header\r\n\x1b[31mred\x1b[0m");

    let (cols, rows) = source.size();
    let mut restored = Terminal::new(cols, rows).unwrap();
    restored.feed(&source.dump());

    // Feed the same subsequent input to both terminals.
    let output = "\r\ncontinued";
    source.feed(output);
    restored.feed(output);
    println!("{}", restored.screen().text().join("\n"));
}
