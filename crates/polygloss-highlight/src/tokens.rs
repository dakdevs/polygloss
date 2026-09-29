//! Compact per-line highlight spans (design §12.4 "Syntax memory": tree-sitter
//! trees are dropped after tokenizing; only `u32` offsets and a style id stay).

/// Index into a [`crate::SyntaxTheme`]'s style table. [`StyleId::DEFAULT`] (0)
/// is the unstyled foreground.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct StyleId(pub u16);

impl StyleId {
    /// No syntax style: the editor foreground.
    pub const DEFAULT: StyleId = StyleId(0);
}

/// One styled byte range within a line: `start..start + len`, offsets relative
/// to the line start. 12 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: u32,
    pub len: u32,
    pub style: StyleId,
}

/// Highlight result for one blob: for every line, its styled spans.
///
/// Lines follow `polygloss_diff::lines::LineIndex`: 0-based, ending at `\n`
/// (not part of the line; a `\r` before it is content), a final line without
/// `\n` counts. A line's spans are sorted, disjoint, non-empty, inside the line
/// and on UTF-8 char boundaries. Only styled text gets spans: bytes no span
/// covers use [`StyleId::DEFAULT`]. A token that crosses lines (a block
/// comment, a multi-line string) is split into one span per line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tokens {
    /// `line_starts[i]..line_starts[i + 1]` indexes line `i`'s spans; the
    /// length is `line_count + 1` (empty for an empty `Tokens`).
    line_starts: Vec<u32>,
    spans: Vec<Span>,
}

impl Tokens {
    /// Spans of line `i` (0-based). Empty for a line without styled text and
    /// for lines past the end.
    pub fn line(&self, i: u32) -> &[Span] {
        let i = i as usize;
        match (
            self.line_starts.get(i),
            i.checked_add(1).and_then(|j| self.line_starts.get(j)),
        ) {
            (Some(&a), Some(&b)) => &self.spans[a as usize..b as usize],
            _ => &[],
        }
    }

    /// Number of lines of the highlighted source.
    pub fn line_count(&self) -> u32 {
        self.line_starts.len().saturating_sub(1) as u32
    }

    /// Heap memory held, for byte-capped caches.
    pub fn heap_bytes(&self) -> usize {
        self.line_starts.capacity() * std::mem::size_of::<u32>()
            + self.spans.capacity() * std::mem::size_of::<Span>()
    }
}

/// Appends spans in source order and builds [`Tokens`].
pub(crate) struct TokensBuilder {
    line_starts: Vec<u32>,
    spans: Vec<Span>,
}

impl TokensBuilder {
    pub(crate) fn with_capacity(lines: usize, spans: usize) -> TokensBuilder {
        TokensBuilder {
            line_starts: Vec::with_capacity(lines + 1),
            spans: Vec::with_capacity(spans),
        }
    }

    /// Adds `start..start + len` of `line` (relative to the line start). Calls
    /// come in source order; a span that continues the previous one of the same
    /// line and style extends it.
    pub(crate) fn push(&mut self, line: u32, start: u32, len: u32, style: StyleId) {
        let line = line as usize;
        let opened = self.line_starts.len() == line + 1;
        if opened
            && self.spans.len() > self.line_starts[line] as usize
            && let Some(last) = self.spans.last_mut()
            && last.style == style
            && last.start + last.len == start
        {
            last.len += len;
            return;
        }
        while self.line_starts.len() <= line {
            self.line_starts.push(self.spans.len() as u32);
        }
        self.spans.push(Span { start, len, style });
    }

    pub(crate) fn finish(mut self, line_count: u32) -> Tokens {
        while self.line_starts.len() <= line_count as usize {
            self.line_starts.push(self.spans.len() as u32);
        }
        self.line_starts.shrink_to_fit();
        self.spans.shrink_to_fit();
        Tokens {
            line_starts: self.line_starts,
            spans: self.spans,
        }
    }
}
