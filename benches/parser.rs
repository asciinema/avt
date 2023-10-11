use avt::parser::{Executor, Parser};
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use std::fs;

pub fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("mixed", |b| {
        b.iter_batched(setup("mixed.txt"), run, BatchSize::SmallInput)
    });

    c.bench_function("cacademo", |b| {
        b.iter_batched(setup("cacademo.txt"), run, BatchSize::SmallInput)
    });

    c.bench_function("licenses", |b| {
        b.iter_batched(setup("licenses.txt"), run, BatchSize::SmallInput)
    });

    c.bench_function("licenses-lolcat", |b| {
        b.iter_batched(setup("licenses-lolcat.txt"), run, BatchSize::SmallInput)
    });
}

struct NoopExecutor();

impl Executor for NoopExecutor {}

fn setup(filename: &str) -> impl Fn() -> (Parser, String, NoopExecutor) {
    let filename = filename.to_owned();

    move || {
        let parser = Parser::default();
        let text = sample_text(&filename);
        let executor = NoopExecutor {};

        (parser, text, executor)
    }
}

fn run<E: Executor>((mut parser, text, mut executor): (Parser, String, E)) -> (Parser, String, E) {
    parser.feed_str(&text, &mut executor);

    (parser, text, executor)
}

fn sample_text(filename: &str) -> String {
    fs::read_to_string(format!("benches/data/{filename}")).unwrap()
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
