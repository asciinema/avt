use avt::parser::Parser;
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use std::fs;

pub fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("parser: feed mixed", |b| {
        b.iter_batched(setup("mixed.txt"), run, BatchSize::SmallInput)
    });

    c.bench_function("parser: feed cacademo", |b| {
        b.iter_batched(setup("cacademo.txt"), run, BatchSize::SmallInput)
    });

    c.bench_function("parser: feed licenses", |b| {
        b.iter_batched(setup("licenses.txt"), run, BatchSize::SmallInput)
    });

    c.bench_function("parser: feed licenses-lolcat", |b| {
        b.iter_batched(setup("licenses-lolcat.txt"), run, BatchSize::SmallInput)
    });
}

fn setup(filename: &str) -> impl Fn() -> (Parser, String) {
    let filename = filename.to_owned();

    move || {
        let parser = Parser::default();
        let text = sample_text(&filename);

        (parser, text)
    }
}

fn run((mut parser, text): (Parser, String)) -> (Parser, String) {
    text.chars().for_each(|ch| {
        parser.feed(ch);
    });

    (parser, text)
}

fn sample_text(filename: &str) -> String {
    fs::read_to_string(format!("benches/data/{filename}")).unwrap()
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
