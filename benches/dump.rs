use avt::Vt;
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

fn setup() -> Vt {
    let mut vt = Vt::new(100, 24);

    for fg in 1..8 {
        for bg in 1..8 {
            for i in 0..8 {
                vt.feed_str(&format!("\x1b[3{};4{};{}mABCD.", fg, bg, i % 2));
            }
        }
    }

    vt
}

fn run(vt: Vt) -> Vt {
    vt.dump();

    vt
}

pub fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("dump", |b| {
        b.iter_batched(setup, run, BatchSize::SmallInput)
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
