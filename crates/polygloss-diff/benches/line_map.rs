//! Criterion benches for `polygloss_diff::line_map`. Empty until its M1 task adds cases.
use criterion::{Criterion, criterion_group, criterion_main};

fn line_map(c: &mut Criterion) {
    c.benchmark_group("line_map").finish();
}

criterion_group!(benches, line_map);
criterion_main!(benches);
