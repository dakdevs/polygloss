//! `polygloss-perf` arguments.
//!
//! ```text
//! polygloss-perf --version
//! polygloss-perf --corpus <name> --layout split|unified --scenario open|scroll|highlight|blocks|sections|reveal
//!                [--json] [--repo <path> --base <rev> --head <rev> [--direct]]
//!                [--seed <n>] [--scroll-secs <s>] [--jumps <n>] [--stops <n>] [--ops <n>]
//! ```
//!
//! Without `--repo/--base/--head` the corpus is looked up with
//! `bun benches/corpora/manifest.ts --corpus <name>` (T2.1: nothing else
//! computes where corpora live); `benches/run-perf.ts` passes them so no
//! lookup happens inside a measured run. The knobs shorten runs for smoke
//! tests; `run-perf.ts` always uses the defaults (the plan's definitions).

use std::path::PathBuf;
use std::time::Duration;

use polygloss_diff::rows::Layout;

pub const USAGE: &str = "usage: polygloss-perf --corpus <name> --layout split|unified \
    --scenario open|scroll|highlight|blocks|sections|reveal [--json] \
    [--repo <path> --base <rev> --head <rev> [--direct]] [--seed <n>] \
    [--scroll-secs <s>] [--jumps <n>] [--stops <n>] [--ops <n>]";

/// A perf scenario (plan T2.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenarioName {
    /// `first_paint_ms`.
    Open,
    /// `scroll_p95_ms` and frame intervals.
    Scroll,
    /// `highlight_ms`.
    Highlight,
    /// `comment_repaint_ms` (M2: `set_blocks` → next frame).
    Blocks,
    /// `sections_scroll_p95_ms` and `section_toggle_ms` (T6.17).
    Sections,
    /// `collapse_anim_p95_ms` and `collapse_commit_ms` (T7.8).
    Reveal,
}

impl ScenarioName {
    const ALL: [ScenarioName; 6] = [
        ScenarioName::Open,
        ScenarioName::Scroll,
        ScenarioName::Highlight,
        ScenarioName::Blocks,
        ScenarioName::Sections,
        ScenarioName::Reveal,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ScenarioName::Open => "open",
            ScenarioName::Scroll => "scroll",
            ScenarioName::Highlight => "highlight",
            ScenarioName::Blocks => "blocks",
            ScenarioName::Sections => "sections",
            ScenarioName::Reveal => "reveal",
        }
    }

    fn parse(s: &str) -> Option<ScenarioName> {
        ScenarioName::ALL.into_iter().find(|n| n.as_str() == s)
    }
}

/// Where a corpus is and what to compare (a manifest entry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusSpec {
    pub repo: PathBuf,
    pub base: String,
    pub head: String,
    /// `compare --direct` (two-dot) instead of three-dot.
    pub direct: bool,
}

/// Scenario sizes. The defaults are the plan's definitions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Knobs {
    /// Scripted scrolling time (`scroll`: 10 s).
    pub scroll: Duration,
    /// Scroll speed in px/s (4,000).
    pub speed: f32,
    /// Random file jumps during `scroll` (20).
    pub jumps: u32,
    /// Scroll stops `highlight` measures (20).
    pub stops: u32,
    /// Blocks `blocks` adds (20).
    pub ops: u32,
    /// Seed of every random choice (jump targets, stop offsets).
    pub seed: u64,
}

impl Default for Knobs {
    fn default() -> Knobs {
        Knobs {
            scroll: Duration::from_secs(10),
            speed: 4_000.0,
            jumps: 20,
            stops: 20,
            ops: 20,
            seed: 1,
        }
    }
}

impl Knobs {
    pub fn is_default(&self) -> bool {
        *self == Knobs::default()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Args {
    /// The corpus name (reported, and the manifest key).
    pub corpus: String,
    pub layout: Layout,
    pub scenario: ScenarioName,
    /// Print one JSON object instead of a summary.
    pub json: bool,
    /// The corpus given on the command line (else the manifest's).
    pub entry: Option<CorpusSpec>,
    pub knobs: Knobs,
}

impl Args {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
        let mut corpus = None;
        let mut layout = None;
        let mut scenario = None;
        let mut json = false;
        let (mut repo, mut base, mut head, mut direct) = (None, None, None, false);
        let mut knobs = Knobs::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
            match arg.as_str() {
                "--corpus" => corpus = Some(value()?),
                "--layout" => {
                    layout = Some(match value()?.as_str() {
                        "split" => Layout::Split,
                        "unified" => Layout::Unified,
                        other => return Err(format!("unknown layout {other:?}")),
                    })
                }
                "--scenario" => {
                    let name = value()?;
                    scenario = Some(
                        ScenarioName::parse(&name)
                            .ok_or_else(|| format!("unknown scenario {name:?}"))?,
                    );
                }
                "--json" => json = true,
                "--repo" => repo = Some(PathBuf::from(value()?)),
                "--base" => base = Some(value()?),
                "--head" => head = Some(value()?),
                "--direct" => direct = true,
                "--seed" => knobs.seed = number(&arg, &value()?)?,
                "--scroll-secs" => {
                    let secs: f64 = number(&arg, &value()?)?;
                    if !(secs > 0.0 && secs.is_finite()) {
                        return Err(format!("{arg} must be positive"));
                    }
                    knobs.scroll = Duration::from_secs_f64(secs);
                }
                "--jumps" => knobs.jumps = number(&arg, &value()?)?,
                "--stops" => knobs.stops = positive(&arg, &value()?)?,
                "--ops" => knobs.ops = positive(&arg, &value()?)?,
                other => return Err(format!("unknown argument {other:?}")),
            }
        }
        let entry = match (repo, base, head) {
            (Some(repo), Some(base), Some(head)) => Some(CorpusSpec {
                repo,
                base,
                head,
                direct,
            }),
            (None, None, None) if !direct => None,
            _ => return Err("--repo, --base and --head go together".to_owned()),
        };
        Ok(Args {
            corpus: corpus.ok_or("--corpus <name> is required")?,
            layout: layout.ok_or("--layout split|unified is required")?,
            scenario: scenario.ok_or("--scenario <name> is required")?,
            json,
            entry,
            knobs,
        })
    }
}

/// `polygloss-perf --version` (or `-V`) alone: print the version (run-perf
/// launches it once before measuring).
pub fn is_version_request(args: &[String]) -> bool {
    matches!(args, [a] if a == "--version" || a == "-V")
}

fn number<T: std::str::FromStr>(flag: &str, value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} needs a number, got {value:?}"))
}

fn positive(flag: &str, value: &str) -> Result<u32, String> {
    match number(flag, value)? {
        0 => Err(format!("{flag} must be at least 1")),
        n => Ok(n),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use polygloss_diff::rows::Layout;

    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        Args::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_the_card_command_line() {
        let args = parse(&[
            "--corpus",
            "typical",
            "--layout",
            "split",
            "--scenario",
            "scroll",
            "--json",
        ])
        .unwrap();
        assert_eq!(args.corpus, "typical");
        assert_eq!(args.layout, Layout::Split);
        assert_eq!(args.scenario, ScenarioName::Scroll);
        assert!(args.json);
        assert_eq!(args.entry, None);
        assert_eq!(args.knobs, Knobs::default());
        assert_eq!(args.knobs.scroll, Duration::from_secs(10));
        assert_eq!(args.knobs.jumps, 20);
        assert_eq!(args.knobs.stops, 20);
        assert_eq!(args.knobs.ops, 20);
        assert_eq!(args.knobs.speed, 4_000.0);
        for (name, scenario) in [
            ("open", ScenarioName::Open),
            ("highlight", ScenarioName::Highlight),
            ("blocks", ScenarioName::Blocks),
            ("sections", ScenarioName::Sections),
            ("reveal", ScenarioName::Reveal),
        ] {
            let a = parse(&[
                "--corpus",
                "linux",
                "--layout",
                "unified",
                "--scenario",
                name,
            ])
            .unwrap();
            assert_eq!(a.scenario, scenario);
            assert_eq!(a.scenario.as_str(), name);
            assert_eq!(a.layout, Layout::Unified);
            assert!(!a.json);
        }
    }

    #[test]
    fn version_is_answered_without_other_arguments() {
        let v = |args: &[&str]| {
            is_version_request(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        assert!(v(&["--version"]));
        assert!(v(&["-V"]));
        assert!(!v(&[]));
        assert!(!v(&["--corpus", "typical"]));
    }

    #[test]
    fn corpus_flags_replace_the_manifest_lookup() {
        let args = parse(&[
            "--corpus",
            "linux",
            "--layout",
            "split",
            "--scenario",
            "open",
            "--repo",
            "/corpora/linux",
            "--base",
            "v6.10",
            "--head",
            "v6.11",
            "--direct",
        ])
        .unwrap();
        assert_eq!(
            args.entry,
            Some(CorpusSpec {
                repo: PathBuf::from("/corpora/linux"),
                base: "v6.10".to_owned(),
                head: "v6.11".to_owned(),
                direct: true,
            })
        );
    }

    #[test]
    fn knobs_shorten_runs_for_smoke_tests() {
        let args = parse(&[
            "--corpus",
            "typical",
            "--layout",
            "split",
            "--scenario",
            "scroll",
            "--scroll-secs",
            "2.5",
            "--jumps",
            "4",
            "--stops",
            "3",
            "--ops",
            "5",
            "--seed",
            "42",
        ])
        .unwrap();
        assert_eq!(args.knobs.scroll, Duration::from_millis(2_500));
        assert_eq!(args.knobs.jumps, 4);
        assert_eq!(args.knobs.stops, 3);
        assert_eq!(args.knobs.ops, 5);
        assert_eq!(args.knobs.seed, 42);
        assert!(!args.knobs.is_default());
    }

    #[test]
    fn rejects_unknown_incomplete_or_conflicting_arguments() {
        let base = ["--corpus", "typical", "--layout", "split", "--scenario"];
        let with = |extra: &[&str]| {
            let mut v: Vec<&str> = base.to_vec();
            v.extend_from_slice(extra);
            parse(&v)
        };
        assert!(with(&["scroll"]).is_ok());
        assert!(with(&["warp"]).unwrap_err().contains("warp"));
        assert!(with(&[]).is_err());
        assert!(parse(&["--layout", "split", "--scenario", "open"]).is_err());
        assert!(parse(&["--corpus", "typical", "--scenario", "open"]).is_err());
        assert!(parse(&["--corpus", "typical", "--layout", "split"]).is_err());
        assert!(
            parse(&[
                "--corpus",
                "typical",
                "--layout",
                "auto",
                "--scenario",
                "open"
            ])
            .is_err()
        );
        assert!(with(&["open", "--frobnicate"]).is_err());
        assert!(with(&["open", "--scroll-secs", "0"]).is_err());
        assert!(with(&["open", "--stops", "zero"]).is_err());
        // The corpus flags come as a set.
        assert!(with(&["open", "--repo", "/r", "--base", "a"]).is_err());
        assert!(with(&["open", "--direct"]).is_err());
    }
}
