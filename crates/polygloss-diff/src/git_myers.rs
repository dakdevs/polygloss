//! Myers with git's own preprocessing (`xdl_optimize_ctxs` in xdiff's `xprepare.c`).
//!
//! Before Myers, git trims the common prefix and suffix (`xdl_trim_ends`) and then
//! discards lines that cannot or should not match (`xdl_cleanup_records`): lines
//! absent from the other file, and "multimatch" lines (frequent ones, such as blank
//! lines and lone braces) that sit inside a run of unmatched lines
//! (`xdl_clean_mmatch`). The frequency limit is `xdl_bogosqrt` of the **whole**
//! file's line count and occurrences are counted over the **whole** other file.
//!
//! gix-imara-diff ports this step but applies it after its own prefix/suffix
//! trimming, so both the limit and the counts come from the trimmed middle only. A
//! blank line inside a rewritten block is then matched by imara but discarded by
//! git, which splits one change into two (T1.16 found this with the parity repos).
//! imara's Myers also repeats the step on whatever it is given, pruning lines git
//! keeps (S14). [`myers`] therefore runs git's step on the full token sequences
//! and then [`myers_core`] (imara's Myers core without that step) on exactly the
//! lines git keeps.

use gix_imara_diff::Token;

use crate::myers_core;

/// `XDL_MAX_EQLIMIT`: cap on the multimatch frequency limit.
const MAX_EQLIMIT: usize = 1024;
/// `XDL_SIMSCAN_WINDOW`: how far `xdl_clean_mmatch` looks either way.
const SIMSCAN_WINDOW: usize = 100;
/// `XDL_KPDIS_RUN`.
const KPDIS_RUN: usize = 4;

/// How a line of one file occurs in the other (`dis` in `xdl_cleanup_records`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Occurs {
    /// Not in the other file: always discarded.
    Never,
    /// In the other file fewer times than the limit.
    Some,
    /// In the other file at least as often as the limit (multimatch).
    Often,
}

/// git's `xdl_bogosqrt`: doubles once per two bits of `n`.
pub(crate) fn bogosqrt(mut n: usize) -> usize {
    let mut i = 1;
    while n > 0 {
        i <<= 1;
        n >>= 2;
    }
    i
}

/// Diffs two token sequences the way `git diff --diff-algorithm=myers` does
/// before its slider post-processing: the removed and added flags of every
/// line. `num_tokens` bounds every token id.
pub(crate) fn myers(before: &[Token], after: &[Token], num_tokens: u32) -> (Vec<bool>, Vec<bool>) {
    let mut removed = vec![false; before.len()];
    let mut added = vec![false; after.len()];

    // xdl_trim_ends
    let prefix = before.iter().zip(after).take_while(|(a, b)| a == b).count();
    let limit = before.len().min(after.len()) - prefix;
    let suffix = before
        .iter()
        .rev()
        .zip(after.iter().rev())
        .take(limit)
        .take_while(|(a, b)| a == b)
        .count();
    let region1 = prefix..before.len() - suffix;
    let region2 = prefix..after.len() - suffix;

    // xdl_cleanup_records: counts over the whole files.
    let mut count1 = vec![0usize; num_tokens as usize];
    let mut count2 = vec![0usize; num_tokens as usize];
    for t in before {
        count1[t.0 as usize] += 1;
    }
    for t in after {
        count2[t.0 as usize] += 1;
    }
    let (keep1, keep2) = (
        cleanup(before, region1, &count2, &mut removed),
        cleanup(after, region2, &count1, &mut added),
    );

    // Myers over exactly the kept lines.
    let tokens1: Vec<Token> = keep1.iter().map(|&i| before[i]).collect();
    let tokens2: Vec<Token> = keep2.iter().map(|&i| after[i]).collect();
    let mut removed_kept = vec![false; tokens1.len()];
    let mut added_kept = vec![false; tokens2.len()];
    myers_core::diff(&tokens1, &tokens2, &mut removed_kept, &mut added_kept);
    for (&i, &changed) in keep1.iter().zip(&removed_kept) {
        removed[i] = changed;
    }
    for (&i, &changed) in keep2.iter().zip(&added_kept) {
        added[i] = changed;
    }
    (removed, added)
}

/// Marks the lines of `region` git discards in `changed` and returns the indexes
/// of the lines it keeps. `other_counts` are the other file's token counts.
fn cleanup(
    tokens: &[Token],
    region: std::ops::Range<usize>,
    other_counts: &[usize],
    changed: &mut [bool],
) -> Vec<usize> {
    let limit = bogosqrt(tokens.len()).min(MAX_EQLIMIT);
    let dis: Vec<Occurs> = tokens[region.clone()]
        .iter()
        .map(|t| match other_counts[t.0 as usize] {
            0 => Occurs::Never,
            n if n >= limit => Occurs::Often,
            _ => Occurs::Some,
        })
        .collect();
    let mut keep = Vec::with_capacity(dis.len());
    for (j, d) in dis.iter().enumerate() {
        let kept = match d {
            Occurs::Some => true,
            Occurs::Often => !clean_mmatch(&dis, j),
            Occurs::Never => false,
        };
        if kept {
            keep.push(region.start + j);
        } else {
            changed[region.start + j] = true;
        }
    }
    keep
}

/// git's `xdl_clean_mmatch`: whether the multimatch line `i` is discarded because
/// the runs of unmatched and multimatch lines around it are mostly unmatched. The
/// line itself counts once on each side.
fn clean_mmatch(dis: &[Occurs], i: usize) -> bool {
    let start = i.saturating_sub(SIMSCAN_WINDOW);
    let end = (i + SIMSCAN_WINDOW).min(dis.len() - 1);
    let run = |lines: &mut dyn Iterator<Item = &Occurs>| {
        let (mut never, mut often) = (0, 1);
        for d in lines {
            match d {
                Occurs::Never => never += 1,
                Occurs::Often => often += 1,
                Occurs::Some => break,
            }
        }
        (never, often)
    };
    let (never_before, often_before) = run(&mut dis[start..i].iter().rev());
    if never_before == 0 {
        return false;
    }
    let (never_after, often_after) = run(&mut dis[i + 1..=end].iter());
    if never_after == 0 {
        return false;
    }
    let never = never_before + never_after;
    let often = often_before + often_after;
    often * KPDIS_RUN < never + often
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(ids: &[u32]) -> Vec<Token> {
        ids.iter().map(|&i| Token(i)).collect()
    }

    #[test]
    fn bogosqrt_matches_git() {
        let cases = [
            (0, 1),
            (1, 2),
            (3, 2),
            (4, 4),
            (15, 4),
            (16, 8),
            (40, 8),
            (150, 16),
        ];
        for (n, want) in cases {
            assert_eq!(bogosqrt(n), want, "bogosqrt({n})");
        }
    }

    #[test]
    fn clean_mmatch_needs_more_than_three_unmatched_per_multimatch() {
        use Occurs::*;
        // The candidate counts twice: 6 unmatched lines keep it, 7 discard it.
        let around = |before: usize, after: usize| {
            let mut dis = vec![Some; 30];
            dis[10 - before..10].fill(Never);
            dis[10] = Often;
            dis[11..11 + after].fill(Never);
            clean_mmatch(&dis, 10)
        };
        assert!(!around(3, 3));
        assert!(around(3, 4));
        assert!(!around(0, 9), "needs unmatched lines on both sides");
        assert!(!around(9, 0), "needs unmatched lines on both sides");
    }

    #[test]
    fn blank_line_inside_a_rewrite_is_discarded_by_whole_file_frequency() {
        // Token 0 is a blank line, frequent in both whole files, but only once in
        // the trimmed middle. Tokens 1..=4 are shared context; 10.. and 20.. differ.
        let mut before = Vec::new();
        let mut after = Vec::new();
        for _ in 0..8 {
            before.extend([1, 0]);
            after.extend([1, 0]);
        }
        before.extend([10, 11, 12, 13, 0, 14, 15, 16]);
        after.extend([20, 21, 22, 23, 0, 24, 25, 26]);
        for _ in 0..4 {
            before.extend([0, 2]);
            after.extend([0, 2]);
        }
        let (before, after) = (toks(&before), toks(&after));
        let (removed, added) = myers(&before, &after, 27);
        let changed = |f: &[bool]| (0..f.len()).filter(|&i| f[i]).collect::<Vec<_>>();
        assert_eq!(changed(&removed), (16..24).collect::<Vec<_>>());
        assert_eq!(changed(&added), (16..24).collect::<Vec<_>>());
    }
}
