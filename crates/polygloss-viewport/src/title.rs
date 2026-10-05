//! A file header's title as styled runs (design §11.6 "File header"): the
//! path with its directory dim and its name bold, `old → new` for a rename,
//! cut from the left with `…` when it does not fit (the end of a path tells
//! files apart). Pure data: [`crate::header`] shapes and paints it.

use polygloss_diff::FileChange;

use crate::debug::TitleStyle;
use crate::text_cache::with_control_pictures;

/// A header title as runs: directories (and the rename arrow) dim, file
/// names bold (design §11.6). Control chars are Control Pictures.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Title {
    pub text: String,
    /// `(bytes, style)`, adjacent runs of one style merged.
    pub runs: Vec<(usize, TitleStyle)>,
}

impl Title {
    pub fn of(change: &FileChange) -> Title {
        let mut title = Title::default();
        match (&change.old_path, &change.new_path) {
            (Some(old), Some(new)) if old.text != new.text => {
                title.path(&old.text);
                title.push(" → ", TitleStyle::Dim);
                title.path(&new.text);
            }
            _ => title.path(change.display_path()),
        }
        title
    }

    fn path(&mut self, path: &str) {
        let path = with_control_pictures(path);
        match path.rfind('/') {
            Some(i) => {
                self.push(&path[..=i], TitleStyle::Dim);
                self.push(&path[i + 1..], TitleStyle::Bold);
            }
            None => self.push(&path, TitleStyle::Bold),
        }
    }

    fn push(&mut self, text: &str, style: TitleStyle) {
        if text.is_empty() {
            return;
        }
        self.text.push_str(text);
        match self.runs.last_mut() {
            Some((len, last)) if *last == style => *len += text.len(),
            _ => self.runs.push((text.len(), style)),
        }
    }

    /// `…` and the last `keep` chars (the end of a path tells files apart).
    pub fn cut(&self, keep: usize) -> Title {
        let total = self.text.chars().count();
        let from = self
            .text
            .char_indices()
            .nth(total.saturating_sub(keep))
            .map_or(self.text.len(), |(i, _)| i);
        let mut cut = Title::default();
        cut.push("…", TitleStyle::Dim);
        let mut at = 0;
        for &(len, style) in &self.runs {
            let (start, end) = (at.max(from), at + len);
            if start < end {
                cut.push(&self.text[start..end], style);
            }
            at = end;
        }
        cut
    }

    /// The runs as `(text, style)`.
    #[cfg_attr(not(feature = "debug-inspect"), allow(dead_code))]
    pub fn styled(&self) -> Vec<(String, TitleStyle)> {
        let mut at = 0;
        self.runs
            .iter()
            .map(|&(len, style)| {
                at += len;
                (self.text[at - len..at].to_owned(), style)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Title;
    use crate::debug::TitleStyle::{Bold, Dim};

    fn runs(t: &Title) -> Vec<(String, crate::debug::TitleStyle)> {
        t.styled()
    }

    #[test]
    fn cut_keeps_the_end_and_its_runs() {
        let mut t = Title::default();
        t.path("src/old.rs");
        t.push(" → ", Dim);
        t.path("lib/new.rs");
        assert_eq!(t.text, "src/old.rs → lib/new.rs");
        // Inside the old name (17 of 23 chars kept): its tail stays bold.
        let cut = t.cut(17);
        assert_eq!(cut.text, "…d.rs → lib/new.rs");
        assert_eq!(
            runs(&cut),
            [
                ("…".to_owned(), Dim),
                ("d.rs".to_owned(), Bold),
                (" → lib/".to_owned(), Dim),
                ("new.rs".to_owned(), Bold)
            ]
        );
        // Inside the new name; nothing kept.
        assert_eq!(
            runs(&t.cut(3)),
            [("…".to_owned(), Dim), (".rs".to_owned(), Bold)]
        );
        assert_eq!(runs(&t.cut(0)), [("…".to_owned(), Dim)]);
    }

    #[test]
    fn path_shows_control_chars_and_splits_at_the_last_slash() {
        let mut t = Title::default();
        t.path("a/b\tc/d\ne.rs");
        assert_eq!(t.text, "a/b␉c/d␊e.rs");
        assert_eq!(
            runs(&t),
            [("a/b␉c/".to_owned(), Dim), ("d␊e.rs".to_owned(), Bold)]
        );
    }
}
