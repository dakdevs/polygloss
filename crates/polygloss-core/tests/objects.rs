//! In-process blob reads with gix, including scratch stores reached through
//! `info/alternates` (T1.4, design §5.1, §6.1, library-choices §3b). Every test runs
//! under `Sandbox::isolate()`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use polygloss_core::git::{Git, discover};
use polygloss_core::objects::{BlobReader, ObjectError, is_binary};
use polygloss_core::testing::{FixtureRepo, Sandbox};
use polygloss_core::{ObjectFormat, Oid};

fn reader(repo: &FixtureRepo) -> BlobReader {
    BlobReader::open(&discover(repo.path()).unwrap()).unwrap()
}

/// `git count-objects -v` (loose and packed counts), to prove nothing was written
/// or fetched.
fn count_objects(repo: &FixtureRepo) -> String {
    repo.git(&["count-objects", "-v"])
}

/// A scratch objects dir outside the worktree, like T1.5's layout.
fn scratch_objects(sb: &Sandbox) -> PathBuf {
    let dir = sb
        .cache_dir()
        .join("scratch")
        .join("repo-hash")
        .join("objects");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes `bytes` as a blob into `objects` only (the repo's own store is an
/// alternate, never written), through the production runner.
fn hash_into(worktree: &Path, repo_objects: &Path, objects: &Path, bytes: &[u8]) -> Oid {
    let out = Git::new(worktree)
        .with_env("GIT_OBJECT_DIRECTORY", objects)
        .with_env("GIT_ALTERNATE_OBJECT_DIRECTORIES", repo_objects)
        .output_stdin(
            &[
                OsStr::new("hash-object"),
                OsStr::new("-w"),
                OsStr::new("--stdin"),
            ],
            bytes,
        )
        .unwrap();
    let text = String::from_utf8(out).unwrap();
    let fmt = discover(worktree).unwrap().object_format;
    Oid::parse(text.trim_end(), fmt).unwrap()
}

fn assert_missing(result: Result<Arc<[u8]>, ObjectError>, oid: &Oid) {
    match result {
        Err(ObjectError::Missing(missing)) => assert_eq!(&missing, oid),
        other => panic!("expected Missing({oid}), got {other:?}"),
    }
}

fn blob_roundtrip(fmt: ObjectFormat) {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(fmt);
    repo.write("a.txt", b"alpha\n");
    repo.write("dir/b.bin", b"bin\0ary\n");
    repo.commit("c1");
    repo.write("a.txt", b"alpha\nbeta\n");
    repo.commit("c2");
    let v1 = repo.oid("HEAD~1:a.txt");
    let v2 = repo.oid("HEAD:a.txt");
    let bin = repo.oid("HEAD:dir/b.bin");
    assert_eq!(v1.object_format(), fmt);

    // Loose objects.
    let blobs = reader(&repo);
    assert_eq!(&*blobs.read(&v1).unwrap(), b"alpha\n");
    assert_eq!(&*blobs.read(&v2).unwrap(), b"alpha\nbeta\n");
    assert_eq!(&*blobs.read(&bin).unwrap(), b"bin\0ary\n");
    assert!(blobs.exists(&v2));
    assert_eq!(blobs.size(&v2).unwrap(), 11);

    // Packed objects (deltas included), through a fresh reader and the old one.
    repo.git(&["repack", "-adq", "--depth=50", "--window=10"]);
    for blobs in [reader(&repo), blobs] {
        assert_eq!(&*blobs.read(&v1).unwrap(), b"alpha\n");
        assert_eq!(&*blobs.read(&v2).unwrap(), b"alpha\nbeta\n");
        assert_eq!(blobs.size(&v1).unwrap(), 6);
    }
}

#[test]
fn blob_read_sha1() {
    blob_roundtrip(ObjectFormat::Sha1);
}

#[test]
fn blob_read_sha256() {
    blob_roundtrip(ObjectFormat::Sha256);
}

#[test]
fn blob_read_from_linked_worktree_uses_common_objects() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"shared\n");
    repo.commit("c1");
    let wt = repo.add_worktree("feature");
    let info = discover(&wt).unwrap();
    assert_ne!(info.git_dir, info.common_dir);
    let blobs = BlobReader::open(&info).unwrap();
    assert_eq!(&*blobs.read(&repo.oid("HEAD:a.txt")).unwrap(), b"shared\n");
}

#[test]
fn blob_read_from_scratch_store_with_alternates() {
    let sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"committed\n");
    repo.commit("c1");
    let committed = repo.oid("HEAD:a.txt");
    let info = discover(repo.path()).unwrap();
    let repo_objects = info.common_dir.join("objects");
    let scratch = scratch_objects(&sb);
    let before = count_objects(&repo);

    let only_scratch = hash_into(repo.path(), &repo_objects, &scratch, b"scratch only\n");
    assert_eq!(
        count_objects(&repo),
        before,
        "scratch writes stay out of the repo"
    );

    let blobs = BlobReader::open(&info).unwrap();
    assert!(
        !blobs.exists(&only_scratch),
        "the repo reader never sees scratch"
    );
    assert_missing(blobs.read(&only_scratch), &only_scratch);

    let live = blobs.with_scratch(&scratch).unwrap();
    assert_eq!(&*live.read(&only_scratch).unwrap(), b"scratch only\n");
    assert_eq!(
        &*live.read(&committed).unwrap(),
        b"committed\n",
        "via alternates"
    );
    assert!(live.exists(&committed));
    assert_eq!(live.size(&only_scratch).unwrap(), 13);
    // The base reader is unchanged by deriving a scratch reader.
    assert!(!blobs.exists(&only_scratch));

    // The alternates file points at the repo's objects dir, so git reads both too.
    let alternates = std::fs::read_to_string(scratch.join("info/alternates")).unwrap();
    assert_eq!(alternates, format!("{}\n", repo_objects.display()));
    let shown = Git::new(repo.path())
        .with_env("GIT_OBJECT_DIRECTORY", &scratch)
        .output(&[
            OsStr::new("cat-file"),
            OsStr::new("blob"),
            OsStr::new(committed.as_str()),
        ])
        .unwrap();
    assert_eq!(shown, b"committed\n");

    // Deriving again (another worktree of the same repo) keeps one entry.
    let again = blobs.with_scratch(&scratch).unwrap();
    assert_eq!(&*again.read(&only_scratch).unwrap(), b"scratch only\n");
    assert_eq!(
        std::fs::read_to_string(scratch.join("info/alternates")).unwrap(),
        alternates
    );
    assert_eq!(count_objects(&repo), before);
}

#[test]
fn blob_read_from_scratch_sees_objects_added_later() {
    let sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha256);
    repo.write("a.txt", b"one\n");
    repo.commit("c1");
    let info = discover(repo.path()).unwrap();
    let repo_objects = info.common_dir.join("objects");
    let scratch = scratch_objects(&sb);
    let live = BlobReader::open(&info)
        .unwrap()
        .with_scratch(&scratch)
        .unwrap();

    // A later snapshot writes new loose objects into the same scratch store.
    let later = hash_into(repo.path(), &repo_objects, &scratch, b"later\n");
    assert_eq!(&*live.read(&later).unwrap(), b"later\n");
}

#[test]
fn blob_read_from_scratch_with_hostile_repo_path() {
    let sb = Sandbox::isolate();
    let parent = sb.home().join("we\"ird\nrepo \\ dir");
    std::fs::create_dir_all(&parent).unwrap();
    let path = parent.join("repo");
    let init = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(init.success());
    let info = discover(&path).unwrap();
    let repo_objects = info.common_dir.join("objects");
    let in_repo = hash_into_repo(&path, b"in repo\n");
    let scratch = scratch_objects(&sb);
    let in_scratch = hash_into(&path, &repo_objects, &scratch, b"in scratch\n");

    let live = BlobReader::open(&info)
        .unwrap()
        .with_scratch(&scratch)
        .unwrap();
    assert_eq!(&*live.read(&in_repo).unwrap(), b"in repo\n");
    assert_eq!(&*live.read(&in_scratch).unwrap(), b"in scratch\n");

    // git parses the quoted alternates entry the same way.
    let shown = Git::new(&path)
        .with_env("GIT_OBJECT_DIRECTORY", &scratch)
        .output(&[
            OsStr::new("cat-file"),
            OsStr::new("blob"),
            OsStr::new(in_repo.as_str()),
        ])
        .unwrap();
    assert_eq!(shown, b"in repo\n");
}

fn hash_into_repo(worktree: &Path, bytes: &[u8]) -> Oid {
    let out = Git::new(worktree)
        .output_stdin(
            &[
                OsStr::new("hash-object"),
                OsStr::new("-w"),
                OsStr::new("--stdin"),
            ],
            bytes,
        )
        .unwrap();
    Oid::parse(
        String::from_utf8(out).unwrap().trim_end(),
        ObjectFormat::Sha1,
    )
    .unwrap()
}

#[test]
fn with_scratch_requires_an_existing_store() {
    let sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"one\n");
    repo.commit("c1");
    let blobs = reader(&repo);
    let absent = sb.cache_dir().join("no-such-scratch/objects");
    match blobs.with_scratch(&absent) {
        Err(ObjectError::Io(err)) => assert_eq!(err.kind(), std::io::ErrorKind::NotFound),
        Err(other) => panic!("expected Io(NotFound), got {other:?}"),
        Ok(_) => panic!("expected Io(NotFound), got a reader"),
    }
    assert!(!absent.exists(), "with_scratch never creates the store");
}

#[test]
fn blob_missing_is_missing_error() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"one\n");
    repo.commit("c1");
    let blobs = reader(&repo);

    let absent = Oid::parse(&"ab".repeat(20), ObjectFormat::Sha1).unwrap();
    assert!(!blobs.exists(&absent));
    assert_missing(blobs.read(&absent), &absent);
    assert!(matches!(blobs.size(&absent), Err(ObjectError::Missing(o)) if o == absent));

    // The null id (the old side of an added file) is never an object.
    let zero = Oid::zero(ObjectFormat::Sha1);
    assert!(!blobs.exists(&zero));
    assert_missing(blobs.read(&zero), &zero);

    // An id of the other object format cannot be in this repo.
    let sha256 = Oid::parse(&"cd".repeat(32), ObjectFormat::Sha256).unwrap();
    assert!(!blobs.exists(&sha256));
    assert_missing(blobs.read(&sha256), &sha256);
}

#[test]
fn blob_read_rejects_non_blob_objects() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"one\n");
    let commit = repo.commit("c1");
    let tree = repo.oid("HEAD^{tree}");
    let blobs = reader(&repo);
    for oid in [&commit, &tree] {
        assert!(blobs.exists(oid), "exists() is true for any object kind");
        match blobs.read(oid) {
            Err(ObjectError::Corrupt(msg)) => assert!(msg.contains("not a blob"), "{msg}"),
            other => panic!("expected Corrupt for {oid}, got {other:?}"),
        }
        assert!(matches!(blobs.size(oid), Err(ObjectError::Corrupt(_))));
    }
}

#[test]
fn blob_read_ignores_replace_refs() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"original\n");
    repo.write("b.txt", b"replacement\n");
    repo.commit("c1");
    let original = repo.oid("HEAD:a.txt");
    let replacement = repo.oid("HEAD:b.txt");
    repo.git(&["replace", original.as_str(), replacement.as_str()]);
    assert_eq!(
        repo.git(&["cat-file", "blob", original.as_str()]),
        "replacement"
    );

    assert_eq!(&*reader(&repo).read(&original).unwrap(), b"original\n");
    // gix treats `core.useReplaceRefs` as "disabled" (the GIT_NO_REPLACE_OBJECTS
    // sense); either value must still read the real object.
    for value in ["false", "true"] {
        repo.git(&["config", "core.useReplaceRefs", value]);
        let blobs = reader(&repo);
        assert_eq!(&*blobs.read(&original).unwrap(), b"original\n", "{value}");
        assert_eq!(blobs.size(&original).unwrap(), 9, "{value}");
    }
}

#[test]
fn blob_read_in_blobless_clone_never_fetches() {
    let _sb = Sandbox::isolate();
    let origin = FixtureRepo::init(ObjectFormat::Sha1);
    origin.write("a.txt", b"old contents\n");
    origin.commit("c1");
    let old = origin.oid("HEAD:a.txt");
    origin.write("a.txt", b"new contents\n");
    origin.commit("c2");
    let new = origin.oid("HEAD:a.txt");

    let clone = origin.clone_blobless();
    // The source is reachable over file://, so a fetching reader would succeed.
    assert!(
        !clone
            .git(&["config", "--get", "remote.origin.promisor"])
            .is_empty()
    );
    let before = count_objects(&clone);
    let packs_before = std::fs::read_dir(clone.path().join(".git/objects/pack"))
        .unwrap()
        .count();

    let blobs = reader(&clone);
    assert_eq!(&*blobs.read(&new).unwrap(), b"new contents\n");
    assert!(!blobs.exists(&old));
    assert_missing(blobs.read(&old), &old);
    assert!(matches!(blobs.size(&old), Err(ObjectError::Missing(_))));

    assert_eq!(count_objects(&clone), before, "no objects were fetched");
    assert_eq!(
        std::fs::read_dir(clone.path().join(".git/objects/pack"))
            .unwrap()
            .count(),
        packs_before
    );
}

#[test]
fn blob_reader_is_send_sync_and_parallel() {
    fn assert_traits<T: Clone + Send + Sync + 'static>() {}
    assert_traits::<BlobReader>();

    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    let files: Vec<(String, Vec<u8>)> = (0..40)
        .map(|i| {
            (
                format!("f{i:02}.txt"),
                format!("file {i}\n{}", "line\n".repeat(i * 10)).into_bytes(),
            )
        })
        .collect();
    for (name, bytes) in &files {
        repo.write(name, bytes);
    }
    repo.commit("c1");
    // Half packed, half loose.
    repo.git(&["repack", "-adq"]);
    for (name, bytes) in files.iter().skip(20) {
        let mut changed = bytes.clone();
        changed.extend_from_slice(b"changed\n");
        repo.write(name, &changed);
    }
    repo.commit("c2");
    let expected: Arc<Vec<(Oid, Vec<u8>)>> = Arc::new(
        files
            .iter()
            .enumerate()
            .map(|(i, (name, bytes))| {
                let mut want = bytes.clone();
                if i >= 20 {
                    want.extend_from_slice(b"changed\n");
                }
                (repo.oid(&format!("HEAD:{name}")), want)
            })
            .collect(),
    );

    let blobs = reader(&repo);
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let blobs = blobs.clone();
            let expected = Arc::clone(&expected);
            std::thread::spawn(move || {
                for round in 0..25 {
                    for (i, (oid, want)) in expected.iter().enumerate() {
                        if (i + t + round) % 3 == 0 {
                            continue;
                        }
                        assert_eq!(&*blobs.read(oid).unwrap(), want.as_slice());
                        assert_eq!(blobs.size(oid).unwrap(), want.len() as u64);
                    }
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
fn is_binary_nul_rule() {
    assert!(!is_binary(b""));
    assert!(!is_binary(b"plain text\n"));
    assert!(!is_binary("héllo wörld\n".as_bytes()));
    assert!(is_binary(b"\0"));
    assert!(is_binary(b"PNG\x89\0\0"));

    let mut near_end = vec![b'a'; 9_000];
    near_end[7_999] = 0;
    assert!(is_binary(&near_end), "NUL at byte 8000 (index 7999) counts");
    let mut past_end = vec![b'a'; 9_000];
    past_end[8_000] = 0;
    assert!(
        !is_binary(&past_end),
        "NUL after the first 8000 bytes does not"
    );
}
