use crate::parser::Function;
use crate::{Line, Observation, Observer, Operation, Terminal};

/// Plain text from primary-buffer eviction and final retained content.
/// ED2-cleared content follows the normal history path, including editable prompts.
/// Hard reset (RIS) finishes retained primary text before clearing the terminal.
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
        self.finish_primary(terminal);

        self.ready
    }

    fn finish_primary(&mut self, terminal: &Terminal) {
        let start = self.ready.len();

        for line in terminal.primary_screen().all_lines() {
            self.push(line);
        }

        // Trim screen padding without removing output from earlier in this call.
        while self.ready.len() > start && self.ready.last().is_some_and(String::is_empty) {
            self.ready.pop();
        }
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

    fn before(&mut self, context: &mut Observation<'_>, operation: &Operation) {
        if matches!(operation, Operation::Function(Function::Ris)) {
            self.finish_primary(context.terminal());
        }
    }

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

    const PROMPT: &str = "\x1b]133;A\x07$ \x1b]133;B\x07";
    const SUBMIT: &str = "\x1b]133;C\x07";

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
    fn transcript_preserves_reflowed_output_and_editable_prompts_on_ed2() {
        let mut terminal = Terminal::new(24, 4).unwrap();
        let mut transcript = Transcript::new();

        let mut lines =
            terminal.feed_with(["previous output\r\n", PROMPT, "wrong"], &mut transcript);

        lines.extend(terminal.resize_with(12, 6, &mut transcript));

        lines.extend(terminal.feed_with(
            ["\x1b[H\x1b[2J", PROMPT, "corrected", SUBMIT],
            &mut transcript,
        ));

        lines.extend(transcript.finish(&terminal));
        assert_eq!(lines, ["previous output", "$ wrong", "$ corrected"]);
    }

    #[test]
    fn repeated_ed2_clears_keep_evicted_editable_input_once() {
        let mut terminal = Terminal::new(8, 2).unwrap();
        let mut transcript = Transcript::new();

        let mut lines = terminal.feed_with(
            [
                "prior\r\n",
                PROMPT,
                "an unfinished and very long command\x1b[H\x1b[2J\x1b[2J",
                PROMPT,
                "ok",
                SUBMIT,
                "\r\nresult",
            ],
            &mut transcript,
        );

        lines.extend(transcript.finish(&terminal));

        assert_eq!(
            lines,
            [
                "prior",
                "$ an unfinished and very long command",
                "$ ok",
                "result"
            ]
        );
    }

    #[test]
    fn ordinary_transcript_unwraps_and_ignores_alternate_screen_output() {
        let mut terminal = Terminal::new(5, 2).unwrap();
        let mut transcript = Transcript::new();

        let mut lines = terminal.feed_with(
            "abcdefgh\r\nsecond\r\n\x1b[?1049hignored\r\nignored\r\n\x1b[?1049lthird",
            &mut transcript,
        );

        lines.extend(transcript.finish(&terminal));
        assert_eq!(lines, ["abcdefgh", "second", "third"]);
    }

    #[test]
    fn unfinished_evicted_line_survives_feed_boundaries() {
        let mut terminal = Terminal::new(4, 1).unwrap();
        let mut transcript = Transcript::new();
        assert!(terminal.feed_with("abcdE", &mut transcript).is_empty());
        assert!(terminal.feed_with("", &mut transcript).is_empty());
        assert_eq!(terminal.feed_with("F\r\n", &mut transcript), ["abcdEF"]);
        assert!(transcript.finish(&terminal).is_empty());
    }

    #[test]
    fn hard_reset_flushes_primary_text_across_retention_and_chunking() {
        for limit in [Some(0), Some(1), None] {
            for text in ["abcdE", "abcdefghi"] {
                for alternate in ["", "\x1b[?1049halt"] {
                    let input = format!("{text}{alternate}\x1bc\x1bcnew");

                    for split in 0..=input.len() {
                        let mut terminal = Terminal::with_scrollback(4, 1, limit).unwrap();
                        let mut transcript = Transcript::new();
                        let mut lines = Vec::new();

                        for chunk in [&input[..split], &input[split..]] {
                            lines.extend(terminal.feed_with(chunk, &mut transcript));
                        }

                        lines.extend(transcript.finish(&terminal));
                        assert_eq!(lines, [text, "new"], "{limit:?}, {input:?}, {split}");
                    }
                }
            }
        }
    }

    #[test]
    fn hard_reset_preserves_already_queued_blank_lines_in_the_same_feed() {
        let input = "old\r\n\r\n\x1bcnew";

        for split in 0..=input.len() {
            let mut terminal = Terminal::new(4, 1).unwrap();
            let mut transcript = Transcript::new();
            let mut lines = Vec::new();

            for chunk in [&input[..split], &input[split..]] {
                lines.extend(terminal.feed_with(chunk, &mut transcript));
            }

            lines.extend(transcript.finish(&terminal));
            assert_eq!(lines, ["old", "", "new"], "split at {split}");
        }
    }

    #[test]
    fn explicit_hard_reset_returns_retained_text_without_screen_padding() {
        let mut terminal = Terminal::with_scrollback(4, 4, None).unwrap();
        let mut transcript = Transcript::new();
        terminal.feed_with("one\r\n\r\ntwo", &mut transcript);

        assert_eq!(
            terminal.execute_with(Function::Ris, &mut transcript),
            ["one", "", "two"]
        );

        assert!(transcript.finish(&terminal).is_empty());
    }

    #[test]
    fn repeated_ed2_preserves_transcript_order_across_retention_limits() {
        for limit in [Some(0), Some(1), Some(2), Some(10), None] {
            for prompt in ["$ ", PROMPT] {
                let input = format!(
                    "aa\r\nbb\r\ncc\r\n{prompt}wrong\x1b[H\x1b[2J\x1b[2J{prompt}right\x1b[H\x1b[2J{prompt}done"
                );

                for chunked in [false, true] {
                    let mut terminal = Terminal::with_scrollback(12, 2, limit).unwrap();
                    let mut transcript = Transcript::new();
                    let mut lines = Vec::new();

                    if chunked {
                        for ch in input.chars() {
                            lines.extend(terminal.feed_with(ch, &mut transcript));
                        }
                    } else {
                        lines.extend(terminal.feed_with(&input, &mut transcript));
                    }

                    lines.extend(transcript.finish(&terminal));
                    assert_eq!(lines, ["aa", "bb", "cc", "$ wrong", "$ right", "$ done"]);
                }
            }
        }
    }

    #[test]
    fn ed2_after_explicit_scrolling_preserves_each_row_once() {
        for limit in [Some(0), Some(1), Some(10), None] {
            let mut terminal = Terminal::with_scrollback(12, 6, limit).unwrap();
            let mut transcript = Transcript::new();

            let mut lines = terminal.feed_with(
                "one\r\ntwo\r\n$ draft\x1b[2S\x1b[2A\x1b[H\x1b[2J$ new",
                &mut transcript,
            );

            lines.extend(transcript.finish(&terminal));
            assert_eq!(lines, ["one", "two", "$ draft", "$ new"]);
        }
    }

    #[test]
    fn ed2_keeps_blank_soft_wrap_terminators_even_after_prefix_eviction() {
        for limit in [Some(0), Some(1), None] {
            for rows in [1, 4] {
                for text in ["abcd ", "     "] {
                    let mut terminal = Terminal::with_scrollback(4, rows, limit).unwrap();
                    let mut transcript = Transcript::new();
                    let mut lines = terminal.feed_with(text, &mut transcript);
                    lines.extend(terminal.feed_with("\x1b[H\x1b[2J\x1b[2Jnew", &mut transcript));
                    lines.extend(transcript.finish(&terminal));

                    assert_eq!(
                        lines,
                        [text.trim_end(), "new"],
                        "{limit:?}, {rows}, {text:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn partial_ed_preserves_history_without_archiving_screen() {
        for clear in ["\x1b[H\x1b[J", "\x1b[2;8H\x1b[1J"] {
            let mut terminal = Terminal::with_scrollback(8, 2, None).unwrap();
            let mut transcript = Transcript::new();
            terminal.feed_with("history\r\none\r\ntwo", &mut transcript);
            assert!(terminal.feed_with(clear, &mut transcript).is_empty());
            assert_eq!(terminal.primary_screen().all_lines().count(), 3);

            assert!(
                terminal
                    .screen()
                    .visible_lines()
                    .all(|line| line.is_blank())
            );

            assert_eq!(transcript.finish(&terminal), ["history"]);
        }
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
