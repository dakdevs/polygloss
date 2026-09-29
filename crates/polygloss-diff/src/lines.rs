//! `LineIndex`: byte ranges of the lines of a blob, CR kept as content (T1.6).

use std::ops::Range;

/// The lines of a blob as byte ranges. Lines end at `\n`, which is not part of
/// the line; a `\r` before it is content, as in git without
/// `--ignore-cr-at-eol`. The index stores offsets only, so it is paired with the
/// blob bytes it was built from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineIndex {
    /// Start offset of every line.
    starts: Vec<usize>,
    /// Length of the blob.
    total: usize,
    /// The last line ends with `\n` (or the blob is empty).
    trailing_newline: bool,
}

impl LineIndex {
    pub fn new(bytes: &[u8]) -> LineIndex {
        let mut starts = Vec::with_capacity(bytes.len() / 32 + 1);
        if !bytes.is_empty() {
            starts.push(0);
        }
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\n' && i + 1 < bytes.len() {
                starts.push(i + 1);
            }
        }
        LineIndex {
            starts,
            total: bytes.len(),
            trailing_newline: bytes.last().is_none_or(|&b| b == b'\n'),
        }
    }

    /// Number of lines. A final line without `\n` counts.
    pub fn len(&self) -> u32 {
        self.starts.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.starts.is_empty()
    }

    /// Byte range of line `i` (0-based) without its `\n`. Panics if out of range.
    pub fn range(&self, i: u32) -> Range<usize> {
        let i = i as usize;
        let start = self.starts[i];
        let end = match self.starts.get(i + 1) {
            Some(&next) => next - 1,
            None if self.trailing_newline => self.total - 1,
            None => self.total,
        };
        start..end
    }

    /// Line `i` (0-based) of `bytes`, without its `\n`. `bytes` must be the blob
    /// this index was built from.
    pub fn line<'a>(&self, bytes: &'a [u8], i: u32) -> &'a [u8] {
        &bytes[self.range(i)]
    }

    /// Whether the blob ends with `\n`. An empty blob counts as ending with one,
    /// so the "No newline at end of file" marker only follows a real last line.
    pub fn has_trailing_newline(&self) -> bool {
        self.trailing_newline
    }
}
