//! App-level perf scenarios (plan T2.9, OQ-P4; test-only):
//!
//! ```text
//! POLYGLOSS_TEST=1 Polygloss --perf-scenario <name> --corpus <name> --layout split|unified --json
//!                            [--repo <path> --base <rev> --head <rev> [--direct]]
//! ```
//!
//! They measure the app itself, through its real startup, where
//! `polygloss-perf` (which does not link the app) cannot. Each prints one
//! result in `polygloss-perf`'s shape (`{ scenario, corpus, layout, metrics,
//! samples, info }`) and exits 0, or exits 1 when the run fails and 2 for
//! usage errors (also without `POLYGLOSS_TEST=1`). Without `--repo`,
//! `--base` and `--head` the corpus comes from `bun benches/corpora/
//! manifest.ts --corpus <name>` in the checkout the binary was built from.
//! `benches/run-perf.ts` runs them with every run in a fresh sandbox; the
//! app also keeps its store, logs and settings in a private temporary dir
//! ([`private_paths`]), removed on exit, so a run by hand never writes the
//! corpus review into the real store.
//!
//! Scenarios: `open` ([`open`], `app_first_paint_ms`) and
//! `watcher-banner` ([`watcher_banner`], `watcher_banner_ms`). T3.10 adds
//! `comment-roundtrip`.

pub mod open;
pub mod watcher_banner;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, bail};
use polygloss_diff::rows::Layout;
use serde_json::{Map, Value, json};

/// Printed with every usage error.
pub const USAGE: &str = "usage: POLYGLOSS_TEST=1 Polygloss --perf-scenario open|watcher-banner --corpus <name> \
    [--layout split|unified] [--json] [--repo <path> --base <rev> --head <rev> [--direct]]";

/// The environment variable that enables test-only surfaces (OQ-P4).
pub const TEST_ENV: &str = "POLYGLOSS_TEST";

/// The scenarios the app runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    /// `app_first_paint_ms` (T3.1).
    Open,
    /// `watcher_banner_ms` (T3.11).
    WatcherBanner,
}

impl Scenario {
    pub fn as_str(self) -> &'static str {
        match self {
            Scenario::Open => "open",
            Scenario::WatcherBanner => "watcher-banner",
        }
    }

    fn parse(s: &str) -> Option<Scenario> {
        match s {
            "open" => Some(Scenario::Open),
            "watcher-banner" => Some(Scenario::WatcherBanner),
            _ => None,
        }
    }
}

/// Where a corpus is and what to compare (a manifest entry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusSpec {
    pub repo: PathBuf,
    pub base: String,
    pub head: String,
    /// Two-dot instead of three-dot.
    pub direct: bool,
}

/// `--perf-scenario` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerfArgs {
    pub scenario: Scenario,
    pub corpus: String,
    pub layout: Layout,
    /// One line of JSON (else a readable summary).
    pub json: bool,
    /// From `--repo/--base/--head`; `None` looks the corpus up.
    pub entry: Option<CorpusSpec>,
}

impl PerfArgs {
    /// Parses the arguments after `--perf-scenario` (the scenario name
    /// first).
    pub fn parse(args: &[String]) -> Result<PerfArgs, String> {
        let mut args = args.iter();
        let name = args.next().ok_or("--perf-scenario needs a scenario")?;
        let scenario = Scenario::parse(name).ok_or_else(|| format!("unknown scenario {name:?}"))?;
        let (mut corpus, mut layout, mut json) = (None, Layout::Split, false);
        let (mut repo, mut base, mut head, mut direct) = (None, None, None, false);
        while let Some(arg) = args.next() {
            let mut value = || {
                args.next()
                    .cloned()
                    .ok_or_else(|| format!("{arg} needs a value"))
            };
            match arg.as_str() {
                "--corpus" => corpus = Some(value()?),
                "--layout" => {
                    layout = match value()?.as_str() {
                        "split" => Layout::Split,
                        "unified" => Layout::Unified,
                        other => return Err(format!("unknown layout {other:?}")),
                    }
                }
                "--json" => json = true,
                "--repo" => repo = Some(PathBuf::from(value()?)),
                "--base" => base = Some(value()?),
                "--head" => head = Some(value()?),
                "--direct" => direct = true,
                other => return Err(format!("unknown argument {other:?}")),
            }
        }
        let corpus = corpus.ok_or("--corpus <name> is required")?;
        let entry = match (repo, base, head) {
            (Some(repo), Some(base), Some(head)) => Some(CorpusSpec {
                repo,
                base,
                head,
                direct,
            }),
            (None, None, None) if !direct => None,
            _ => return Err("give all of --repo, --base and --head, or none".to_owned()),
        };
        Ok(PerfArgs {
            scenario,
            corpus,
            layout,
            json,
            entry,
        })
    }
}

/// `Polygloss --perf-scenario …`: `args` are the arguments after it.
pub fn main(args: &[String], clock: Clock) -> ExitCode {
    if std::env::var(TEST_ENV).as_deref() != Ok("1") {
        eprintln!(
            "Polygloss: --perf-scenario is a test-only surface; set {TEST_ENV}=1 (OQ-P4)\n{USAGE}"
        );
        return ExitCode::from(2);
    }
    let args = match PerfArgs::parse(args) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("Polygloss --perf-scenario: {err}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let spec = match &args.entry {
        Some(spec) => spec.clone(),
        None => match manifest_entry(&args.corpus) {
            Ok(spec) => spec,
            Err(err) => {
                eprintln!("Polygloss --perf-scenario: {err:#}");
                return ExitCode::from(1);
            }
        },
    };
    match args.scenario {
        Scenario::Open => open::run(args, spec, clock),
        Scenario::WatcherBanner => watcher_banner::run(args, spec, clock),
    }
}

/// The data paths of a perf run: everything (store, caches, logs,
/// settings) under `root`, nothing from the environment, like
/// `polygloss-perf`'s private stores.
pub fn private_paths(root: &Path) -> anyhow::Result<polygloss_core::paths::DataPaths> {
    let home = root.join("home");
    std::fs::create_dir_all(&home).context("creating the run's home")?;
    Ok(polygloss_core::paths::DataPaths::resolve_with(
        |k| match k {
            "HOME" => Some(home.clone().into_os_string()),
            "POLYGLOSS_DATA_DIR" => Some(root.join("data").into_os_string()),
            "XDG_CONFIG_HOME" => Some(home.join(".config").into_os_string()),
            _ => None,
        },
    )?)
}

/// The run's private dir ([`run_dir`]), removed by [`finish`] (the process
/// exits without running destructors).
static RUN_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Creates the run's private temporary dir and its data paths
/// ([`private_paths`]); [`finish`] removes it.
pub fn run_dir() -> anyhow::Result<(PathBuf, polygloss_core::paths::DataPaths)> {
    let dir = tempfile::Builder::new()
        .prefix("polygloss-app-perf-")
        .tempdir()
        .context("creating the run's data dir")?;
    let root = dir.keep();
    let _ = RUN_DIR.set(root.clone());
    let paths = private_paths(&root)?;
    Ok((root, paths))
}

/// Prints the result (or the error) and exits: 0 with a result, 1 on
/// failure. GPUI's macOS run loop never returns, so the process exits here,
/// after removing the run's private dir.
pub fn finish(outcome: anyhow::Result<ScenarioResult>, args: &PerfArgs) -> ! {
    use std::io::Write as _;
    let code = match outcome {
        Ok(result) => {
            let mut stdout = std::io::stdout().lock();
            let _ = writeln!(stdout, "{}", result.render(args.json).trim_end());
            let _ = stdout.flush();
            0
        }
        Err(err) => {
            eprintln!(
                "Polygloss --perf-scenario {} {}: {err:#}",
                args.scenario.as_str(),
                args.corpus
            );
            1
        }
    };
    if let Some(dir) = RUN_DIR.get() {
        let _ = std::fs::remove_dir_all(dir);
    }
    std::process::exit(code)
}

/// The nearest-rank 95th percentile of `values` (the 19th smallest of 20,
/// as `polygloss-perf` and `run-perf.ts` compute it); `None` when empty.
pub fn p95(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = (0.95 * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

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
pub fn parse_manifest_entry(text: &str) -> anyhow::Result<CorpusSpec> {
    let v: Value = serde_json::from_str(text).context("manifest entry")?;
    let field = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .with_context(|| format!("manifest entry has no {k}: {text}"))
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

/// This process's clock: when the kernel started it and when `main` began,
/// both as [`Instant`]s, so every measurement is on one monotonic clock.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    pub process_start: Instant,
    pub main: Instant,
    /// `false` when the kernel's start time was unavailable and
    /// `process_start` is `main`.
    pub from_kernel: bool,
}

impl Clock {
    /// Call first thing in `main`.
    pub fn now() -> Clock {
        let main = Instant::now();
        let wall = SystemTime::now();
        let since_start = process_start().and_then(|s| wall.duration_since(s).ok());
        match since_start.and_then(|d| main.checked_sub(d)) {
            Some(process_start) => Clock {
                process_start,
                main,
                from_kernel: true,
            },
            None => Clock {
                process_start: main,
                main,
                from_kernel: false,
            },
        }
    }
}

/// When the kernel started this process (wall clock), from
/// `proc_pidinfo(PROC_PIDTBSDINFO)`.
fn process_start() -> Option<SystemTime> {
    // SAFETY: `proc_bsdinfo` is plain old data; `proc_pidinfo` writes at most
    // `size` bytes into it and returns how many it wrote.
    unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let written = libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast(),
            size,
        );
        if written != size {
            return None;
        }
        let since_epoch = Duration::from_secs(info.pbi_start_tvsec)
            + Duration::from_micros(info.pbi_start_tvusec);
        SystemTime::UNIX_EPOCH.checked_add(since_epoch)
    }
}

/// `d` in milliseconds, rounded to microseconds.
pub fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1_000_000.0).round() / 1_000.0
}

/// This process's peak resident set size so far (`getrusage`), in MB.
pub fn max_rss_mb() -> Option<f64> {
    // SAFETY: `rusage` is plain old data that `getrusage` fills in.
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) != 0 {
            return None;
        }
        // macOS reports bytes.
        Some((usage.ru_maxrss as f64 / (1024.0 * 1024.0) * 1000.0).round() / 1000.0)
    }
}

/// Startup marks as milliseconds since `start`; a mark not reached is
/// `null`.
pub fn timeline_ms(start: Instant, marks: &[(&str, Option<Instant>)]) -> Value {
    Value::Object(
        marks
            .iter()
            .map(|(name, at)| {
                let since = at.map(|t| ms(t.saturating_duration_since(start)));
                ((*name).to_owned(), json!(since))
            })
            .collect::<Map<_, _>>(),
    )
}

/// One run's result, in `polygloss-perf`'s shape.
pub struct ScenarioResult {
    pub scenario: &'static str,
    pub corpus: String,
    pub layout: &'static str,
    pub metrics: Map<String, Value>,
    pub samples: Map<String, Value>,
    pub info: Map<String, Value>,
}

impl ScenarioResult {
    pub fn new(args: &PerfArgs) -> ScenarioResult {
        ScenarioResult {
            scenario: args.scenario.as_str(),
            corpus: args.corpus.clone(),
            layout: layout_name(args.layout),
            metrics: Map::new(),
            samples: Map::new(),
            info: Map::new(),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "scenario": self.scenario,
            "corpus": self.corpus,
            "layout": self.layout,
            "metrics": self.metrics,
            "samples": self.samples,
            "info": self.info,
        })
    }

    /// One line of JSON, or `name value` lines for people.
    pub fn render(&self, json: bool) -> String {
        if json {
            return self.to_json().to_string();
        }
        let mut out = format!("{} {} {}\n", self.scenario, self.corpus, self.layout);
        for (k, v) in &self.metrics {
            out.push_str(&format!("  {k} {v}\n"));
        }
        out
    }
}

pub fn layout_name(layout: Layout) -> &'static str {
    match layout {
        Layout::Split => "split",
        Layout::Unified => "unified",
    }
}
