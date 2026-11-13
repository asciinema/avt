use avt::Terminal;
use avt::observer::{DirtyRows, ShellTracker, Transcript};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use std::{fs, hint::black_box, iter};

pub fn criterion_benchmark(c: &mut Criterion) {
    let mixed = sample_text("mixed.txt");
    let cacademo = sample_text("cacademo.txt");
    let licenses = sample_text("licenses.txt");
    let licenses_lolcat = sample_text("licenses-lolcat.txt");
    let mixed_chunks = chunk(&mixed);
    let cacademo_chunks = chunk(&cacademo);
    let licenses_chunks = chunk(&licenses);
    let licenses_lolcat_chunks = chunk(&licenses_lolcat);

    c.bench_function("terminal: feed mixed in bulk", |b| {
        b.iter_batched(setup_feed(&[&mixed]), run_feed, BatchSize::SmallInput)
    });

    c.bench_function("terminal: feed mixed in chunks", |b| {
        b.iter_batched(setup_feed(&mixed_chunks), run_feed, BatchSize::SmallInput)
    });

    c.bench_function("terminal: feed cacademo in bulk", |b| {
        b.iter_batched(setup_feed(&[&cacademo]), run_feed, BatchSize::SmallInput)
    });

    c.bench_function("terminal: feed cacademo in chunks", |b| {
        b.iter_batched(
            setup_feed(&cacademo_chunks),
            run_feed,
            BatchSize::SmallInput,
        )
    });

    c.bench_function("terminal: feed licenses in bulk", |b| {
        b.iter_batched(setup_feed(&[&licenses]), run_feed, BatchSize::SmallInput)
    });

    c.bench_function("terminal: feed licenses in chunks", |b| {
        b.iter_batched(
            setup_feed(&licenses_chunks),
            run_feed,
            BatchSize::SmallInput,
        )
    });

    c.bench_function("terminal: feed licenses-lolcat in bulk", |b| {
        b.iter_batched(
            setup_feed(&[&licenses_lolcat]),
            run_feed,
            BatchSize::SmallInput,
        )
    });

    c.bench_function("terminal: feed licenses-lolcat in chunks", |b| {
        b.iter_batched(
            setup_feed(&licenses_lolcat_chunks),
            run_feed,
            BatchSize::SmallInput,
        )
    });

    let newlines = vec![".\n"; 100_000];

    for limit in [0, 100, 1000] {
        c.bench_function(
            &format!("terminal: feed newlines - scrollback limit = {limit}"),
            |b| {
                b.iter_batched(
                    || {
                        (
                            Terminal::with_scrollback(100, 24, Some(limit)).unwrap(),
                            newlines.as_slice(),
                        )
                    },
                    run_feed,
                    BatchSize::SmallInput,
                )
            },
        );
    }

    for (name, text) in [
        ("plain", "abcdefghijklmnopqrstuvwxyz012345".repeat(2048)),
        ("unicode", "hé界🦀".repeat(4096)),
        ("CJK with spaces", "界 ".repeat(8192)),
        (
            "ASCII over wide cells",
            format!("{}\r{}\r\n", "界".repeat(50), "x".repeat(100)).repeat(256),
        ),
    ] {
        let chunks = [text.as_str()];

        c.bench_function(&format!("terminal: feed {name}"), |b| {
            b.iter_batched(
                || (Terminal::new(100, 24).unwrap(), chunks.as_slice()),
                run_feed,
                BatchSize::SmallInput,
            )
        });
    }

    let newlines = "line\r\n".repeat(10_000);
    let chunks = [newlines.as_str()];

    c.bench_function("terminal: feed newlines in one batch", |b| {
        b.iter_batched(
            || (Terminal::new(100, 24).unwrap(), chunks.as_slice()),
            run_feed,
            BatchSize::SmallInput,
        )
    });

    c.bench_function("terminal: feed cacademo with dirty rows", |b| {
        b.iter_batched(
            setup_feed(&[&cacademo]),
            |(mut terminal, text)| {
                let rows = terminal.feed_with(text[0], DirtyRows::new());

                (terminal, rows)
            },
            BatchSize::SmallInput,
        )
    });

    let session: String = (0..256)
        .map(|n| {
            format!(
                "\x1b]133;A\x07$ \x1b]133;B\x07echo item-{n:04}\x1b]133;C\x07\r\nitem-{n:04}\r\n\x1b]133;D;0\x07"
            )
        })
        .collect();

    c.bench_function("terminal: feed shell session with observers", |b| {
        b.iter_batched(
            || {
                (
                    Terminal::new(80, 24).unwrap(),
                    DirtyRows::new(),
                    Transcript::new(),
                    ShellTracker::new(),
                    session.as_str(),
                )
            },
            |(mut terminal, mut dirty, mut transcript, mut shell, text)| {
                let reports = terminal.feed_with(text, (&mut dirty, (&mut transcript, &mut shell)));

                let tail = transcript.finish(&terminal);

                (terminal, dirty, shell, reports, tail)
            },
            BatchSize::SmallInput,
        )
    });

    for prompts in [0, 10_000] {
        c.bench_function(
            &format!("terminal: feed with {prompts} retained prompts"),
            |b| {
                let mut terminal = Terminal::with_scrollback(80, 24, None).unwrap();
                let mut shell = ShellTracker::new();

                terminal.feed_with(
                    iter::repeat_n("\x1b]133;A\x07$ \x1b]133;B\x07x\x1b]133;C\x07\r\n", prompts),
                    &mut shell,
                );

                b.iter(|| {
                    let events = terminal.feed_with(black_box("\rX"), &mut shell);
                    black_box((&terminal, &shell));

                    events
                });
            },
        );
    }

    for chars in [10_000, 160_000] {
        let text = "x".repeat(chars);

        c.bench_function(
            &format!("terminal: feed long shell input {chars} chars"),
            |b| {
                b.iter_batched(
                    || {
                        let mut terminal = Terminal::with_scrollback(80, 24, None).unwrap();
                        let mut shell = ShellTracker::new();
                        terminal.feed_with("\x1b]133;A\x07$ \x1b]133;B\x07", &mut shell);

                        (terminal, shell)
                    },
                    |(mut terminal, mut shell)| {
                        terminal.feed_with(text.chars(), &mut shell);

                        (terminal, shell)
                    },
                    BatchSize::PerIteration,
                )
            },
        );
    }

    c.bench_function("terminal: dump", |b| {
        let mut terminal = Terminal::new(100, 40).unwrap();
        terminal.feed(&mixed[..128 * 1024]);

        b.iter(|| {
            black_box(terminal.dump());
        });
    });

    let mut terminal = Terminal::with_scrollback(80, 24, None).unwrap();
    terminal.feed(iter::repeat_n("line\r\n", 10_000));

    c.bench_function("terminal: resize height with scrollback", |b| {
        b.iter(|| {
            terminal.resize(80, 25);
            terminal.resize(80, 24);
            black_box(&terminal);
        });
    });

    terminal.feed("\x1b[?1049h");

    c.bench_function(
        "terminal: resize height with hidden primary scrollback",
        |b| {
            b.iter(|| {
                terminal.resize(80, 25);
                terminal.resize(80, 24);
                black_box(&terminal);
            });
        },
    );

    c.bench_function(
        "terminal: resize width with hidden primary scrollback",
        |b| {
            b.iter(|| {
                terminal.resize(79, 24);
                terminal.resize(80, 24);
                black_box(&terminal);
            });
        },
    );
}

fn setup_feed<'a>(chunks: &'a [&'a str]) -> impl Fn() -> (Terminal, &'a [&'a str]) {
    move || {
        let vt = Terminal::with_scrollback(100, 24, Some(1000)).unwrap();

        (vt, chunks)
    }
}

fn run_feed((mut vt, chunks): (Terminal, &[&str])) -> Terminal {
    for text in chunks {
        vt.feed(text);
    }

    vt
}

fn chunk(text: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0;

    for (end, _) in text.char_indices().skip(32).step_by(32) {
        chunks.push(&text[start..end]);
        start = end;
    }

    if start < text.len() {
        chunks.push(&text[start..]);
    }

    chunks
}

fn sample_text(filename: &str) -> String {
    fs::read_to_string(format!("benches/data/{filename}")).unwrap()
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
