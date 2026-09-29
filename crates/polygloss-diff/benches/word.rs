//! Criterion benches for `polygloss_diff::word`: `word_ranges` per granularity
//! over a batch of realistic code line pairs, one line at the char limit, and
//! the skip path for a 5 MB minified line.
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use polygloss_diff::word::{Granularity, WORD_DIFF_MAX_LINE_CHARS, pair_lines, word_ranges};

/// 1,000 modified line pairs, each changing an identifier and a literal.
fn code_pairs() -> Vec<(Vec<u8>, Vec<u8>)> {
    (0..1_000u32)
        .map(|i| {
            let old = format!(
                "        let value_{i} = compute(&context, {i}, \"label {i}\").unwrap_or_default();"
            );
            let new = format!(
                "        let value_{i} = compute_fast(&context, {}, \"label {i}\")?;",
                i + 1
            );
            (old.into_bytes(), new.into_bytes())
        })
        .collect()
}

fn word(c: &mut Criterion) {
    let pairs = code_pairs();
    let at_limit_old = "ab cd ".repeat(WORD_DIFF_MAX_LINE_CHARS / 6).into_bytes();
    let at_limit_new = "ab ce ".repeat(WORD_DIFF_MAX_LINE_CHARS / 6).into_bytes();
    let huge = "var a=1;".repeat(5 * 1024 * 1024 / 8).into_bytes();

    let mut group = c.benchmark_group("word");
    for (name, g) in [("word", Granularity::Word), ("char", Granularity::Char)] {
        group.bench_function(format!("word_ranges_{name}_1000_pairs"), |b| {
            b.iter(|| {
                for (old, new) in &pairs {
                    black_box(word_ranges(black_box(old), black_box(new), g));
                }
            })
        });
        group.bench_function(format!("word_ranges_{name}_at_limit"), |b| {
            b.iter(|| word_ranges(black_box(&at_limit_old), black_box(&at_limit_new), g))
        });
    }
    group.bench_function("word_ranges_skip_5mb_line", |b| {
        b.iter(|| word_ranges(black_box(&huge), black_box(&huge), Granularity::Word))
    });
    group.bench_function("pair_lines_1000", |b| {
        b.iter(|| pair_lines(black_box(0..1_000), black_box(500..1_700)))
    });
    group.finish();
}

criterion_group!(benches, word);
criterion_main!(benches);
