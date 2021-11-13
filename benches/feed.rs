use criterion::{criterion_group, criterion_main, Criterion};
use vt::VT;
use std::fs;

fn go(t: &str) {
    let mut vt = VT::new(100, 24);

    for _n in 0..10 {
        vt.feed_str(t);
    }
}

pub fn criterion_benchmark(c: &mut Criterion) {
    let t = fs::read_to_string("benches/sample.txt").unwrap();

    c.bench_function("feed", |b|
        b.iter(|| go(&t))
	);
}

criterion_group!(
    name = benches;
    config = Criterion::default().measurement_time(std::time::Duration::from_secs(30));
    targets = criterion_benchmark
);

criterion_main!(benches);
