use avt::ansi::Parser;
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use std::{fs, hint::black_box};

pub fn criterion_benchmark(c: &mut Criterion) {
    let mixed = sample_text("mixed.txt");
    let cacademo = sample_text("cacademo.txt");
    let licenses = sample_text("licenses.txt");
    let licenses_lolcat = sample_text("licenses-lolcat.txt");

    c.bench_function("parser: feed mixed", |b| {
        b.iter_batched(setup(&mixed), run, BatchSize::SmallInput)
    });

    c.bench_function("parser: feed cacademo", |b| {
        b.iter_batched(setup(&cacademo), run, BatchSize::SmallInput)
    });

    c.bench_function("parser: feed licenses", |b| {
        b.iter_batched(setup(&licenses), run, BatchSize::SmallInput)
    });

    c.bench_function("parser: feed licenses-lolcat", |b| {
        b.iter_batched(setup(&licenses_lolcat), run, BatchSize::SmallInput)
    });
}

fn setup<'a>(text: &'a str) -> impl Fn() -> (Parser, &'a str) {
    move || (Parser::default(), text)
}

fn run((mut parser, text): (Parser, &str)) -> Parser {
    for function in parser.feed(text) {
        black_box(function);
    }

    parser
}

fn sample_text(filename: &str) -> String {
    fs::read_to_string(format!("benches/data/{filename}")).unwrap()
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
