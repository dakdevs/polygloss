//! The corpus a run measures: its manifest entry, the diff opened through
//! `polygloss-core` like the app opens one, and the blobs behind the
//! viewport's [`DiffProvider`].
//!
//! Every run opens its corpus in a store of its own (a fresh temp dir, never
//! the user's data dir), so the file list is computed from scratch each time
//! (first paint includes resolve and `diff-tree`) and no run touches real
//! state. `polygloss-perf` does not link the app, so [`CorpusProvider`] is its
//! own copy of the app's `CoreDiffProvider` (a blob reader over the repo).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::paths::DataPaths;
use polygloss_core::review::{Core, OpenRequest, OpenedDiff};
use polygloss_core::store::events::Actor;
use polygloss_diff::{FileChange, ObjectFormat, Oid};
use polygloss_viewport::DiffProvider;

pub use crate::args::CorpusSpec;

/// The checkout this binary was built from (for the corpus manifest).
const CHECKOUT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

/// Looks `corpus` up with `bun benches/corpora/manifest.ts --corpus <name>`.
pub fn manifest_entry(corpus: &str) -> anyhow::Result<CorpusSpec> {
    let manifest = Path::new(CHECKOUT).join("benches/corpora/manifest.ts");
    let out = Command::new("bun")
        .arg(&manifest)
        .args(["--corpus", corpus])
        .output()
        .with_context(|| format!("running bun {}", manifest.display()))?;
    if !out.status.success() {
        bail!(
            "the corpus manifest has no {corpus:?}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    parse_manifest_entry(&String::from_utf8_lossy(&out.stdout))
}

/// A manifest entry (`{ name, repo, base, head, mode }`).
pub fn parse_manifest_entry(json: &str) -> anyhow::Result<CorpusSpec> {
    let v: serde_json::Value = serde_json::from_str(json).context("manifest entry")?;
    let field = |k: &str| {
        v.get(k)
            .and_then(|s| s.as_str())
            .with_context(|| format!("manifest entry has no {k}: {json}"))
    };
    let direct = match field("mode")? {
        "direct" => true,
        "three-dot" => false,
        other => bail!("unknown compare mode {other:?}"),
    };
    Ok(CorpusSpec {
        repo: PathBuf::from(field("repo")?),
        base: field("base")?.to_owned(),
        head: field("head")?.to_owned(),
        direct,
    })
}

/// An opened corpus and the private store it was opened in.
pub struct OpenedCorpus {
    pub opened: OpenedDiff,
    pub provider: Arc<CorpusProvider>,
    /// How long creating the private store took (`Core::with_paths`: a new
    /// database and its migrations).
    pub store_time: Duration,
    /// How long `Core::open` took (resolve, `diff-tree`, store writes).
    pub open_time: Duration,
    /// The run's temp dir: `home/` and the store's `data/`.
    root: tempfile::TempDir,
}

impl OpenedCorpus {
    /// The run's private data dir (`polygloss.db` is here).
    #[cfg(test)]
    pub fn data_dir(&self) -> PathBuf {
        self.root.path().canonicalize().unwrap().join("data")
    }

    /// Deletes the run's temp dir (the process exits without running
    /// destructors, so this is called explicitly).
    pub fn close(self) {
        let _ = self.root.close();
    }
}

/// Opens `spec` as a compare, as the app would, in a fresh private store.
pub fn open_corpus(spec: &CorpusSpec) -> anyhow::Result<OpenedCorpus> {
    let tmp = tempfile::Builder::new()
        .prefix("polygloss-perf-")
        .tempdir()
        .context("creating the run's data dir")?;
    let root = tmp.path().canonicalize()?;
    let home = root.join("home");
    let data_dir = root.join("data");
    std::fs::create_dir_all(&home)?;
    let paths = DataPaths::resolve_with(|k| match k {
        "HOME" => Some(home.clone().into_os_string()),
        "POLYGLOSS_DATA_DIR" => Some(data_dir.clone().into_os_string()),
        "XDG_CONFIG_HOME" => Some(home.join(".config").into_os_string()),
        _ => None,
    })?;
    let started = Instant::now();
    let core = Core::with_paths(paths)?;
    let store_time = started.elapsed();
    let started = Instant::now();
    let opened = core
        .open(&OpenRequest {
            worktree: spec.repo.clone(),
            source: Source::Compare {
                base: spec.base.clone(),
                head: spec.head.clone(),
                mode: if spec.direct {
                    CompareMode::Direct
                } else {
                    CompareMode::ThreeDot
                },
            },
            label: None,
            pin: None,
            actor: Actor::human(),
        })
        .with_context(|| {
            format!(
                "opening {} {}..{}",
                spec.repo.display(),
                spec.base,
                spec.head
            )
        })?;
    let open_time = started.elapsed();
    let provider = Arc::new(CorpusProvider::open(&opened)?);
    Ok(OpenedCorpus {
        opened,
        provider,
        store_time,
        open_time,
        root: tmp,
    })
}

/// An opened diff's files and in-process blob reads (gix, never a fetch).
pub struct CorpusProvider {
    object_format: ObjectFormat,
    files: Arc<Vec<FileChange>>,
    blobs: BlobReader,
}

impl CorpusProvider {
    fn open(opened: &OpenedDiff) -> anyhow::Result<CorpusProvider> {
        let repo_blobs = BlobReader::open(&opened.repo)?;
        let blobs = match &opened.live {
            Some(live) => repo_blobs.with_scratch(&live.scratch_objects)?,
            None => repo_blobs,
        };
        Ok(CorpusProvider {
            object_format: opened.repo.object_format,
            files: opened.files.clone(),
            blobs,
        })
    }
}

impl DiffProvider for CorpusProvider {
    fn object_format(&self) -> ObjectFormat {
        self.object_format
    }

    fn files(&self) -> Arc<Vec<FileChange>> {
        self.files.clone()
    }

    fn load_blob(&self, oid: &Oid) -> anyhow::Result<Arc<[u8]>> {
        Ok(self.blobs.read(oid)?)
    }

    fn blob_size(&self, oid: &Oid) -> anyhow::Result<u64> {
        Ok(self.blobs.size(oid)?)
    }
}

#[cfg(test)]
mod tests {
    use polygloss_core::testing::{FixtureRepo, Sandbox};
    use polygloss_diff::ObjectFormat;
    use polygloss_viewport::DiffProvider;

    use super::*;

    #[test]
    fn manifest_entries_parse_into_corpus_specs() {
        let spec = parse_manifest_entry(
            r#"{"name":"linux","repo":"/c/linux","base":"v6.10","head":"v6.11","mode":"direct"}"#,
        )
        .unwrap();
        assert_eq!(
            spec,
            CorpusSpec {
                repo: "/c/linux".into(),
                base: "v6.10".to_owned(),
                head: "v6.11".to_owned(),
                direct: true,
            }
        );
        let generated = parse_manifest_entry(
            r#"{"name":"typical","repo":"/c/typical","base":"corpus-base","head":"corpus-head","mode":"three-dot"}"#,
        )
        .unwrap();
        assert!(!generated.direct);
        assert!(parse_manifest_entry("[]").is_err());
        assert!(
            parse_manifest_entry(
                r#"{"name":"x","repo":"/r","base":"a","head":"b","mode":"sideways"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn open_corpus_reads_a_repo_through_a_private_store() {
        let sb = Sandbox::isolate();
        let repo = FixtureRepo::init(ObjectFormat::Sha1);
        repo.write("a.rs", b"fn main() {}\n");
        repo.write("b.txt", b"one\n");
        repo.commit("base");
        repo.git(&["tag", "base"]);
        repo.write("a.rs", b"fn main() {\n    println!(\"hi\");\n}\n");
        repo.commit("head");
        repo.git(&["tag", "head"]);

        let corpus = open_corpus(&CorpusSpec {
            repo: repo.path().to_owned(),
            base: "base".to_owned(),
            head: "head".to_owned(),
            direct: true,
        })
        .unwrap();
        let files = corpus.provider.files();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].display_path(), "a.rs");
        let new = corpus.provider.load_blob(&files[0].new_blob).unwrap();
        assert!(new.starts_with(b"fn main() {\n"));
        assert_eq!(
            corpus.provider.blob_size(&files[0].old_blob).unwrap(),
            b"fn main() {}\n".len() as u64
        );
        // The store lives in the run's own temp dir: nothing is written to
        // the (sandboxed) data dir the environment names.
        let data = corpus.data_dir();
        assert!(data.join("polygloss.db").exists());
        assert!(!data.starts_with(sb.home()));
        assert_eq!(std::fs::read_dir(sb.data_dir()).unwrap().count(), 0);
        corpus.close();
        assert!(!data.exists());
    }

    #[test]
    fn open_corpus_reports_a_missing_revision() {
        let _sb = Sandbox::isolate();
        let repo = FixtureRepo::init(ObjectFormat::Sha1);
        repo.write("a.txt", b"a\n");
        repo.commit("only");
        let err = open_corpus(&CorpusSpec {
            repo: repo.path().to_owned(),
            base: "nope".to_owned(),
            head: "HEAD".to_owned(),
            direct: false,
        })
        .err()
        .expect("a missing base fails");
        assert!(format!("{err:#}").contains("nope"), "{err:#}");
    }
}
