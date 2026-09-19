mod terminal;

use crate::parser::{EdScope, Function};
use crate::{
    DirtyRows, Input, IntoInputs, Observation, Observer, Operation, ShellEvent, ShellTracker,
    Terminal, Transcript,
};
use proptest::prelude::*;

const PROMPT: &str = "\x1b]133;A\x07$ \x1b]133;B\x07";
const SUBMIT: &str = "\x1b]133;C\x07";

fn collect(input: &str, split: usize) -> (Vec<ShellEvent>, Vec<String>) {
    let mut terminal = Terminal::new(24, 4).unwrap();
    let mut shell = ShellTracker::new();
    let mut transcript = Transcript::new();
    let mut events = Vec::new();
    let mut lines = Vec::new();

    for chunk in [&input[..split], &input[split..]] {
        let (shell_events, text) = terminal.feed_with(chunk, (&mut shell, &mut transcript));
        events.extend(shell_events);
        lines.extend(text);
    }

    lines.extend(transcript.finish(&terminal));

    (events, lines)
}

#[test]
fn captures_commands_and_status_before_same_feed_overwrite() {
    let input = format!(
        "{PROMPT}ecxo\x1b[2Dh\x1b[C{SUBMIT}\x1b[2J\x1b[Hresult\x1b]133;D;7\x07{PROMPT}cancel\x1b]133;D\x07"
    );

    let (events, _) = collect(&input, 0);

    let command = events
        .iter()
        .find_map(|event| match event {
            ShellEvent::CommandSubmitted(command) => Some(command),
            _ => None,
        })
        .unwrap();

    assert_eq!(command.text.as_deref(), Some("echo"));
    assert_eq!(command.prompt, Some(crate::PromptId(0)));

    assert!(events.contains(&ShellEvent::CommandFinished {
        id: command.id,
        exit_status: Some(7)
    }));

    assert!(events.contains(&ShellEvent::CommandCancelled {
        prompt: Some(crate::PromptId(1))
    }));
}

#[test]
fn paired_observers_are_independent_of_every_input_split() {
    let input = format!(
        "old\r\n{PROMPT}wrong\x1b[H\x1b[2J{PROMPT}echo 界{SUBMIT}\r\nresult\r\n\x1b]133;D;0\x1b\\{PROMPT}"
    );

    for (input, expected_lines) in [
        (
            input.as_str(),
            &["old", "$ wrong", "$ echo 界", "result", "$"][..],
        ),
        (
            "before\r\n$ 界\x1b[H\x1b[2J$ corrected",
            &["before", "$ 界", "$ corrected"][..],
        ),
    ] {
        let expected = collect(input, 0);
        assert_eq!(expected.1, expected_lines);

        for split in input
            .char_indices()
            .map(|(index, _)| index)
            .chain([input.len()])
        {
            assert_eq!(collect(input, split), expected, "split at byte {split}");
        }

        let mut terminal = Terminal::new(24, 4).unwrap();
        let mut shell = ShellTracker::new();
        let mut transcript = Transcript::new();
        let mut events = Vec::new();
        let mut lines = Vec::new();

        for ch in input.chars() {
            let (batch, text) = terminal.feed_with(ch, (&mut shell, &mut transcript));
            events.extend(batch);
            lines.extend(text);
        }

        lines.extend(transcript.finish(&terminal));
        assert_eq!((events, lines), expected);
    }
}

#[test]
fn wraps_and_scrolls_command_while_start_is_retained() {
    let mut terminal = Terminal::new(6, 3).unwrap();
    let mut shell = ShellTracker::new();

    let events = terminal.feed_with(["old\r\nold\r\n", PROMPT, "echo 界", SUBMIT], &mut shell);

    let texts: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ShellEvent::CommandSubmitted(command) => Some(command.text.as_deref()),
            _ => None,
        })
        .collect();

    assert_eq!(texts, [Some("echo 界")]);
}

#[test]
fn unavailable_input_is_explicit() {
    for input in [
        format!("missing B{SUBMIT}"),
        format!("{PROMPT}first\r\nsecond{SUBMIT}"),
        format!("{PROMPT}lost\x1b[2J{SUBMIT}"),
        format!("{PROMPT}long enough to evict the start of input completely{SUBMIT}"),
    ] {
        let mut terminal = Terminal::new(8, 3).unwrap();
        let mut shell = ShellTracker::new();
        let events = terminal.feed_with(&input, &mut shell);

        let command = events
            .iter()
            .find_map(|event| match event {
                ShellEvent::CommandSubmitted(command) => Some(command),
                _ => None,
            })
            .unwrap();

        assert!(command.text.is_none(), "{input:?}");

        assert_eq!(
            command.prompt,
            input.starts_with(PROMPT).then_some(crate::PromptId(0))
        );
    }
}

#[test]
fn command_capture_survives_reflow_edits_and_scrolling() {
    let mut terminal = Terminal::new(24, 4).unwrap();
    let mut shell = ShellTracker::new();
    terminal.feed_with([PROMPT, "echo ok"], &mut shell);
    terminal.resize_with(4, 6, &mut shell);
    let events = terminal.feed_with(SUBMIT, &mut shell);

    assert!(matches!(
        &events[0],
        ShellEvent::CommandSubmitted(command) if command.text.as_deref() == Some("echo ok")
    ));

    for input in [
        format!("{PROMPT}echo\r\x1b[P{SUBMIT}"),
        format!("\x1b[2;4r\x1b[4;1H{PROMPT}echo{SUBMIT}"),
    ] {
        let mut terminal = Terminal::new(6, 5).unwrap();
        let mut shell = ShellTracker::new();
        let events = terminal.feed_with(&input, &mut shell);

        assert!(events.iter().any(|event| matches!(
            event,
            ShellEvent::CommandSubmitted(command) if command.text.as_deref() == Some("echo")
        )));
    }

    let mut terminal = Terminal::new(6, 5).unwrap();
    let mut shell = ShellTracker::new();

    let events = terminal.feed_with(
        ["\x1b[2;4r\x1b[4;1H", PROMPT, "echo ok", SUBMIT],
        &mut shell,
    );

    // The existing engine breaks the logical wrap link when scrolling a region
    // whose bottom is above the screen bottom. The position still tracks, but
    // reconstruction must stay unknown rather than return a partial command.
    assert!(!terminal.screen().line(2).unwrap().is_wrapped());
    assert_eq!(shell.visible_prompts(&terminal).next().unwrap().row, 2);

    assert!(events.iter().any(
        |event| matches!(event, ShellEvent::CommandSubmitted(command) if command.text.is_none())
    ));
}

#[test]
fn transcript_preserves_reflowed_output_and_editable_prompts_on_ed2() {
    let mut terminal = Terminal::new(24, 4).unwrap();
    let mut transcript = Transcript::new();

    let mut lines = terminal.feed_with(["previous output\r\n", PROMPT, "wrong"], &mut transcript);

    lines.extend(terminal.resize_with(12, 6, &mut transcript));

    lines.extend(terminal.feed_with(
        ["\x1b[H\x1b[2J", PROMPT, "corrected", SUBMIT],
        &mut transcript,
    ));

    lines.extend(transcript.finish(&terminal));
    assert_eq!(lines, ["previous output", "$ wrong", "$ corrected"]);
}

#[test]
fn clamped_cursor_before_input_makes_command_unavailable() {
    let mut terminal = Terminal::new(8, 4).unwrap();
    let mut shell = ShellTracker::new();
    terminal.feed_with(PROMPT, &mut shell);
    terminal.resize_with(2, 4, &mut shell);
    let events = terminal.feed_with(["x", SUBMIT], &mut shell);
    assert!(matches!(&events[0], ShellEvent::CommandSubmitted(command) if command.text.is_none()));
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
fn unknown_exit_status_is_not_reported_as_success() {
    let mut terminal = Terminal::new(24, 4).unwrap();
    let mut shell = ShellTracker::new();

    let events = terminal.feed_with([PROMPT, "cmd", SUBMIT, "\x1b]133;D\x07"], &mut shell);

    assert!(matches!(
        events.last(),
        Some(ShellEvent::CommandFinished {
            exit_status: None,
            ..
        })
    ));
}

#[test]
fn reset_discards_editing_and_running_state() {
    for input in [format!("{PROMPT}draft"), format!("{PROMPT}cmd{SUBMIT}")] {
        let mut terminal = Terminal::new(24, 4).unwrap();
        let mut shell = ShellTracker::new();
        terminal.feed_with(input, &mut shell);
        terminal.execute_with(Function::Ris, &mut shell);
        assert!(shell.visible_prompts(&terminal).next().is_none());
        assert!(terminal.feed_with("\x1b]133;D\x07", &mut shell).is_empty());
    }
}

#[test]
fn missing_prompt_does_not_reuse_a_previous_commands_prompt() {
    let mut terminal = Terminal::new(24, 4).unwrap();
    let mut shell = ShellTracker::new();
    terminal.feed_with([PROMPT, "one", SUBMIT, "\x1b]133;D;0\x07"], &mut shell);
    let events = terminal.feed_with(["\r\n\x1b]133;B\x07two", SUBMIT], &mut shell);

    assert!(matches!(
        &events[0],
        ShellEvent::CommandSubmitted(command)
            if command.prompt.is_none() && command.text.as_deref() == Some("two")
    ));
}

#[derive(Debug, PartialEq)]
enum Notification {
    Resized(usize, usize),
    Dirty(std::ops::Range<usize>),
    Evicted(String),
}

// Observer outputs need not implement Default.
#[derive(Debug, PartialEq)]
struct CallReport {
    number: usize,
    before: String,
    after: String,
    size_before: (usize, usize),
    size_after: (usize, usize),
    operations: Vec<Operation>,
    hooks: Vec<&'static str>,
    notifications: Vec<Notification>,
}

#[derive(Default)]
struct Calls {
    notifications: Vec<Notification>,
    number: usize,
    before: String,
    size_before: (usize, usize),
    operations: Vec<Operation>,
    hooks: Vec<&'static str>,
    active: bool,
    in_action: bool,
    order: Option<(usize, std::rc::Rc<std::cell::Cell<usize>>)>,
}

impl Calls {
    fn check_order(&self) {
        if let Some((index, next)) = &self.order {
            assert_eq!(next.get(), *index);
            next.set(1 - index);
        }
    }
}

impl Observer for Calls {
    type Output = CallReport;

    fn begin(&mut self, context: &mut Observation<'_>) {
        self.check_order();
        assert!(!self.active);
        self.active = true;
        self.number += 1;
        self.before = context.terminal().dump();
        self.size_before = context.terminal().size();
        self.hooks.push("begin");
    }

    fn before(&mut self, _: &mut Observation<'_>, operation: &Operation) {
        self.check_order();
        assert!(self.active);
        assert!(!self.in_action);
        self.in_action = true;
        self.operations.push(operation.clone());
        self.hooks.push("before");
    }

    fn resized(&mut self, cols: usize, rows: usize) {
        self.check_order();
        assert!(self.in_action);
        self.notifications.push(Notification::Resized(cols, rows));
    }

    fn dirty(&mut self, rows: std::ops::Range<usize>) {
        self.check_order();
        assert!(self.in_action);
        self.notifications.push(Notification::Dirty(rows));
    }

    fn after(&mut self, context: &mut Observation<'_>, operation: &Operation) {
        self.check_order();
        assert!(self.active);
        assert!(self.in_action);
        self.in_action = false;
        assert_eq!(self.operations.last(), Some(operation));

        if let Operation::Resize { cols, rows } = operation {
            assert_eq!(context.terminal().size(), (*cols, *rows));
        }

        self.hooks.push("after");
    }

    fn evicted(&mut self, _: &mut Observation<'_>, line: &crate::Line) {
        self.check_order();
        assert!(self.active);
        assert!(!self.in_action);
        self.notifications.push(Notification::Evicted(line.text()));
    }

    fn end(&mut self, context: &mut Observation<'_>) -> Self::Output {
        self.check_order();
        assert!(self.active);
        assert!(!self.in_action);
        self.active = false;
        self.hooks.push("end");

        CallReport {
            number: self.number,
            before: std::mem::take(&mut self.before),
            after: context.terminal().dump(),
            size_before: self.size_before,
            size_after: context.terminal().size(),
            operations: std::mem::take(&mut self.operations),
            hooks: std::mem::take(&mut self.hooks),
            notifications: std::mem::take(&mut self.notifications),
        }
    }
}

#[test]
#[allow(clippy::needless_borrows_for_generic_args)] // Exercise borrowed forms too.
fn all_input_forms_share_one_feed_lifecycle() {
    fn check<I, Kind>(input: I, text: &str)
    where
        I: IntoInputs<Kind>,
    {
        let mut expected = Terminal::new(16, 3).unwrap();
        let mut actual = Terminal::new(16, 3).unwrap();
        let expected_report = expected.feed_with(text, Calls::default());
        let report = actual.feed_with(input, Calls::default());
        assert_eq!(report, expected_report);
        assert_eq!(report.number, 1);
        actual.engine.assert_eq(&expected.engine);
    }

    let text = "hé界";
    let owned = text.to_owned();
    check(text, text);
    check(&owned, text);
    check(owned.clone(), text);
    check('界', "界");
    check(&'界', "界");
    check('界'.encode_utf8(&mut [0; 4]), "界");
    check(Input::Text(text), text);
    check(Input::Char('界'), "界");
    check(Input::from(&owned), text);
    check(Input::from('界'), "界");

    let chunks = ["hé", "界"];
    check(chunks, text);
    check(&chunks[..], text);
    check(chunks.iter(), text);
    check(chunks.to_vec(), text);
    let chunks: Vec<String> = chunks.iter().map(|text| text.to_string()).collect();
    check(&chunks, text);
    check(&chunks[..], text);
    check(chunks.iter(), text);
    check(chunks, text);
    check(text.chars(), text);
    check(['h', 'é', '界'], text);
    check(&['h', 'é', '界'][..], text);
    check((0..3).map(|n| format!("{n}")), "012");

    let parts = [Input::Char('h'), Input::Text("é界")];
    check(parts, text);
    check(&parts[..], text);
    check(parts.iter(), text);
    check(["\x1b[3", "1m", "X"], "\x1b[31mX");
    check(Vec::<String>::new(), "");
    check(std::iter::empty::<Input<'_>>(), "");
}

#[test]
fn feed_processes_owned_chunks_before_advancing_iterator() {
    use std::cell::Cell;

    struct Progress<'a> {
        produced: &'a Cell<usize>,
        printed: usize,
    }

    impl Observer for Progress<'_> {
        type Output = usize;

        fn begin(&mut self, _: &mut Observation<'_>) {
            assert_eq!(self.produced.get(), 0);
        }

        fn after(&mut self, _: &mut Observation<'_>, operation: &Operation) {
            assert!(matches!(operation, Operation::Function(Function::Print(_))));
            self.printed += 1;
            assert_eq!(self.produced.get(), self.printed);
        }

        fn end(&mut self, _: &mut Observation<'_>) -> Self::Output {
            self.printed
        }
    }

    let produced = Cell::new(0);

    let chunks = (0..3).map(|n| {
        produced.set(n + 1);

        n.to_string()
    });

    let mut terminal = Terminal::new(8, 2).unwrap();

    let printed = terminal.feed_with(
        chunks,
        Progress {
            produced: &produced,
            printed: 0,
        },
    );

    assert_eq!(printed, 3);
    assert_eq!(terminal.screen().text()[0], "012");
}

#[test]
fn shell_events_and_transcript_survive_chunking_and_reflow() {
    fn replay(
        character_fragments: bool,
        separate_calls: bool,
    ) -> (Vec<ShellEvent>, Vec<String>, String) {
        let first = [
            "pré\r\n\x1b]13",
            "3;A\x07$ \x1b]133;B\x07ecxo\x1b[2Dh\x1b[C\x1b]133;C",
        ];

        let second = [
            "\x07\x1b[2J\x1b[Hdone\r\n\x1b]133;D;0\x07",
            "\x1b]133;A\x07$ \x1b]133;B\x07pwd\x1b]133;C\x07\r\n\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07cancel\x1b]133;D\x07",
        ];

        let mut terminal = Terminal::new(24, 5).unwrap();
        let mut shell = ShellTracker::new();
        let mut transcript = Transcript::new();
        let mut events = Vec::new();
        let mut lines = Vec::new();

        for (index, parts) in [first, second].into_iter().enumerate() {
            if index == 1 {
                let (batch, text) = terminal.resize_with(4, 8, (&mut shell, &mut transcript));
                events.extend(batch);
                lines.extend(text);
            }

            let fragments: Vec<_> = if character_fragments {
                parts
                    .into_iter()
                    .flat_map(str::chars)
                    .map(Input::Char)
                    .collect()
            } else {
                parts.into_iter().map(Input::Text).collect()
            };

            if separate_calls {
                for input in fragments {
                    let (batch, text) = terminal.feed_with(input, (&mut shell, &mut transcript));
                    events.extend(batch);
                    lines.extend(text);
                }
            } else {
                let (batch, text) = terminal.feed_with(fragments, (&mut shell, &mut transcript));
                events.extend(batch);
                lines.extend(text);
            }
        }

        lines.extend(transcript.finish(&terminal));

        (events, lines, terminal.dump())
    }

    let expected = replay(false, false);

    let commands: Vec<_> = expected
        .0
        .iter()
        .filter_map(|event| match event {
            ShellEvent::CommandSubmitted(command) => {
                Some((command.prompt, command.text.as_deref()))
            }

            _ => None,
        })
        .collect();

    assert_eq!(
        commands,
        [
            (Some(crate::PromptId(0)), Some("echo")),
            (Some(crate::PromptId(1)), Some("pwd")),
        ]
    );

    for (characters, separate) in [(false, true), (true, false), (true, true)] {
        assert_eq!(replay(characters, separate), expected);
    }
}

#[test]
fn dump_restoration_preserves_pending_shell_marker_recognition() {
    let mut original = Terminal::new(24, 4).unwrap();
    original.feed("\x1b]133;");
    let mut restored = Terminal::new(24, 4).unwrap();
    restored.feed(original.dump());
    let mut original_shell = ShellTracker::new();
    let mut restored_shell = ShellTracker::new();
    let tail = "A\x07$ \x1b]133;B\x07cmd\x1b]133;C\x07";
    let original_events = original.feed_with(tail, &mut original_shell);
    let restored_events = restored.feed_with(tail, &mut restored_shell);
    assert_eq!(original_events, restored_events);

    assert_eq!(
        restored_events[0],
        ShellEvent::PromptStarted(crate::PromptId(0))
    );

    assert!(matches!(
        &restored_events[1],
        ShellEvent::CommandSubmitted(command)
            if command.prompt == Some(crate::PromptId(0))
                && command.text.as_deref() == Some("cmd")
    ));

    original.engine.assert_eq(&restored.engine);
}

#[test]
fn tuple_observers_preserve_call_lifecycle_and_output_order() {
    let mut terminal = Terminal::new(3, 2).unwrap();
    // Every callback must visit the left observer before the right observer.
    let next = std::rc::Rc::new(std::cell::Cell::new(0));

    let mut first = Calls {
        order: Some((0, next.clone())),
        ..Calls::default()
    };

    let mut second = Calls {
        order: Some((1, next.clone())),
        ..Calls::default()
    };

    let (empty, other) = terminal.feed_with("", (&mut first, &mut second));
    assert_eq!(empty, other);
    assert_eq!(empty.number, 1);
    assert_eq!(empty.before, empty.after);
    assert_eq!(empty.hooks, ["begin", "end"]);
    assert!(empty.notifications.is_empty());

    let (partial, other) = terminal.feed_with("\x1b[3", (&mut first, &mut second));
    assert_eq!(partial, other);
    assert_eq!(partial.number, 2);
    assert_eq!(partial.before, empty.after);
    assert_ne!(partial.before, partial.after); // Parser continuation changed.
    assert_eq!(partial.hooks, ["begin", "end"]);
    assert!(partial.notifications.is_empty());

    let (complete, other) = terminal.feed_with("1mX", (&mut first, &mut second));
    assert_eq!(complete, other);
    assert_eq!(complete.before, partial.after);

    assert_eq!(
        complete.hooks,
        ["begin", "before", "after", "before", "after", "end"]
    );

    assert_eq!(complete.notifications, [Notification::Dirty(0..1)]);

    let (resize, other) = terminal.resize_with(3, 2, (&mut first, &mut second));
    assert_eq!(resize, other);
    assert_eq!(resize.before, complete.after);
    assert_eq!(resize.before, resize.after);
    assert_eq!(resize.hooks, ["begin", "before", "after", "end"]);
    assert_eq!(resize.operations, [Operation::Resize { cols: 3, rows: 2 }]);
    // The engine requests a repaint even when the requested size is unchanged.
    assert_eq!(resize.notifications, [Notification::Dirty(0..2)]);

    let (reset, other) = terminal.execute_with(Function::Ris, (&mut first, &mut second));
    assert_eq!(reset, other);
    assert_eq!(reset.number, 5);
    assert_eq!(reset.before, resize.after);
    assert_eq!(reset.hooks, ["begin", "before", "after", "end"]);
    assert_eq!(reset.operations, [Operation::Function(Function::Ris)]);
    assert_eq!(reset.notifications, [Notification::Dirty(0..2)]);
    assert_eq!(empty.hooks, ["begin", "end"]); // Earlier reports remain owned.

    terminal.feed_with("a\r\nb", (&mut first, &mut second));
    second.number = 10; // Distinguish outputs to detect swapped tuple results.
    let (resize, other) = terminal.resize_with(3, 1, (&mut first, &mut second));
    assert_eq!((resize.number, other.number), (7, 11));

    for report in [resize, other] {
        assert_eq!(report.size_before, (3, 2));
        assert_eq!(report.size_after, (3, 1));
        assert_eq!(report.hooks, ["begin", "before", "after", "end"]);
        assert_eq!(report.operations, [Operation::Resize { cols: 3, rows: 1 }]);

        assert_eq!(
            report.notifications,
            [
                Notification::Resized(3, 1),
                Notification::Dirty(0..1),
                Notification::Evicted("a  ".to_owned()),
            ]
        );
    }

    assert_eq!(next.get(), 0);
}

#[test]
fn resize_matches_explicit_operation_lifecycle_and_clamping() {
    for (cols, rows) in [(8, 4), (4, 3), (0, 0), (0, 2)] {
        let mut convenience = Terminal::new(4, 3).unwrap();
        let mut explicit = Terminal::new(4, 3).unwrap();

        for terminal in [&mut convenience, &mut explicit] {
            terminal.feed("abcd\r\nefgh\r\nlast");
        }

        let report = convenience.resize_with(cols, rows, Calls::default());

        let other_report =
            explicit.execute_with(Operation::Resize { cols, rows }, Calls::default());

        assert_eq!(report, other_report);
        assert_eq!(report.number, 1);
        assert_eq!(report.hooks, ["begin", "before", "after", "end"]);
        assert_eq!(report.size_before, (4, 3));
        assert_eq!(report.size_after, (cols.max(1), rows.max(1)));

        assert_eq!(
            report.operations,
            [Operation::Resize {
                cols: cols.max(1),
                rows: rows.max(1)
            }]
        );

        assert_eq!(convenience.dump(), explicit.dump());
    }
}

#[test]
fn only_host_resize_changes_geometry() {
    use crate::parser::XtwinopsOp;

    let mut terminal = Terminal::new(4, 3).unwrap();
    let mut calls = Calls::default();
    let request = terminal.feed_with("\x1b[8;7;11t", &mut calls);

    assert_eq!(
        request.operations,
        [Operation::Function(Function::Xtwinops(XtwinopsOp::Resize(
            11, 7
        )))]
    );

    // Only host resize actions change terminal geometry.
    assert_eq!(request.size_before, (4, 3));
    assert_eq!(request.size_after, (4, 3));

    let resize = terminal.resize_with(11, 7, &mut calls);
    assert_eq!(resize.operations, [Operation::Resize { cols: 11, rows: 7 }]);
    assert_eq!(resize.size_before, (4, 3));
    assert_eq!(resize.size_after, (11, 7));
    assert_eq!(resize.hooks, request.hooks);
}

#[test]
fn observed_functions_and_host_resizes_form_one_replayable_stream() {
    let mut terminal = Terminal::new(12, 5).unwrap();
    let mut calls = Calls::default();

    // Exercise recording a heap-backed operation across both hooks.
    let reports = [
        terminal.feed_with("hello\x1b[1;2;3;4;5;7;9m界\r\nworld", &mut calls),
        terminal.resize_with(8, 3, &mut calls),
        terminal.execute_with(Function::Cup(1, 1), &mut calls),
        terminal.feed_with("\x1b[?1049h\x1b[31malt", &mut calls),
        terminal.execute_with(Operation::Resize { cols: 6, rows: 4 }, &mut calls),
        terminal.feed_with("\x1b[?1049l\x1b[0m!", &mut calls),
    ];

    let mut replay = Terminal::new(12, 5).unwrap();

    for report in reports {
        assert_eq!(replay.dump(), report.before);
        assert_eq!(replay.size(), report.size_before);
        replay.execute(report.operations);
        assert_eq!(replay.dump(), report.after);
        assert_eq!(replay.size(), report.size_after);
    }

    replay.engine.assert_eq(&terminal.engine);
}

#[test]
fn evicted_lines_are_borrowed_and_released_one_at_a_time() {
    struct Evictions {
        anchors: Vec<crate::Anchor>,
        addresses: Vec<*const crate::Cell>,
        lines: Vec<String>,
    }

    impl Observer for Evictions {
        type Output = Vec<String>;

        fn after(&mut self, context: &mut Observation<'_>, _: &Operation) {
            for anchor in &self.anchors {
                assert!(context.resolve(anchor).is_some());
            }
        }

        fn evicted(&mut self, context: &mut Observation<'_>, line: &crate::Line) {
            let index = self.lines.len();
            assert_eq!(line.cells().as_ptr(), self.addresses[index]);
            assert!(context.resolve(&self.anchors[index]).is_none());

            for anchor in &self.anchors[..index] {
                // Retaining an original evicted line would keep its weak marks alive.
                assert_eq!(std::sync::Arc::weak_count(&anchor.0), 0);
            }

            for anchor in &self.anchors[index + 1..] {
                assert!(context.resolve(anchor).is_some());
            }

            self.lines.push(line.text().trim_end().to_owned());
        }

        fn end(&mut self, context: &mut Observation<'_>) -> Self::Output {
            for anchor in &self.anchors[..self.lines.len()] {
                assert!(context.resolve(anchor).is_none());
                assert_eq!(std::sync::Arc::weak_count(&anchor.0), 0);
            }

            assert_eq!(
                context.terminal().primary_screen().all_lines().count(),
                context.terminal().size().1
            );

            std::mem::take(&mut self.lines)
        }
    }

    for (operation, expected, visible) in [
        (
            Operation::Resize { cols: 6, rows: 1 },
            vec!["first", "second", "third"],
            vec!["last"],
        ),
        (
            Function::Ed(EdScope::All).into(),
            vec!["first", "second", "third", "last"],
            vec!["", "", "", ""],
        ),
    ] {
        let mut terminal = Terminal::new(6, 4).unwrap();
        terminal.feed("first\r\nsecond\r\nthird\r\nlast");

        let anchors = (0..4)
            .map(|row| {
                terminal
                    .anchor_at(crate::Position {
                        screen: crate::ScreenKind::Primary,
                        col: 0,
                        row,
                    })
                    .unwrap()
            })
            .collect();

        let mut evictions = Evictions {
            anchors,
            addresses: terminal
                .screen()
                .visible_lines()
                .map(|line| line.cells().as_ptr())
                .collect(),
            lines: Vec::new(),
        };

        let mut other = Evictions {
            anchors: evictions.anchors.clone(),
            addresses: evictions.addresses.clone(),
            lines: Vec::new(),
        };

        let (rows, (lines, other_lines)) =
            terminal.execute_with(operation, (DirtyRows::new(), (&mut evictions, &mut other)));

        assert_eq!(lines, other_lines);
        assert_eq!(lines, expected);
        assert_eq!(rows, (0..visible.len()).collect::<Vec<_>>());
        assert_eq!(terminal.screen().text(), visible);
    }
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
fn new_prompt_replaces_annotation_at_same_position() {
    let mut terminal = Terminal::new(24, 4).unwrap();
    let mut shell = ShellTracker::new();

    terminal.feed_with(
        [PROMPT, "one", SUBMIT, "\x1b]133;D;0\x07\r", PROMPT],
        &mut shell,
    );

    let prompts: Vec<_> = shell.visible_prompts(&terminal).collect();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].id, crate::PromptId(1));
    assert_eq!(prompts[0].outcome, crate::PromptOutcome::Editing);
}

fn input() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(vec![
            "abc",
            "界",
            "\r\n",
            "\x1b[H",
            "\x1b[2J",
            "\x1b[31m",
            "\x1b[0m",
            "\x1b7",
            "\x1b8",
            "\x1b[?1049h",
            "\x1b[?1049l",
            "\x1b[2;4r",
            "\x1b[?25l",
            "\x1b[?25h",
            "\x1b[2L",
            "\x1b[2M",
            "\x1bc",
            PROMPT,
            SUBMIT,
            "\x1b]133;D;42\x07",
            "\x1b[3",
        ]),
        0..40,
    )
    .prop_map(|parts| parts.concat())
}

proptest! {
    #[test]
    fn observed_execution_preserves_engine_state(
        first in input(),
        second in input(),
        cols in 2..20usize,
        rows in 2..8usize
    ) {
        let mut plain = Terminal::new(12, 5).unwrap();
        let mut observed = Terminal::new(12, 5).unwrap();
        let mut shell = ShellTracker::new();
        let mut transcript = Transcript::new();
        plain.feed(&first);
        let (mut events, mut lines) = observed.feed_with(&first, (&mut shell, &mut transcript));
        plain.resize(cols, rows);
        let (batch, text) = observed.resize_with(cols, rows, (&mut shell, &mut transcript));
        events.extend(batch);
        lines.extend(text);
        plain.feed(&second);
        let (batch, text) = observed.feed_with(&second, (&mut shell, &mut transcript));
        events.extend(batch);
        lines.extend(text);
        lines.extend(transcript.finish(&observed));
        observed.engine.verify();
        plain.engine.assert_eq(&observed.engine);
        plain.parser.assert_eq(&observed.parser);

        let mut chunked = Terminal::new(12, 5).unwrap();
        let mut chunked_shell = ShellTracker::new();
        let mut chunked_transcript = Transcript::new();
        let mut chunked_events = Vec::new();
        let mut chunked_lines = Vec::new();

        for (index, input) in [&first, &second].into_iter().enumerate() {
            if index == 1 {
                let (batch, text) =
                    chunked.resize_with(cols, rows, (&mut chunked_shell, &mut chunked_transcript));

                chunked_events.extend(batch);
                chunked_lines.extend(text);
            }

            for ch in input.chars() {
                let (batch, text) =
                    chunked.feed_with(ch, (&mut chunked_shell, &mut chunked_transcript));

                chunked_events.extend(batch);
                chunked_lines.extend(text);
            }
        }

        chunked_lines.extend(chunked_transcript.finish(&chunked));
        prop_assert_eq!(events, chunked_events);
        prop_assert_eq!(lines, chunked_lines);

        prop_assert_eq!(
            shell.visible_prompts(&observed).collect::<Vec<_>>(),
            chunked_shell.visible_prompts(&chunked).collect::<Vec<_>>()
        );

        plain.engine.assert_eq(&chunked.engine);

        // Standalone parsing and explicit execution preserve the same state.
        let mut parsed = Terminal::new(12, 5).unwrap();
        let mut parser = crate::Parser::new();
        parsed.execute(first.chars().filter_map(|ch| parser.feed(ch)));
        parsed.resize(cols, rows);
        parsed.execute(second.chars().filter_map(|ch| parser.feed(ch)));
        plain.engine.assert_eq(&parsed.engine);
        plain.parser.assert_eq(&parser);
    }
}

#[test]
fn eviction_before_buffer_switch_is_independent_of_feed_boundaries() {
    let mut batched = Terminal::new(12, 5).unwrap();
    let mut split = Terminal::new(12, 5).unwrap();
    batched.feed("\x1b[2M\x1b[?1049h");
    split.feed("\x1b[2M");
    split.feed("\x1b[?1049h");

    for terminal in [&mut batched, &mut split] {
        terminal.resize(2, 6);
        terminal.feed("\x1b[?1049l");
    }

    batched.engine.assert_eq(&split.engine);
    assert_eq!(batched.cursor().position(), Some((0, 0)));
}

#[test]
fn constructors_validate_both_dimensions_for_every_retention_policy() {
    for (cols, rows) in [(0, 0), (0, 3), (4, 0)] {
        let error = Some(crate::InvalidSize { cols, rows });
        assert_eq!(Terminal::new(cols, rows).err(), error);

        for limit in [Some(0), Some(100), None] {
            assert_eq!(Terminal::with_scrollback(cols, rows, limit).err(), error);
        }
    }

    for limit in [Some(0), Some(100), None] {
        assert_eq!(
            Terminal::with_scrollback(1, 1, limit).unwrap().size(),
            (1, 1)
        );
    }
}

#[test]
fn retention_controls_eviction_anchors_and_transcript_delivery() {
    for (limit, evicted, retained) in [
        (Some(0), vec!["aa", "bb"], 2),
        (Some(1), vec!["aa"], 3),
        (Some(2), vec![], 4),
        (None, vec![], 4),
    ] {
        let mut terminal = Terminal::with_scrollback(4, 2, limit).unwrap();
        let anchor = terminal.anchor_at_cursor();
        let mut transcript = Transcript::new();
        let mut text = terminal.feed_with("aa\r\nbb\r\ncc\r\ndd", &mut transcript);
        assert_eq!(text, evicted);
        assert_eq!(terminal.primary_screen().all_lines().count(), retained);

        if retained == 4 {
            assert_eq!(terminal.resolve(&anchor).unwrap().row, -2);
        } else {
            assert!(terminal.resolve(&anchor).is_none());
        }

        text.extend(terminal.resize_with(4, 1, &mut transcript));
        text.extend(transcript.finish(&terminal));
        assert_eq!(text, ["aa", "bb", "cc", "dd"]);
    }
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
fn ed2_archives_rows_and_anchors_preserving_cursor_and_margins() {
    let mut terminal = Terminal::with_scrollback(8, 6, None).unwrap();
    terminal.feed("one\r\n\r\nthree\x1b[2;4r\x1b[?6h\x1b[2;5H\x1b[?25l\x1b7");
    let cursor = terminal.cursor();

    let occupied = terminal
        .anchor_at(crate::Position {
            screen: crate::ScreenKind::Primary,
            col: 1,
            row: 2,
        })
        .unwrap();

    let blank = terminal
        .anchor_at(crate::Position {
            screen: crate::ScreenKind::Primary,
            col: 0,
            row: 5,
        })
        .unwrap();

    let addresses: Vec<_> = terminal
        .screen()
        .visible_lines()
        .take(3)
        .map(|line| line.cells().as_ptr())
        .collect();

    let mut dirty = DirtyRows::new();
    terminal.feed_with("", &mut dirty);

    assert_eq!(
        terminal.feed_with("\x1b[2J", &mut dirty),
        [0, 1, 2, 3, 4, 5]
    );

    assert_eq!(terminal.cursor(), cursor);
    assert_eq!(terminal.primary_screen().all_lines().count(), 9);

    assert_eq!(
        terminal
            .primary_screen()
            .all_lines()
            .take(3)
            .map(|line| line.cells().as_ptr())
            .collect::<Vec<_>>(),
        addresses
    );

    assert_eq!(
        terminal.screen().text(),
        ["one", "", "three", "", "", "", "", "", ""]
    );

    assert!(
        terminal
            .screen()
            .visible_lines()
            .all(|line| line.is_blank())
    );

    assert_eq!(terminal.resolve(&occupied).unwrap().row, -1);
    assert!(terminal.resolve(&blank).is_none());
    terminal.feed("\x1b[2J");
    assert_eq!(terminal.primary_screen().all_lines().count(), 9);
    terminal.feed("\x1b[H\x1b8");
    assert_eq!(terminal.cursor(), cursor);
    terminal.feed("\x1b[H");
    assert_eq!(terminal.cursor().row, 1); // Origin mode still uses the top margin.
    terminal.feed("\x1b[99B");
    assert_eq!(terminal.cursor().row, 3); // The bottom margin also survives.
}

#[test]
fn ed2_counts_styled_blanks_but_not_default_spaces_as_occupied() {
    for style in ["0", "31", "41", "1", "2", "3", "4", "5", "7", "9"] {
        let mut terminal = Terminal::with_scrollback(6, 5, None).unwrap();
        terminal.feed(format!("one\x1b[4;1H\x1b[{style}m \x1b[0m\x1b[2J"));
        let preserved = if style == "0" { 1 } else { 4 };
        assert_eq!(terminal.primary_screen().all_lines().count(), 5 + preserved);

        assert!(
            terminal
                .screen()
                .visible_lines()
                .all(|line| line.is_blank())
        );

        if style != "0" {
            let line = terminal.primary_screen().all_lines().nth(3).unwrap();
            assert!(!line.cells()[0].pen().is_default());
            assert_eq!(line.text(), "      ");
        }
    }
}

#[test]
fn ed2_uses_erase_pen_and_skips_default_blank_screens() {
    let mut terminal = Terminal::with_scrollback(6, 3, None).unwrap();
    terminal.feed("\x1b[2J\x1b[2J\x1b[41m\x1b[2J");
    assert_eq!(terminal.primary_screen().all_lines().count(), 3);

    assert!(terminal.screen().visible_lines().all(|line| {
        line.cells()
            .iter()
            .all(|cell| cell.pen().background() == Some(crate::Color::Indexed(1)))
    }));

    terminal.feed("\x1b[0m\x1b[2J\x1b[2J");
    assert_eq!(terminal.primary_screen().all_lines().count(), 6);

    assert!(
        terminal
            .screen()
            .visible_lines()
            .all(|line| line.is_blank())
    );
}

#[test]
fn ed2_preserves_pending_wrap() {
    let mut terminal = Terminal::with_scrollback(4, 2, None).unwrap();
    terminal.feed("abcd");
    let cursor = terminal.cursor();
    assert_eq!(cursor.col, 4);
    terminal.feed("\x1b[2J");
    assert_eq!(terminal.cursor(), cursor);
    terminal.feed("X");
    assert_eq!(terminal.cursor().position(), Some((1, 1)));
    assert_eq!(terminal.screen().line(0).unwrap().text(), "    ");
    assert_eq!(terminal.screen().line(1).unwrap().text(), "X   ");
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
fn ed2_preserved_wrapped_text_and_anchors_reflow_with_history() {
    for limit in [Some(0), Some(1), Some(10), None] {
        let mut terminal = Terminal::with_scrollback(4, 3, limit).unwrap();
        let mut transcript = Transcript::new();
        let mut lines = terminal.feed_with("abcd界ef", &mut transcript);

        let anchor = terminal
            .anchor_at(crate::Position {
                screen: crate::ScreenKind::Primary,
                col: 0,
                row: 1,
            })
            .unwrap();

        lines.extend(terminal.feed_with("\x1b[H\x1b[2Jnew", &mut transcript));
        lines.extend(terminal.resize_with(8, 3, &mut transcript));

        match limit {
            Some(0) => assert!(terminal.resolve(&anchor).is_none()),

            _ => {
                let position = terminal.resolve(&anchor).unwrap();
                assert_eq!(position.row, -1);
                assert_eq!(position.col, if limit == Some(1) { 0 } else { 4 });
            }
        }

        lines.extend(transcript.finish(&terminal));
        assert_eq!(lines, ["abcd界ef", "new"]);
        terminal.engine.verify();
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
fn alternate_screen_ed2_erases_without_archiving() {
    let mut terminal = Terminal::with_scrollback(8, 2, None).unwrap();
    let mut transcript = Transcript::new();
    terminal.feed_with("history\r\none\r\ntwo\x1b[?1049halt", &mut transcript);
    let cursor = terminal.cursor();
    let anchor = terminal.anchor_at_cursor();
    assert!(terminal.feed_with("\x1b[2J", &mut transcript).is_empty());
    assert_eq!(terminal.cursor(), cursor);
    assert!(terminal.resolve(&anchor).is_none());

    assert!(
        terminal
            .screen()
            .visible_lines()
            .all(|line| line.is_blank())
    );

    assert_eq!(terminal.screen().all_lines().count(), 2);
    assert_eq!(terminal.primary_screen().text(), ["history", "one", "two"]);
    terminal.feed_with("\x1b[?1049l", &mut transcript);
    assert_eq!(transcript.finish(&terminal), ["history", "one", "two"]);
}

#[test]
fn dump_restores_visible_state_and_parser_without_history() {
    let mut original = Terminal::with_scrollback(8, 2, Some(2)).unwrap();
    original.feed("one\r\ntwo\r\nthree\r\nfour\r\nfive\x1b[31");
    let mut restored = Terminal::with_scrollback(8, 2, Some(2)).unwrap();
    restored.feed(original.dump());
    // Dump initializes visible buffers; retained history is not serialized.
    assert_eq!(original.primary_screen().all_lines().count(), 4);
    assert_eq!(restored.primary_screen().all_lines().count(), 2);

    for tail in ["", "m!", "\r\nsix", "\x1b[?1049halt\x1b[?1049l"] {
        original.feed(tail);
        restored.feed(tail);
        assert_eq!(original.dump(), restored.dump());

        assert!(
            original
                .primary_screen()
                .visible_lines()
                .eq(restored.primary_screen().visible_lines())
        );
    }
}

#[test]
fn terminal_and_owned_types_are_send_sync() {
    fn send_sync<T: Send + Sync>() {}

    send_sync::<Terminal>();
    send_sync::<std::sync::RwLock<Terminal>>();
    send_sync::<crate::Line>();
    send_sync::<crate::Anchor>();
    send_sync::<crate::Parser>();
    send_sync::<ShellTracker>();
    send_sync::<Transcript>();
}

#[test]
fn all_operation_forms_share_one_execution_lifecycle() {
    fn check<I, Kind>(actions: I, text: &str)
    where
        I: crate::IntoOperations<Kind>,
    {
        let mut expected = Terminal::new(16, 3).unwrap();
        let mut actual = Terminal::new(16, 3).unwrap();
        let reference = expected.feed_with(text, Calls::default());
        let report = actual.execute_with(actions, Calls::default());
        assert_eq!(report, reference);
        actual.engine.assert_eq(&expected.engine);
    }

    check(Function::Print('x'), "x");
    check(Operation::Function(Function::Print('x')), "x");
    let functions = [Function::Print('x'), Function::Cr, Function::Lf];
    check(functions.clone(), "x\r\n");
    check(functions.to_vec(), "x\r\n");
    check(functions.iter().cloned(), "x\r\n");

    check(
        functions
            .into_iter()
            .map(Operation::from)
            .collect::<Vec<_>>(),
        "x\r\n",
    );

    check(Vec::<Function>::new(), "");
    check(std::iter::empty::<Operation>(), "");

    let mut parser = crate::Parser::new();

    check(
        "\x1b[31mhello".chars().filter_map(|ch| parser.feed(ch)),
        "\x1b[31mhello",
    );
}

#[test]
fn explicit_execution_preserves_unfinished_text_parsing() {
    let mut terminal = Terminal::new(8, 2).unwrap();
    terminal.feed("\x1b[31");
    terminal.execute(Function::Print('x'));
    terminal.feed("mY");
    let cells = terminal.screen().line(0).unwrap().cells();
    assert_eq!(cells[0].char(), 'x');
    assert_eq!(cells[0].pen().foreground(), None);
    assert_eq!(cells[1].char(), 'Y');
    assert_eq!(cells[1].pen().foreground(), Some(crate::Color::Indexed(1)));
}

#[test]
fn execute_finishes_eviction_before_advancing_iterator() {
    // A local-only observer also verifies that observers need no Send bound.
    struct Evictions(std::rc::Rc<std::cell::Cell<usize>>);

    impl Observer for Evictions {
        type Output = usize;

        fn evicted(&mut self, _: &mut Observation<'_>, _: &crate::Line) {
            self.0.set(self.0.get() + 1);
        }

        fn end(&mut self, _: &mut Observation<'_>) -> Self::Output {
            self.0.get()
        }
    }

    let count = std::rc::Rc::new(std::cell::Cell::new(0));

    let actions = (0..3).map(|index| {
        assert_eq!(count.get(), index);

        Function::Lf
    });

    let mut terminal = Terminal::new(4, 1).unwrap();
    assert_eq!(terminal.execute_with(actions, Evictions(count.clone())), 3);
}

#[test]
fn execution_hooks_borrow_original_sgr_parameters() {
    use crate::parser::SgrOp;

    struct Parameters {
        address: *const SgrOp,
        seen: usize,
    }

    impl Observer for Parameters {
        type Output = usize;

        fn before(&mut self, _: &mut Observation<'_>, operation: &Operation) {
            let Operation::Function(Function::Sgr(ops)) = operation else {
                panic!("expected SGR")
            };

            assert_eq!(ops.as_slice().as_ptr(), self.address);
            self.seen += 1;
        }

        fn after(&mut self, context: &mut Observation<'_>, operation: &Operation) {
            self.before(context, operation);
        }

        fn end(&mut self, _: &mut Observation<'_>) -> Self::Output {
            self.seen
        }
    }

    let ops = crate::parser::SgrOps::from(vec![SgrOp::SetBoldIntensity; 8]);

    let observer = Parameters {
        address: ops.as_slice().as_ptr(),
        seen: 0,
    };

    let mut terminal = Terminal::new(4, 2).unwrap();
    assert_eq!(terminal.execute_with([Function::Sgr(ops)], observer), 2);
}

proptest! {
    #[test]
    fn dirty_observer_is_batching_independent(
        first in input(),
        second in input(),
        cols in 2..20usize,
        rows in 2..8usize
    ) {
        let mut terminal = Terminal::new(12, 5).unwrap();
        let mut dirty = DirtyRows::new();
        let mut split = Terminal::new(12, 5).unwrap();
        let mut split_dirty = DirtyRows::new();

        prop_assert_eq!(
            terminal.feed_with("", &mut dirty),
            split.feed_with("", &mut split_dirty)
        );

        for text in [first, second] {
            // Compare a batched report with the union of per-character reports.
            let mut expected = std::collections::BTreeSet::new();

            for ch in text.chars() {
                expected.extend(split.feed_with(ch, &mut split_dirty));
            }

            prop_assert_eq!(
                terminal.feed_with(text, &mut dirty),
                expected.into_iter().collect::<Vec<_>>()
            );

            prop_assert_eq!(
                terminal.resize_with(cols, rows, &mut dirty),
                split.resize_with(cols, rows, &mut split_dirty)
            );
        }
    }
}

#[test]
fn cursor_only_changes_emit_no_dirty_notifications() {
    let mut terminal = Terminal::new(8, 3).unwrap();
    let report = terminal.feed_with("\x1b[2;2H\x1b[?25l", Calls::default());
    assert!(report.notifications.is_empty());
}

#[test]
fn resize_notifications_precede_dirty_ranges() {
    #[derive(Default)]
    struct Geometry {
        rows: usize,
        sizes: Vec<(usize, usize)>,
    }

    impl Observer for Geometry {
        type Output = Vec<(usize, usize)>;

        fn begin(&mut self, context: &mut Observation<'_>) {
            self.rows = context.terminal().size().1;
        }

        fn resized(&mut self, cols: usize, rows: usize) {
            self.rows = rows;
            self.sizes.push((cols, rows));
        }

        fn dirty(&mut self, rows: std::ops::Range<usize>) {
            assert!(rows.end <= self.rows);
        }

        fn end(&mut self, _: &mut Observation<'_>) -> Self::Output {
            std::mem::take(&mut self.sizes)
        }
    }

    let mut terminal = Terminal::new(8, 6).unwrap();

    let sizes = terminal.execute_with(
        [
            Function::Cup(6, 1).into(),
            Function::Print('x').into(),
            Operation::Resize { cols: 8, rows: 2 },
            Function::Print('y').into(),
            Operation::Resize { cols: 8, rows: 2 },
        ],
        Geometry::default(),
    );

    assert_eq!(sizes, [(8, 2)]);

    let sizes = terminal.execute_with(
        [
            Operation::Resize { cols: 8, rows: 5 },
            Operation::Resize { cols: 4, rows: 5 },
            Function::Cup(5, 1).into(),
            Function::Print('z').into(),
        ],
        Geometry::default(),
    );

    assert_eq!(sizes, [(8, 5), (4, 5)]);
}
