use avt::Charset;
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use std::{fs, hint::black_box};

pub fn criterion_benchmark(c: &mut Criterion) {
    let text = fs::read_to_string("benches/data/licenses.txt").unwrap();

    c.bench_function("charset: ascii", |b| {
        b.iter_batched(setup(Charset::Ascii, &text), run, BatchSize::SmallInput)
    });

    c.bench_function("charset: drawing", |b| {
        b.iter_batched(setup(Charset::Drawing, &text), run, BatchSize::SmallInput)
    });
}

fn setup(charset: Charset, text: &str) -> impl Fn() -> (Charset, String) {
    move || (charset, text.to_owned())
}

fn run((charset, text): (Charset, String)) -> (Charset, String) {
    for ch in text.chars() {
        black_box(charset.translate(ch));
    }

    (charset, text)
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
