//! Criterion benches for `polygloss_diff::hunks`: `diff_blobs` per algorithm and
//! whitespace mode, and `unified_text`, on a synthetic 20k-line file with an edit
//! every 50 lines.
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use polygloss_diff::hunks::diff_blobs;
use polygloss_diff::options::{Algorithm, DiffOptions};
use polygloss_diff::unified_text::unified_text;

fn synthetic() -> (Vec<u8>, Vec<u8>) {
    let mut old = String::new();
    let mut new = String::new();
    for i in 0..20_000u32 {
        let depth = (i % 4) as usize * 4;
        let line = format!("{:depth$}let value_{i} = compute({i});\n", "");
        old.push_str(&line);
        match i % 50 {
            0 => new.push_str(&format!("{:depth$}let value_{i} = changed({i});\n", "")),
            17 => {}
            31 => {
                new.push_str(&line);
                new.push_str(&format!("{:depth$}inserted({i});\n", ""));
            }
            _ => new.push_str(&line),
        }
    }
    (old.into_bytes(), new.into_bytes())
}

fn hunks(c: &mut Criterion) {
    let (old, new) = synthetic();
    let myers = DiffOptions::default();
    let histogram = DiffOptions {
        algorithm: Algorithm::Histogram,
        ..myers
    };
    let whitespace = DiffOptions {
        ignore_whitespace: true,
        ..myers
    };
    let mut group = c.benchmark_group("hunks");
    group.bench_function("diff_blobs_myers_20k", |b| {
        b.iter(|| diff_blobs(black_box(&old), black_box(&new), &myers))
    });
    group.bench_function("diff_blobs_histogram_20k", |b| {
        b.iter(|| diff_blobs(black_box(&old), black_box(&new), &histogram))
    });
    group.bench_function("diff_blobs_ignore_whitespace_20k", |b| {
        b.iter(|| diff_blobs(black_box(&old), black_box(&new), &whitespace))
    });
    let fd = diff_blobs(&old, &new, &myers);
    group.bench_function("unified_text_20k", |b| {
        b.iter(|| unified_text(black_box(&fd), &old, &new))
    });
    group.finish();
}

criterion_group!(benches, hunks);
criterion_main!(benches);
