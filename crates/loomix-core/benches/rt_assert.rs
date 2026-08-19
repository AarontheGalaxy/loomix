use criterion::{criterion_group, criterion_main, Criterion};
use loomix_core::rt_assert::assert_realtime;

// Placeholder baseline until M3 adds the engine's process() bench (spec
// layer 9). Tracks that the guard itself stays effectively free.
fn bench_guard_overhead(c: &mut Criterion) {
    c.bench_function("rt_assert_guard_overhead", |b| {
        b.iter(|| assert_realtime(|| std::hint::black_box(1_u32 + 1)));
    });
}

criterion_group!(benches, bench_guard_overhead);
criterion_main!(benches);
