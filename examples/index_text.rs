//! Compose a custom primary-text indexer with shell tracking.
use std::collections::BTreeSet;

use avt::observer::{ShellEvent, ShellTracker, Transcript};
use avt::{Line, Observation, Observer, Operation, Terminal};

#[derive(Default)]
struct TextIndex {
    transcript: Transcript,
    words: BTreeSet<String>,
}

impl TextIndex {
    fn index(&mut self, lines: Vec<String>) -> Vec<String> {
        let mut added = Vec::new();

        for line in lines {
            for word in line.split(|ch: char| !ch.is_alphanumeric()) {
                if !word.is_empty() {
                    let word = word.to_lowercase();

                    if self.words.insert(word.clone()) {
                        added.push(word);
                    }
                }
            }
        }

        added
    }

    fn finish(mut self, terminal: &Terminal) -> Vec<String> {
        let lines = std::mem::take(&mut self.transcript).finish(terminal);

        self.index(lines)
    }
}

impl Observer for TextIndex {
    type Output = Vec<String>;

    // Delegate primary-line reconstruction to Transcript, including evictions.
    fn before(&mut self, context: &mut Observation<'_>, operation: &Operation) {
        self.transcript.before(context, operation);
    }

    fn evicted(&mut self, context: &mut Observation<'_>, line: &Line) {
        self.transcript.evicted(context, line);
    }

    fn end(&mut self, context: &mut Observation<'_>) -> Self::Output {
        let lines = self.transcript.end(context);

        self.index(lines)
    }
}

fn main() {
    let mut terminal = Terminal::new(24, 2).unwrap();
    let mut indexer = TextIndex::default();
    let mut shell = ShellTracker::new();

    for output in [
        "\x1b]133;A\x07$ \x1b]133;B\x07echo hello\x1b]133;C\x07\r\n",
        "hello world\r\n\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07",
    ] {
        let (words, events) = terminal.feed_with(output, (&mut indexer, &mut shell));
        println!("new words: {words:?}");

        for event in events {
            if let ShellEvent::CommandSubmitted(command) = event {
                println!("command: {:?}", command.text);
            }
        }
    }

    println!("remaining words: {:?}", indexer.finish(&terminal));
}
