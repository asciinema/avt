use avt::Vt;
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use std::fs;

pub fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("vt: feed mixed in bulk", |b| {
        b.iter_batched(setup_feed("mixed.txt"), run_feed, BatchSize::SmallInput)
    });

    c.bench_function("vt: feed mixed in chunks", |b| {
        b.iter_batched(
            chunk(setup_feed("mixed.txt")),
            run_feed,
            BatchSize::SmallInput,
        )
    });

    c.bench_function("vt: feed cacademo in bulk", |b| {
        b.iter_batched(setup_feed("cacademo.txt"), run_feed, BatchSize::SmallInput)
    });

    c.bench_function("vt: feed cacademo in chunks", |b| {
        b.iter_batched(
            chunk(setup_feed("cacademo.txt")),
            run_feed,
            BatchSize::SmallInput,
        )
    });

    c.bench_function("vt: feed licenses in bulk", |b| {
        b.iter_batched(setup_feed("licenses.txt"), run_feed, BatchSize::SmallInput)
    });

    c.bench_function("vt: feed licenses in chunks", |b| {
        b.iter_batched(
            chunk(setup_feed("licenses.txt")),
            run_feed,
            BatchSize::SmallInput,
        )
    });

    c.bench_function("vt: feed licenses-lolcat in bulk", |b| {
        b.iter_batched(
            setup_feed("licenses-lolcat.txt"),
            run_feed,
            BatchSize::SmallInput,
        )
    });

    c.bench_function("vt: feed licenses-lolcat in chunks", |b| {
        b.iter_batched(
            chunk(setup_feed("licenses-lolcat.txt")),
            run_feed,
            BatchSize::SmallInput,
        )
    });

    c.bench_function("vt: dump", |b| {
        b.iter_batched(setup_dump, run_dump, BatchSize::SmallInput)
    });
}

fn setup_feed(filename: &str) -> impl Fn() -> (Vt, Vec<String>) {
    let filename = filename.to_owned();

    move || {
        let vt = Vt::builder().size(100, 24).scrollback_limit(1000).build();
        let text = sample_text(&filename);

        (vt, vec![text])
    }
}

fn run_feed((mut vt, chunks): (Vt, Vec<String>)) -> (Vt, Vec<String>) {
    for text in &chunks {
        vt.feed_str(text);
    }

    (vt, chunks)
}

fn chunk(f: impl Fn() -> (Vt, Vec<String>)) -> impl Fn() -> (Vt, Vec<String>) {
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

fn sample_text(filename: &str) -> String {
    fs::read_to_string(format!("benches/data/{filename}")).unwrap()
}

fn setup_dump() -> Vt {
    let mut vt = Vt::new(100, 40);
    let text = &sample_text("mixed.txt")[0..128 * 1024];
    vt.feed_str(text);

    vt
}

fn run_dump(vt: Vt) -> Vt {
    vt.dump();

    vt
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
