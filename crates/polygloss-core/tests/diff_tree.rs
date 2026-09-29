//! `diff-tree` listing, attribute classification, binary and generated detection
//! (T1.3, design §6.1, §6.2, §6.4, RF1, OQ-25, OQ-P3). Every test runs under
//! `Sandbox::isolate()`. Hostile and non-UTF-8 paths cannot live in a checkout
//! (APFS rejects non-UTF-8 names), so those trees are built in code with a
//! throwaway index.
#![allow(unsafe_code)] // tests set process env (one process per test under nextest)

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use polygloss_core::git::{
    BUILTIN_GENERATED, Git, Source, classify, discover, list_changes, resolve,
};
use polygloss_core::testing::{FixtureRepo, Sandbox, git_spawns};
use polygloss_core::{ObjectFormat, Oid};
use polygloss_diff::{FileChange, FileKind, FileStatus, GitPath, Mode};

fn os<'a>(args: &'a [&'a str]) -> Vec<&'a OsStr> {
    args.iter().map(OsStr::new).collect()
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap().trim_end().to_owned()
}

fn set_env(key: &str, val: impl AsRef<OsStr>) {
    // SAFETY: one process per test (nextest); no other threads read env here.
    unsafe { std::env::set_var(key, val) }
}

fn tree(repo: &FixtureRepo, rev: &str) -> Oid {
    repo.oid(&format!("{rev}^{{tree}}"))
}

fn fmt_of(repo: &FixtureRepo) -> ObjectFormat {
    ObjectFormat::from_name(&repo.git(&["rev-parse", "--show-object-format"])).unwrap()
}

/// `list_changes` between two revisions' trees, run from the repo root.
fn changes(repo: &FixtureRepo, base: &str, head: &str) -> Vec<FileChange> {
    list_changes(
        &Git::new(repo.path()),
        fmt_of(repo),
        &tree(repo, base),
        &tree(repo, head),
    )
    .unwrap()
}

/// `list_changes` then `classify` against the head tree.
fn classified(repo: &FixtureRepo, base: &Oid, head: &Oid, extra: &[String]) -> Vec<FileChange> {
    let git = Git::new(repo.path());
    let mut files = list_changes(&git, fmt_of(repo), base, head).unwrap();
    classify(&git, head, &mut files, extra).unwrap();
    files
}

fn by_path(files: &[FileChange]) -> HashMap<Vec<u8>, &FileChange> {
    files
        .iter()
        .map(|f| {
            let p = f.new_path.as_ref().or(f.old_path.as_ref()).unwrap();
            (p.to_bytes(), f)
        })
        .collect()
}

fn path(bytes: &[u8]) -> Option<GitPath> {
    Some(GitPath::from_bytes(bytes))
}

/// Writes `bytes` as a blob; returns its id.
fn blob(repo: &FixtureRepo, bytes: &[u8]) -> String {
    text(
        Git::new(repo.path())
            .output_stdin(&os(&["hash-object", "-w", "--stdin"]), bytes)
            .unwrap(),
    )
}

/// One tree entry: mode, raw path bytes, object id.
type Entry = (&'static str, Vec<u8>, String);

fn file(repo: &FixtureRepo, path: &[u8], content: &[u8]) -> Entry {
    ("100644", path.to_vec(), blob(repo, content))
}

/// Builds a tree from entries with a throwaway index (`update-index -z
/// --index-info` + `write-tree`); the user's index is never touched.
fn mktree(repo: &FixtureRepo, entries: &[Entry]) -> Oid {
    static N: AtomicU32 = AtomicU32::new(0);
    let index = repo.path().join(".git").join(format!(
        "polygloss-test-index-{}",
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let git = Git::new(repo.path()).with_env("GIT_INDEX_FILE", &index);
    let mut input = Vec::new();
    for (mode, path, oid) in entries {
        input.extend_from_slice(format!("{mode} {oid}\t").as_bytes());
        input.extend_from_slice(path);
        input.push(0);
    }
    git.output_stdin(&os(&["update-index", "-z", "--index-info"]), &input)
        .unwrap();
    let t = text(git.output(&os(&["write-tree"])).unwrap());
    std::fs::remove_file(&index).unwrap();
    Oid::parse(&t, fmt_of(repo)).unwrap()
}

fn ten_lines(tag: &str) -> Vec<u8> {
    (0..10)
        .map(|i| format!("{tag} line {i} with enough text to compare\n"))
        .collect::<String>()
        .into_bytes()
}

fn write_script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

// ---------------------------------------------------------------- diff-tree

#[test]
fn diff_tree_parses_modify_add_delete() {
    let _sb = Sandbox::isolate();
    for fmt in [ObjectFormat::Sha1, ObjectFormat::Sha256] {
        let repo = FixtureRepo::init(fmt);
        repo.write("a.txt", b"one\n");
        repo.write("b.txt", b"bee\n");
        repo.commit("c1");
        repo.write("a.txt", b"two\n");
        std::fs::remove_file(repo.path().join("b.txt")).unwrap();
        repo.write("c.txt", b"sea\n");
        repo.commit("c2");

        let files = changes(&repo, "HEAD~1", "HEAD");
        assert_eq!(files.len(), 3, "{files:#?}");
        let zero = Oid::zero(fmt);

        let a = &files[0];
        assert_eq!(a.idx, 0);
        assert_eq!(a.status, FileStatus::Modified);
        assert_eq!(a.old_path, path(b"a.txt"));
        assert_eq!(a.new_path, path(b"a.txt"));
        assert_eq!(a.old_mode, Some(Mode(0o100644)));
        assert_eq!(a.new_mode, Some(Mode(0o100644)));
        assert_eq!(a.old_blob, repo.oid("HEAD~1:a.txt"));
        assert_eq!(a.new_blob, repo.oid("HEAD:a.txt"));
        assert_eq!(a.old_blob.object_format(), fmt);
        assert_eq!(a.similarity, None);
        assert_eq!(a.kind, FileKind::Text);
        assert!(!a.generated);

        let b = &files[1];
        assert_eq!(b.idx, 1);
        assert_eq!(b.status, FileStatus::Deleted);
        assert_eq!(b.old_path, path(b"b.txt"));
        assert_eq!(b.new_path, None);
        assert_eq!(b.old_mode, Some(Mode(0o100644)));
        assert_eq!(b.new_mode, None);
        assert_eq!(b.old_blob, repo.oid("HEAD~1:b.txt"));
        assert_eq!(b.new_blob, zero);
        assert_eq!(b.display_path(), "b.txt");

        let c = &files[2];
        assert_eq!(c.idx, 2);
        assert_eq!(c.status, FileStatus::Added);
        assert_eq!(c.old_path, None);
        assert_eq!(c.new_path, path(b"c.txt"));
        assert_eq!(c.old_mode, None);
        assert_eq!(c.new_mode, Some(Mode(0o100644)));
        assert_eq!(c.old_blob, zero);
        assert_eq!(c.new_blob, repo.oid("HEAD:c.txt"));
    }
}

#[test]
fn diff_tree_identical_trees_is_empty() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"one\n");
    repo.commit("c1");
    assert!(changes(&repo, "HEAD", "HEAD").is_empty());

    // Against the empty tree every file is an add.
    let git = Git::new(repo.path());
    let files = list_changes(
        &git,
        ObjectFormat::Sha1,
        &ObjectFormat::Sha1.empty_tree(),
        &tree(&repo, "HEAD"),
    )
    .unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].status, FileStatus::Added);
}

#[test]
fn diff_tree_parses_rename_with_score() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("old/exact.txt", &ten_lines("exact"));
    repo.write("old/fuzzy.txt", &ten_lines("fuzzy"));
    repo.commit("c1");
    std::fs::create_dir_all(repo.path().join("new")).unwrap();
    for name in ["exact.txt", "fuzzy.txt"] {
        std::fs::rename(
            repo.path().join("old").join(name),
            repo.path().join("new").join(name),
        )
        .unwrap();
    }
    let mut fuzzy = ten_lines("fuzzy");
    fuzzy.extend_from_slice(b"one more line\n");
    repo.write("new/fuzzy.txt", &fuzzy);
    repo.commit("c2");

    // git's own score for the fuzzy rename, from its text output.
    let raw = repo.git(&[
        "diff-tree",
        "-r",
        "--raw",
        "-M50%",
        "HEAD~1^{tree}",
        "HEAD^{tree}",
    ]);
    let fuzzy_score: u8 = raw
        .lines()
        .find(|l| l.ends_with("new/fuzzy.txt"))
        .and_then(|l| l.split_whitespace().nth(4))
        .and_then(|s| s.strip_prefix('R'))
        .unwrap()
        .parse()
        .unwrap();
    assert!((50..100).contains(&fuzzy_score), "{raw}");

    let files = changes(&repo, "HEAD~1", "HEAD");
    assert_eq!(files.len(), 2, "{files:#?}");
    let map = by_path(&files);
    let exact = map[b"new/exact.txt".as_slice()];
    assert_eq!(exact.status, FileStatus::Renamed);
    assert_eq!(exact.old_path, path(b"old/exact.txt"));
    assert_eq!(exact.new_path, path(b"new/exact.txt"));
    assert_eq!(exact.similarity, Some(100));
    assert_eq!(exact.old_blob, exact.new_blob);

    let fuzzy = map[b"new/fuzzy.txt".as_slice()];
    assert_eq!(fuzzy.status, FileStatus::Renamed);
    assert_eq!(fuzzy.old_path, path(b"old/fuzzy.txt"));
    assert_eq!(fuzzy.similarity, Some(fuzzy_score));
    assert_eq!(fuzzy.old_blob, repo.oid("HEAD~1:old/fuzzy.txt"));
    assert_eq!(fuzzy.new_blob, repo.oid("HEAD:new/fuzzy.txt"));
    assert_eq!(fuzzy.display_path(), "new/fuzzy.txt");
}

#[test]
fn diff_tree_parses_typechange_and_modes() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("run.sh", b"echo hi\n");
    repo.write("target.txt", b"target\n");
    repo.write("was-file", b"plain\n");
    repo.commit("c1");
    let script = repo.path().join("run.sh");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::remove_file(repo.path().join("was-file")).unwrap();
    std::os::unix::fs::symlink("target.txt", repo.path().join("was-file")).unwrap();
    repo.commit("c2");

    let files = changes(&repo, "HEAD~1", "HEAD");
    assert_eq!(files.len(), 2, "{files:#?}");
    let map = by_path(&files);

    // Mode-only change: modes differ, blobs equal (design §6.4).
    let run = map[b"run.sh".as_slice()];
    assert_eq!(run.status, FileStatus::Modified);
    assert_eq!(run.old_mode, Some(Mode(0o100644)));
    assert_eq!(run.new_mode, Some(Mode(0o100755)));
    assert_eq!(run.old_blob, run.new_blob);
    assert_eq!(run.kind, FileKind::Text);

    let tc = map[b"was-file".as_slice()];
    assert_eq!(tc.status, FileStatus::TypeChanged);
    assert_eq!(tc.old_mode, Some(Mode(0o100644)));
    assert_eq!(tc.new_mode, Some(Mode::SYMLINK));
    assert_eq!(tc.old_blob, repo.oid("HEAD~1:was-file"));
    assert_eq!(tc.new_blob, repo.oid("HEAD:was-file"));
    assert_eq!(tc.kind, FileKind::Symlink);
}

#[test]
fn diff_tree_submodule_kind() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"one\n");
    let c1 = repo.commit("c1");
    repo.write("a.txt", b"two\n");
    let c2 = repo.commit("c2");
    let readme = file(&repo, b"README", b"hi\n");

    let base = mktree(
        &repo,
        &[
            readme.clone(),
            ("160000", b"vendor/lib".to_vec(), c1.to_string()),
        ],
    );
    let head = mktree(
        &repo,
        &[
            readme,
            ("160000", b"vendor/lib".to_vec(), c2.to_string()),
            ("160000", b"vendor/new".to_vec(), c1.to_string()),
        ],
    );
    let files = classified(&repo, &base, &head, &[]);
    assert_eq!(files.len(), 2, "{files:#?}");
    let map = by_path(&files);

    let bumped = map[b"vendor/lib".as_slice()];
    assert_eq!(bumped.status, FileStatus::Modified);
    assert_eq!(bumped.kind, FileKind::Submodule);
    assert_eq!(bumped.old_mode, Some(Mode::SUBMODULE));
    assert_eq!(bumped.new_mode, Some(Mode::SUBMODULE));
    assert_eq!(bumped.old_blob, c1);
    assert_eq!(bumped.new_blob, c2);

    let added = map[b"vendor/new".as_slice()];
    assert_eq!(added.status, FileStatus::Added);
    assert_eq!(added.kind, FileKind::Submodule);
    assert_eq!(added.new_blob, c1);
}

#[test]
fn diff_tree_symlink_kind() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("target.txt", b"target\n");
    std::os::unix::fs::symlink("target.txt", repo.path().join("link")).unwrap();
    repo.commit("c1");
    std::fs::remove_file(repo.path().join("link")).unwrap();
    std::os::unix::fs::symlink("elsewhere.txt", repo.path().join("link")).unwrap();
    std::os::unix::fs::symlink("target.txt", repo.path().join("new-link")).unwrap();
    repo.commit("c2");

    let files = changes(&repo, "HEAD~1", "HEAD");
    assert_eq!(files.len(), 2, "{files:#?}");
    let map = by_path(&files);
    let link = map[b"link".as_slice()];
    assert_eq!(link.status, FileStatus::Modified);
    assert_eq!(link.kind, FileKind::Symlink);
    assert_eq!(link.old_mode, Some(Mode::SYMLINK));
    assert_eq!(link.new_mode, Some(Mode::SYMLINK));
    let new_link = map[b"new-link".as_slice()];
    assert_eq!(new_link.status, FileStatus::Added);
    assert_eq!(new_link.kind, FileKind::Symlink);
}

#[test]
fn diff_tree_parses_hostile_paths() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    let into_src: &[u8] = b"plain-src.txt";
    let into_dst: &[u8] = b"dest \"q\"\tx\n.txt";
    let out_src: &[u8] = "src\n🙂 x.txt".as_bytes();
    let out_dst: &[u8] = b"plain-dest.txt";
    let added: [&[u8]; 7] = [
        b"quo\"te.txt",
        b"tab\there.txt",
        b"new\nline.txt",
        "emoji 🙂/naïve.txt".as_bytes(),
        b"-leading-dash.txt",
        b"back\\slash.txt",
        b"trailing space ",
    ];

    let base = mktree(
        &repo,
        &[
            file(&repo, b"with space.txt", b"v1\n"),
            file(&repo, into_src, &ten_lines("into")),
            file(&repo, out_src, &ten_lines("out")),
        ],
    );
    let mut head_entries = vec![
        file(&repo, b"with space.txt", b"v2\n"),
        file(&repo, into_dst, &ten_lines("into")),
        file(&repo, out_dst, &ten_lines("out")),
    ];
    for p in added {
        head_entries.push(file(&repo, p, p));
    }
    let head = mktree(&repo, &head_entries);

    let files = classified(&repo, &base, &head, &[]);
    assert_eq!(files.len(), 3 + added.len(), "{files:#?}");
    for (i, f) in files.iter().enumerate() {
        assert_eq!(f.idx as usize, i);
    }
    let map = by_path(&files);

    let modified = map[b"with space.txt".as_slice()];
    assert_eq!(modified.status, FileStatus::Modified);
    assert_eq!(modified.display_path(), "with space.txt");

    let into = map[into_dst];
    assert_eq!(into.status, FileStatus::Renamed);
    assert_eq!(into.old_path, path(into_src));
    assert_eq!(into.new_path, path(into_dst));
    assert_eq!(into.display_path(), "dest \"q\"\tx\n.txt");
    assert_eq!(into.similarity, Some(100));

    let out = map[out_dst];
    assert_eq!(out.status, FileStatus::Renamed);
    assert_eq!(out.old_path, path(out_src));
    assert_eq!(out.old_path.as_ref().unwrap().text, "src\n🙂 x.txt");

    for p in added {
        let f = map
            .get(p)
            .unwrap_or_else(|| panic!("missing {:?}", String::from_utf8_lossy(p)));
        assert_eq!(f.status, FileStatus::Added);
        let np = f.new_path.as_ref().unwrap();
        assert!(!np.escaped, "{np:?}");
        assert_eq!(np.text.as_bytes(), p);
        assert_eq!(f.new_blob.as_str(), blob(&repo, p));
        assert_eq!(f.kind, FileKind::Text);
    }
}

#[test]
fn diff_tree_non_utf8_path_is_escaped_and_flagged() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    let old_raw: &[u8] = b"old\xff.txt";
    let bin_raw: &[u8] = b"caf\xe9.dat";
    let gen_raw: &[u8] = b"gen\xfe/app.min.js";
    let base = mktree(&repo, &[file(&repo, old_raw, &ten_lines("latin"))]);
    let head = mktree(
        &repo,
        &[
            file(&repo, b".gitattributes", b"*.dat binary\n"),
            file(&repo, b"new.txt", &ten_lines("latin")),
            file(&repo, bin_raw, b"data\n"),
            file(&repo, gen_raw, b"x\n"),
        ],
    );

    let files = classified(&repo, &base, &head, &[]);
    assert_eq!(files.len(), 4, "{files:#?}");
    let map = by_path(&files);

    let renamed = map[b"new.txt".as_slice()];
    assert_eq!(renamed.status, FileStatus::Renamed);
    let old = renamed.old_path.as_ref().unwrap();
    assert!(old.escaped);
    assert_eq!(old.text, r#""old\377.txt""#);
    assert_eq!(old.to_bytes(), old_raw);
    assert!(!renamed.new_path.as_ref().unwrap().escaped);

    // Attributes and the built-in list still apply to non-UTF-8 paths: the raw
    // bytes go to `check-attr`, not the escaped text.
    let bin = map[bin_raw];
    let np = bin.new_path.as_ref().unwrap();
    assert!(np.escaped);
    assert_eq!(np.text, r#""caf\351.dat""#);
    assert_eq!(bin.display_path(), r#""caf\351.dat""#);
    assert_eq!(bin.kind, FileKind::Binary);

    let generated = map[gen_raw];
    assert!(generated.new_path.as_ref().unwrap().escaped);
    assert!(generated.generated);
}

#[test]
fn diff_tree_ignores_user_rename_limit() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    for i in 0..4 {
        repo.write(&format!("src/f{i}.txt"), &ten_lines(&format!("file{i}")));
    }
    repo.commit("c1");
    for i in 0..4 {
        std::fs::remove_file(repo.path().join(format!("src/f{i}.txt"))).unwrap();
        let mut body = ten_lines(&format!("file{i}"));
        body.extend_from_slice(b"changed tail\n");
        repo.write(&format!("dst/g{i}.txt"), &body);
    }
    repo.commit("c2");
    repo.git(&["config", "diff.renameLimit", "1"]);
    repo.git(&["config", "diff.renames", "false"]);

    let files = changes(&repo, "HEAD~1", "HEAD");
    assert_eq!(files.len(), 4, "{files:#?}");
    for f in &files {
        assert_eq!(f.status, FileStatus::Renamed, "{f:#?}");
        assert!(f.similarity.unwrap() >= 50);
    }
}

#[test]
fn diff_tree_ignores_ext_diff_and_textconv() {
    let sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    let marker = sb.home().join("driver-ran");
    let driver = sb.home().join("driver.sh");
    write_script(
        &driver,
        &format!("touch '{}'\ncat \"$1\" 2>/dev/null", marker.display()),
    );
    let driver = driver.to_str().unwrap();
    repo.git(&["config", "diff.external", driver]);
    repo.git(&["config", "diff.mark.textconv", driver]);
    repo.git(&["config", "diff.mark.command", driver]);
    repo.write(".gitattributes", b"* diff=mark\n");
    repo.write("a.txt", b"one\n");
    repo.commit("c1");
    repo.write("a.txt", b"two\n");
    repo.commit("c2");

    // The drivers are live: porcelain diff runs them.
    repo.git(&["diff", "HEAD~1", "HEAD"]);
    assert!(marker.exists(), "fixture driver did not run");
    std::fs::remove_file(&marker).unwrap();

    let files = classified(&repo, &tree(&repo, "HEAD~1"), &tree(&repo, "HEAD"), &[]);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].status, FileStatus::Modified);
    // `diff=mark` is a driver name, not `-diff`: still text.
    assert_eq!(files[0].kind, FileKind::Text);
    assert!(!marker.exists(), "an external diff or textconv driver ran");
}

#[test]
fn merge_commit_diffs_first_parent() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"one\n");
    repo.commit("c1");
    repo.branch("feature");
    repo.write("main.txt", b"main\n");
    repo.commit("main work");
    repo.checkout("feature");
    repo.write("feature.txt", b"feature\n");
    repo.commit("feature work");
    repo.checkout("main");
    repo.git(&["merge", "-q", "--no-ff", "-m", "merge feature", "feature"]);

    let info = discover(repo.path()).unwrap();
    let r = resolve(&info, repo.path(), &Source::Commit { rev: "HEAD".into() }).unwrap();
    let head = match &r.head {
        polygloss_core::git::HeadSpec::Tree(side) => side.tree.clone(),
        polygloss_core::git::HeadSpec::Worktree => panic!("commit source has a tree head"),
    };
    let files = list_changes(&Git::new(repo.path()), r.object_format, &r.base.tree, &head).unwrap();
    assert_eq!(files.len(), 1, "{files:#?}");
    assert_eq!(files[0].status, FileStatus::Added);
    assert_eq!(files[0].display_path(), "feature.txt");
}

// ---------------------------------------------------------------- classify

#[test]
fn generated_builtin_list_and_attribute() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    let generated = [
        "package-lock.json",
        "web/yarn.lock",
        "pnpm-lock.yaml",
        "bun.lock",
        "bun.lockb",
        "Cargo.lock",
        "ruby/Gemfile.lock",
        "poetry.lock",
        "composer.lock",
        "go.sum",
        "dist/app.min.js",
        "dist/app.min.css",
        "dist/app.js.map",
        "api/v1/service.pb.go",
        "proto/msg_pb2.py",
        // `linguist-generated` from the head tree's .gitattributes.
        "gen/deep/out.ts",
        "schema.x",
        // Setting patterns (`diff.generated_patterns`) extend the list.
        "tests/__snapshots__/a.snap",
        "docs/api/index.html",
    ];
    let not_generated = [
        "src/main.rs",
        "yarn.lock.txt",
        "Cargo.lock.bak",
        "app.min.jsx",
        "minjs",
        // An explicit `-linguist-generated` / `=false` beats the built-in list.
        "vendor/yarn.lock",
        "keep/app.min.js",
        "docs/index.html",
    ];
    repo.write("seed.txt", b"seed\n");
    repo.commit("c1");
    repo.write(
        ".gitattributes",
        b"gen/** linguist-generated\n*.x linguist-generated=true\nvendor/yarn.lock -linguist-generated\nkeep/*.min.js linguist-generated=false\n",
    );
    for p in generated.iter().chain(&not_generated) {
        repo.write(p, format!("{p}\n").as_bytes());
    }
    repo.commit("c2");

    let extra = vec!["*.snap".to_owned(), "docs/api/**".to_owned()];
    let files = classified(&repo, &tree(&repo, "HEAD~1"), &tree(&repo, "HEAD"), &extra);
    let map = by_path(&files);
    for p in generated {
        assert!(map[p.as_bytes()].generated, "{p} should be generated");
    }
    for p in not_generated {
        assert!(!map[p.as_bytes()].generated, "{p} should not be generated");
    }
    assert!(!map[b".gitattributes".as_slice()].generated);

    // The contract list names every built-in pattern.
    for want in [
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "bun.lock",
        "bun.lockb",
        "Cargo.lock",
        "Gemfile.lock",
        "poetry.lock",
        "composer.lock",
        "go.sum",
        "*.min.js",
        "*.min.css",
        "*.map",
        "*.pb.go",
        "*_pb2.py",
    ] {
        assert!(BUILTIN_GENERATED.contains(&want), "missing {want}");
    }
}

#[test]
fn binary_attribute_marks_kind() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("old.dat", b"old\n");
    repo.write("seed.txt", b"seed\n");
    repo.commit("c1");
    std::fs::remove_file(repo.path().join("old.dat")).unwrap();
    repo.write(".gitattributes", b"*.dat binary\n*.bin -diff\n*.txt diff\n");
    repo.write("a.dat", b"a\n");
    repo.write("b.bin", b"b\n");
    repo.write("c.txt", b"c\n");
    repo.write("d.png", b"no attribute: the NUL rule decides later\n");
    std::os::unix::fs::symlink("a.dat", repo.path().join("link.dat")).unwrap();
    repo.commit("c2");
    // Uncommitted worktree attributes must not leak in: they come from the head tree.
    repo.write(".gitattributes", b"*.txt binary\n*.png binary\n");

    let files = classified(&repo, &tree(&repo, "HEAD~1"), &tree(&repo, "HEAD"), &[]);
    let map = by_path(&files);
    assert_eq!(map[b"a.dat".as_slice()].kind, FileKind::Binary);
    assert_eq!(map[b"b.bin".as_slice()].kind, FileKind::Binary);
    assert_eq!(map[b"old.dat".as_slice()].kind, FileKind::Binary);
    assert_eq!(map[b"old.dat".as_slice()].status, FileStatus::Deleted);
    assert_eq!(map[b"c.txt".as_slice()].kind, FileKind::Text);
    assert_eq!(map[b"d.png".as_slice()].kind, FileKind::Text);
    // Symlinks and submodules keep their kind whatever the attributes say.
    assert_eq!(map[b"link.dat".as_slice()].kind, FileKind::Symlink);
}

#[test]
fn classify_batches_one_check_attr_call() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("seed.txt", b"seed\n");
    repo.commit("c1");
    for i in 0..50 {
        repo.write(&format!("f{i}.txt"), b"x\n");
    }
    repo.commit("c2");
    let git = Git::new(repo.path());
    let head = tree(&repo, "HEAD");

    let mut none: Vec<FileChange> = Vec::new();
    classify(&git, &head, &mut none, &[]).unwrap();
    assert_eq!(git_spawns("check-attr"), 0, "no files, no git");

    let mut files = list_changes(&git, ObjectFormat::Sha1, &tree(&repo, "HEAD~1"), &head).unwrap();
    assert_eq!(files.len(), 50);
    classify(&git, &head, &mut files, &[]).unwrap();
    assert_eq!(git_spawns("check-attr"), 1);
    assert_eq!(git_spawns("diff-tree"), 1);
}

fn real_git() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join("git"))
        .find(|p| p.is_file())
        .expect("git on PATH")
}

#[test]
fn classify_on_git_2_39_uses_builtin_list_only() {
    let sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("seed.txt", b"seed\n");
    repo.commit("c1");
    repo.write(
        ".gitattributes",
        b"*.dat binary\ngen/** linguist-generated\n",
    );
    repo.write("a.dat", b"a\n");
    repo.write("gen/out.ts", b"x\n");
    repo.write("yarn.lock", b"y\n");
    repo.write("x.snap", b"s\n");
    repo.commit("c2");

    // A git that reports 2.39 (no `check-attr --source`) and otherwise delegates.
    let wrapper = sb.home().join("git-2.39");
    write_script(
        &wrapper,
        &format!(
            "for a in \"$@\"; do [ \"$a\" = --version ] && {{ echo 'git version 2.39.5 (Apple Git-154)'; exit 0; }}; done\nexec '{}' \"$@\"",
            real_git().display()
        ),
    );
    set_env("POLYGLOSS_GIT_BIN", &wrapper);

    let extra = vec!["*.snap".to_owned()];
    for _ in 0..2 {
        let files = classified(&repo, &tree(&repo, "HEAD~1"), &tree(&repo, "HEAD"), &extra);
        let map = by_path(&files);
        assert_eq!(map[b"a.dat".as_slice()].kind, FileKind::Text);
        assert!(!map[b"gen/out.ts".as_slice()].generated);
        assert!(map[b"yarn.lock".as_slice()].generated);
        assert!(map[b"x.snap".as_slice()].generated);
    }
    assert_eq!(git_spawns("check-attr"), 0);
}
