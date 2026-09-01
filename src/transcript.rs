use crate::{Line, Observation, Observer, Terminal};

/// Plain text from primary-buffer eviction and final retained content.
#[derive(Default)]
pub struct Transcript {
    pending: String,
    ready: Vec<String>,
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn finish(mut self, terminal: &Terminal) -> Vec<String> {
        for line in terminal.primary_screen().all_lines() {
            self.push(line);
        }

        if !self.pending.is_empty() {
            self.ready.push(self.pending.trim_end().to_owned());
        }

        while self.ready.last().is_some_and(String::is_empty) {
            self.ready.pop();
        }

        self.ready
    }

    fn push(&mut self, line: &Line) {
        self.pending.push_str(&line.text());

        if !line.is_wrapped() {
            self.ready.push(self.pending.trim_end().to_owned());
            self.pending.clear();
        }
    }
}

impl Observer for Transcript {
    type Output = Vec<String>;

    fn evicted(&mut self, _: &mut Observation<'_>, line: &Line) {
        self.push(line);
    }

    fn end(&mut self, _: &mut Observation<'_>) -> Self::Output {
        std::mem::take(&mut self.ready)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_wrapped_rows_preserving_internal_spaces() {
        let mut terminal = Terminal::new(5, 2).unwrap();
        let mut transcript = Transcript::new();
        let mut lines = terminal.feed_with("a   b\r\nc   de   fgh\r\ni", &mut transcript);
        lines.extend(transcript.finish(&terminal));
        assert_eq!(lines, ["a   b", "c   de   fgh", "i"]);
    }
}
