//! Repo default, root URIs, session ids and serve options (T4.4, design §15.1).

use std::path::{Path, PathBuf};

use polygloss_core::ObjectFormat;
use polygloss_core::testing::{FixtureRepo, Sandbox};
use polygloss_mcp::ServeOptions;
use polygloss_mcp::context::{is_worktree, resolve_repo, root_uri_to_path};
use polygloss_mcp::session::session_id_from;

#[test]
fn repo_default_prefers_param_then_worktree_root_then_project_dir_then_cwd() {
    let _sandbox = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"a\n");
    repo.commit("init");
    let not_repo = tempfile::tempdir().unwrap();
    let cwd = Path::new("/work/cwd");
    let project = Path::new("/work/project");

    // An explicit repo wins; a relative one is relative to the cwd.
    assert_eq!(
        resolve_repo(
            Some("/elsewhere"),
            &[repo.path().into()],
            Some(project),
            cwd
        ),
        PathBuf::from("/elsewhere")
    );
    assert_eq!(
        resolve_repo(Some("sub/dir"), &[], None, cwd),
        PathBuf::from("/work/cwd/sub/dir")
    );
    // Else the first root inside a git worktree (non-repo roots are skipped).
    let roots = vec![not_repo.path().to_path_buf(), repo.path().to_path_buf()];
    assert_eq!(
        resolve_repo(None, &roots, Some(project), cwd),
        repo.path().to_path_buf()
    );
    assert_eq!(
        resolve_repo(Some("  "), &roots, Some(project), cwd),
        repo.path().to_path_buf()
    );
    // Else CLAUDE_PROJECT_DIR, else the cwd.
    let only_non_repo = vec![not_repo.path().to_path_buf()];
    assert_eq!(
        resolve_repo(None, &only_non_repo, Some(project), cwd),
        project.to_path_buf()
    );
    assert_eq!(resolve_repo(None, &[], None, cwd), cwd.to_path_buf());
    assert_eq!(
        resolve_repo(None, &[], Some(Path::new("")), cwd),
        cwd.to_path_buf()
    );

    assert!(is_worktree(repo.path()));
    assert!(!is_worktree(not_repo.path()));
    // A bare repository is not a worktree.
    let bare = tempfile::tempdir().unwrap();
    let st = std::process::Command::new("git")
        .args(["init", "-q", "--bare"])
        .arg(bare.path())
        .status()
        .unwrap();
    assert!(st.success());
    assert!(!is_worktree(bare.path()));
}

#[test]
fn root_uris_decode_to_local_paths() {
    assert_eq!(
        root_uri_to_path("file:///Users/me/src/app"),
        Some(PathBuf::from("/Users/me/src/app"))
    );
    assert_eq!(
        root_uri_to_path("file:///Users/me/My%20Project/%C3%A9t%C3%A9"),
        Some(PathBuf::from("/Users/me/My Project/été"))
    );
    assert_eq!(
        root_uri_to_path("file://localhost/tmp/x"),
        Some(PathBuf::from("/tmp/x"))
    );
    assert_eq!(root_uri_to_path("file://server/share"), None);
    assert_eq!(root_uri_to_path("https://example.com/x"), None);
    assert_eq!(root_uri_to_path("file:///bad%2"), None);
}

#[test]
fn session_id_uses_claude_env_or_a_fresh_pg_uuid() {
    let from_env =
        session_id_from(|k| (k == "CLAUDE_CODE_SESSION_ID").then(|| "4f1c7e2a-session".to_owned()));
    assert_eq!(from_env, "4f1c7e2a-session");

    for env in [None, Some(""), Some("   ")] {
        let id = session_id_from(|_| env.map(str::to_owned));
        let uuid = id.strip_prefix("pg-").expect("pg- prefix");
        assert_eq!(uuid.len(), 36, "{id}");
        // UUIDv7: version nibble 7.
        assert_eq!(&uuid[14..15], "7", "{id}");
    }
    let a = session_id_from(|_| None);
    let b = session_id_from(|_| None);
    assert_ne!(a, b);
}

#[test]
fn channel_option_from_flag_or_env() {
    let none = |_: &str| None;
    assert!(!ServeOptions::from_flag_and_env(false, none).channel);
    assert!(ServeOptions::from_flag_and_env(true, none).channel);
    let on = |k: &str| (k == "POLYGLOSS_MCP_CHANNEL").then(|| "1".to_owned());
    assert!(ServeOptions::from_flag_and_env(false, on).channel);
    let off = |k: &str| (k == "POLYGLOSS_MCP_CHANNEL").then(|| "0".to_owned());
    assert!(!ServeOptions::from_flag_and_env(false, off).channel);
}
