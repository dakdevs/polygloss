//! Criterion benches for `polygloss_diff::line_map`: building a `LineMap` on a
//! synthetic 150k-line file (an edit every 211 lines, a 500-line insertion and a
//! 300-line deletion), building one from an existing `FileDiff`, and 10k
//! line/range queries.
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use polygloss_diff::hunks::diff_blobs;
use polygloss_diff::line_map::LineMap;
use polygloss_diff::options::DiffOptions;

fn synthetic() -> (Vec<u8>, Vec<u8>) {
    let old: Vec<String> = (0..150_000u32)
        .map(|i| format!("    let v{i} = compute({i});\n"))
        .collect();
    let mut new = old.clone();
    for i in (0..150_000usize).step_by(211) {
        new[i] = format!("    let v{i} = changed({i});\n");
    }
    new.splice(
        75_000..75_000,
        (0..500)
            .map(|i| format!("inserted {i}\n"))
            .collect::<Vec<_>>(),
    );
    new.drain(120_000..120_300);
    (old.concat().into_bytes(), new.concat().into_bytes())
}

fn line_map(c: &mut Criterion) {
    let (old, new) = synthetic();
    let mut group = c.benchmark_group("line_map");
    group.bench_function("new_150k", |b| {
        b.iter(|| LineMap::new(black_box(&old), black_box(&new)))
    });
    let fd = diff_blobs(&old, &new, &DiffOptions::default());
    group.bench_function("from_diff_150k", |b| {
        b.iter(|| LineMap::from_diff(black_box(&fd)))
    });
    let map = LineMap::new(&old, &new);
    group.bench_function("queries_10k", |b| {
        b.iter(|| {
            for line in (0..150_000u32).step_by(15) {
                black_box(map.map_line(line));
                black_box(map.map_line_back(line));
                black_box(map.map_range(line, line.saturating_add(4)));
            }
        })
    });
    group.finish();
}

criterion_group!(benches, line_map);
criterion_main!(benches);
