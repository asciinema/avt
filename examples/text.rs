use avt::{Terminal, Transcript};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = std::io::stdin();
    let mut buf = String::new();
    let mut terminal = Terminal::with_scrollback(1000, 100, Some(100))?;
    let mut transcript = Transcript::new();

    while input.read_line(&mut buf)? != 0 {
        for line in terminal.feed_with(&buf, &mut transcript) {
            println!("{}", line);
        }

        buf.clear();
    }

    for line in transcript.finish(&terminal) {
        println!("{}", line);
    }

    Ok(())
}
