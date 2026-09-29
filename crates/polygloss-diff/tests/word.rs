//! Word and char diff ranges with GitHub-style line pairing (T1.7, design §6.3).

// Expected values are lists of byte ranges, and many hold exactly one range.
#![allow(clippy::single_range_in_vec_init)]

use std::ops::Range;
use std::time::{Duration, Instant};

use polygloss_diff::word::{
    Granularity, LinePair, WORD_DIFF_MAX_LINE_CHARS, WordRanges, pair_lines, word_ranges,
};

fn pair(old: u32, new: u32) -> LinePair {
    LinePair { old, new }
}

fn ranges(old: &[Range<u32>], new: &[Range<u32>]) -> WordRanges {
    WordRanges {
        old: old.to_vec(),
        new: new.to_vec(),
    }
}

fn words(old: &str, new: &str) -> WordRanges {
    word_ranges(old.as_bytes(), new.as_bytes(), Granularity::Word).expect("short lines diff")
}

fn chars(old: &str, new: &str) -> WordRanges {
    word_ranges(old.as_bytes(), new.as_bytes(), Granularity::Char).expect("short lines diff")
}

/// Every range is non-empty, in bounds, sorted, non-overlapping and on char
/// boundaries of `line`.
fn assert_well_formed(line: &str, rs: &[Range<u32>]) {
    let mut prev_end = 0;
    for r in rs {
        let (start, end) = (r.start as usize, r.end as usize);
        assert!(start < end, "empty range {r:?} in {line:?}");
        assert!(
            start >= prev_end,
            "unsorted or overlapping {rs:?} in {line:?}"
        );
        assert!(end <= line.len(), "range {r:?} out of bounds in {line:?}");
        assert!(
            line.is_char_boundary(start),
            "{r:?} splits a char in {line:?}"
        );
        assert!(
            line.is_char_boundary(end),
            "{r:?} splits a char in {line:?}"
        );
        prev_end = end;
    }
}

#[test]
fn pair_lines_zips_in_order() {
    assert_eq!(
        pair_lines(3..6, 10..13),
        vec![pair(3, 10), pair(4, 11), pair(5, 12)]
    );
    assert_eq!(pair_lines(0..1, 7..8), vec![pair(0, 7)]);
}

#[test]
fn pair_lines_unbalanced_block() {
    // More removed than added: the extra removed lines stay unpaired.
    assert_eq!(pair_lines(0..3, 5..6), vec![pair(0, 5)]);
    // More added than removed: the extra added lines stay unpaired.
    assert_eq!(pair_lines(4..5, 0..3), vec![pair(4, 0)]);
    // Pure insertion or removal: nothing pairs.
    assert_eq!(pair_lines(2..2, 0..4), vec![]);
    assert_eq!(pair_lines(0..4, 9..9), vec![]);
}

#[test]
fn word_ranges_single_identifier_change() {
    // Only the changed identifier is marked, as a whole word.
    let old = "let total = compute(alpha);";
    let new = "let total = compute(bravo);";
    assert_eq!(words(old, new), ranges(&[20..25], &[20..25]));

    // `_` is a word character: the whole identifier is one token.
    let old = "    let foo_bar = 1;";
    let new = "    let foo_baz = 1;";
    assert_eq!(words(old, new), ranges(&[8..15], &[8..15]));

    // A pure insertion inside the line marks only the new side.
    let old = "call(a)";
    let new = "call(a, b)";
    assert_eq!(words(old, new), ranges(&[], &[6..9]));
}

#[test]
fn word_ranges_char_granularity() {
    let old = "    let foo_bar = 1;";
    let new = "    let foo_baz = 1;";
    assert_eq!(chars(old, new), ranges(&[14..15], &[14..15]));

    let old = "color";
    let new = "colour";
    assert_eq!(chars(old, new), ranges(&[], &[4..5]));
}

#[test]
fn word_diff_skips_long_lines() {
    // RF4: a 5 MB minified single line gets no word diff, and fast.
    let huge: String = "var a=1;".repeat(5 * 1024 * 1024 / 8);
    let huge_changed = huge.replacen("a=1", "a=2", 1);
    let start = Instant::now();
    assert_eq!(
        word_ranges(huge.as_bytes(), huge_changed.as_bytes(), Granularity::Word),
        None
    );
    assert_eq!(
        word_ranges(huge.as_bytes(), b"short", Granularity::Char),
        None
    );
    assert_eq!(
        word_ranges(b"short", huge.as_bytes(), Granularity::Word),
        None
    );
    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_millis(50), "took {elapsed:?}");

    // The limit counts chars, not bytes: exactly at the limit still diffs.
    let at_limit = "x".repeat(WORD_DIFF_MAX_LINE_CHARS);
    let over_limit = "x".repeat(WORD_DIFF_MAX_LINE_CHARS + 1);
    assert!(word_ranges(at_limit.as_bytes(), b"y", Granularity::Word).is_some());
    assert!(word_ranges(b"y", over_limit.as_bytes(), Granularity::Word).is_none());
    let wide_at_limit = "é".repeat(WORD_DIFF_MAX_LINE_CHARS);
    let wide_over_limit = "é".repeat(WORD_DIFF_MAX_LINE_CHARS + 1);
    assert!(word_ranges(wide_at_limit.as_bytes(), b"e", Granularity::Char).is_some());
    assert!(word_ranges(wide_over_limit.as_bytes(), b"e", Granularity::Char).is_none());
}

#[test]
fn word_ranges_utf8_boundaries() {
    // "naïve " is 7 bytes; "café" is 5.
    let old = "naïve café";
    let new = "naïve cafe";
    let w = words(old, new);
    assert_eq!(w, ranges(&[7..12], &[7..11]));
    let c = chars(old, new);
    assert_eq!(c, ranges(&[10..12], &[10..11]));

    // Multi-byte chars on both sides, including a 4-byte emoji.
    for (old, new) in [
        ("日本語のテキスト", "日本語のテスト"),
        ("ok 👍 done", "ok 👎 done"),
        ("Grüße, Welt", "Grüße, Wält"),
    ] {
        for g in [Granularity::Word, Granularity::Char] {
            let r = word_ranges(old.as_bytes(), new.as_bytes(), g).expect("short lines");
            assert!(!r.old.is_empty() || !r.new.is_empty(), "{old:?} vs {new:?}");
            assert_well_formed(old, &r.old);
            assert_well_formed(new, &r.new);
        }
    }
    assert_eq!(chars("ok 👍 done", "ok 👎 done"), ranges(&[3..7], &[3..7]));

    // Non-UTF-8 bytes never panic; an invalid byte is one char.
    let r = word_ranges(b"ab\xffcd", b"ab\xfecd", Granularity::Char).expect("short");
    assert_eq!(r, ranges(&[2..3], &[2..3]));
    let r = word_ranges(b"x \xff\xfe y", b"x z y", Granularity::Word).expect("short");
    assert_eq!(r, ranges(&[2..4], &[2..3]));
}

#[test]
fn word_ranges_identical_lines_empty() {
    for line in ["", "fn main() {}", "  naïve café 👍", "a\r"] {
        assert_eq!(words(line, line), WordRanges::default());
        assert_eq!(chars(line, line), WordRanges::default());
    }
}

#[test]
fn word_ranges_crlf_ignores_cr() {
    // A trailing CR is line-ending noise: never compared, never marked.
    assert_eq!(words("foo bar\r", "foo baz\r"), ranges(&[4..7], &[4..7]));
    assert_eq!(chars("foo bar\r", "foo baz\r"), ranges(&[6..7], &[6..7]));
    // CRLF -> LF only: nothing to mark.
    assert_eq!(words("foo bar\r", "foo bar"), WordRanges::default());
    assert_eq!(chars("foo\r", "foo"), WordRanges::default());
    // A changed line ending in CR on one side only.
    assert_eq!(words("a\r", "b"), ranges(&[0..1], &[0..1]));
    // A CR in the middle of a line is content.
    assert_eq!(chars("a\rb", "ab"), ranges(&[1..2], &[]));
}

#[test]
fn word_ranges_whitespace_runs_are_one_token() {
    // Indentation change: one run of spaces replaced by a tab.
    assert_eq!(words("    x = 1", "\tx = 1"), ranges(&[0..4], &[0..1]));
    // Punctuation is one token per char.
    assert_eq!(words("a+=b", "a-=b"), ranges(&[1..2], &[1..2]));
}
