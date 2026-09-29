//! Word and char diff ranges with GitHub-style line pairing (T1.7, design §6.3).
//!
//! In a change block, removed and added lines pair up in order ([`pair_lines`]);
//! each pair then gets the byte ranges that differ on either side
//! ([`word_ranges`]), computed with gix-imara-diff's Myers over word or char
//! tokens. Ranges are byte offsets within the line, always on char boundaries,
//! so the viewport can paint them directly over GPUI text runs.

use std::ops::Range;

use gix_imara_diff::{Algorithm, Diff, InternedInput};
use serde::{Deserialize, Serialize};

/// Token size for intra-line highlights. Words by default (design §6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Granularity {
    /// Runs of word characters (alphanumeric or `_`), runs of whitespace, and
    /// single punctuation chars.
    #[default]
    Word,
    /// Every char is a token.
    Char,
}

/// Lines longer than this many chars (on either side) get no word diff
/// (**Provisional**, design §6.3). A trailing CR does not count.
pub const WORD_DIFF_MAX_LINE_CHARS: usize = 1000;

/// A removed line (`old`) paired with an added line (`new`), 0-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LinePair {
    pub old: u32,
    pub new: u32,
}

/// GitHub-style pairing of a change block: the i-th removed line pairs with the
/// i-th added line. Extra lines on the longer side stay unpaired (and are not
/// in the result).
pub fn pair_lines(old: Range<u32>, new: Range<u32>) -> Vec<LinePair> {
    old.zip(new)
        .map(|(old, new)| LinePair { old, new })
        .collect()
}

/// Changed byte ranges within a paired old and new line: sorted, disjoint,
/// non-empty and on char boundaries. Empty on both sides when the lines are
/// equal (ignoring a trailing CR).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WordRanges {
    pub old: Vec<Range<u32>>,
    pub new: Vec<Range<u32>>,
}

/// The ranges that differ between `old_line` and `new_line` (each without its
/// `\n`). A single trailing `\r` on either line is ignored, so CRLF files and
/// CRLF→LF conversions never mark the line ending. Returns `None` when either
/// line is longer than [`WORD_DIFF_MAX_LINE_CHARS`]; that check is O(1) for
/// long lines, so a multi-megabyte minified line costs nothing.
///
/// Lines need not be UTF-8: each maximal invalid byte sequence (as in
/// `String::from_utf8_lossy`) is one char token.
pub fn word_ranges(old_line: &[u8], new_line: &[u8], g: Granularity) -> Option<WordRanges> {
    let old = strip_trailing_cr(old_line);
    let new = strip_trailing_cr(new_line);
    if exceeds_limit(old) || exceeds_limit(new) {
        return None;
    }
    if old == new {
        return Some(WordRanges::default());
    }
    let old_tokens = tokenize(old, g);
    let new_tokens = tokenize(new, g);

    let mut input: InternedInput<&[u8]> = InternedInput::default();
    input.reserve(old_tokens.len() as u32, new_tokens.len() as u32);
    input.update_before(old_tokens.iter().map(|r| &old[r.clone()]));
    input.update_after(new_tokens.iter().map(|r| &new[r.clone()]));
    let mut diff = Diff::compute(Algorithm::Myers, &input);
    diff.postprocess_no_heuristic(&input);

    let mut out = WordRanges::default();
    for hunk in diff.hunks() {
        if let Some(span) = byte_span(&old_tokens, hunk.before) {
            out.old.push(span);
        }
        if let Some(span) = byte_span(&new_tokens, hunk.after) {
            out.new.push(span);
        }
    }
    Some(out)
}

fn strip_trailing_cr(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// Whether `line` has more than [`WORD_DIFF_MAX_LINE_CHARS`] chars. A char is
/// at most 4 bytes (an invalid sequence at most 3), so only lines between the
/// limit and 4× the limit in bytes need counting.
fn exceeds_limit(line: &[u8]) -> bool {
    if line.len() <= WORD_DIFF_MAX_LINE_CHARS {
        return false;
    }
    if line.len() > 4 * WORD_DIFF_MAX_LINE_CHARS {
        return true;
    }
    let chars: usize = line
        .utf8_chunks()
        .map(|chunk| chunk.valid().chars().count() + usize::from(!chunk.invalid().is_empty()))
        .sum();
    chars > WORD_DIFF_MAX_LINE_CHARS
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Space,
    Other,
}

fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Other
    }
}

/// Byte ranges of the tokens of `line`, covering it exactly, in order.
fn tokenize(line: &[u8], g: Granularity) -> Vec<Range<usize>> {
    let mut tokens: Vec<Range<usize>> = Vec::with_capacity(match g {
        Granularity::Word => line.len() / 3 + 1,
        Granularity::Char => line.len(),
    });
    let mut offset = 0;
    for chunk in line.utf8_chunks() {
        // Class of the token at the end of `tokens` if it may still grow.
        let mut open: Option<Class> = None;
        for (i, c) in chunk.valid().char_indices() {
            let (start, end) = (offset + i, offset + i + c.len_utf8());
            let cls = class(c);
            match (g, open) {
                (Granularity::Word, Some(prev)) if prev == cls => {
                    if let Some(last) = tokens.last_mut() {
                        last.end = end;
                    }
                }
                _ => {
                    tokens.push(start..end);
                    open = (g == Granularity::Word && cls != Class::Other).then_some(cls);
                }
            }
        }
        offset += chunk.valid().len();
        let invalid = chunk.invalid().len();
        if invalid > 0 {
            tokens.push(offset..offset + invalid);
            offset += invalid;
        }
    }
    tokens
}

/// The byte range spanned by the tokens `range`, or `None` if it is empty.
fn byte_span(tokens: &[Range<usize>], range: Range<u32>) -> Option<Range<u32>> {
    if range.is_empty() {
        return None;
    }
    let start = tokens[range.start as usize].start as u32;
    let end = tokens[range.end as usize - 1].end as u32;
    Some(start..end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(line: &str, g: Granularity) -> Vec<&str> {
        tokenize(line.as_bytes(), g)
            .into_iter()
            .map(|r| &line[r])
            .collect()
    }

    #[test]
    fn tokenize_words_groups_word_and_space_runs() {
        assert_eq!(
            texts("  foo_bar(x1, ÿé)->", Granularity::Word),
            ["  ", "foo_bar", "(", "x1", ",", " ", "ÿé", ")", "-", ">"]
        );
    }

    #[test]
    fn tokenize_chars_one_token_per_char() {
        assert_eq!(texts("a 👍é", Granularity::Char), ["a", " ", "👍", "é"]);
    }

    #[test]
    fn tokenize_invalid_utf8_is_one_token_per_sequence() {
        let tokens = tokenize(b"ab\xff\xfecd", Granularity::Word);
        assert_eq!(tokens, [0..2, 2..3, 3..4, 4..6]);
        // A truncated multi-byte sequence is one token, like one U+FFFD.
        let tokens = tokenize(b"a\xe2\x82b", Granularity::Char);
        assert_eq!(tokens, [0..1, 1..3, 3..4]);
    }

    #[test]
    fn exceeds_limit_counts_chars_not_bytes() {
        assert!(!exceeds_limit(
            "é".repeat(WORD_DIFF_MAX_LINE_CHARS).as_bytes()
        ));
        assert!(exceeds_limit(
            "é".repeat(WORD_DIFF_MAX_LINE_CHARS + 1).as_bytes()
        ));
        assert!(exceeds_limit(&[b'x'; 4 * WORD_DIFF_MAX_LINE_CHARS + 1]));
    }
}
