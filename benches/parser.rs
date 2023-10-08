use avt::parser::{Executor, Parser};
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use std::fs;

struct NoopExecutor();

impl Executor for NoopExecutor {}

fn setup() -> (Parser, String, NoopExecutor) {
    let parser = Parser::default();
    let text = sample_text();
    let executor = NoopExecutor {};

    (parser, text, executor)
}

fn run<E: Executor>((mut parser, text, mut executor): (Parser, String, E)) -> (Parser, String, E) {
    parser.feed_str(&text, &mut executor);

    (parser, text, executor)
}

fn sample_text() -> String {
    fs::read_to_string("benches/sample.txt").unwrap()
}

pub fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("feed_str", |b| {
        b.iter_batched(setup, run, BatchSize::SmallInput)
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
