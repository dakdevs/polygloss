//! Ref and commit listings for the open flow (T3.5, design §11.3): `for-each-ref`
//! over branches, remote branches and tags, and a paged `log -z`; and the header
//! card's commit details and ranges (T6.13, design §11.6). Every test runs under
//! `Sandbox::isolate()`.

use polygloss_core::git::Git;
use polygloss_core::git::listing::{
    CommitDetails, CommitInfo, RefInfo, RefKind, commit_details, list_commits, list_refs,
    range_commits,
};
use polygloss_core::testing::{FixtureRepo, Sandbox};
use polygloss_core::{ObjectFormat, Oid};

fn names(refs: &[RefInfo]) -> Vec<&str> {
    refs.iter().map(|r| r.name.as_str()).collect()
}

fn find<'a>(refs: &'a [RefInfo], name: &str) -> &'a RefInfo {
    refs.iter()
        .find(|r| r.name == name)
        .unwrap_or_else(|| panic!("no ref {name} in {:?}", names(refs)))
}

#[test]
fn listing_refs_heads_remotes_tags() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"one\n");
    let c1 = repo.commit("c1");
    repo.git(&["tag", "v1"]);
    repo.branch("feature/login");
    repo.write("a.txt", b"two\n");
    let c2 = repo.commit("c2: second change");
    repo.git(&["tag", "-a", "v2", "-m", "Release two"]);
    // A remote-tracking branch and origin/HEAD pointing at it (a symref).
    repo.git(&["update-ref", "refs/remotes/origin/main", c1.as_str()]);
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    // Refs that are not branches or tags never show.
    repo.git(&["update-ref", "refs/polygloss/snapshots/x", c1.as_str()]);
    repo.git(&["update-ref", "refs/notes/commits", c1.as_str()]);
    // A tag pointing at a tree: not a commit, so it cannot be compared.
    let tree = repo.git(&["rev-parse", "HEAD^{tree}"]);
    repo.git(&["tag", "tree-tag", &tree]);

    let refs = list_refs(&Git::new(repo.path())).expect("list refs");

    // Branches, then remote branches, then tags; the newest first inside each
    // kind (ties by name). The symref and the other namespaces are left out.
    assert_eq!(
        names(&refs),
        [
            "refs/heads/main",
            "refs/heads/feature/login",
            "refs/remotes/origin/main",
            "refs/tags/v2",
            "refs/tags/v1",
        ]
    );
    let kinds: Vec<RefKind> = refs.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        [
            RefKind::Branch,
            RefKind::Branch,
            RefKind::RemoteBranch,
            RefKind::Tag,
            RefKind::Tag,
        ]
    );

    let main = find(&refs, "refs/heads/main");
    assert_eq!(main.short_name, "main");
    assert_eq!(main.commit, c2);
    assert!(main.is_head, "main is checked out");
    assert_eq!(main.subject, "c2: second change");
    assert_eq!(main.committed_at, 1_767_225_600 + 2 * 60);

    let feature = find(&refs, "refs/heads/feature/login");
    assert_eq!(feature.short_name, "feature/login");
    assert_eq!(feature.commit, c1);
    assert!(!feature.is_head);

    let origin = find(&refs, "refs/remotes/origin/main");
    assert_eq!(origin.short_name, "origin/main");
    assert_eq!(origin.commit, c1);

    // An annotated tag is peeled to its commit; its subject is the commit's.
    let v2 = find(&refs, "refs/tags/v2");
    assert_eq!(v2.short_name, "v2");
    assert_eq!(v2.commit, c2);
    assert_eq!(v2.subject, "c2: second change");
    let v1 = find(&refs, "refs/tags/v1");
    assert_eq!(v1.commit, c1);
    assert_eq!(v1.subject, "c1");

    // A detached HEAD marks no branch.
    repo.git(&["checkout", "-q", "--detach", "HEAD"]);
    let refs = list_refs(&Git::new(repo.path())).unwrap();
    assert!(refs.iter().all(|r| !r.is_head));
}

#[test]
fn listing_refs_in_a_repo_without_commits_is_empty() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha256);
    assert!(list_refs(&Git::new(repo.path())).unwrap().is_empty());
    assert!(
        list_commits(&Git::new(repo.path()), 0, 50)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn listing_log_parses_nul_fields() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha256);
    repo.write("a.txt", b"one\n");
    let root = repo.commit("root");
    repo.branch("side");
    repo.write("a.txt", b"two\n");
    // Subjects and names keep spaces, tabs, quotes and non-ASCII exactly; a
    // body never leaks into the subject.
    repo.git(&["add", "-A"]);
    repo.git(&[
        "commit",
        "-q",
        "--author",
        "Zoë Q. Public <zoe@example.invalid>",
        "-m",
        "fix:\tparse \"quoted\" ✓ values",
        "-m",
        "The body.\nSecond line.",
    ]);
    let fix = repo.oid("HEAD");
    repo.checkout("side");
    repo.write("b.txt", b"side\n");
    let side = repo.commit("side work");
    repo.checkout("main");
    repo.git(&["merge", "-q", "--no-ff", "-m", "Merge side", "side"]);
    let merge = repo.oid("HEAD");

    let git = Git::new(repo.path());
    let all = list_commits(&git, 0, 100).expect("log");
    let oids: Vec<&Oid> = all.iter().map(|c| &c.oid).collect();
    assert_eq!(all.len(), 4);
    assert_eq!(oids[0], &merge, "newest first");
    assert_eq!(oids.last().copied(), Some(&root));

    let by = |oid: &Oid| -> &CommitInfo { all.iter().find(|c| &c.oid == oid).unwrap() };
    let m = by(&merge);
    assert_eq!(m.parents, [fix.clone(), side.clone()], "first parent first");
    assert_eq!(m.subject, "Merge side");
    assert_eq!(m.tree, repo.oid("HEAD^{tree}"));

    let f = by(&fix);
    assert_eq!(f.subject, "fix:\tparse \"quoted\" ✓ values");
    assert_eq!(f.author, "Zoë Q. Public");
    assert_eq!(f.parents, std::slice::from_ref(&root));
    assert_eq!(f.tree, repo.oid(&format!("{}^{{tree}}", fix.as_str())));
    let at = repo.git(&["log", "-1", "--format=%at", fix.as_str()]);
    assert_eq!(f.authored_at, at.parse::<i64>().unwrap());

    let r = by(&root);
    assert!(r.parents.is_empty(), "the root commit has no parents");
    assert_eq!(r.author, "Polygloss Fixture");

    // Pages: `skip` and `limit` walk the same order.
    let page1 = list_commits(&git, 0, 2).unwrap();
    let page2 = list_commits(&git, 2, 2).unwrap();
    let page3 = list_commits(&git, 4, 2).unwrap();
    let paged: Vec<CommitInfo> = page1.into_iter().chain(page2).collect();
    assert_eq!(paged, all);
    assert!(page3.is_empty());
    assert!(list_commits(&git, 0, 0).unwrap().is_empty());
}

#[test]
fn commit_details_reads_author_email_and_time() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"one\n");
    repo.commit("base");
    repo.write("a.txt", b"two\n");
    repo.git(&["add", "-A"]);
    // The fixture's second commit date: its epoch plus one minute. The
    // email keeps its case; the subject its tab, quotes and non-ASCII; the
    // body stays out.
    repo.git(&[
        "commit",
        "-q",
        "--author",
        "Zoë Q. Public <Zoe@Example.invalid>",
        "-m",
        "fix:\tparse \"quoted\" ✓ values",
        "-m",
        "The body.",
    ]);
    let head = repo.oid("HEAD");

    let details = commit_details(&Git::new(repo.path()), &head).expect("commit details");
    assert_eq!(
        details,
        CommitDetails {
            oid: head,
            subject: "fix:\tparse \"quoted\" ✓ values".to_owned(),
            author_name: "Zoë Q. Public".to_owned(),
            author_email: "Zoe@Example.invalid".to_owned(),
            committed_at: 1_767_225_660,
        }
    );

    // An id that names no commit is an error, not an empty answer.
    let missing = Oid::parse(&"1".repeat(40), ObjectFormat::Sha1).unwrap();
    assert!(commit_details(&Git::new(repo.path()), &missing).is_err());
}

#[test]
fn range_commits_lists_base_to_head_newest_first_with_a_total() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha256);
    repo.write("a.txt", b"0\n");
    let base = repo.commit("base");
    repo.branch("feature");
    repo.checkout("feature");
    let mut made = Vec::new();
    for i in 1..=5 {
        repo.write("a.txt", format!("{i}\n").as_bytes());
        made.push(repo.commit(&format!("step {i}")));
    }
    // A commit on main after the fork is not in `base..head`.
    repo.checkout("main");
    repo.write("b.txt", b"main\n");
    repo.commit("main moves on");
    let head = made[4].clone();
    let git = Git::new(repo.path());

    let (three, total) = range_commits(&git, &base, &head, 3).expect("range");
    assert_eq!(total, 5, "every commit of the range is counted");
    let subjects: Vec<&str> = three.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, ["step 5", "step 4", "step 3"], "newest first");
    assert_eq!(three[0].oid, made[4]);
    assert_eq!(three[2].oid, made[2]);
    assert_eq!(three[0].author_email, "fixture@polygloss.invalid");
    assert_eq!(three[0].committed_at, 1_767_225_600 + 6 * 60);

    let (all, total) = range_commits(&git, &base, &head, 50).unwrap();
    assert_eq!((all.len(), total), (5, 5));
    // An empty range.
    assert_eq!(
        range_commits(&git, &head, &head, 50).unwrap(),
        (Vec::new(), 0)
    );
    // No limit: the count alone.
    assert_eq!(
        range_commits(&git, &base, &head, 0).unwrap(),
        (Vec::new(), 5)
    );
}
