//! Criterion benches for `polygloss_diff::word`. Empty until its M1 task adds cases.
use criterion::{Criterion, criterion_group, criterion_main};

fn word(c: &mut Criterion) {
    c.benchmark_group("word").finish();
}

criterion_group!(benches, word);
criterion_main!(benches);
