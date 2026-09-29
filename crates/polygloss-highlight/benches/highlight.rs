//! Criterion benches for `polygloss-highlight`: whole-blob highlighting (the
//! background cost per file side), language guessing and scope lookup.
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use polygloss_highlight::{
    Appearance, Budget, Highlighter, Language, SyntaxTheme, guess_language, pierre_theme,
};

/// `n` lines of plausible Rust.
fn rust_source(n: usize) -> String {
    let mut s = String::with_capacity(n * 44);
    for i in 0..n {
        match i % 4 {
            0 => s.push_str(&format!("fn f{i}(x: u32) -> u32 {{\n")),
            1 => s.push_str(&format!("    let y = x + {i}; // note {i}\n")),
            2 => s.push_str("    y * 2\n"),
            _ => s.push_str("}\n"),
        }
    }
    s
}

/// `n` lines of plausible TypeScript.
fn ts_source(n: usize) -> String {
    let mut s = String::with_capacity(n * 48);
    for i in 0..n {
        match i % 5 {
            0 => s.push_str(&format!("export function f{i}(x: number): string {{\n")),
            1 => s.push_str(&format!("  const label = `item ${{x}} of {i}`;\n")),
            2 => s.push_str("  if (x > 10) { return label.toUpperCase(); }\n"),
            3 => s.push_str("  return label; // plain\n"),
            _ => s.push_str("}\n"),
        }
    }
    s
}

fn highlight(c: &mut Criterion) {
    let theme = Arc::new(SyntaxTheme::from_zed(pierre_theme(Appearance::Light)));
    let hl = Highlighter::new(theme.clone());
    let budget = Budget {
        time: Duration::from_secs(60),
        max_lines: u32::MAX,
    };
    let cancel = AtomicUsize::new(0);
    let rust = Language::from_name("rust").unwrap();
    let ts = Language::from_name("typescript").unwrap();
    let rust_10k = rust_source(10_000);
    let ts_10k = ts_source(10_000);

    let mut group = c.benchmark_group("highlight");
    group.sample_size(10);
    group.bench_function("rust_10k_lines", |b| {
        b.iter(|| hl.highlight(black_box(rust_10k.as_bytes()), &rust, &cancel, budget))
    });
    group.bench_function("typescript_10k_lines", |b| {
        b.iter(|| hl.highlight(black_box(ts_10k.as_bytes()), &ts, &cancel, budget))
    });
    // Baseline: lumis's event stream alone, so the adapter's own cost (scope
    // mapping, line splitting, span packing) is the difference.
    group.bench_function("lumis_events_only_rust_10k_lines", |b| {
        b.iter(|| {
            lumis::highlight::highlight_events_with_options(
                black_box(&rust_10k),
                lumis::languages::Language::Rust,
                lumis::HighlightOptions::new(),
            )
        })
    });
    group.finish();

    let paths = [
        "crates/polygloss-highlight/src/lib.rs",
        "web/src/components/App.tsx",
        "docs/README.md",
        "Makefile",
        "assets/logo.png",
    ];
    c.bench_function("guess_language_5_paths", |b| {
        b.iter(|| {
            for p in paths {
                black_box(guess_language(black_box(p), b""));
            }
        })
    });
    c.bench_function("style_for_scope", |b| {
        b.iter(|| theme.style_for_scope(black_box("keyword.function.rust")))
    });
}

criterion_group!(benches, highlight);
criterion_main!(benches);
