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

fn sample_text() -> String {
    fs::read_to_string("benches/sample.txt").unwrap()
}

pub fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("feed", |b| {
        b.iter_batched(setup_bulk, run_bulk, BatchSize::SmallInput)
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
