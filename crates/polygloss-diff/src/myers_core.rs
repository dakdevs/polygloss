// Derived from gix-imara-diff 0.3.0 (https://github.com/GitoxideLabs/gitoxide),
// files src/myers.rs, src/myers/middle_snake.rs, src/myers/slice.rs and
// src/util.rs, which gitoxide modified from the upstream imara-diff crate
// (https://github.com/pascalkuthe/imara-diff). Licensed under the Apache License,
// Version 2.0 (LICENSE-APACHE); see NOTICE.
//
// Changes from gix-imara-diff:
// - No preprocessing step: imara's `preprocess` module is not ported; callers
//   pass the lines git keeps (see `git_myers`), and the cost limit is sized from
//   exactly those lines.
// - Safe code: the raw-pointer k-vectors are two `Vec<i32>`s indexed by diagonal
//   plus an offset; a `FileSlice` is a token slice with its own slice of changed
//   flags instead of an index map into the whole file. `cov_mark` probes removed.
// - The top level never asks for a minimal diff (git's default, no `--minimal`).
// - Three fixes that make the heuristics agree with `git diff` (each checked
//   against the git binary): the edit cost counts from 1, not 0, so the cost
//   cutoff comes one d-step earlier; the good-snake score subtracts the distance
//   from the search's mid-diagonal instead of adding the distance from diagonal 0;
//   the forward good-snake check compares the tokens that end at the candidate
//   point instead of two prefixes aligned at their start.

//! The Myers core that git runs on its preprocessed lines (`--diff-algorithm=myers`).
//!
//! gix-imara-diff ports git's linear-space Myers with its cost heuristics, but
//! only behind its own preprocessing, which prunes lines git keeps and re-trims
//! the common ends before sizing the cost limit. [`diff`] is that core alone, so
//! [`crate::git_myers`] can feed it exactly the lines git keeps.

use gix_imara_diff::Token;

use crate::git_myers::bogosqrt;

/// Minimum edit cost before the good-snake heuristic may cut the search.
const HEUR_MIN_COST: u32 = 256;
/// Minimum value for the maximum cost threshold.
const MAX_COST_MIN: u32 = 256;
/// Minimum snake length to be considered a "good" snake for heuristics.
const SNAKE_CNT: u32 = 20;
/// Heuristic multiplier used to evaluate snake quality.
const K_HEUR: u32 = 4;

/// Marks the changed tokens of `before` in `removed` and of `after` in `added`
/// (each the length of its tokens; callers pass them all `false`). The cost limit
/// is `bogosqrt(before.len() + after.len() + 3)`, at least 256, as in git.
pub(crate) fn diff(before: &[Token], after: &[Token], removed: &mut [bool], added: &mut [bool]) {
    assert!(
        before.len() < i32::MAX as usize && after.len() < i32::MAX as usize,
        "the Myers core supports fewer than {} tokens per side",
        i32::MAX
    );
    assert_eq!((before.len(), after.len()), (removed.len(), added.len()));
    Myers::new(before.len(), after.len()).run(
        FileSlice {
            tokens: before,
            changed: removed,
        },
        FileSlice {
            tokens: after,
            changed: added,
        },
        false,
    );
}

/// The linear-space Myers state: one k-vector per search direction, shared by
/// every sub-box of the divide and conquer.
struct Myers {
    /// Forward furthest-reaching `x` per diagonal.
    kforward: Vec<i32>,
    /// Backward furthest-reaching `x` per diagonal.
    kbackward: Vec<i32>,
    /// Index of diagonal 0 in both k-vectors. Every sub-box's diagonals, with the
    /// sentinels one past each end, lie in `-(len2 + 1)..=len1 + 1`.
    origin: i32,
    /// Maximum edit cost before applying heuristics.
    max_cost: u32,
}

impl Myers {
    fn new(len1: usize, len2: usize) -> Self {
        let ndiags = len1 + len2 + 3;
        Self {
            kforward: vec![0; ndiags],
            kbackward: vec![0; ndiags],
            origin: len2 as i32 + 1,
            max_cost: u32::try_from(bogosqrt(ndiags))
                .unwrap_or(u32::MAX)
                .max(MAX_COST_MIN),
        }
    }

    fn run(&mut self, mut file1: FileSlice<'_>, mut file2: FileSlice<'_>, mut need_min: bool) {
        loop {
            (file1, file2) = strip_common(file1, file2);

            if file1.is_empty() {
                file2.changed.fill(true);
                return;
            } else if file2.is_empty() {
                file1.changed.fill(true);
                return;
            }

            let split = self.split(&file1, &file2, need_min);
            let (lo1, hi1) = file1.split_at(split.token_idx1 as usize);
            let (lo2, hi2) = file2.split_at(split.token_idx2 as usize);
            self.run(lo1, lo2, split.minimized_lo);

            file1 = hi1;
            file2 = hi2;
            need_min = split.minimized_hi;
        }
    }

    /// See "An O(ND) Difference Algorithm and its Variations", by Eugene Myers.
    /// Basically considers a "box" (off1, off2, lim1, lim2) and scan from both
    /// the forward diagonal starting from (off1, off2) and the backward diagonal
    /// starting from (lim1, lim2). If the K values on the same diagonal crosses
    /// returns the furthest point of reach. We might encounter expensive edge cases
    /// using this algorithm, so a little bit of heuristic is needed to cut the
    /// search and to return a suboptimal point.
    fn split(&mut self, file1: &FileSlice, file2: &FileSlice, need_min: bool) -> Split {
        let mut forward_search =
            MiddleSnakeSearch::<false>::new(&mut self.kforward, self.origin, file1, file2);
        let mut backwards_search =
            MiddleSnakeSearch::<true>::new(&mut self.kbackward, self.origin, file1, file2);
        let is_odd = file2.len().wrapping_sub(file1.len()) & 1 != 0;

        // git counts the first d-step as cost 1; imara counts it as 0 and so
        // searched one d-step more before the cost cutoff.
        let mut ec = 1;

        while ec <= self.max_cost {
            let mut found_snake = false;
            forward_search.next_d();
            if is_odd {
                if let Some(res) = forward_search.run(file1, file2, |k, token_idx1| {
                    backwards_search.contains(k)
                        && backwards_search.x_pos_at_diagonal(k) <= token_idx1
                }) {
                    match res {
                        SearchResult::Snake => found_snake = true,
                        SearchResult::Found {
                            token_idx1,
                            token_idx2,
                        } => {
                            #[cfg(test)]
                            tests::hit(tests::Exit::MiddleSnake);
                            return Split {
                                token_idx1,
                                token_idx2,
                                minimized_lo: true,
                                minimized_hi: true,
                            };
                        }
                    }
                }
            } else {
                found_snake |= forward_search.run(file1, file2, |_, _| false).is_some()
            };

            backwards_search.next_d();
            if !is_odd {
                if let Some(res) = backwards_search.run(file1, file2, |k, token_idx1| {
                    forward_search.contains(k) && token_idx1 <= forward_search.x_pos_at_diagonal(k)
                }) {
                    match res {
                        SearchResult::Snake => found_snake = true,
                        SearchResult::Found {
                            token_idx1,
                            token_idx2,
                        } => {
                            #[cfg(test)]
                            tests::hit(tests::Exit::MiddleSnake);
                            return Split {
                                token_idx1,
                                token_idx2,
                                minimized_lo: true,
                                minimized_hi: true,
                            };
                        }
                    }
                }
            } else {
                found_snake |= backwards_search.run(file1, file2, |_, _| false).is_some()
            };

            if need_min {
                ec += 1;
                continue;
            }

            // If the edit cost is above the heuristic trigger and if
            // we got a good snake, we sample current diagonals to see
            // if some of them have reached an "interesting" path. Our
            // measure is a function of the distance from the diagonal
            // corner (i1 + i2) penalized with the distance from the
            // mid-diagonal itself. If this value is above the current
            // edit cost times a magic factor (XDL_K_HEUR) we consider
            // it interesting.
            if found_snake && ec > HEUR_MIN_COST {
                if let Some((token_idx1, token_idx2)) = forward_search.found_snake(ec, file1, file2)
                {
                    #[cfg(test)]
                    tests::hit(tests::Exit::GoodSnake);
                    return Split {
                        token_idx1,
                        token_idx2,
                        minimized_lo: true,
                        minimized_hi: false,
                    };
                }

                if let Some((token_idx1, token_idx2)) =
                    backwards_search.found_snake(ec, file1, file2)
                {
                    #[cfg(test)]
                    tests::hit(tests::Exit::GoodSnake);
                    return Split {
                        token_idx1,
                        token_idx2,
                        minimized_lo: false,
                        minimized_hi: true,
                    };
                }
            }

            ec += 1;
        }

        #[cfg(test)]
        tests::hit(tests::Exit::CostCutoff);
        let (distance_forward, token_idx1_forward) = forward_search.best_position(file1, file2);
        let (distance_backwards, token_idx1_backwards) =
            backwards_search.best_position(file1, file2);
        if distance_forward > file1.len() as isize + file2.len() as isize - distance_backwards {
            Split {
                token_idx1: token_idx1_forward,
                token_idx2: (distance_forward - token_idx1_forward as isize) as i32,
                minimized_lo: true,
                minimized_hi: false,
            }
        } else {
            Split {
                token_idx1: token_idx1_backwards,
                token_idx2: (distance_backwards - token_idx1_backwards as isize) as i32,
                minimized_lo: false,
                minimized_hi: true,
            }
        }
    }
}

/// Represents a split point in the divide-and-conquer approach.
///
/// The split divides the problem into two subproblems at the given token positions.
#[derive(Debug)]
struct Split {
    /// Token index in the first sequence where the split occurs.
    token_idx1: i32,
    /// Token index in the second sequence where the split occurs.
    token_idx2: i32,
    /// Whether the lower subproblem was minimized.
    minimized_lo: bool,
    /// Whether the upper subproblem was minimized.
    minimized_hi: bool,
}

/// One side of a sub-box: its tokens and their changed flags.
struct FileSlice<'a> {
    tokens: &'a [Token],
    changed: &'a mut [bool],
}

impl<'a> FileSlice<'a> {
    fn len(&self) -> u32 {
        self.tokens.len() as u32
    }

    fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    fn split_at(self, mid: usize) -> (Self, Self) {
        let (tokens_lo, tokens_hi) = self.tokens.split_at(mid);
        let (changed_lo, changed_hi) = self.changed.split_at_mut(mid);
        (
            FileSlice {
                tokens: tokens_lo,
                changed: changed_lo,
            },
            FileSlice {
                tokens: tokens_hi,
                changed: changed_hi,
            },
        )
    }

    fn range(self, range: std::ops::Range<usize>) -> Self {
        FileSlice {
            tokens: &self.tokens[range.clone()],
            changed: &mut self.changed[range],
        }
    }
}

/// Both slices without their common prefix and then their common suffix.
fn strip_common<'a, 'b>(
    file1: FileSlice<'a>,
    file2: FileSlice<'b>,
) -> (FileSlice<'a>, FileSlice<'b>) {
    let prefix = common_prefix(file1.tokens, file2.tokens) as usize;
    let postfix = common_postfix(&file1.tokens[prefix..], &file2.tokens[prefix..]) as usize;
    let (end1, end2) = (file1.tokens.len() - postfix, file2.tokens.len() - postfix);
    (file1.range(prefix..end1), file2.range(prefix..end2))
}

/// Computes the number of common tokens at the start of two sequences.
fn common_prefix(file1: &[Token], file2: &[Token]) -> u32 {
    let mut off = 0;
    for (token1, token2) in file1.iter().zip(file2) {
        if token1 != token2 {
            break;
        }
        off += 1;
    }
    off
}

/// Computes the number of common tokens at the end of two sequences.
fn common_postfix(file1: &[Token], file2: &[Token]) -> u32 {
    let mut off = 0;
    for (token1, token2) in file1.iter().rev().zip(file2.iter().rev()) {
        if token1 != token2 {
            break;
        }
        off += 1;
    }
    off
}

/// Performs forward or backward search for the middle snake in Myers' algorithm.
///
/// The `BACK` const generic parameter determines the search direction:
/// `false` for forward search, `true` for backward search.
struct MiddleSnakeSearch<'k, const BACK: bool> {
    /// The k-vector for this search direction.
    kvec: &'k mut [i32],
    /// Index of diagonal 0 in `kvec`.
    origin: i32,
    /// Minimum k-diagonal currently being searched.
    kmin: i32,
    /// Maximum k-diagonal currently being searched.
    kmax: i32,
    /// Minimum possible k-diagonal value.
    dmin: i32,
    /// Maximum possible k-diagonal value.
    dmax: i32,
}

impl<'k, const BACK: bool> MiddleSnakeSearch<'k, BACK> {
    /// `kvec` must hold every diagonal between `-file2.len() - 1` and
    /// `file1.len() + 1` once offset by `origin`.
    fn new(kvec: &'k mut [i32], origin: i32, file1: &FileSlice, file2: &FileSlice) -> Self {
        let dmin = -(file2.len() as i32);
        let dmax = file1.len() as i32;
        let kmid = if BACK { dmin + dmax } else { 0 };
        let mut res = Self {
            kvec,
            origin,
            kmin: kmid,
            kmax: kmid,
            dmin,
            dmax,
        };
        let init = if BACK { file1.len() as i32 } else { 0 };
        res.write_xpos_at_diagonal(kmid, init);
        res
    }

    fn contains(&self, k: i32) -> bool {
        (self.kmin..=self.kmax).contains(&k)
    }

    fn index(&self, k: i32) -> usize {
        debug_assert!((self.dmin - 1..=self.dmax + 1).contains(&k));
        (self.origin + k) as usize
    }

    fn write_xpos_at_diagonal(&mut self, k: i32, token_idx1: i32) {
        let i = self.index(k);
        self.kvec[i] = token_idx1;
    }

    fn x_pos_at_diagonal(&self, diagonal: i32) -> i32 {
        self.kvec[self.index(diagonal)]
    }

    fn pos_at_diagonal(&self, diagonal: i32) -> (i32, i32) {
        let token_idx1 = self.x_pos_at_diagonal(diagonal);
        let token_idx2 = token_idx1 - diagonal;
        (token_idx1, token_idx2)
    }

    /// We need to extend the diagonal "domain" by one. If the next
    /// values exits the box boundaries we need to change it in the
    /// opposite direction because (max - min) must be a power of
    /// two.
    ///
    /// Also we initialize the external K value to -1 so that we can
    /// avoid extra conditions in the check inside the core loop.
    fn next_d(&mut self) {
        let init_val = if BACK {
            // value should always be larger then bounds
            i32::MAX
        } else {
            // value should always be smaller then bounds
            i32::MIN
        };

        if self.kmin > self.dmin {
            self.kmin -= 1;
            self.write_xpos_at_diagonal(self.kmin - 1, init_val);
        } else {
            self.kmin += 1;
        }

        if self.kmax < self.dmax {
            self.kmax += 1;
            self.write_xpos_at_diagonal(self.kmax + 1, init_val);
        } else {
            self.kmax -= 1;
        }
    }

    fn run(
        &mut self,
        file1: &FileSlice,
        file2: &FileSlice,
        mut f: impl FnMut(i32, i32) -> bool,
    ) -> Option<SearchResult> {
        let mut res = None;
        let mut k = self.kmax;
        while k >= self.kmin {
            let mut token_idx1 = if BACK {
                if self.x_pos_at_diagonal(k - 1) < self.x_pos_at_diagonal(k + 1) {
                    self.x_pos_at_diagonal(k - 1)
                } else {
                    self.x_pos_at_diagonal(k + 1) - 1
                }
            } else if self.x_pos_at_diagonal(k - 1) >= self.x_pos_at_diagonal(k + 1) {
                self.x_pos_at_diagonal(k - 1) + 1
            } else {
                self.x_pos_at_diagonal(k + 1)
            };

            let mut token_idx2 = token_idx1 - k;
            let off = if BACK {
                if token_idx1 > 0 && token_idx2 > 0 {
                    let tokens1 = &file1.tokens[..token_idx1 as usize];
                    let tokens2 = &file2.tokens[..token_idx2 as usize];
                    common_postfix(tokens1, tokens2)
                } else {
                    0
                }
            } else if token_idx1 < file1.len() as i32 && token_idx2 < file2.len() as i32 {
                let tokens1 = &file1.tokens[token_idx1 as usize..];
                let tokens2 = &file2.tokens[token_idx2 as usize..];
                common_prefix(tokens1, tokens2)
            } else {
                0
            };

            if off > SNAKE_CNT {
                res = Some(SearchResult::Snake)
            }

            if BACK {
                token_idx1 -= off as i32;
                token_idx2 -= off as i32;
            } else {
                token_idx1 += off as i32;
                token_idx2 += off as i32;
            }
            self.write_xpos_at_diagonal(k, token_idx1);

            if f(k, token_idx1) {
                return Some(SearchResult::Found {
                    token_idx1,
                    token_idx2,
                });
            }

            k -= 2;
        }

        res
    }

    fn best_position(&self, file1: &FileSlice, file2: &FileSlice) -> (isize, i32) {
        let mut best_distance: isize = if BACK { isize::MAX } else { -1 };
        let mut best_token_idx1 = if BACK { i32::MAX } else { -1 };
        let mut k = self.kmax;
        while k >= self.kmin {
            let mut token_idx1 = self.x_pos_at_diagonal(k);
            if BACK {
                token_idx1 = token_idx1.max(0);
            } else {
                token_idx1 = token_idx1.min(file1.len() as i32);
            }
            let mut token_idx2 = token_idx1 - k;
            if BACK {
                if token_idx2 < 0 {
                    token_idx1 = k;
                    token_idx2 = 0;
                }
            } else if token_idx2 > file2.len() as i32 {
                token_idx1 = file2.len() as i32 + k;
                token_idx2 = file2.len() as i32;
            }

            let distance = token_idx1 as isize + token_idx2 as isize;
            if BACK && distance < best_distance || !BACK && distance > best_distance {
                best_distance = distance;
                best_token_idx1 = token_idx1;
            }

            k -= 2;
        }
        (best_distance, best_token_idx1)
    }

    fn found_snake(&self, ec: u32, file1: &FileSlice, file2: &FileSlice) -> Option<(i32, i32)> {
        let mut best_score = 0;
        let mut best_token_idx1 = 0;
        let mut best_token_idx2 = 0;
        let mut k = self.kmax;
        while k >= self.kmin {
            let (token_idx1, token_idx2) = self.pos_at_diagonal(k);
            if BACK {
                if !(0..file1.len() as i32 - SNAKE_CNT as i32).contains(&token_idx1) {
                    k -= 2;
                    continue;
                }
                if !(0..file2.len() as i32 - SNAKE_CNT as i32).contains(&token_idx2) {
                    k -= 2;
                    continue;
                }
            } else {
                if !(SNAKE_CNT as i32..file1.len() as i32).contains(&token_idx1) {
                    k -= 2;
                    continue;
                }
                if !(SNAKE_CNT as i32..file2.len() as i32).contains(&token_idx2) {
                    k -= 2;
                    continue;
                }
            }

            // The distance from the search's corner, penalized with the distance
            // from its mid-diagonal (imara added the distance from diagonal 0).
            let kmid = if BACK { self.dmin + self.dmax } else { 0 };
            let distance = if BACK {
                (file1.len() - token_idx1 as u32) + (file2.len() - token_idx2 as u32)
            } else {
                token_idx1 as u32 + token_idx2 as u32
            };
            let score = i64::from(distance) - i64::from((k - kmid).unsigned_abs());
            if score > i64::from(K_HEUR * ec) && score > best_score {
                let is_snake = if BACK {
                    file1.tokens[token_idx1 as usize..]
                        .iter()
                        .zip(&file2.tokens[token_idx2 as usize..])
                        .take(SNAKE_CNT as usize)
                        .all(|(token1, token2)| token1 == token2)
                } else {
                    // The snake ending at (token_idx1, token_idx2). imara zipped
                    // before reversing, which aligns both prefixes at their start.
                    file1.tokens[..token_idx1 as usize]
                        .iter()
                        .rev()
                        .zip(file2.tokens[..token_idx2 as usize].iter().rev())
                        .take(SNAKE_CNT as usize)
                        .all(|(token1, token2)| token1 == token2)
                };
                if is_snake {
                    best_token_idx1 = token_idx1;
                    best_token_idx2 = token_idx2;
                    best_score = score
                }
            }

            k -= 2;
        }

        (best_score > 0).then_some((best_token_idx1, best_token_idx2))
    }
}

/// The result of a middle snake search iteration.
#[derive(Debug)]
enum SearchResult {
    /// A good snake was found but not necessarily the middle snake.
    Snake,
    /// The middle snake was found at the specified token positions.
    Found {
        /// Token index in the first sequence.
        token_idx1: i32,
        /// Token index in the second sequence.
        token_idx2: i32,
    },
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// How a `split` ended.
    #[derive(Clone, Copy)]
    pub(super) enum Exit {
        MiddleSnake,
        GoodSnake,
        CostCutoff,
    }

    thread_local! {
        /// `split` exits on this thread, by `Exit`.
        static EXITS: Cell<[usize; 3]> = const { Cell::new([0; 3]) };
    }

    pub(super) fn hit(exit: Exit) {
        EXITS.with(|exits| {
            let mut counts = exits.get();
            counts[exit as usize] += 1;
            exits.set(counts);
        });
    }

    /// The changed flags of both sides and how many splits ended each way.
    fn run(before: &[Token], after: &[Token]) -> (Vec<bool>, Vec<bool>, [usize; 3]) {
        EXITS.with(|exits| exits.set([0; 3]));
        let mut removed = vec![false; before.len()];
        let mut added = vec![false; after.len()];
        diff(before, after, &mut removed, &mut added);
        (removed, added, EXITS.with(Cell::get))
    }

    fn toks(ids: &[u32]) -> Vec<Token> {
        ids.iter().map(|&i| Token(i)).collect()
    }

    /// xorshift64: the next pseudo-random number below `n`.
    fn below(state: &mut u64, n: usize) -> usize {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        (*state % n as u64) as usize
    }

    /// `len` tokens drawn from `alphabet` values.
    fn random_tokens(state: &mut u64, len: usize, alphabet: u32) -> Vec<Token> {
        (0..len)
            .map(|_| Token(below(state, alphabet as usize) as u32))
            .collect()
    }

    /// The number of changed tokens, after checking that the flags are an edit
    /// script: the unchanged tokens of both sides are the same sequence.
    fn edits(before: &[Token], after: &[Token], removed: &[bool], added: &[bool]) -> usize {
        let unchanged = |tokens: &[Token], changed: &[bool]| -> Vec<Token> {
            tokens
                .iter()
                .zip(changed)
                .filter(|(_, changed)| !**changed)
                .map(|(token, _)| *token)
                .collect()
        };
        assert_eq!(
            unchanged(before, removed),
            unchanged(after, added),
            "the unchanged tokens of both sides differ"
        );
        removed
            .iter()
            .chain(added)
            .filter(|&&changed| changed)
            .count()
    }

    /// The fewest changed tokens of any edit script: `len(a) + len(b) - 2 * LCS`.
    fn min_edits(a: &[Token], b: &[Token]) -> usize {
        let mut prev = vec![0; b.len() + 1];
        for x in a {
            let mut row = vec![0; b.len() + 1];
            for (j, y) in b.iter().enumerate() {
                row[j + 1] = if x == y {
                    prev[j] + 1
                } else {
                    prev[j + 1].max(row[j])
                };
            }
            prev = row;
        }
        a.len() + b.len() - 2 * prev[b.len()]
    }

    #[test]
    fn empty_sides_change_the_other_side_entirely() {
        let flags = |before: &[Token], after: &[Token]| {
            let (removed, added, _) = run(before, after);
            (removed, added)
        };
        let ab = toks(&[1, 2]);
        assert_eq!(flags(&[], &[]), (vec![], vec![]));
        assert_eq!(flags(&[], &ab), (vec![], vec![true, true]));
        assert_eq!(flags(&ab, &[]), (vec![true, true], vec![]));
    }

    #[test]
    fn identical_sides_have_no_changes() {
        let tokens = toks(&[1, 2, 3, 2, 1]);
        let (removed, added, _) = run(&tokens, &tokens);
        assert_eq!((removed, added), (vec![false; 5], vec![false; 5]));
    }

    #[test]
    fn disjoint_sides_change_everything() {
        let (removed, added, _) = run(&toks(&[1, 2, 3]), &toks(&[4, 5]));
        assert_eq!((removed, added), (vec![true; 3], vec![true; 2]));
    }

    #[test]
    fn below_the_cost_limit_the_script_is_minimal() {
        // At most 280 edits, so the two searches meet within 140 d-steps, below
        // the 256-step cost limit: only middle snakes split, and Myers is minimal.
        let mut state = 1;
        for len in [1, 2, 5, 20, 60, 120] {
            for alphabet in [2, 4, 16] {
                let a = random_tokens(&mut state, len, alphabet);
                let b = random_tokens(&mut state, len + len / 3, alphabet);
                let (removed, added, exits) = run(&a, &b);
                let what = format!("len {len}, alphabet {alphabet}");
                assert_eq!(edits(&a, &b, &removed, &added), min_edits(&a, &b), "{what}");
                assert_eq!(exits[Exit::GoodSnake as usize], 0, "{what}");
                assert_eq!(exits[Exit::CostCutoff as usize], 0, "{what}");
            }
        }
    }

    #[test]
    fn past_the_cost_limit_the_cutoff_splits_and_the_script_stays_valid() {
        // Random sides of 1,500 tokens from 4 values need about 1,000 edits, far
        // past the 256 d-steps the searches may take; the cutoff then splits at
        // the furthest point reached, which is valid but not minimal.
        let mut state = 7;
        let a = random_tokens(&mut state, 1500, 4);
        let b = random_tokens(&mut state, 1500, 4);
        let (removed, added, exits) = run(&a, &b);
        assert!(exits[Exit::CostCutoff as usize] > 0, "{exits:?}");
        assert!(edits(&a, &b, &removed, &added) > min_edits(&a, &b));
    }

    #[test]
    fn past_the_heuristic_minimum_good_snakes_split_and_the_script_stays_valid() {
        // 66,000 tokens in total raise the cost limit to 512 d-steps, so a split
        // may take a good snake (> 20 equal tokens) once past 256. Shuffled runs
        // of 25 distinct tokens give it plenty.
        let mut state = 3;
        let runs: Vec<Vec<Token>> = (0..1320u32)
            .map(|r| (0..25).map(|i| Token(r * 25 + i)).collect())
            .collect();
        let n = runs.len();
        let mut order: Vec<usize> = (0..n).collect();
        for _ in 0..200 {
            let i = below(&mut state, n);
            order.swap(i, (i + 1 + below(&mut state, 37)).min(n - 1));
        }
        let a: Vec<Token> = runs.concat();
        let b: Vec<Token> = order
            .iter()
            .flat_map(|&r| runs[r].iter().copied())
            .collect();
        let (removed, added, exits) = run(&a, &b);
        assert!(exits[Exit::GoodSnake as usize] > 0, "{exits:?}");
        edits(&a, &b, &removed, &added);
    }
}
