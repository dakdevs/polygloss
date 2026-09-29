//! Criterion benches for `polygloss_diff::hunks`. Empty until its M1 task adds cases.
use criterion::{Criterion, criterion_group, criterion_main};

fn hunks(c: &mut Criterion) {
    c.benchmark_group("hunks").finish();
}

criterion_group!(benches, hunks);
criterion_main!(benches);
