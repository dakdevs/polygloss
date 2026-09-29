//! Git-compatible unified text of a [`FileDiff`], for parity checks and copy (T1.6).
//!
//! The output is the hunk part of `git diff` without file headers and without
//! the function context git appends to `@@` lines. Use [`strip_git_headers`] to
//! bring real git output into the same shape.

use crate::hunks::{Block, FileDiff};
use crate::lines::LineIndex;

const NO_NEWLINE: &[u8] = b"\\ No newline at end of file\n";

/// `@@ -a,b +c,d @@` headers and ` `/`-`/`+` lines for every hunk, with git's
/// `\ No newline at end of file` markers. Context lines come from the new side,
/// as git prints them (it matters in whitespace mode). `old` and `new` must be
/// the blobs `fd` was computed from. Bytes that are not UTF-8 are replaced
/// with U+FFFD.
pub fn unified_text(fd: &FileDiff, old: &[u8], new: &[u8]) -> String {
    let mut out = Vec::new();
    for hunk in &fd.hunks {
        out.extend_from_slice(b"@@ -");
        push_range(&mut out, &hunk.old);
        out.extend_from_slice(b" +");
        push_range(&mut out, &hunk.new);
        out.extend_from_slice(b" @@\n");
        for block in &hunk.blocks {
            match block {
                Block::Equal { new: range, .. } => {
                    for i in range.clone() {
                        push_line(&mut out, b' ', &fd.new, new, i);
                    }
                }
                Block::Change {
                    old: old_range,
                    new: new_range,
                } => {
                    for i in old_range.clone() {
                        push_line(&mut out, b'-', &fd.old, old, i);
                    }
                    for i in new_range.clone() {
                        push_line(&mut out, b'+', &fd.new, new, i);
                    }
                }
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// git's `xdl_emit_hunk_hdr`: 1-based start, `,count` omitted when the count is
/// 1, and an empty range printed as the line before it.
fn push_range(out: &mut Vec<u8>, range: &std::ops::Range<u32>) {
    let count = range.end - range.start;
    let start = if count == 0 {
        range.start
    } else {
        range.start + 1
    };
    out.extend_from_slice(start.to_string().as_bytes());
    if count != 1 {
        out.push(b',');
        out.extend_from_slice(count.to_string().as_bytes());
    }
}

fn push_line(out: &mut Vec<u8>, prefix: u8, index: &LineIndex, bytes: &[u8], i: u32) {
    out.push(prefix);
    out.extend_from_slice(index.line(bytes, i));
    out.push(b'\n');
    if i + 1 == index.len() && !index.has_trailing_newline() {
        out.extend_from_slice(NO_NEWLINE);
    }
}

/// Normalizes one file's `git diff` output for comparison with [`unified_text`]: drops
/// everything before the first `@@` line (the `diff --git`, `index`, `---` and
/// `+++` headers) and the function context after each `@@ … @@`. Body lines
/// always start with ` `, `-`, `+` or `\`, so only hunk headers start with `@@`.
pub fn strip_git_headers(git_output: &str) -> String {
    let mut out = String::with_capacity(git_output.len());
    let mut in_body = false;
    for line in git_output.split_inclusive('\n') {
        if line.starts_with("@@ -") {
            in_body = true;
            match line[3..].find(" @@") {
                Some(end) => {
                    out.push_str(&line[..3 + end + 3]);
                    out.push('\n');
                }
                None => out.push_str(line),
            }
        } else if in_body {
            out.push_str(line);
        }
    }
    out
}
