use criterion::{criterion_group, criterion_main, Criterion};
use avt::VT;

fn go(vt: &VT) {

    for _n in 0..10 {
        vt.dump();
    }
}

pub fn criterion_benchmark(c: &mut Criterion) {
    let mut vt = VT::new(100, 24);

    for fg in 1..8 {
        for bg in 1..8 {
            for i in 0..8 {
                vt.feed_str(&format!("\x1b[3{};4{};{}mABCD.", fg, bg, i % 2));
            }
        }
    }

    c.bench_function("dump", |b|
        b.iter(|| go(&vt))
	);
}

criterion_group!(
    name = benches;
    config = Criterion::default().measurement_time(std::time::Duration::from_secs(30));
    targets = criterion_benchmark
);

criterion_main!(benches);
