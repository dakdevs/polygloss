//! Split and unified row model with gaps and expanders (T1.9, design §11.6).

use std::fmt::Write as _;
use std::ops::Range;
use std::time::{Duration, Instant};

use polygloss_diff::Side;
use polygloss_diff::hunks::{FileDiff, diff_blobs};
use polygloss_diff::options::DiffOptions;
use polygloss_diff::rows::{Cell, ExpandBy, Expansions, GapId, Layout, LineKind, Row, build_rows};

/// `n` distinct lines `line 0\n` .. `line n-1\n`.
fn numbered(n: u32) -> Vec<String> {
    (0..n).map(|i| format!("line {i}\n")).collect()
}

fn join(lines: &[String]) -> Vec<u8> {
    lines.concat().into_bytes()
}

fn diff(old: &[u8], new: &[u8]) -> FileDiff {
    diff_blobs(old, new, &DiffOptions::default())
}

/// 40 numbered lines; new: line 5 edited, a line inserted after line 20 and
/// line 35 deleted. Hunks: old 2..9, 18..24 and 32..39; gaps before each hunk
/// (old 0..2, 9..18, 24..32) and a 1-line trailing gap (old 39..40).
fn three_hunks() -> (Vec<u8>, Vec<u8>) {
    let old = numbered(40);
    let mut new = old.clone();
    new.remove(35);
    new.insert(21, "inserted\n".to_owned());
    new[5] = "LINE 5\n".to_owned();
    (join(&old), join(&new))
}

fn text(fd_index: &polygloss_diff::lines::LineIndex, bytes: &[u8], i: u32) -> String {
    String::from_utf8_lossy(fd_index.line(bytes, i)).into_owned()
}

fn kind_sign(kind: LineKind) -> char {
    match kind {
        LineKind::Context => ' ',
        LineKind::Removed => '-',
        LineKind::Added => '+',
    }
}

fn num(n: Option<u32>) -> String {
    n.map(|n| n.to_string()).unwrap_or_default()
}

fn pair(p: Option<u32>) -> String {
    p.map(|p| format!(" p{p}")).unwrap_or_default()
}

fn render_gap(
    out: &mut String,
    id: &GapId,
    old: &Range<u32>,
    new: &Range<u32>,
    can_up: bool,
    can_down: bool,
) {
    writeln!(
        out,
        "··· gap {} old {old:?} new {new:?}{}{}",
        id.0,
        if can_up { " ↑" } else { "" },
        if can_down { " ↓" } else { "" },
    )
    .unwrap();
}

/// One line per row: `old new sign text [pN]` in unified, `left | right` in split.
fn render(rows: &[Row], fd: &FileDiff, old: &[u8], new: &[u8]) -> String {
    let mut out = String::new();
    let cell = |c: &Option<Cell>, side: Side| -> String {
        match c {
            None => String::new(),
            Some(c) => {
                let t = match side {
                    Side::Old => text(&fd.old, old, c.line),
                    Side::New => text(&fd.new, new, c.line),
                };
                format!("{:>3} {} {}{}", c.line, kind_sign(c.kind), t, pair(c.pair))
            }
        }
    };
    for row in rows {
        match row {
            Row::Gap {
                id,
                old,
                new,
                can_up,
                can_down,
            } => render_gap(&mut out, id, old, new, *can_up, *can_down),
            Row::Unified {
                old: o,
                new: n,
                kind,
                pair: p,
            } => {
                let t = match (o, n) {
                    (_, Some(n)) => text(&fd.new, new, *n),
                    (Some(o), None) => text(&fd.old, old, *o),
                    (None, None) => "<no line>".to_owned(),
                };
                writeln!(
                    out,
                    "{:>3} {:>3} {} {}{}",
                    num(*o),
                    num(*n),
                    kind_sign(*kind),
                    t,
                    pair(*p)
                )
                .unwrap();
            }
            Row::Split { left, right } => {
                let line = format!("{:<24}| {}", cell(left, Side::Old), cell(right, Side::New));
                writeln!(out, "{}", line.trim_end()).unwrap();
            }
            Row::NoNewline { side } => writeln!(out, "\\ no newline ({side:?})").unwrap(),
        }
    }
    out
}

/// A gap row as `(id, old, new, can_up, can_down)`.
type GapRow = (u32, Range<u32>, Range<u32>, bool, bool);

fn gaps(rows: &[Row]) -> Vec<GapRow> {
    rows.iter()
        .filter_map(|r| match r {
            Row::Gap {
                id,
                old,
                new,
                can_up,
                can_down,
            } => Some((id.0, old.clone(), new.clone(), *can_up, *can_down)),
            _ => None,
        })
        .collect()
}

/// Every old line shown by a row, in row order.
fn shown_old_lines(rows: &[Row]) -> Vec<u32> {
    rows.iter()
        .filter_map(|r| match r {
            Row::Unified { old, .. } => *old,
            Row::Split { left, .. } => left.as_ref().map(|c| c.line),
            _ => None,
        })
        .collect()
}

#[test]
fn rows_unified_basic() {
    let (old, new) = three_hunks();
    let fd = diff(&old, &new);
    let rows = build_rows(&fd, &Expansions::default(), Layout::Unified);
    insta::assert_snapshot!(render(&rows, &fd, &old, &new));
}

#[test]
fn rows_split_pairs_change_blocks() {
    let old = numbered(12);
    let mut new = old.clone();
    // Lines 3..5 (two) become three lines; line 9 is deleted.
    new.splice(3..5, ["three!\n", "four!\n", "extra\n"].map(str::to_owned));
    new.remove(10);
    let (old, new) = (join(&old), join(&new));
    let fd = diff(&old, &new);
    let rows = build_rows(&fd, &Expansions::default(), Layout::Split);
    insta::assert_snapshot!(render(&rows, &fd, &old, &new));
}

#[test]
fn rows_split_unbalanced_block_leaves_empty_cells() {
    let old = join(&["a\n", "b\n", "c\n", "d\n"].map(str::to_owned));
    let new = join(&["a\n", "B\n", "d\n"].map(str::to_owned));
    let fd = diff(&old, &new);
    let rows = build_rows(&fd, &Expansions::default(), Layout::Split);
    assert_eq!(
        rows,
        vec![
            Row::Split {
                left: Some(Cell {
                    line: 0,
                    kind: LineKind::Context,
                    pair: None
                }),
                right: Some(Cell {
                    line: 0,
                    kind: LineKind::Context,
                    pair: None
                }),
            },
            Row::Split {
                left: Some(Cell {
                    line: 1,
                    kind: LineKind::Removed,
                    pair: Some(0)
                }),
                right: Some(Cell {
                    line: 1,
                    kind: LineKind::Added,
                    pair: Some(0)
                }),
            },
            Row::Split {
                left: Some(Cell {
                    line: 2,
                    kind: LineKind::Removed,
                    pair: None
                }),
                right: None,
            },
            Row::Split {
                left: Some(Cell {
                    line: 3,
                    kind: LineKind::Context,
                    pair: None
                }),
                right: Some(Cell {
                    line: 2,
                    kind: LineKind::Context,
                    pair: None
                }),
            },
        ]
    );

    // The other direction: extra added lines leave the left cell empty.
    let rows = build_rows(&diff(&new, &old), &Expansions::default(), Layout::Split);
    assert_eq!(
        rows[2],
        Row::Split {
            left: None,
            right: Some(Cell {
                line: 2,
                kind: LineKind::Added,
                pair: None
            }),
        }
    );
}

#[test]
fn rows_gaps_between_hunks_and_at_edges() {
    let (old, new) = three_hunks();
    let fd = diff(&old, &new);
    for layout in [Layout::Unified, Layout::Split] {
        let rows = build_rows(&fd, &Expansions::default(), layout);
        assert_eq!(
            gaps(&rows),
            vec![
                (0, 0..2, 0..2, true, false),
                (1, 9..18, 9..18, true, true),
                (2, 24..32, 25..33, true, true),
                (3, 39..40, 39..40, false, true),
            ],
            "{layout:?}"
        );
        assert!(matches!(rows[0], Row::Gap { .. }));
        assert!(matches!(rows[rows.len() - 1], Row::Gap { .. }));
    }

    // A change on the first and last line: no edge gaps at all.
    let old = numbered(30);
    let mut new = old.clone();
    new[0] = "first\n".to_owned();
    new[29] = "last\n".to_owned();
    let fd = diff(&join(&old), &join(&new));
    let rows = build_rows(&fd, &Expansions::default(), Layout::Unified);
    assert_eq!(gaps(&rows), vec![(1, 4..26, 4..26, true, true)]);
    assert!(!matches!(rows[0], Row::Gap { .. }));
    assert!(!matches!(rows[rows.len() - 1], Row::Gap { .. }));
}

#[test]
fn rows_file_without_hunks_is_one_gap() {
    let text = join(&numbered(10));
    let fd = diff(&text, &text);
    let rows = build_rows(&fd, &Expansions::default(), Layout::Unified);
    assert_eq!(gaps(&rows), vec![(0, 0..10, 0..10, false, false)]);
    assert_eq!(rows.len(), 1);

    let mut exp = Expansions::default();
    exp.expand(&fd, GapId(0), ExpandBy::All);
    let rows = build_rows(&fd, &exp, Layout::Unified);
    assert_eq!(shown_old_lines(&rows), (0..10).collect::<Vec<_>>());

    // Two empty blobs: nothing at all.
    let fd = diff(b"", b"");
    assert!(build_rows(&fd, &Expansions::default(), Layout::Split).is_empty());
}

#[test]
fn rows_expand_up_20_down_20_all() {
    // Changes at lines 5 and 95 of 100: gap 1 is old 9..92 (83 lines).
    let old = numbered(100);
    let mut new = old.clone();
    new[5] = "five\n".to_owned();
    new[95] = "ninety-five\n".to_owned();
    let fd = diff(&join(&old), &join(&new));
    let mut exp = Expansions::default();
    let gap1 = |exp: &Expansions| {
        gaps(&build_rows(&fd, exp, Layout::Unified))
            .into_iter()
            .filter(|g| g.0 == 1)
            .collect::<Vec<_>>()
    };
    assert_eq!(gap1(&exp), vec![(1, 9..92, 9..92, true, true)]);

    // ↑20 reveals the 20 lines just above the next hunk.
    exp.expand(&fd, GapId(1), ExpandBy::Up(20));
    assert_eq!(exp.to_ranges(), vec![[72, 92]]);
    assert_eq!(gap1(&exp), vec![(1, 9..72, 9..72, true, true)]);
    let rows = build_rows(&fd, &exp, Layout::Unified);
    let shown = shown_old_lines(&rows);
    assert!(!shown.contains(&71));
    assert!(shown.contains(&72));
    // Revealed lines are ordinary context rows with both line numbers.
    assert!(rows.contains(&Row::Unified {
        old: Some(80),
        new: Some(80),
        kind: LineKind::Context,
        pair: None
    }));

    // ↓20 reveals the 20 lines just below the previous hunk.
    exp.expand(&fd, GapId(1), ExpandBy::Down(20));
    assert_eq!(exp.to_ranges(), vec![[9, 29], [72, 92]]);
    assert_eq!(gap1(&exp), vec![(1, 29..72, 29..72, true, true)]);

    // More than what is left clamps to the gap.
    exp.expand(&fd, GapId(1), ExpandBy::Up(1000));
    assert_eq!(exp.to_ranges(), vec![[9, 92]]);
    assert_eq!(gap1(&exp), vec![]);

    // Expand all on the leading gap removes it; the trailing gap stays.
    exp.expand(&fd, GapId(0), ExpandBy::All);
    let rows = build_rows(&fd, &exp, Layout::Split);
    assert_eq!(gaps(&rows), vec![(2, 99..100, 99..100, false, true)]);
    assert_eq!(shown_old_lines(&rows)[..3], [0, 1, 2]);

    // Edge gaps: ↑ on the leading gap and ↓ on the trailing gap.
    let mut exp = Expansions::default();
    exp.expand(&fd, GapId(0), ExpandBy::Up(1));
    exp.expand(&fd, GapId(2), ExpandBy::Down(1));
    assert_eq!(exp.to_ranges(), vec![[1, 2], [99, 100]]);
}

#[test]
fn rows_expand_all_merges_adjacent_hunks() {
    let (old, new) = three_hunks();
    let fd = diff(&old, &new);
    let mut exp = Expansions::default();
    exp.expand(&fd, GapId(1), ExpandBy::All);
    let rows = build_rows(&fd, &exp, Layout::Unified);
    assert_eq!(
        gaps(&rows).iter().map(|g| g.0).collect::<Vec<_>>(),
        vec![0, 2, 3]
    );
    // Hunks 0 and 1 now read as one continuous run of old lines 2..24.
    let shown = shown_old_lines(&rows);
    let start = shown.iter().position(|&l| l == 2).unwrap();
    assert_eq!(shown[start..start + 22], (2..24).collect::<Vec<_>>()[..]);

    // Expanding the whole file removes every gap and shows every line once.
    let mut exp = Expansions::default();
    exp.expand_file(&fd);
    for layout in [Layout::Unified, Layout::Split] {
        let rows = build_rows(&fd, &exp, layout);
        assert!(gaps(&rows).is_empty(), "{layout:?}");
        assert_eq!(shown_old_lines(&rows), (0..40).collect::<Vec<_>>());
    }
}

#[test]
fn rows_no_newline_marker() {
    // Both sides end without a newline and the last line changes.
    let old = b"a\nb";
    let new = b"a\nc";
    let fd = diff(old, new);
    let rows = build_rows(&fd, &Expansions::default(), Layout::Unified);
    assert_eq!(
        rows,
        vec![
            Row::Unified {
                old: Some(0),
                new: Some(0),
                kind: LineKind::Context,
                pair: None
            },
            Row::Unified {
                old: Some(1),
                new: None,
                kind: LineKind::Removed,
                pair: Some(0)
            },
            Row::NoNewline { side: Side::Old },
            Row::Unified {
                old: None,
                new: Some(1),
                kind: LineKind::Added,
                pair: Some(0)
            },
            Row::NoNewline { side: Side::New },
        ]
    );
    // Split: the markers follow the change block, old side first.
    let rows = build_rows(&fd, &Expansions::default(), Layout::Split);
    assert_eq!(
        rows[2..],
        [
            Row::NoNewline { side: Side::Old },
            Row::NoNewline { side: Side::New }
        ]
    );

    // Only the new side lacks it (a newline was removed).
    let fd = diff(b"a\nb\n", b"a\nb");
    let rows = build_rows(&fd, &Expansions::default(), Layout::Unified);
    assert_eq!(
        rows.iter()
            .filter(|r| matches!(r, Row::NoNewline { .. }))
            .collect::<Vec<_>>(),
        vec![&Row::NoNewline { side: Side::New }]
    );
    assert_eq!(rows.last(), Some(&Row::NoNewline { side: Side::New }));

    // An unchanged last line without a newline: one marker in unified (as git
    // prints it), one per side in split, only once the line is revealed.
    let mut old = numbered(20).concat();
    old.pop();
    let new = old.replacen("line 2\n", "two\n", 1);
    let fd = diff(old.as_bytes(), new.as_bytes());
    let hidden = build_rows(&fd, &Expansions::default(), Layout::Unified);
    assert!(!hidden.iter().any(|r| matches!(r, Row::NoNewline { .. })));
    let mut exp = Expansions::default();
    exp.expand_file(&fd);
    let unified = build_rows(&fd, &exp, Layout::Unified);
    assert_eq!(unified.last(), Some(&Row::NoNewline { side: Side::New }));
    assert_eq!(
        unified
            .iter()
            .filter(|r| matches!(r, Row::NoNewline { .. }))
            .count(),
        1
    );
    let split = build_rows(&fd, &exp, Layout::Split);
    assert_eq!(
        split[split.len() - 2..],
        [
            Row::NoNewline { side: Side::Old },
            Row::NoNewline { side: Side::New }
        ]
    );

    // Empty blobs never get a marker.
    let rows = build_rows(&diff(b"", b"x"), &Expansions::default(), Layout::Unified);
    assert_eq!(rows.last(), Some(&Row::NoNewline { side: Side::New }));
    assert_eq!(
        rows.iter()
            .filter(|r| matches!(r, Row::NoNewline { .. }))
            .count(),
        1
    );
}

#[test]
fn rows_unified_line_numbers_both_columns() {
    let (old, new) = three_hunks();
    let fd = diff(&old, &new);
    let mut exp = Expansions::default();
    exp.expand_file(&fd);
    let rows = build_rows(&fd, &exp, Layout::Unified);
    let mut context = 0;
    for row in &rows {
        match row {
            Row::Unified {
                old: Some(o),
                new: Some(n),
                kind,
                pair,
            } => {
                assert_eq!(*kind, LineKind::Context);
                assert_eq!(*pair, None);
                // Offset by the insertion after old line 20 and the deletion of 35.
                let expected = match o {
                    0..=20 => *o,
                    21..=34 => o + 1,
                    _ => *o,
                };
                assert_eq!(*n, expected, "old line {o}");
                context += 1;
            }
            Row::Unified {
                old: Some(o),
                new: None,
                kind,
                ..
            } => {
                assert_eq!(*kind, LineKind::Removed);
                assert!([5, 35].contains(o));
            }
            Row::Unified {
                old: None,
                new: Some(n),
                kind,
                ..
            } => {
                assert_eq!(*kind, LineKind::Added);
                assert!([5, 21].contains(n));
            }
            other => panic!("unexpected row {other:?}"),
        }
    }
    assert_eq!(context, 38);
    // Unified shows removed lines of a block before its added lines, paired.
    let at = rows
        .iter()
        .position(|r| {
            matches!(
                r,
                Row::Unified {
                    old: Some(5),
                    new: None,
                    ..
                }
            )
        })
        .unwrap();
    assert_eq!(
        rows[at..at + 2],
        [
            Row::Unified {
                old: Some(5),
                new: None,
                kind: LineKind::Removed,
                pair: Some(0)
            },
            Row::Unified {
                old: None,
                new: Some(5),
                kind: LineKind::Added,
                pair: Some(0)
            },
        ]
    );
}

#[test]
fn expansions_roundtrip_ranges() {
    let (old, new) = three_hunks();
    let fd = diff(&old, &new);
    let mut exp = Expansions::default();
    exp.expand(&fd, GapId(2), ExpandBy::Down(3));
    exp.expand(&fd, GapId(1), ExpandBy::Up(2));
    let ranges = exp.to_ranges();
    assert_eq!(ranges, vec![[16, 18], [24, 27]]);
    let back = Expansions::from_ranges(&ranges);
    assert_eq!(back, exp);
    assert_eq!(back.to_ranges(), ranges);
    assert_eq!(
        build_rows(&fd, &back, Layout::Split),
        build_rows(&fd, &exp, Layout::Split)
    );

    // from_ranges normalizes: sorts, merges overlapping and touching ranges,
    // drops empty or reversed ones.
    let exp = Expansions::from_ranges(&[[30, 40], [5, 5], [2, 4], [9, 3], [4, 6], [35, 50]]);
    assert_eq!(exp.to_ranges(), vec![[2, 6], [30, 50]]);
    assert!(Expansions::from_ranges(&[]).is_empty());
    assert!(!exp.is_empty());
}

#[test]
fn rows_mid_gap_reveal_splits_the_gap() {
    let (old, new) = three_hunks();
    let fd = diff(&old, &new);
    // A restored or mapped range in the middle of gap 1 (old 9..18).
    let mut exp = Expansions::from_ranges(&[[12, 14]]);
    let rows = build_rows(&fd, &exp, Layout::Unified);
    let gap1: Vec<_> = gaps(&rows).into_iter().filter(|g| g.0 == 1).collect();
    assert_eq!(
        gap1,
        vec![
            (1, 9..12, 9..12, true, true),
            (1, 14..18, 14..18, true, true)
        ]
    );
    // ↑ acts on the hidden lines nearest the next hunk, ↓ on those nearest the
    // previous hunk.
    exp.expand(&fd, GapId(1), ExpandBy::Up(1));
    exp.expand(&fd, GapId(1), ExpandBy::Down(1));
    assert_eq!(exp.to_ranges(), vec![[9, 10], [12, 14], [17, 18]]);
    // `reveal` opens any old-line range, e.g. one hidden piece.
    exp.reveal(10..12);
    exp.reveal(14..17);
    assert_eq!(exp.to_ranges(), vec![[9, 18]]);
}

#[test]
fn rows_stale_ranges_past_eof_do_not_panic() {
    let (old, new) = three_hunks();
    let fd = diff(&old, &new);
    let exp = Expansions::from_ranges(&[[38, 500], [1000, 2000]]);
    let rows = build_rows(&fd, &exp, Layout::Unified);
    assert_eq!(shown_old_lines(&rows).last(), Some(&39));
    assert!(gaps(&rows).iter().all(|g| g.0 != 3));

    // Expanding a gap id that does not exist is a no-op.
    let mut exp = Expansions::default();
    exp.expand(&fd, GapId(99), ExpandBy::All);
    assert!(exp.is_empty());
}

#[test]
fn rows_whitespace_mode_context_is_ordinary_context() {
    let old = b"a\n  b\nc\n";
    let new = b"a\nb\nc\nd\n";
    let fd = diff_blobs(
        old,
        new,
        &DiffOptions {
            ignore_whitespace: true,
            ..DiffOptions::default()
        },
    );
    let rows = build_rows(&fd, &Expansions::default(), Layout::Unified);
    assert_eq!(
        rows[1],
        Row::Unified {
            old: Some(1),
            new: Some(1),
            kind: LineKind::Context,
            pair: None
        }
    );
    assert_eq!(
        rows[3],
        Row::Unified {
            old: None,
            new: Some(3),
            kind: LineKind::Added,
            pair: None
        }
    );
}

#[test]
fn layout_serde_names() {
    assert_eq!(serde_json::to_string(&Layout::Split).unwrap(), "\"split\"");
    assert_eq!(
        serde_json::to_string(&Layout::Unified).unwrap(),
        "\"unified\""
    );
    assert_eq!(
        serde_json::from_str::<Layout>("\"unified\"").unwrap(),
        Layout::Unified
    );
}

#[test]
fn rows_200k_lines_expand_all_fast() {
    let old = numbered(200_000);
    let mut new = old.clone();
    for i in (0..200_000).step_by(1000) {
        new[i] = format!("changed {i}\n");
    }
    let fd = diff(&join(&old), &join(&new));
    let mut exp = Expansions::default();
    let start = Instant::now();
    let collapsed = build_rows(&fd, &exp, Layout::Split);
    exp.expand_file(&fd);
    let unified = build_rows(&fd, &exp, Layout::Unified);
    let split = build_rows(&fd, &exp, Layout::Split);
    let elapsed = start.elapsed();
    assert_eq!(gaps(&collapsed).len(), 200);
    assert_eq!(unified.len(), 200_000 + 200);
    assert_eq!(split.len(), 200_000);
    assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");
}
