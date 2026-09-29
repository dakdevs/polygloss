//! Fuzzy ranking for the open flow's lists with `nucleo-matcher` (design
//! §11.3, library-choices "Fuzzy"): gpui-kit's own list filters are
//! substring-only.
//!
//! [`Ranker`] keeps one `Matcher` (its scratch memory is reused between
//! keystrokes). Paths use `Config::DEFAULT.match_paths()`, so a query like
//! `src/pgl` prefers matches at path segment starts; other lists (commits,
//! refs) use the default config. Smart case and smart normalization: an
//! upper-case letter or an accent in the query makes that character exact.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// A reusable fuzzy ranker.
pub struct Ranker {
    matcher: Matcher,
}

impl Ranker {
    /// A ranker for file-system paths.
    pub fn paths() -> Ranker {
        Ranker {
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
        }
    }

    /// A ranker for plain text (subjects, names, ids).
    pub fn text() -> Ranker {
        Ranker {
            matcher: Matcher::new(Config::DEFAULT),
        }
    }

    /// The indices of the `haystacks` that match `query`, best first; equal
    /// scores keep their input order. An empty (or blank) query matches
    /// everything in input order.
    pub fn rank<S: AsRef<str>>(&mut self, query: &str, haystacks: &[S]) -> Vec<usize> {
        if query.trim().is_empty() {
            return (0..haystacks.len()).collect();
        }
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let mut buf = Vec::new();
        let mut scored: Vec<(u32, usize)> = haystacks
            .iter()
            .enumerate()
            .filter_map(|(ix, h)| {
                let hay = Utf32Str::new(h.as_ref(), &mut buf);
                pattern.score(hay, &mut self.matcher).map(|s| (s, ix))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().map(|(_, ix)| ix).collect()
    }
}

/// [`Ranker::rank`] with a one-off path ranker.
pub fn rank_paths<S: AsRef<str>>(query: &str, paths: &[S]) -> Vec<usize> {
    Ranker::paths().rank(query, paths)
}

/// [`Ranker::rank`] with a one-off text ranker.
pub fn rank_text<S: AsRef<str>>(query: &str, items: &[S]) -> Vec<usize> {
    Ranker::text().rank(query, items)
}
