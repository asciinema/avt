use avt::Vt;
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use std::fs;

fn setup_bulk() -> (Vt, String) {
    let vt = Vt::with_scrollback_limit(100, 24, Some(1000));
    let text = sample_text();

    (vt, text)
}

fn run_bulk((mut vt, text): (Vt, String)) -> (Vt, String) {
    vt.feed_str(&text);

    (vt, text)
}

fn setup_chunks() -> (Vt, Vec<String>) {
    let vt = Vt::with_scrollback_limit(100, 24, Some(1000));
    let text = sample_text();

    let chunks: Vec<String> = text
        .chars()
        .collect::<Vec<char>>()
        .chunks(32)
        .map(String::from_iter)
        .collect();

    (vt, chunks)
}

fn run_chunks((mut vt, chunks): (Vt, Vec<String>)) -> (Vt, Vec<String>) {
    for text in &chunks {
        vt.feed_str(text);
    }

    (vt, chunks)
}

fn sample_text() -> String {
    fs::read_to_string("benches/sample.txt").unwrap()
}

pub fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("feed in bulk", |b| {
        b.iter_batched(setup_bulk, run_bulk, BatchSize::SmallInput)
    });

    c.bench_function("feed in chunks", |b| {
        b.iter_batched(setup_chunks, run_chunks, BatchSize::SmallInput)
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
