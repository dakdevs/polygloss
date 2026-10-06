//! The pure search behind find (T3.15, `find::search`): matches over two
//! blobs in display order, previews, the query compiler, and the chunk
//! walk's limit, binary files and cancellation.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use polygloss_app::find::search::{self, FindMatch, FindOptions};
use polygloss_diff::options::DiffOptions;
use polygloss_diff::{
    FileChange, FileKind, FileStatus, GeneratedAttr, GitPath, ObjectFormat, Oid, Side,
};
use polygloss_viewport::DiffProvider;

/// The pure search over two blobs: context lines once (on the new side,
/// with their old line), removed lines before the lines that replace
/// them, matches inside a line in order, and hidden context flagged.
#[test]
fn search_blobs_orders_by_display_and_dedupes_context() {
    let old = b"a needle\nkeep\nold needle\nb\nc\nd\ne\nf\ng\nh\nctx needle\n";
    let new = b"a needle\nkeep\nnew needle needle\nb\nc\nd\ne\nf\ng\nh\nctx needle\n";
    let re = search::compile("needle", FindOptions::default())
        .unwrap()
        .unwrap();
    /// `(side, line, old_line, hidden, preview_match)`.
    type Row = (Side, u32, Option<u32>, bool, std::ops::Range<usize>);
    let got: Vec<Row> = search::search_blobs(7, old, new, &re, &DiffOptions::default(), usize::MAX)
        .into_iter()
        .map(|m: FindMatch| {
            assert_eq!(m.file_idx, 7);
            (m.side, m.line, m.old_line, m.hidden, m.preview_match)
        })
        .collect();
    assert_eq!(
        got,
        [
            (Side::New, 0, Some(0), false, 2..8),
            (Side::Old, 2, None, false, 4..10),
            (Side::New, 2, None, false, 4..10),
            (Side::New, 2, None, false, 11..17),
            // Old line 10 is more than 3 lines past the change: hidden.
            (Side::New, 10, Some(10), true, 4..10),
        ]
    );
}

/// Long lines are cut around the match (marked `…`); indentation is
/// dropped without a mark.
#[test]
fn search_previews_cut_long_lines_around_the_match() {
    let re = search::compile("needle", FindOptions::default())
        .unwrap()
        .unwrap();
    let indented = b"        let needle = 1;\n";
    let long = format!("{}needle{}\n", "x".repeat(300), "y".repeat(300));
    let got = search::search_blobs(0, b"", indented, &re, &DiffOptions::default(), usize::MAX);
    assert_eq!(got[0].preview.as_ref(), "let needle = 1;");
    assert_eq!(got[0].preview_match, 4..10);
    let got = search::search_blobs(
        0,
        b"",
        long.as_bytes(),
        &re,
        &DiffOptions::default(),
        usize::MAX,
    );
    let p = got[0].preview.as_ref();
    assert!(p.starts_with('…') && p.ends_with('…'), "{p}");
    assert_eq!(&p[got[0].preview_match.clone()], "needle");
    assert!(p.len() < 200, "{}", p.len());
}

#[test]
fn search_compile_handles_case_regex_and_errors() {
    let opts = |case_sensitive, regex| FindOptions {
        case_sensitive,
        regex,
    };
    assert!(search::compile("", opts(false, false)).unwrap().is_none());
    let lit = search::compile("a.b", opts(false, false)).unwrap().unwrap();
    assert!(lit.is_match(b"A.B"));
    assert!(!lit.is_match(b"axb"));
    let cased = search::compile("a.b", opts(true, false)).unwrap().unwrap();
    assert!(!cased.is_match(b"A.B"));
    let re = search::compile("a.b", opts(false, true)).unwrap().unwrap();
    assert!(re.is_match(b"AXB"));
    assert!(search::compile("a(", opts(false, true)).is_err());
}

/// `^` and `$` are a line's ends in regex mode, on lines past the first,
/// before a final newline and before a CRLF (the whole-blob pre-check
/// agrees with the per-line pass).
#[test]
fn search_regex_anchors_match_at_every_line() {
    let re = |q: &str| {
        search::compile(
            q,
            FindOptions {
                case_sensitive: false,
                regex: true,
            },
        )
        .unwrap()
        .unwrap()
    };
    let lines = |q: &str, new: &[u8]| -> Vec<u32> {
        search::search_blobs(0, b"", new, &re(q), &DiffOptions::default(), usize::MAX)
            .into_iter()
            .map(|m| m.line)
            .collect()
    };
    let new = b"use x;\nfn main() {\n    let y = 1;\n}\n";
    assert_eq!(lines("^fn", new), [1]);
    assert_eq!(lines(";$", new), [0, 2]);
    assert_eq!(lines(r"\{$", new), [1]);
    assert_eq!(lines("^}$", new), [3]);
    let crlf = b"use x;\r\nfn main() {\r\n}\r\n";
    assert_eq!(lines("^fn", crlf), [1]);
    assert_eq!(lines(";$", crlf), [0]);
    assert_eq!(lines(r"\{$", crlf), [1]);
    // Still never across lines.
    assert_eq!(lines(r";\sfn", new), Vec::<u32>::new());
}

/// A limit keeps the first matches in display order (removed lines before
/// the lines replacing them) and stops collecting there.
#[test]
fn search_blobs_stops_at_the_limit_in_display_order() {
    let re = search::compile("e", FindOptions::default())
        .unwrap()
        .unwrap();
    let old = b"keep\nold one\nold three\nctx\n";
    let new = b"keep\nnew one\nnew two\nctx\n";
    let at = |limit| -> Vec<(Side, u32)> {
        search::search_blobs(0, old, new, &re, &DiffOptions::default(), limit)
            .into_iter()
            .map(|m| (m.side, m.line))
            .collect()
    };
    let all = at(usize::MAX);
    assert_eq!(
        all,
        [
            (Side::New, 0),
            (Side::New, 0),
            (Side::Old, 1),
            (Side::Old, 2),
            (Side::Old, 2),
            (Side::New, 1),
            (Side::New, 1),
            (Side::New, 2),
        ]
    );
    for limit in 0..all.len() {
        assert_eq!(at(limit), all[..limit], "limit {limit}");
    }
}

/// An in-memory provider of one text file.
struct OneFile {
    files: Arc<Vec<FileChange>>,
    old: Arc<[u8]>,
    new: Arc<[u8]>,
}

impl DiffProvider for OneFile {
    fn object_format(&self) -> ObjectFormat {
        ObjectFormat::Sha1
    }
    fn files(&self) -> Arc<Vec<FileChange>> {
        self.files.clone()
    }
    fn load_blob(&self, oid: &Oid) -> anyhow::Result<Arc<[u8]>> {
        Ok(if *oid == self.files[0].old_blob {
            self.old.clone()
        } else {
            self.new.clone()
        })
    }
    fn blob_size(&self, oid: &Oid) -> anyhow::Result<u64> {
        Ok(self.load_blob(oid)?.len() as u64)
    }
}

#[test]
fn search_file_skips_binary_and_stops_when_cancelled() {
    let change = |kind| FileChange {
        idx: 0,
        status: FileStatus::Modified,
        old_path: Some(GitPath::from_bytes(b"a.txt")),
        new_path: Some(GitPath::from_bytes(b"a.txt")),
        old_mode: None,
        new_mode: None,
        old_blob: Oid::parse(&"1".repeat(40), ObjectFormat::Sha1).unwrap(),
        new_blob: Oid::parse(&"2".repeat(40), ObjectFormat::Sha1).unwrap(),
        similarity: None,
        kind,
        generated: false,
        generated_attr: GeneratedAttr::Unspecified,
    };
    let provider = |kind, new: &[u8]| OneFile {
        files: Arc::new(vec![change(kind)]),
        old: Arc::from(&b"x\n"[..]),
        new: Arc::from(new),
    };
    let re = search::compile("needle", FindOptions::default())
        .unwrap()
        .unwrap();
    let diff = DiffOptions::default();
    let go = |p: &OneFile, cancel: bool| {
        search::search_file(
            &p.files[0],
            p,
            &re,
            &diff,
            &AtomicBool::new(cancel),
            usize::MAX,
        )
        .len()
    };
    assert_eq!(go(&provider(FileKind::Text, b"needle\n"), false), 1);
    assert_eq!(go(&provider(FileKind::Text, b"needle\n"), true), 0);
    assert_eq!(go(&provider(FileKind::Binary, b"needle\n"), false), 0);
    // Listed as text, but a NUL byte: binary after all.
    assert_eq!(go(&provider(FileKind::Text, b"needle\0\n"), false), 0);
    // A limit caps the matches kept.
    let p = provider(FileKind::Text, b"needle\nneedle\nneedle\n");
    let limited = |limit| {
        search::search_file(&p.files[0], &p, &re, &diff, &AtomicBool::new(false), limit).len()
    };
    assert_eq!(limited(usize::MAX), 3);
    assert_eq!(limited(2), 2);
    assert_eq!(limited(0), 0);
}

/// Many files of one provider, each `needle` twice; the cancel flag is set
/// while the third file is read.
struct CancelOnThird {
    files: Arc<Vec<FileChange>>,
    cancel: Arc<AtomicBool>,
    loads: std::sync::atomic::AtomicUsize,
}

impl DiffProvider for CancelOnThird {
    fn object_format(&self) -> ObjectFormat {
        ObjectFormat::Sha1
    }
    fn files(&self) -> Arc<Vec<FileChange>> {
        self.files.clone()
    }
    fn load_blob(&self, _: &Oid) -> anyhow::Result<Arc<[u8]>> {
        use std::sync::atomic::Ordering;
        // Two blobs per file.
        if self.loads.fetch_add(1, Ordering::SeqCst) == 4 {
            self.cancel.store(true, Ordering::SeqCst);
        }
        Ok(Arc::from(&b"needle\nneedle\n"[..]))
    }
    fn blob_size(&self, _: &Oid) -> anyhow::Result<u64> {
        Ok(14)
    }
}

/// One background job's work: files in order, stopping when cancelled or
/// once `limit` matches are kept.
#[test]
fn search_chunk_stops_when_cancelled_or_at_the_limit() {
    let files: Vec<FileChange> = (0..6u32)
        .map(|i| FileChange {
            idx: i,
            status: FileStatus::Modified,
            old_path: Some(GitPath::from_bytes(b"a.txt")),
            new_path: Some(GitPath::from_bytes(b"a.txt")),
            old_mode: None,
            new_mode: None,
            old_blob: Oid::parse(&format!("{:040x}", 2 * i + 1), ObjectFormat::Sha1).unwrap(),
            new_blob: Oid::parse(&format!("{:040x}", 2 * i + 2), ObjectFormat::Sha1).unwrap(),
            similarity: None,
            kind: FileKind::Text,
            generated: false,
            generated_attr: GeneratedAttr::Unspecified,
        })
        .collect();
    let re = search::compile("needle", FindOptions::default())
        .unwrap()
        .unwrap();
    let diff = DiffOptions::default();
    let provider = || {
        let cancel = Arc::new(AtomicBool::new(false));
        CancelOnThird {
            files: Arc::new(files.clone()),
            cancel,
            loads: Default::default(),
        }
    };
    let ids = |got: Vec<FindMatch>| -> Vec<u32> { got.into_iter().map(|m| m.file_idx).collect() };

    // Cancelled while reading the third file: nothing from it on.
    let p = provider();
    let got = search::search_chunk(&files, &p, &re, &diff, &p.cancel, usize::MAX);
    assert_eq!(ids(got), [0, 0, 1, 1]);

    // Not cancelled: stops once the limit is reached, mid-file too.
    let p = provider();
    let never = AtomicBool::new(false);
    let got = search::search_chunk(&files, &p, &re, &diff, &never, 5);
    assert_eq!(ids(got), [0, 0, 1, 1, 2]);
    assert_eq!(
        p.loads.load(std::sync::atomic::Ordering::SeqCst),
        6,
        "no file read past the limit"
    );
}
