mod terminal;

use crate::parser::Function;
use crate::{DirtyRows, Input, IntoInputs, Observation, Observer, Operation, Terminal, Transcript};
use proptest::prelude::*;

const PROMPT: &str = "\x1b]133;A\x07$ \x1b]133;B\x07";
const SUBMIT: &str = "\x1b]133;C\x07";

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
fn unfinished_evicted_line_survives_feed_boundaries() {
    let mut terminal = Terminal::new(4, 1).unwrap();
    let mut transcript = Transcript::new();
    assert!(terminal.feed_with("abcdE", &mut transcript).is_empty());
    assert!(terminal.feed_with("", &mut transcript).is_empty());
    assert_eq!(terminal.feed_with("F\r\n", &mut transcript), ["abcdEF"]);
    assert!(transcript.finish(&terminal).is_empty());
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
    send_sync::<crate::Parser>();
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
    terminal.execute([Function::Print('x')]);
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
