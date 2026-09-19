use avt::Terminal;
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use std::{fs, hint::black_box, iter};

pub fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("terminal: feed mixed in bulk", |b| {
        b.iter_batched(setup_feed("mixed.txt"), run_feed, BatchSize::SmallInput)
    });

    c.bench_function("terminal: feed mixed in chunks", |b| {
        b.iter_batched(
            chunk(setup_feed("mixed.txt")),
            run_feed,
            BatchSize::SmallInput,
        )
    });

    c.bench_function("terminal: feed cacademo in bulk", |b| {
        b.iter_batched(setup_feed("cacademo.txt"), run_feed, BatchSize::SmallInput)
    });

    c.bench_function("terminal: feed cacademo in chunks", |b| {
        b.iter_batched(
            chunk(setup_feed("cacademo.txt")),
            run_feed,
            BatchSize::SmallInput,
        )
    });

    c.bench_function("terminal: feed licenses in bulk", |b| {
        b.iter_batched(setup_feed("licenses.txt"), run_feed, BatchSize::SmallInput)
    });

    c.bench_function("terminal: feed licenses in chunks", |b| {
        b.iter_batched(
            chunk(setup_feed("licenses.txt")),
            run_feed,
            BatchSize::SmallInput,
        )
    });

    c.bench_function("terminal: feed licenses-lolcat in bulk", |b| {
        b.iter_batched(
            setup_feed("licenses-lolcat.txt"),
            run_feed,
            BatchSize::SmallInput,
        )
    });

    c.bench_function("terminal: feed licenses-lolcat in chunks", |b| {
        b.iter_batched(
            chunk(setup_feed("licenses-lolcat.txt")),
            run_feed,
            BatchSize::SmallInput,
        )
    });

    c.bench_function("terminal: feed newlines - scrollback limit = 0", |b| {
        b.iter_batched(gen_newlines(0), run_feed, BatchSize::SmallInput)
    });

    c.bench_function("terminal: feed newlines - scrollback limit = 100", |b| {
        b.iter_batched(gen_newlines(100), run_feed, BatchSize::SmallInput)
    });

    c.bench_function("terminal: feed newlines - scrollback limit = 1000", |b| {
        b.iter_batched(gen_newlines(1000), run_feed, BatchSize::SmallInput)
    });

    c.bench_function("terminal: dump", |b| {
        b.iter_batched(setup_dump, run_dump, BatchSize::SmallInput)
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
}

fn setup_feed(filename: &str) -> impl Fn() -> (Terminal, Vec<String>) + use<> {
    let filename = filename.to_owned();

    move || {
        let vt = Terminal::with_scrollback(100, 24, Some(1000)).unwrap();
        let text = sample_text(&filename);

        (vt, vec![text])
    }
}

fn run_feed((mut vt, chunks): (Terminal, Vec<String>)) -> (Terminal, Vec<String>) {
    for text in &chunks {
        vt.feed(text);
    }

    (vt, chunks)
}

fn chunk(f: impl Fn() -> (Terminal, Vec<String>)) -> impl Fn() -> (Terminal, Vec<String>) {
    move || {
        let (vt, text) = f();

        let chunks: Vec<String> = text
            .first()
            .unwrap()
            .chars()
            .collect::<Vec<char>>()
            .chunks(32)
            .map(String::from_iter)
            .collect();

        (vt, chunks)
    }
}

fn gen_newlines(scrollback_limit: usize) -> impl Fn() -> (Terminal, Vec<String>) {
    move || {
        let vt = Terminal::with_scrollback(100, 24, Some(scrollback_limit)).unwrap();

        let chunks = iter::repeat_n(".\n".to_owned(), 100_000).collect::<Vec<_>>();

        (vt, chunks)
    }
}

fn sample_text(filename: &str) -> String {
    fs::read_to_string(format!("benches/data/{filename}")).unwrap()
}

fn setup_dump() -> Terminal {
    let mut vt = Terminal::new(100, 40).unwrap();
    let text = &sample_text("mixed.txt")[0..128 * 1024];
    vt.feed(text);

    vt
}

fn run_dump(vt: Terminal) -> Terminal {
    black_box(vt.dump());

    vt
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
