use avt::Vt;
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use std::fs;

fn go(mut vt: Vt, t: &str) {
    for _n in 0..10 {
        vt.feed_str(t);
    }
}

fn setup() -> Vt {
    Vt::with_scrollback_limit(100, 24, Some(1000))
}

pub fn criterion_benchmark(c: &mut Criterion) {
    let t = fs::read_to_string("benches/sample.txt").unwrap();

    c.bench_function("feed", |b| {
        b.iter_batched(setup, |vt| go(vt, &t), BatchSize::SmallInput)
    });
}

criterion_group!(
    name = benches;
    config = Criterion::default().measurement_time(std::time::Duration::from_secs(30));
    targets = criterion_benchmark
);

criterion_main!(benches);
