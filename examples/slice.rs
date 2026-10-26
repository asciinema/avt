//! Keep only the operations needed to reproduce selected screen cells and the
//! cursor, using backward slicing through the observation and execution APIs.
//!
//! This example starts from initial_terminal(): fixed size, default modes and
//! charset, full-screen margins, and ASCII cells. Only the foreground can change.
//! Supported actions are in-bounds CUP, foreground-only SGR, ASCII printing away
//! from the last column, and SU 1. Other actions are rejected before execution.
use std::collections::BTreeSet;
use std::ops::Range;

use avt::parser::{Function, SgrOp};
use avt::{Cursor, Observation, Observer, Operation, ScreenKind, Terminal};

const COLS: usize = 8;
const ROWS: usize = 3;
const INPUT: [&str; 4] = [
    "\x1b[34mtrash",       // Discarded by the later scroll, including its blue pen.
    "\x1b[31m\x1b[2;1HxA", // x advances the cursor to place A.
    "\x1b[2;1HB",          // B overwrites x, but A still depends on printing x.
    "\x1b[1S\x1b[2;2H",    // CUP writes the cursor even though its value is unchanged.
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Slot {
    Cursor,
    Foreground,
    Cell { row: usize, col: usize },
}

fn cells(rows: Range<usize>) -> impl Iterator<Item = Slot> {
    rows.flat_map(|row| (0..COLS).map(move |col| Slot::Cell { row, col }))
}

struct Step {
    operation: Operation,
    reads: Vec<Slot>,
    writes: Vec<Slot>,
}

#[derive(Default)]
struct Trace {
    steps: Vec<Step>,
}

impl Observer for Trace {
    type Output = ();

    fn before(&mut self, context: &mut Observation<'_>, operation: &Operation) {
        let terminal = context.terminal();
        assert_eq!(terminal.size(), (COLS, ROWS));
        assert_eq!(terminal.active_screen_kind(), ScreenKind::Primary);

        // Semantic reads/writes, including writes whose values happen to match.
        // A screen diff would miss dependencies such as the final no-op CUP.
        let (reads, writes) = match operation {
            Operation::Function(Function::Cup(row, col))
                if (1..=ROWS).contains(&usize::from(*row))
                    && (1..=COLS).contains(&usize::from(*col)) =>
            {
                (vec![], vec![Slot::Cursor])
            }

            Operation::Function(Function::Sgr(ops))
                if matches!(ops.as_slice(), [SgrOp::SetForegroundColor(_)]) =>
            {
                (vec![], vec![Slot::Foreground])
            }

            Operation::Function(Function::Print(ch)) if (' '..='~').contains(ch) => {
                let Cursor { row, col, .. } = terminal.cursor();

                assert!(
                    row < ROWS && col < COLS - 1,
                    "printing must stay clear of wrapping"
                );

                (
                    vec![Slot::Cursor, Slot::Foreground],
                    vec![Slot::Cell { row, col }, Slot::Cursor],
                )
            }

            Operation::Function(Function::Su(1)) => (
                // The top row is discarded; other rows move up. The new bottom
                // row uses the current foreground. Reading all moved cells is
                // conservative when only part of the final screen is selected.
                cells(1..ROWS).chain([Slot::Foreground]).collect(),
                cells(0..ROWS).collect(),
            ),

            _ => panic!("unsupported action in slice example: {operation:?}"),
        };

        self.steps.push(Step {
            operation: operation.clone(),
            reads,
            writes,
        });
    }

    // The trace spans calls; only the consumer decides when to slice it.
    fn end(&mut self, _: &mut Observation<'_>) {}
}

fn backward_slice(steps: &[Step], mut needed: BTreeSet<Slot>) -> Vec<usize> {
    let mut kept = Vec::new();

    for (index, step) in steps.iter().enumerate().rev() {
        if step.writes.iter().any(|slot| needed.contains(slot)) {
            kept.push(index);

            for slot in &step.writes {
                needed.remove(slot);
            }

            needed.extend(&step.reads);
        }
    }

    // Remaining needs are supplied by the identical initial state on replay.
    kept.reverse();

    kept
}

fn initial_terminal() -> Terminal {
    let mut terminal = Terminal::new(COLS, ROWS).unwrap();
    terminal.feed("old\x1b[3;1Hseed\x1b[1;1H");

    terminal
}

fn replay(steps: &[Step], kept: &[usize]) -> Terminal {
    let mut terminal = initial_terminal();

    for &index in kept {
        terminal.execute(steps[index].operation.clone());
    }

    terminal
}

fn main() {
    let mut original = initial_terminal();
    let mut trace = Trace::default();
    original.feed_with(INPUT, &mut trace);

    let selected = cells(0..ROWS).chain([Slot::Cursor]).collect();
    let kept = backward_slice(&trace.steps, selected);
    let reduced = replay(&trace.steps, &kept);

    println!(
        "styled screen + cursor: kept {} of {} operations",
        kept.len(),
        trace.steps.len()
    );

    println!(
        "screen: {:?}, cursor: {:?}",
        reduced.screen().text(),
        reduced.cursor().position()
    );

    for &index in &kept {
        println!("{:?}", trace.steps[index].operation);
    }

    println!("overwritten 'x' retained for its cursor advance");
}
