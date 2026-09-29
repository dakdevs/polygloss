//! Whitespace mode: lines compared with all whitespace removed, like git `-w`
//! (T1.6, design §6.3). Only the comparison changes; line numbers and the
//! blobs shown stay the real ones, so anchors do not move.

/// git's `XDL_ISSPACE` under `LC_ALL=C`: space, `\t`, `\n`, `\v`, `\f`, `\r`.
pub fn is_git_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// The comparison key of a line in whitespace mode: its bytes without any
/// whitespace.
pub fn strip_whitespace(line: &[u8]) -> Vec<u8> {
    line.iter().copied().filter(|&b| !is_git_space(b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_whitespace_removes_every_git_space() {
        assert_eq!(strip_whitespace(b" \ta b\x0b\x0cc\r\n"), b"abc");
        assert_eq!(strip_whitespace(b"   \n"), b"");
        // Non-ASCII bytes are content (git's C-locale isspace).
        assert_eq!(strip_whitespace("\u{a0}x".as_bytes()), "\u{a0}x".as_bytes());
    }
}
