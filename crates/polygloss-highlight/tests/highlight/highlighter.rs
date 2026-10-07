use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use polygloss_diff::testing::assert_ratio_below;
use polygloss_highlight::{
    Budget, HighlightError, Highlighter, Language, Span, StyleId, SyntaxTheme, Tokens,
};

use crate::support::{minimal_syntax, rust_source, theme_with_keys};

fn rust() -> Language {
    Language::from_name("rust").unwrap()
}

fn unbounded() -> Budget {
    Budget {
        time: Duration::from_secs(60),
        max_lines: u32::MAX,
    }
}

fn highlight(theme: &Arc<SyntaxTheme>, src: &str, lang: Language) -> Tokens {
    Highlighter::new(theme.clone())
        .highlight(src.as_bytes(), &lang, &AtomicUsize::new(0), unbounded())
        .unwrap()
}

/// The style covering byte `at` of line `line`, if a span covers it.
fn style_at(tokens: &Tokens, line: u32, at: u32) -> Option<StyleId> {
    tokens
        .line(line)
        .iter()
        .find(|s| s.start <= at && at < s.start + s.len)
        .map(|s| s.style)
}

#[test]
fn highlight_rust_marks_keywords() {
    let theme = minimal_syntax();
    let tokens = highlight(&theme, "fn main() {\n    let x = 1;\n}\n", rust());
    assert_eq!(tokens.line_count(), 3);
    // `fn` is `keyword.function` (its own entry), `let` falls back to `keyword`.
    let function_kw = theme.style_for_scope("keyword.function");
    let keyword = theme.style_for_scope("keyword");
    assert_ne!(function_kw, StyleId::DEFAULT);
    assert_ne!(keyword, StyleId::DEFAULT);
    assert_eq!(style_at(&tokens, 0, 0), Some(function_kw));
    assert_eq!(style_at(&tokens, 0, 1), Some(function_kw));
    assert_eq!(style_at(&tokens, 1, 4), Some(keyword));
    assert_eq!(style_at(&tokens, 1, 6), Some(keyword));
    // `main` is not a keyword.
    assert_ne!(style_at(&tokens, 0, 3), Some(keyword));
    assert_ne!(style_at(&tokens, 0, 3), Some(function_kw));
}

/// Every span of every line is non-empty, sorted, disjoint, inside the line
/// (which excludes its `\n`) and on char boundaries.
fn assert_well_formed(src: &str, tokens: &Tokens) {
    let lines: Vec<&str> = if src.is_empty() {
        Vec::new()
    } else {
        src.strip_suffix('\n').unwrap_or(src).split('\n').collect()
    };
    assert_eq!(tokens.line_count() as usize, lines.len());
    for (i, line) in lines.iter().enumerate() {
        let mut end = 0;
        for &Span { start, len, style } in tokens.line(i as u32) {
            assert!(len > 0, "line {i}: empty span");
            assert!(start >= end, "line {i}: spans overlap or are unsorted");
            end = start + len;
            assert!(end as usize <= line.len(), "line {i}: span past the line");
            assert!(line.is_char_boundary(start as usize), "line {i}: start");
            assert!(line.is_char_boundary(end as usize), "line {i}: end");
            assert_ne!(style, StyleId::DEFAULT, "line {i}: unstyled span stored");
        }
    }
}

#[test]
fn spans_are_byte_ranges_within_lines() {
    let theme = Arc::new(SyntaxTheme::from_zed(&theme_with_keys(&[
        "comment", "string", "keyword", "function", "type", "number",
    ])));
    let src = "/* multi\n   line ✓ */\nlet s = \"héllo wörld\";\r\nfn g() -> u8 { 7 }\n\n// tail ✓";
    let tokens = highlight(&theme, src, rust());
    assert_well_formed(src, &tokens);
    // The block comment spans lines 0 and 1, split at the line boundary.
    let comment = theme.style_for_scope("comment");
    assert_eq!(
        tokens.line(0),
        &[Span {
            start: 0,
            len: 8,
            style: comment
        }]
    );
    assert_eq!(
        tokens.line(1),
        &[Span {
            start: 0,
            len: "   line ✓ */".len() as u32,
            style: comment
        }]
    );
    // The string on line 2 covers its non-ASCII bytes exactly.
    let string = theme.style_for_scope("string");
    let open = "let s = ".len() as u32;
    let close = "let s = \"héllo wörld\"".len() as u32;
    assert_eq!(style_at(&tokens, 2, open), Some(string));
    assert_eq!(style_at(&tokens, 2, close - 1), Some(string));
    assert_ne!(style_at(&tokens, 2, close), Some(string));
    // An empty line has no spans; lines past the end are empty too.
    assert!(tokens.line(4).is_empty());
    assert_eq!(style_at(&tokens, 5, 0), Some(comment));
    assert!(tokens.line(6).is_empty());
    assert!(tokens.line(u32::MAX).is_empty());
}

#[test]
fn line_count_follows_line_index_rules() {
    let theme = minimal_syntax();
    for (src, lines) in [
        ("", 0),
        ("\n", 1),
        ("a", 1),
        ("a\nb", 2),
        ("a\nb\n", 2),
        ("\n\n", 2),
    ] {
        assert_eq!(
            highlight(&theme, src, rust()).line_count(),
            lines,
            "{src:?}"
        );
    }
    let big = rust_source(1_000);
    assert_well_formed(&big, &highlight(&theme, &big, rust()));
}

#[test]
fn unstyled_inner_scopes_inherit_the_enclosing_style() {
    // Only `markup.raw` is styled: the fenced Rust code inside it (keywords,
    // punctuation, ...) keeps the block's style instead of dropping to default.
    let theme = Arc::new(SyntaxTheme::from_zed(&theme_with_keys(&["markup.raw"])));
    let raw = theme.style_for_scope("markup.raw");
    let src = "# Title\n\n```rust\nfn main() { let x = 1; }\n```\n";
    let tokens = highlight(&theme, src, Language::from_name("markdown").unwrap());
    assert_well_formed(src, &tokens);
    let code = "fn main() { let x = 1; }";
    for at in 0..code.len() as u32 {
        assert_eq!(
            style_at(&tokens, 3, at),
            Some(raw),
            "byte {at} of the code line"
        );
    }
}

#[test]
fn highlight_cancel_returns_cancelled() {
    let hl = Highlighter::new(minimal_syntax());
    let cancel = AtomicUsize::new(1);
    let src = rust_source(10);
    assert_eq!(
        hl.highlight(src.as_bytes(), &rust(), &cancel, unbounded()),
        Err(HighlightError::Cancelled)
    );
}

/// Highlights `src` while another thread cancels it 10 ms in.
fn cancelled_after_10ms(hl: &Highlighter, src: &str) -> Result<Tokens, HighlightError> {
    let cancel = Arc::new(AtomicUsize::new(0));
    let canceller = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            cancel.store(1, Ordering::Relaxed);
        })
    };
    let result = hl.highlight(src.as_bytes(), &rust(), &cancel, unbounded());
    canceller.join().unwrap();
    result
}

#[test]
fn highlight_cancel_from_another_thread_stops_early() {
    let hl = Highlighter::new(minimal_syntax());
    // Warm the grammar so the timed calls are highlighting only.
    hl.highlight(b"fn a() {}", &rust(), &AtomicUsize::new(0), unbounded())
        .unwrap();
    let sources = [rust_source(20_000), rust_source(200_000)];
    assert_eq!(
        cancelled_after_10ms(&hl, &sources[1]),
        Err(HighlightError::Cancelled)
    );
    // Stopping early takes about 10 ms whatever the source; checked only at
    // the end, it would take the whole highlight, 10x longer for 10x lines.
    assert_ratio_below(
        "cancelling 10 ms in, 20k -> 200k lines",
        4.0,
        [&sources[0], &sources[1]],
        |src| cancelled_after_10ms(&hl, src),
    );
}

/// Highlights `src` with a 20 ms time budget.
fn with_20ms_budget(hl: &Highlighter, src: &str) -> Result<Tokens, HighlightError> {
    let budget = Budget {
        time: Duration::from_millis(20),
        max_lines: u32::MAX,
    };
    hl.highlight(src.as_bytes(), &rust(), &AtomicUsize::new(0), budget)
}

#[test]
fn highlight_budget_exceeded_returns_within_budget() {
    let hl = Highlighter::new(minimal_syntax());
    // Compiling a grammar's queries is a one-time cost outside the budget.
    hl.highlight(b"fn a() {}", &rust(), &AtomicUsize::new(0), unbounded())
        .unwrap();
    let sources = [rust_source(20_000), rust_source(200_000)];
    assert_eq!(
        with_20ms_budget(&hl, &sources[1]),
        Err(HighlightError::BudgetExceeded)
    );
    // Returning at the budget takes about 20 ms whatever the source; checked
    // only at the end, it would take the whole highlight, 10x longer for 10x
    // lines.
    let [_, large] = assert_ratio_below(
        "a 20 ms budget, 20k -> 200k lines",
        4.0,
        [&sources[0], &sources[1]],
        |src| with_20ms_budget(&hl, src),
    );
    // And at the budget, not a multiple of it: the budget is wall-clock, so a
    // slower machine stops sooner in the source, not later in time.
    assert!(
        large < Duration::from_millis(80),
        "a 20 ms budget returned after {large:?} (limit 80ms)"
    );
}

#[test]
fn highlight_over_max_lines_is_budget_exceeded() {
    let hl = Highlighter::new(minimal_syntax());
    let budget = Budget {
        time: Duration::from_secs(60),
        max_lines: 10,
    };
    let cancel = AtomicUsize::new(0);
    let ten = rust_source(10);
    assert_eq!(
        hl.highlight(ten.as_bytes(), &rust(), &cancel, budget)
            .unwrap()
            .line_count(),
        10
    );
    let eleven = rust_source(11);
    assert_eq!(
        hl.highlight(eleven.as_bytes(), &rust(), &cancel, budget),
        Err(HighlightError::BudgetExceeded)
    );
    assert_eq!(Budget::default().max_lines, 100_000);
}

#[test]
fn highlight_non_utf8_is_unsupported_not_panic() {
    let hl = Highlighter::new(minimal_syntax());
    let src = b"fn main() {\n    let s = \"\xff\xfe\";\n}\n";
    assert_eq!(
        hl.highlight(src, &rust(), &AtomicUsize::new(0), unbounded()),
        Err(HighlightError::Unsupported)
    );
}

#[test]
fn tokens_heap_bytes_counts_spans_and_line_table() {
    let theme = minimal_syntax();
    let small = highlight(&theme, &rust_source(10), rust());
    let large = highlight(&theme, &rust_source(1_000), rust());
    assert!(small.heap_bytes() > 0);
    assert!(large.heap_bytes() > 50 * small.heap_bytes());
    assert_eq!(std::mem::size_of::<Span>(), 12);
}

#[test]
fn highlight_after_an_interrupted_one_on_the_same_thread_is_complete() {
    // lumis reuses a parser per thread; a parse stopped by the budget or by
    // cancellation must not leak into the next highlight on that thread.
    let theme = minimal_syntax();
    let hl = Highlighter::new(theme.clone());
    let small = "fn main() {\n    let x = \"s\"; // c\n}\n";
    let expected = highlight(&theme, small, rust());
    let big = rust_source(200_000);
    let tight = Budget {
        time: Duration::from_millis(5),
        max_lines: u32::MAX,
    };
    assert_eq!(
        hl.highlight(big.as_bytes(), &rust(), &AtomicUsize::new(0), tight),
        Err(HighlightError::BudgetExceeded)
    );
    assert_eq!(highlight(&theme, small, rust()), expected);
    let cancel = Arc::new(AtomicUsize::new(0));
    let canceller = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(5));
            cancel.store(1, Ordering::Relaxed);
        })
    };
    assert_eq!(
        hl.highlight(big.as_bytes(), &rust(), &cancel, unbounded()),
        Err(HighlightError::Cancelled)
    );
    canceller.join().unwrap();
    assert_eq!(highlight(&theme, small, rust()), expected);
}
