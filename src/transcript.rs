use crate::{Line, Observation, Observer, Terminal};

/// Plain text from primary-buffer eviction and final retained content.
/// ED2-cleared content follows the normal history path, including editable prompts.
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
        self.pending.extend(line.chars());

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

    #[test]
    fn finish_reads_primary_content_while_alternate_is_active() {
        let mut terminal = Terminal::with_scrollback(10, 2, None).unwrap();
        let mut transcript = Transcript::new();

        assert!(
            terminal
                .feed_with("a\r\nb\r\nc\r\n\x1b[?1049halt\x1b[2Jnew", &mut transcript)
                .is_empty()
        );

        assert_eq!(transcript.finish(&terminal), ["a", "b", "c"]);
    }

    #[test]
    fn issue_739_transcript_survives_clears_and_st_markers() {
        let input = concat!(
            "echo \"before clear\"\r\n",
            "before clear\r\n",
            "\x1b[H\x1b[2J\x1b[3J",
            "echo \"after clear\"\r\n",
            "after clear\r\n",
            "\x1b[H\x1b[2J",
            "\x1b]133;D;0\x1b\\",
            "\x1b]133;A\x1b\\",
            "prompt text",
            "\x1b]133;B\x1b\\",
            "echo \"after ctrl+l\"\r\n",
            "after ctrl+l\r\n",
        );

        for limit in [Some(0), Some(1), None] {
            let mut terminal = Terminal::with_scrollback(40, 5, limit).unwrap();
            let mut transcript = Transcript::new();
            let mut lines = terminal.feed_with(input, &mut transcript);
            lines.extend(transcript.finish(&terminal));

            assert_eq!(
                lines,
                [
                    "echo \"before clear\"",
                    "before clear",
                    "echo \"after clear\"",
                    "after clear",
                    "prompt textecho \"after ctrl+l\"",
                    "after ctrl+l",
                ]
            );
        }
    }
}
