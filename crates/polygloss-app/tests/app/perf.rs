//! `Polygloss --perf-scenario` (T3.1, plan T2.9, OQ-P4): arguments, the
//! corpus manifest entry and first paint from a run's frames.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use polygloss_app::perf::open::first_paint;
use polygloss_app::perf::{CorpusSpec, PerfArgs, Scenario, parse_manifest_entry};
use polygloss_diff::rows::Layout;
use polygloss_viewport::FrameStats;

use crate::support::strings;

#[test]
fn perf_args_parse_the_run_perf_command_line() {
    let parse = |args: &[&str]| PerfArgs::parse(&strings(args));
    let args = parse(&[
        "open", "--corpus", "linux", "--layout", "unified", "--json", "--repo", "/c/linux",
        "--base", "v6.10", "--head", "v6.11", "--direct",
    ])
    .unwrap();
    assert_eq!(
        args,
        PerfArgs {
            scenario: Scenario::Open,
            corpus: "linux".into(),
            layout: Layout::Unified,
            json: true,
            entry: Some(CorpusSpec {
                repo: PathBuf::from("/c/linux"),
                base: "v6.10".into(),
                head: "v6.11".into(),
                direct: true,
            }),
        }
    );
    let lookup = parse(&["open", "--corpus", "typical"]).unwrap();
    assert_eq!(
        (lookup.layout, lookup.json, lookup.entry),
        (Layout::Split, false, None)
    );
    for bad in [
        &[][..],
        &["nope", "--corpus", "typical"],
        &["open"],
        &["open", "--corpus", "typical", "--repo", "/r"],
        &["open", "--corpus", "typical", "--direct"],
        &["open", "--corpus", "typical", "--layout", "diagonal"],
        &["open", "--corpus", "typical", "--frobnicate"],
    ] {
        assert!(parse(bad).is_err(), "{bad:?}");
    }
    let spec = parse_manifest_entry(
        r#"{"name":"typical","repo":"/c/t","base":"corpus-base","head":"corpus-head","mode":"three-dot"}"#,
    )
    .unwrap();
    assert_eq!(
        spec,
        CorpusSpec {
            repo: PathBuf::from("/c/t"),
            base: "corpus-base".into(),
            head: "corpus-head".into(),
            direct: false,
        }
    );
    assert!(parse_manifest_entry(r#"{"repo":"/c"}"#).is_err());
}

#[test]
fn app_first_paint_is_the_first_frame_without_loading_rows() {
    let start = Instant::now();
    let frame = |ms: u64, loading_rows: u32| {
        (
            start + Duration::from_millis(ms),
            FrameStats {
                prepaint: Duration::from_millis(1),
                paint: Duration::from_millis(1),
                visible_rows: 40,
                shaped_lines: 40,
                loading_rows,
                unhighlighted_rows: loading_rows,
            },
        )
    };
    let frames = [frame(90, 12), frame(110, 3), frame(140, 0), frame(150, 0)];
    let (painted, first, n) = first_paint(&frames).unwrap();
    assert_eq!(painted - start, Duration::from_millis(140));
    assert_eq!(first - start, Duration::from_millis(90));
    assert_eq!(n, 3);
    assert!(first_paint(&frames[..2]).is_none());
    assert!(first_paint(&[]).is_none());
}

#[test]
fn perf_runs_keep_every_path_in_their_private_dir() {
    let sb = crate::support::Sandbox::isolate();
    let root = tempfile::tempdir().unwrap();
    let paths = polygloss_app::perf::private_paths(root.path()).unwrap();
    for (name, path) in [
        ("data_dir", &paths.data_dir),
        ("db", &paths.db),
        ("cache_dir", &paths.cache_dir),
        ("scratch_dir", &paths.scratch_dir),
        ("logs_dir", &paths.logs_dir),
        ("config_dir", &paths.config_dir),
    ] {
        assert!(path.starts_with(root.path()), "{name}: {}", path.display());
        // Not the environment's (the sandbox's) paths.
        assert!(
            !path.starts_with(sb.home()) && !path.starts_with(sb.data_dir()),
            "{name}: {}",
            path.display()
        );
    }
}
