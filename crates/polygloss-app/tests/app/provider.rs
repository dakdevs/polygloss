//! `CoreDiffProvider`: core's opened diffs and blob reader behind the
//! viewport's `DiffProvider` trait.

use std::sync::Arc;

use polygloss_app::CoreDiffProvider;
use polygloss_core::categories::{CategoriesConfig, Categorizer};
use polygloss_core::git::{Since, Source};
use polygloss_core::objects::BlobReader;
use polygloss_diff::{ObjectFormat, Oid};
use polygloss_viewport::DiffProvider;

use crate::support::{
    CONFIG_RS_BASE, CONFIG_RS_HEAD, FixtureRepo, Sandbox, code_change_repo, open, open_compare,
};

/// The categorizer of the default settings (design §11.15).
fn defaults() -> Categorizer {
    Categorizer::new(&CategoriesConfig::default(), &[]).expect("the defaults compile")
}

fn path_of(change: &polygloss_diff::FileChange) -> String {
    change
        .new_path
        .as_ref()
        .or(change.old_path.as_ref())
        .map(|p| p.text.to_string())
        .unwrap_or_default()
}

#[test]
fn core_provider_reads_blobs_for_fixture_repo() {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let opened = open_compare(repo.path());
    let blobs = BlobReader::open(&opened.repo).expect("open the blob reader");
    let provider = CoreDiffProvider::new(&opened, blobs, &defaults());

    assert_eq!(provider.object_format(), ObjectFormat::Sha1);
    let files = provider.files();
    assert!(
        Arc::ptr_eq(&files, &opened.files),
        "files are shared, not copied"
    );
    assert_eq!(
        files.iter().map(path_of).collect::<Vec<_>>(),
        ["src/config.rs", "src/greet.ts", "src/main.rs"]
    );

    let config = &files[0];
    let old = provider.load_blob(&config.old_blob).expect("old blob");
    let new = provider.load_blob(&config.new_blob).expect("new blob");
    assert_eq!(&*old, CONFIG_RS_BASE.as_bytes());
    assert_eq!(&*new, CONFIG_RS_HEAD.as_bytes());
    assert_eq!(
        provider.blob_size(&config.new_blob).expect("size"),
        CONFIG_RS_HEAD.len() as u64
    );

    let absent = Oid::parse(&"ab".repeat(20), ObjectFormat::Sha1).unwrap();
    let err = provider.load_blob(&absent).expect_err("absent blob");
    assert!(
        err.to_string().contains(absent.as_str()),
        "the error names the missing object: {err}"
    );
    assert!(provider.blob_size(&absent).is_err());
}

#[test]
fn core_provider_reports_sha256_repos() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha256);
    repo.write("a.txt", b"one\n");
    repo.commit("base");
    repo.git(&["tag", "base"]);
    repo.write("a.txt", b"two\n");
    repo.commit("head");
    repo.git(&["tag", "head"]);

    let provider =
        CoreDiffProvider::open(&open_compare(repo.path()), &defaults()).expect("open provider");
    assert_eq!(provider.object_format(), ObjectFormat::Sha256);
    let change = &provider.files()[0];
    assert_eq!(&*provider.load_blob(&change.new_blob).unwrap(), b"two\n");
}

#[test]
fn core_provider_open_reads_live_blobs_from_the_scratch_store() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"committed\n");
    repo.commit("init");
    repo.write("a.txt", b"edited, not committed\n");

    let opened = open(repo.path(), Source::Live { since: Since::Head });
    assert!(opened.live.is_some());
    let change = &opened.files[0];

    // The working-tree blob lives only in the snapshot's scratch store.
    let plain = BlobReader::open(&opened.repo).unwrap();
    assert!(plain.read(&change.new_blob).is_err());

    let provider = CoreDiffProvider::open(&opened, &defaults()).expect("open provider");
    assert_eq!(
        &*provider.load_blob(&change.new_blob).unwrap(),
        b"edited, not committed\n"
    );
    assert_eq!(
        &*provider.load_blob(&change.old_blob).unwrap(),
        b"committed\n"
    );
}

#[test]
fn generated_flags_follow_is_generated() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    // `yarn.lock` is a lockfile, but the repo says it is not generated.
    repo.write(".gitattributes", b"yarn.lock -linguist-generated\n");
    for p in [
        "Cargo.lock",
        "api.pb.go",
        "src/a.rs",
        "uv.lock",
        "yarn.lock",
    ] {
        repo.write(p, b"one\n");
    }
    repo.commit("base");
    repo.git(&["tag", "base"]);
    for p in [
        "Cargo.lock",
        "api.pb.go",
        "src/a.rs",
        "uv.lock",
        "yarn.lock",
    ] {
        repo.write(p, b"two\n");
    }
    repo.commit("head");
    repo.git(&["tag", "head"]);

    let opened = open_compare(repo.path());
    let stored: Vec<bool> = opened.files.iter().map(|f| f.generated).collect();
    // v1's bit: its built-in list, `uv.lock` not in it, the attribute wins.
    assert_eq!(stored, [true, true, false, false, false]);
    let provider = CoreDiffProvider::open(&opened, &defaults()).expect("open provider");
    let files = provider.files();
    assert_eq!(
        files.iter().map(path_of).collect::<Vec<_>>(),
        [
            "Cargo.lock",
            "api.pb.go",
            "src/a.rs",
            "uv.lock",
            "yarn.lock"
        ]
    );
    // design §11.15: lockfiles and generated code by pattern (`uv.lock` is
    // in geld's lockfiles), an unset attribute never generated.
    assert_eq!(
        files.iter().map(|f| f.generated).collect::<Vec<_>>(),
        [true, true, false, true, false]
    );
    assert!(
        !Arc::ptr_eq(&files, &opened.files),
        "a verdict differs, so the list is a copy"
    );
    // The store's list is left alone.
    assert_eq!(
        opened.files.iter().map(|f| f.generated).collect::<Vec<_>>(),
        stored
    );
}
