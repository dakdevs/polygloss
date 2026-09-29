//! `diff_id` golden vectors, id helpers and review keys (T1.1, design §4).

use std::path::Path;
use std::process::Command;

use polygloss_core::ids::review_key;
use polygloss_core::{DiffId, DiffIdPrefix, IdError, ObjectFormat, Oid, diff_id, new_uuid};

/// The vectors shared with `tests/scripts/diff-id-vectors.test.ts`, which recomputes
/// them with an independent implementation (`Bun.CryptoHasher`).
fn golden(fmt: ObjectFormat) -> (Oid, Oid, String) {
    let json: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/diff-id-vectors.json")).unwrap();
    let v = json["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["object_format"] == fmt.as_str())
        .unwrap_or_else(|| panic!("no {} vector", fmt.as_str()));
    let field = |k: &str| v[k].as_str().unwrap().to_owned();
    (
        Oid::parse(&field("base_tree"), fmt).unwrap(),
        Oid::parse(&field("head_tree"), fmt).unwrap(),
        field("diff_id"),
    )
}

#[test]
fn diff_id_golden_sha1() {
    let (base, head, expected) = golden(ObjectFormat::Sha1);
    assert_eq!(base, ObjectFormat::Sha1.empty_tree());
    let id = diff_id(ObjectFormat::Sha1, &base, &head);
    assert_eq!(id.as_str(), expected);
    assert_eq!(id.short(), &expected[..12]);
}

#[test]
fn diff_id_golden_sha256() {
    let (base, head, expected) = golden(ObjectFormat::Sha256);
    assert_eq!(base, ObjectFormat::Sha256.empty_tree());
    let id = diff_id(ObjectFormat::Sha256, &base, &head);
    assert_eq!(id.as_str(), expected);
    assert_eq!(id.short(), &expected[..12]);
}

/// Runs git in `dir` with a throwaway HOME and config and no inherited repo env,
/// returning trimmed stdout.
fn git(home: &Path, dir: &Path, args: &[&str]) -> String {
    git_env(home, dir, args, &[])
}

fn git_env(home: &Path, dir: &Path, args: &[&str], extra_env: &[(&str, &str)]) -> String {
    let empty_config = home.join("empty-gitconfig");
    std::fs::write(&empty_config, "").unwrap();
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("GIT_CONFIG_GLOBAL", &empty_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Polygloss Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Polygloss Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .envs(extra_env.iter().copied())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[test]
fn diff_id_ignores_nothing_but_trees() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&repo).unwrap();
    git(&home, &repo, &["init", "-q", "--object-format=sha1"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&home, &repo, &["add", "a.txt"]);
    let tree = git(&home, &repo, &["write-tree"]);

    // Two different commits (message, author date) with the same tree, as after
    // an amend or a no-op rebase (ADR-0006).
    let c1 = git(&home, &repo, &["commit-tree", &tree, "-m", "first"]);
    let c2 = git_env(
        &home,
        &repo,
        &["commit-tree", &tree, "-m", "reworded"],
        &[
            ("GIT_AUTHOR_NAME", "Someone Else"),
            ("GIT_COMMITTER_NAME", "Someone Else"),
            ("GIT_AUTHOR_DATE", "2001-01-01T00:00:00Z"),
        ],
    );
    assert_ne!(c1, c2);
    let t1 = git(&home, &repo, &["rev-parse", &format!("{c1}^{{tree}}")]);
    let t2 = git(&home, &repo, &["rev-parse", &format!("{c2}^{{tree}}")]);

    let fmt = ObjectFormat::Sha1;
    let base = fmt.empty_tree();
    let id1 = diff_id(fmt, &base, &Oid::parse(&t1, fmt).unwrap());
    let id2 = diff_id(fmt, &base, &Oid::parse(&t2, fmt).unwrap());
    assert_eq!(
        id1, id2,
        "same trees, different commits must share a diff_id"
    );

    // Every input that is part of the id changes it: head tree, base tree, order.
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    git(&home, &repo, &["add", "a.txt"]);
    let other = Oid::parse(&git(&home, &repo, &["write-tree"]), fmt).unwrap();
    let head = Oid::parse(&t1, fmt).unwrap();
    assert_ne!(diff_id(fmt, &base, &other), id1);
    assert_ne!(diff_id(fmt, &other, &head), id1);
    assert_ne!(diff_id(fmt, &head, &base), id1);
}

#[test]
fn diff_id_is_64_lowercase_hex_and_parses() {
    let fmt = ObjectFormat::Sha256;
    let id = diff_id(fmt, &fmt.empty_tree(), &fmt.empty_tree());
    assert_eq!(id.as_str().len(), 64);
    assert!(
        id.as_str()
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    );
    assert_eq!(DiffId::parse(id.as_str()).unwrap(), id);
    assert_eq!(id.to_string(), id.as_str());
    assert!(DiffId::parse(&id.as_str().to_uppercase()).is_err());
    assert!(DiffId::parse(&id.as_str()[..63]).is_err());
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(serde_json::from_str::<DiffId>(&json).unwrap(), id);
    assert!(serde_json::from_str::<DiffId>("\"abc\"").is_err());
}

#[test]
fn diff_id_prefix_requires_eight_hex_chars() {
    let (base, head, expected) = golden(ObjectFormat::Sha1);
    let id = diff_id(ObjectFormat::Sha1, &base, &head);

    assert!(matches!(
        DiffIdPrefix::parse("07d807d"),
        Err(IdError::PrefixTooShort(7))
    ));
    assert!(matches!(
        DiffIdPrefix::parse("07d807dz"),
        Err(IdError::NotHex(_))
    ));
    assert!(
        DiffIdPrefix::parse(&format!("{expected}0")).is_err(),
        "longer than 64"
    );

    let p = DiffIdPrefix::parse("07d807de").unwrap();
    assert_eq!(p.as_str(), "07d807de");
    assert!(p.matches(&id));
    // Upper case input is accepted and normalized; the full id is a prefix too.
    let upper = DiffIdPrefix::parse(&expected[..12].to_uppercase()).unwrap();
    assert_eq!(upper.as_str(), &expected[..12]);
    assert!(upper.matches(&id));
    assert!(DiffIdPrefix::parse(&expected).unwrap().matches(&id));
    assert!(!DiffIdPrefix::parse("ffffffff").unwrap().matches(&id));
}

#[test]
fn new_uuid_is_v7_text() {
    let a = new_uuid();
    let b = new_uuid();
    assert_ne!(a, b);
    for u in [&a, &b] {
        assert_eq!(u.len(), 36);
        assert_eq!(u.as_bytes()[14], b'7', "version nibble of {u}");
        assert!(
            u.bytes()
                .all(|c| c == b'-' || c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
    // v7 text sorts by creation time (millisecond prefix).
    assert!(a[..8] <= b[..8]);
}

#[test]
fn review_key_live_detached() {
    let wt = Path::new("/Users/d/src/app");
    assert_eq!(
        review_key::live(wt, Some("feature/login"), "merge-base"),
        "worktree:/Users/d/src/app@feature/login#since=merge-base"
    );
    assert_eq!(
        review_key::live(wt, None, "HEAD"),
        "worktree:/Users/d/src/app@detached#since=HEAD"
    );
    let oid = "9f1c000000000000000000000000000000000000";
    assert_eq!(
        review_key::live(wt, None, oid),
        format!("worktree:/Users/d/src/app@detached#since={oid}")
    );
}

#[test]
fn review_key_compare_three_dot_and_direct() {
    assert_eq!(
        review_key::compare(
            "refs/remotes/origin/main",
            "refs/heads/feature/login",
            false
        ),
        "compare:refs/remotes/origin/main...refs/heads/feature/login"
    );
    assert_eq!(
        review_key::compare("refs/heads/main", "refs/heads/spike", true),
        "compare:refs/heads/main..refs/heads/spike"
    );
}

#[test]
fn review_key_commit() {
    let oid = Oid::parse(
        "853694aae8816094a0d875fee7ea26278dbf5d0f",
        ObjectFormat::Sha1,
    )
    .unwrap();
    assert_eq!(
        review_key::commit(&oid),
        "commit:853694aae8816094a0d875fee7ea26278dbf5d0f"
    );
}
