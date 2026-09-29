//! Blob-to-blob line mapping (T1.8, design §8.6, ADR-0010, ADR-0021).

use std::time::{Duration, Instant};

use polygloss_diff::hunks::diff_blobs;
use polygloss_diff::line_map::{LineMap, Mapped, MappedRange};
use polygloss_diff::options::DiffOptions;

/// `n` distinct lines `line 0\n` .. `line n-1\n`.
fn numbered(n: u32) -> Vec<String> {
    (0..n).map(|i| format!("line {i}\n")).collect()
}

fn join(lines: &[String]) -> Vec<u8> {
    lines.concat().into_bytes()
}

#[test]
fn line_map_identity() {
    let text = join(&numbered(20));
    let map = LineMap::new(&text, &text);
    assert_eq!(map.old_len(), 20);
    assert_eq!(map.new_len(), 20);
    for i in 0..20 {
        assert_eq!(map.map_line(i), Mapped::Unchanged(i));
        assert_eq!(map.map_line_back(i), Mapped::Unchanged(i));
    }
    assert_eq!(
        map.map_range(0, 19),
        MappedRange::Moved { start: 0, end: 19 }
    );
    assert_eq!(map.map_range(7, 7), MappedRange::Moved { start: 7, end: 7 });

    let empty = LineMap::new(b"", b"");
    assert_eq!(empty.old_len(), 0);
    assert_eq!(empty.new_len(), 0);
}

#[test]
fn line_map_shift_after_insert_above() {
    let old = numbered(20);
    let mut new = old.clone();
    new.splice(
        5..5,
        ["inserted a\n", "inserted b\n", "inserted c\n"].map(String::from),
    );
    let map = LineMap::new(&join(&old), &join(&new));

    for i in 0..5 {
        assert_eq!(map.map_line(i), Mapped::Unchanged(i), "line {i} above");
    }
    for i in 5..20 {
        assert_eq!(map.map_line(i), Mapped::Unchanged(i + 3), "line {i} below");
    }
    assert_eq!(
        map.map_range(10, 12),
        MappedRange::Moved { start: 13, end: 15 }
    );
    assert_eq!(map.map_range(2, 4), MappedRange::Moved { start: 2, end: 4 });
}

#[test]
fn line_map_changed_line_reports_nearest() {
    let old = numbered(20);

    // Modification that grows: old 5..7 -> new 5..8. Changed lines pair up by
    // offset inside the block.
    let mut new = old.clone();
    new.splice(5..7, ["x\n", "y\n", "z\n"].map(String::from));
    let map = LineMap::new(&join(&old), &join(&new));
    assert_eq!(map.map_line(5), Mapped::Changed { nearest: 5 });
    assert_eq!(map.map_line(6), Mapped::Changed { nearest: 6 });
    assert_eq!(map.map_line(7), Mapped::Unchanged(8));
    assert_eq!(map.map_line_back(7), Mapped::Changed { nearest: 6 });

    // Modification that shrinks: old 5..9 -> new 5..6. Extra old lines clamp to
    // the last line of the replacement.
    let mut new = old.clone();
    new.splice(5..9, ["only\n"].map(String::from));
    let map = LineMap::new(&join(&old), &join(&new));
    assert_eq!(map.map_line(5), Mapped::Changed { nearest: 5 });
    assert_eq!(map.map_line(8), Mapped::Changed { nearest: 5 });
    assert_eq!(map.map_line(9), Mapped::Unchanged(6));

    // Pure deletion in the middle: nearest is the first line after the gap.
    let mut new = old.clone();
    new.drain(10..13);
    let map = LineMap::new(&join(&old), &join(&new));
    for i in 10..13 {
        assert_eq!(map.map_line(i), Mapped::Changed { nearest: 10 });
    }
    assert_eq!(map.map_line(13), Mapped::Unchanged(10));

    // Pure deletion at the end: nothing follows, so the last remaining line.
    let mut new = old.clone();
    new.truncate(15);
    let map = LineMap::new(&join(&old), &join(&new));
    assert_eq!(map.map_line(17), Mapped::Changed { nearest: 14 });

    // Whitespace is exact: re-indenting a line changes it.
    let mut new = old.clone();
    new[3] = "    line 3\n".to_string();
    let map = LineMap::new(&join(&old), &join(&new));
    assert_eq!(map.map_line(3), Mapped::Changed { nearest: 3 });
}

#[test]
fn line_map_range_partially_changed_is_outdated() {
    let old = numbered(20);
    let mut new = old.clone();
    new.insert(0, "header\n".to_string());
    new[9] = "edited\n".to_string(); // old line 8
    let map = LineMap::new(&join(&old), &join(&new));

    assert_eq!(map.map_range(2, 6), MappedRange::Moved { start: 3, end: 7 });
    // The range 6..=10 contains old line 8, which changed: outdated, placed
    // at the mapping of the range's last line (where the thread renders).
    assert_eq!(map.map_range(6, 10), MappedRange::Outdated { nearest: 11 });
    // A range ending on the changed line takes that line's nearest.
    assert_eq!(map.map_range(5, 8), MappedRange::Outdated { nearest: 9 });
    assert_eq!(map.map_range(8, 8), MappedRange::Outdated { nearest: 9 });
}

#[test]
fn line_map_range_split_by_insertion_is_outdated() {
    // Every line of old 4..=6 is unchanged, but a new line now sits between
    // old 5 and old 6: the commented block is no longer contiguous.
    let old = numbered(12);
    let mut new = old.clone();
    new.insert(6, "wedge\n".to_string());
    let map = LineMap::new(&join(&old), &join(&new));

    assert_eq!(map.map_line(5), Mapped::Unchanged(5));
    assert_eq!(map.map_line(6), Mapped::Unchanged(7));
    assert_eq!(map.map_range(4, 6), MappedRange::Outdated { nearest: 7 });
    assert_eq!(map.map_range(4, 5), MappedRange::Moved { start: 4, end: 5 });
    assert_eq!(map.map_range(6, 8), MappedRange::Moved { start: 7, end: 9 });
}

#[test]
fn line_map_range_accepts_reversed_bounds() {
    let text = join(&numbered(10));
    let map = LineMap::new(&text, &text);
    assert_eq!(map.map_range(6, 3), MappedRange::Moved { start: 3, end: 6 });
}

#[test]
fn line_map_delete_everything() {
    let old = join(&numbered(10));
    let map = LineMap::new(&old, b"");
    assert_eq!(map.new_len(), 0);
    for i in 0..10 {
        assert_eq!(map.map_line(i), Mapped::Changed { nearest: 0 });
    }
    assert_eq!(map.map_range(0, 9), MappedRange::Outdated { nearest: 0 });

    // And the reverse: everything is new, so no new line has an old origin.
    let back = LineMap::new(b"", &old);
    for i in 0..10 {
        assert_eq!(back.map_line_back(i), Mapped::Changed { nearest: 0 });
    }
}

#[test]
fn line_map_append_at_eof() {
    let old = numbered(10);
    let mut new = old.clone();
    new.extend(["tail 1\n", "tail 2\n"].map(String::from));
    let map = LineMap::new(&join(&old), &join(&new));
    for i in 0..10 {
        assert_eq!(map.map_line(i), Mapped::Unchanged(i));
    }
    assert_eq!(map.map_range(0, 9), MappedRange::Moved { start: 0, end: 9 });
    // Appended lines map back to the last old line.
    assert_eq!(map.map_line_back(10), Mapped::Changed { nearest: 9 });
    assert_eq!(map.map_line_back(11), Mapped::Changed { nearest: 9 });

    // Appending after a last line without a newline changes that line (the
    // `\n` belongs to the line, as in git).
    let map = LineMap::new(b"a\nb", b"a\nb\nc\n");
    assert_eq!(map.map_line(0), Mapped::Unchanged(0));
    assert_eq!(map.map_line(1), Mapped::Changed { nearest: 1 });
}

#[test]
fn line_map_back_roundtrip() {
    let old: Vec<String> = (0..300u32).map(|i| format!("row {i}\n")).collect();
    let mut new = old.clone();
    new.drain(40..45);
    new[100] = "rewritten\n".to_string();
    new.splice(
        150..150,
        (0..7).map(|i| format!("added {i}\n")).collect::<Vec<_>>(),
    );
    new.push("trailer\n".to_string());
    let map = LineMap::new(&join(&old), &join(&new));

    let mut unchanged_new = 0;
    for n in 0..map.new_len() {
        match map.map_line_back(n) {
            Mapped::Unchanged(o) => {
                unchanged_new += 1;
                assert_eq!(map.map_line(o), Mapped::Unchanged(n), "new {n} <- old {o}");
                assert_eq!(old[o as usize], new[n as usize]);
            }
            Mapped::Changed { nearest } => assert!(nearest < map.old_len()),
        }
    }
    let mut unchanged_old = 0;
    for o in 0..map.old_len() {
        match map.map_line(o) {
            Mapped::Unchanged(n) => {
                unchanged_old += 1;
                assert_eq!(
                    map.map_line_back(n),
                    Mapped::Unchanged(o),
                    "old {o} -> new {n}"
                );
            }
            Mapped::Changed { nearest } => assert!(nearest < map.new_len()),
        }
    }
    assert_eq!(unchanged_old, unchanged_new);
    assert_eq!(unchanged_old, 300 - 5 - 1);
}

#[test]
fn line_map_from_diff_ignores_context_settings() {
    let old: Vec<String> = (0..500u32).map(|i| format!("item {i}\n")).collect();
    let mut new = old.clone();
    for i in (0..500usize).step_by(37) {
        new[i] = format!("changed {i}\n");
    }
    new.drain(200..210);
    new.insert(50, "extra\n".to_string());
    let (old, new) = (join(&old), join(&new));

    let direct = LineMap::new(&old, &new);
    for context in [0, 3, 25] {
        let opts = DiffOptions {
            context,
            ..DiffOptions::default()
        };
        let fd = diff_blobs(&old, &new, &opts);
        assert_eq!(LineMap::from_diff(&fd), direct, "context {context}");
    }
}

#[test]
fn line_map_out_of_range_lines_do_not_panic() {
    let old = join(&numbered(5));
    let mut new_lines = numbered(5);
    new_lines.push("more\n".to_string());
    let map = LineMap::new(&old, &join(&new_lines));
    assert_eq!(map.map_line(5), Mapped::Changed { nearest: 5 });
    assert_eq!(map.map_line(u32::MAX), Mapped::Changed { nearest: 5 });
    assert_eq!(map.map_line_back(100), Mapped::Changed { nearest: 4 });
    assert_eq!(map.map_range(3, 99), MappedRange::Outdated { nearest: 5 });
}

#[test]
fn line_map_150k_lines_fast() {
    let old: Vec<String> = (0..150_000u32)
        .map(|i| format!("    let v{i} = compute({i});\n"))
        .collect();
    let mut new = old.clone();
    for i in (0..150_000usize).step_by(211) {
        new[i] = format!("    let v{i} = changed({i});\n");
    }
    new.splice(
        75_000..75_000,
        (0..500)
            .map(|i| format!("inserted {i}\n"))
            .collect::<Vec<_>>(),
    );
    new.drain(120_000..120_300);
    let (old, new) = (join(&old), join(&new));

    let start = Instant::now();
    let map = LineMap::new(&old, &new);
    let built = start.elapsed();
    let mut moved = 0u32;
    for line in (0..150_000u32).step_by(7) {
        if matches!(map.map_line(line), Mapped::Unchanged(_)) {
            moved += 1;
        }
    }
    let elapsed = start.elapsed();
    assert_eq!(map.map_line(1), Mapped::Unchanged(1));
    assert_eq!(map.map_line(100_000), Mapped::Unchanged(100_500));
    assert_eq!(map.map_line(149_999), Mapped::Unchanged(150_199));
    assert!(moved > 20_000);
    assert!(
        elapsed < Duration::from_millis(500),
        "build {built:?}, total {elapsed:?}"
    );
}
