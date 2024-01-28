use avt::util::{TextCollector, TextCollectorOutput};
use avt::Vt;
use std::convert::Infallible;
use std::error::Error;

struct StdoutOutput;

impl TextCollectorOutput for StdoutOutput {
    type Error = Infallible;

    fn push(&mut self, line: String) -> Result<(), Self::Error> {
        println!("{}", line);

        Ok(())
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let vt = Vt::builder()
        .size(1000, 100)
        .resizable(true)
        .scrollback_limit(100)
        .build();

    let input = std::io::stdin();
    let mut collector = TextCollector::new(vt, StdoutOutput);
    let mut line = String::new();

    while let Ok(n) = input.read_line(&mut line) {
        if n == 0 {
            break;
        };

        collector.feed_str(&line)?;

        line = String::new();
    }

    collector.flush()?;

    Ok(())
}
