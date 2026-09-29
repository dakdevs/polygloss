//! Hunk computation, context grouping and whitespace mode (T1.6, design §6.3).

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use polygloss_diff::hunks::{Block, FileDiff, Hunk, diff_blobs};
use polygloss_diff::lines::LineIndex;
use polygloss_diff::options::{Algorithm, DiffOptions};
use polygloss_diff::unified_text::unified_text;

fn fixture(case: &str) -> (Vec<u8>, Vec<u8>) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/parity")
        .join(case);
    (
        fs::read(dir.join("old")).expect("read old"),
        fs::read(dir.join("new")).expect("read new"),
    )
}

/// `n` distinct lines `line 0\n` .. `line n-1\n`.
fn numbered(n: u32) -> Vec<String> {
    (0..n).map(|i| format!("line {i}\n")).collect()
}

fn join(lines: &[String]) -> Vec<u8> {
    lines.concat().into_bytes()
}

fn equal(old: std::ops::Range<u32>, new: std::ops::Range<u32>) -> Block {
    Block::Equal { old, new }
}

fn change(old: std::ops::Range<u32>, new: std::ops::Range<u32>) -> Block {
    Block::Change { old, new }
}

fn whitespace_opts() -> DiffOptions {
    DiffOptions {
        ignore_whitespace: true,
        ..DiffOptions::default()
    }
}

#[test]
fn diff_options_default_is_myers_context_3_inter_hunk_1() {
    let opts = DiffOptions::default();
    assert_eq!(opts.algorithm, Algorithm::Myers);
    assert!(!opts.ignore_whitespace);
    assert_eq!(opts.context, 3);
    assert_eq!(opts.inter_hunk_context, 1);
}

#[test]
fn hunks_context_merge_gap_7() {
    let old = numbered(40);

    // Changes at lines 5 and 13: 7 unchanged lines between them -> one hunk.
    let mut new = old.clone();
    new[5] = "changed 5\n".into();
    new[13] = "changed 13\n".into();
    let fd = diff_blobs(&join(&old), &join(&new), &DiffOptions::default());
    assert_eq!(
        fd.hunks,
        vec![Hunk {
            old: 2..17,
            new: 2..17,
            blocks: vec![
                equal(2..5, 2..5),
                change(5..6, 5..6),
                equal(6..13, 6..13),
                change(13..14, 13..14),
                equal(14..17, 14..17),
            ],
        }]
    );

    // Changes at lines 5 and 14: 8 unchanged lines between them -> two hunks.
    let mut new = old.clone();
    new[5] = "changed 5\n".into();
    new[14] = "changed 14\n".into();
    let fd = diff_blobs(&join(&old), &join(&new), &DiffOptions::default());
    assert_eq!(
        fd.hunks,
        vec![
            Hunk {
                old: 2..9,
                new: 2..9,
                blocks: vec![equal(2..5, 2..5), change(5..6, 5..6), equal(6..9, 6..9)],
            },
            Hunk {
                old: 11..18,
                new: 11..18,
                blocks: vec![
                    equal(11..14, 11..14),
                    change(14..15, 14..15),
                    equal(15..18, 15..18),
                ],
            },
        ]
    );
}

#[test]
fn hunks_context_options_are_honored() {
    let old = numbered(30);
    let edit = |lines: &[usize]| {
        let mut new = old.clone();
        for &i in lines {
            new[i] = format!("changed {i}\n");
        }
        join(&new)
    };
    // context 1, inter-hunk 0: merge only when the gap is <= 2 (git -U1
    // --inter-hunk-context=0 prints `@@ -10,6` for 10+13 and two hunks for 10+14).
    let opts = DiffOptions {
        context: 1,
        inter_hunk_context: 0,
        ..DiffOptions::default()
    };
    let ranges = |new: &[u8]| -> Vec<_> {
        diff_blobs(&join(&old), new, &opts)
            .hunks
            .iter()
            .map(|h| (h.old.clone(), h.new.clone()))
            .collect()
    };
    assert_eq!(ranges(&edit(&[10, 13])), vec![(9..15, 9..15)]);
    assert_eq!(
        ranges(&edit(&[10, 14])),
        vec![(9..12, 9..12), (13..16, 13..16)]
    );
    let new = edit(&[10, 13]);

    // context 0 keeps only the changed lines.
    let opts = DiffOptions {
        context: 0,
        inter_hunk_context: 0,
        ..DiffOptions::default()
    };
    let fd = diff_blobs(&join(&old), &new, &opts);
    let ranges: Vec<_> = fd
        .hunks
        .iter()
        .map(|h| (h.old.clone(), h.new.clone()))
        .collect();
    assert_eq!(ranges, vec![(10..11, 10..11), (13..14, 13..14)]);
}

#[test]
fn hunks_context_clamps_at_file_edges() {
    let old = numbered(4);
    let mut new = old.clone();
    new[0] = "first\n".into();
    new[3] = "last\n".into();
    let fd = diff_blobs(&join(&old), &join(&new), &DiffOptions::default());
    assert_eq!(
        fd.hunks,
        vec![Hunk {
            old: 0..4,
            new: 0..4,
            blocks: vec![change(0..1, 0..1), equal(1..3, 1..3), change(3..4, 3..4)],
        }]
    );
}

#[test]
fn hunks_crlf_and_no_trailing_newline() {
    // CR is content: a CRLF -> LF line ending is a change; the other lines keep their CR.
    let old = b"alpha\r\nbeta\r\ngamma\r\n";
    let new = b"alpha\r\nbeta\ngamma\r\n";
    let fd = diff_blobs(old, new, &DiffOptions::default());
    assert_eq!(
        fd.hunks,
        vec![Hunk {
            old: 0..3,
            new: 0..3,
            blocks: vec![equal(0..1, 0..1), change(1..2, 1..2), equal(2..3, 2..3)],
        }]
    );
    assert_eq!(fd.old.line(old, 1), b"beta\r");
    assert_eq!(fd.new.line(new, 1), b"beta");
    assert_eq!((fd.additions, fd.deletions), (1, 1));
    assert_eq!(
        unified_text(&fd, old, new),
        "@@ -1,3 +1,3 @@\n alpha\r\n-beta\r\n+beta\n gamma\r\n"
    );

    // Adding the missing final newline changes the last line.
    let old = b"one\ntwo";
    let new = b"one\ntwo\n";
    let fd = diff_blobs(old, new, &DiffOptions::default());
    assert!(!fd.old.has_trailing_newline());
    assert!(fd.new.has_trailing_newline());
    assert_eq!(fd.old.len(), 2);
    assert_eq!(fd.new.len(), 2);
    assert_eq!(
        unified_text(&fd, old, new),
        "@@ -1,2 +1,2 @@\n one\n-two\n\\ No newline at end of file\n+two\n"
    );

    // An unchanged last line without a newline is context followed by the marker.
    let old = b"one\ntwo";
    let new = b"ONE\ntwo";
    let fd = diff_blobs(old, new, &DiffOptions::default());
    assert_eq!(
        unified_text(&fd, old, new),
        "@@ -1,2 +1,2 @@\n-one\n+ONE\n two\n\\ No newline at end of file\n"
    );
}

#[test]
fn hunks_ignore_whitespace_mode() {
    let (old, new) = fixture("whitespace-only");
    let fd = diff_blobs(&old, &new, &whitespace_opts());
    // Only `return total` -> `return total + 1` (line 5) is a real change.
    assert_eq!((fd.additions, fd.deletions), (1, 1));
    assert_eq!(fd.hunks.len(), 1);
    let blocks = &fd.hunks[0].blocks;
    let changes: Vec<_> = blocks
        .iter()
        .filter(|b| matches!(b, Block::Change { .. }))
        .collect();
    assert_eq!(changes, vec![&change(5..6, 5..6)]);

    // Without the mode, the re-indented lines are changes too.
    let fd = diff_blobs(&old, &new, &DiffOptions::default());
    assert_eq!((fd.additions, fd.deletions), (6, 6));

    // Line endings and blank-line padding alone produce no hunks.
    let old = b"a b\r\n\r\nc\r\n";
    let new = b"a  b\n   \nc\n";
    let fd = diff_blobs(old, new, &whitespace_opts());
    assert!(fd.hunks.is_empty());
    assert_eq!((fd.additions, fd.deletions), (0, 0));
    // Line indexes still describe the real blobs (anchors do not change, §6.3).
    assert_eq!(fd.old.line(old, 0), b"a b\r");
    assert_eq!(fd.new.line(new, 0), b"a  b");
}

/// Myers and Histogram disagree on this move (the classic Chunk_copy example).
const CHUNK_OLD: &str =
    "void Chunk_copy(Chunk *src, size_t src_start, Chunk *dst, size_t dst_start, size_t n)
{
    if (!Chunk_bounds_check(src, src_start, n)) return;
    if (!Chunk_bounds_check(dst, dst_start, n)) return;

    memcpy(dst->data + dst_start, src->data + src_start, n);
}

int Chunk_bounds_check(Chunk *chk, size_t start, size_t n)
{
    if (chk == NULL) return 0;

    return start <= chk->length && n <= chk->length - start;
}
";

const CHUNK_NEW: &str = "int Chunk_bounds_check(Chunk *chk, size_t start, size_t n)
{
    if (chk == NULL) return 0;

    return start <= chk->length && n <= chk->length - start;
}

void Chunk_copy(Chunk *src, size_t src_start, Chunk *dst, size_t dst_start, size_t n)
{
    if (!Chunk_bounds_check(src, src_start, n)) return;
    if (!Chunk_bounds_check(dst, dst_start, n)) return;

    memcpy(dst->data + dst_start, src->data + src_start, n);
}
";

#[test]
fn hunks_histogram_option() {
    let old = CHUNK_OLD.as_bytes();
    let new = CHUNK_NEW.as_bytes();
    let myers = diff_blobs(old, new, &DiffOptions::default());
    let histogram = diff_blobs(
        old,
        new,
        &DiffOptions {
            algorithm: Algorithm::Histogram,
            ..DiffOptions::default()
        },
    );
    assert_ne!(myers.hunks, histogram.hunks);
    // Histogram keeps Chunk_copy whole and moves Chunk_bounds_check as one block:
    // 7 removed at the end (the function plus its blank separator), 7 added on top.
    assert_eq!((histogram.additions, histogram.deletions), (7, 7));
    let text = unified_text(&histogram, old, new);
    assert!(text.contains("+int Chunk_bounds_check"), "{text}");
    assert!(text.contains("-int Chunk_bounds_check"), "{text}");
    assert!(!text.contains("-void Chunk_copy"), "{text}");
}

#[test]
fn hunks_counts_additions_deletions() {
    let old = b"a\nb\nc\nd\ne\n";
    let new = b"a\nB\nc\nx\ny\ne\nf\n";
    let fd = diff_blobs(old, new, &DiffOptions::default());
    // b -> B (1/1), d -> x y (1/2), + f (1/0)
    assert_eq!((fd.additions, fd.deletions), (4, 2));

    let (old, new) = fixture("large-insert");
    let fd = diff_blobs(&old, &new, &DiffOptions::default());
    assert_eq!((fd.additions, fd.deletions), (2502, 2));

    let fd = diff_blobs(b"same\n", b"same\n", &DiffOptions::default());
    assert!(fd.hunks.is_empty());
    assert_eq!((fd.additions, fd.deletions), (0, 0));
}

#[test]
fn hunks_empty_old_or_new() {
    let content = b"alpha\nbeta\ngamma\n";

    let fd = diff_blobs(b"", content, &DiffOptions::default());
    assert_eq!(fd.old.len(), 0);
    assert_eq!(
        fd.hunks,
        vec![Hunk {
            old: 0..0,
            new: 0..3,
            blocks: vec![change(0..0, 0..3)],
        }]
    );
    assert_eq!((fd.additions, fd.deletions), (3, 0));
    assert_eq!(
        unified_text(&fd, b"", content),
        "@@ -0,0 +1,3 @@\n+alpha\n+beta\n+gamma\n"
    );

    let fd = diff_blobs(content, b"", &DiffOptions::default());
    assert_eq!(
        fd.hunks,
        vec![Hunk {
            old: 0..3,
            new: 0..0,
            blocks: vec![change(0..3, 0..0)],
        }]
    );
    assert_eq!((fd.additions, fd.deletions), (0, 3));
    assert_eq!(
        unified_text(&fd, content, b""),
        "@@ -1,3 +0,0 @@\n-alpha\n-beta\n-gamma\n"
    );

    let fd = diff_blobs(b"", b"", &DiffOptions::default());
    assert!(fd.hunks.is_empty());
    assert_eq!(unified_text(&fd, b"", b""), "");
}

#[test]
fn unified_text_omits_count_of_one() {
    let fd = diff_blobs(b"x\n", b"y\n", &DiffOptions::default());
    assert_eq!(unified_text(&fd, b"x\n", b"y\n"), "@@ -1 +1 @@\n-x\n+y\n");
}

#[test]
fn line_index_ranges_and_trailing_newline() {
    let bytes = b"a\r\n\nlast";
    let idx = LineIndex::new(bytes);
    assert_eq!(idx.len(), 3);
    assert!(!idx.is_empty());
    assert_eq!(idx.line(bytes, 0), b"a\r");
    assert_eq!(idx.line(bytes, 1), b"");
    assert_eq!(idx.line(bytes, 2), b"last");
    assert!(!idx.has_trailing_newline());

    let idx = LineIndex::new(b"x\n");
    assert_eq!(idx.len(), 1);
    assert!(idx.has_trailing_newline());

    // An empty blob has no lines and nothing missing at its end.
    let idx = LineIndex::new(b"");
    assert_eq!(idx.len(), 0);
    assert!(idx.is_empty());
    assert!(idx.has_trailing_newline());
}

#[test]
fn hunks_200k_lines_stay_fast() {
    // RF4: no quadratic blowups on a 200k-line file with scattered edits.
    let old: Vec<String> = (0..200_000u32)
        .map(|i| format!("    let v{i} = compute({i});\n"))
        .collect();
    let mut new = old.clone();
    for i in (0..200_000usize).step_by(997) {
        new[i] = format!("    let v{i} = changed({i});\n");
    }
    let (old, new) = (join(&old), join(&new));
    let start = Instant::now();
    let fd = diff_blobs(&old, &new, &DiffOptions::default());
    let elapsed = start.elapsed();
    assert_eq!(fd.additions, 201);
    assert_eq!(fd.deletions, 201);
    assert!(elapsed < Duration::from_secs(5), "took {elapsed:?}");
}

/// Compact, stable rendering of a `FileDiff`'s structure for snapshots.
fn render(fd: &FileDiff) -> String {
    let mut out = format!(
        "old lines: {} (trailing newline: {})\nnew lines: {} (trailing newline: {})\n\
         additions: {}, deletions: {}\n",
        fd.old.len(),
        fd.old.has_trailing_newline(),
        fd.new.len(),
        fd.new.has_trailing_newline(),
        fd.additions,
        fd.deletions
    );
    for h in &fd.hunks {
        writeln!(out, "hunk -{:?} +{:?}", h.old, h.new).unwrap();
        for b in &h.blocks {
            match b {
                Block::Equal { old, new } => writeln!(out, "  equal  -{old:?} +{new:?}"),
                Block::Change { old, new } => writeln!(out, "  change -{old:?} +{new:?}"),
            }
            .unwrap();
        }
    }
    out
}

macro_rules! snapshot_cases {
    ($($name:ident => $case:literal),* $(,)?) => {$(
        #[test]
        fn $name() {
            let (old, new) = fixture($case);
            let fd = diff_blobs(&old, &new, &DiffOptions::default());
            insta::assert_snapshot!(render(&fd));
        }
    )*};
}

snapshot_cases! {
    hunks_snapshot_indent_heuristic_slider => "indent-heuristic-slider",
    hunks_snapshot_crlf => "crlf",
    hunks_snapshot_no_trailing_newline => "no-trailing-newline",
    hunks_snapshot_whitespace_only => "whitespace-only",
    hunks_snapshot_large_insert => "large-insert",
    hunks_snapshot_moved_block => "moved-block",
    hunks_snapshot_empty_to_content => "empty-to-content",
    hunks_snapshot_content_to_empty => "content-to-empty",
    hunks_snapshot_unicode => "unicode",
}
