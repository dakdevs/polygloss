//! Small helpers shared with the host: counts as the redesign prints them,
//! and which files show one side only (design §11.6, OQ-54).

use polygloss_diff::{FileChange, FileKind, Side};

/// `n` with its digits grouped by thousands with commas ("7,726",
/// "1,234,567"), never by the locale: counts look the same everywhere.
pub fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, d) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(d);
    }
    out
}

/// The side a one-sided file shows: `Some(New)` for an added text file,
/// `Some(Old)` for a deleted one, `None` for anything else (a modified or
/// renamed file, a binary, symlink or submodule). Such a file renders as one
/// full-width pane with one number column in both layouts.
pub fn one_sided(change: &FileChange) -> Option<Side> {
    if change.kind != FileKind::Text {
        return None;
    }
    match (&change.old_path, &change.new_path) {
        (None, Some(_)) => Some(Side::New),
        (Some(_), None) => Some(Side::Old),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::group_digits;

    #[test]
    fn group_digits_groups_by_thousands() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(group_digits(7726), "7,726");
        assert_eq!(group_digits(1234567), "1,234,567");
    }
}
