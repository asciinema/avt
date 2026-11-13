use avt::Charset;
use criterion::{Criterion, criterion_group, criterion_main};
use std::{fs, hint::black_box};

pub fn criterion_benchmark(c: &mut Criterion) {
    let text = fs::read_to_string("benches/data/licenses.txt").unwrap();

    c.bench_function("charset: ascii", |b| {
        b.iter(|| run(black_box(Charset::Ascii), black_box(&text)))
    });

    c.bench_function("charset: drawing", |b| {
        b.iter(|| run(black_box(Charset::Drawing), black_box(&text)))
    });
}

fn run(charset: Charset, text: &str) {
    for ch in text.chars() {
        black_box(charset.translate(ch));
    }
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
