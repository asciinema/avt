use avt::Charset;
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use std::fs;

pub fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("charset: ascii", |b| {
        b.iter_batched(setup(Charset::Ascii), run, BatchSize::SmallInput)
    });

    c.bench_function("charset: drawing", |b| {
        b.iter_batched(setup(Charset::Drawing), run, BatchSize::SmallInput)
    });
}

fn setup(charset: Charset) -> impl Fn() -> (Charset, String) {
    move || {
        let text = fs::read_to_string("benches/data/licenses.txt").unwrap();

        (charset, text)
    }
}

fn run((charset, text): (Charset, String)) -> (Charset, String) {
    for ch in text.chars() {
        charset.translate(ch);
    }

    (charset, text)
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
