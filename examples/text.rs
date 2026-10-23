//! Convert a UTF-8 terminal stream from stdin into plain text.
//!
//! ```sh
//! printf '\033[31mHello\033[0m\r\nWorking...\rDone\033[K\033[H\033[2JAfter clear' \
//!   | cargo run --quiet --example text
//! ```
//!
//! Output:
//!
//! ```text
//! Hello
//! Done
//! After clear
//! ```

use avt::Terminal;
use avt::observer::Transcript;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = std::io::stdin();
    let mut buf = String::new();

    // Use the source terminal's dimensions. No retained scrollback is needed.
    let mut terminal = Terminal::new(80, 24)?;
    let mut transcript = Transcript::new();

    while input.read_line(&mut buf)? != 0 {
        // Emit completed logical lines as they leave the terminal buffer.
        for line in terminal.feed_with(&buf, &mut transcript) {
            println!("{}", line);
        }

        buf.clear();
    }

    // Include the remaining primary-screen text at EOF.
    for line in transcript.finish(&terminal) {
        println!("{}", line);
    }

    Ok(())
}
