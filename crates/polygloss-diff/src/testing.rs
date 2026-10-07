//! Test support (feature `test-support`): machine-independent complexity
//! checks for the workspace's tests. Never enabled by shipping binaries.
//!
//! A wall-clock limit fails on a slow or loaded machine (CI's shared runners)
//! and passes a regression on a fast one. These checks time the same work at
//! two sizes in one process, interleaved so both see the same load, and compare
//! the medians: the machine's speed cancels out, the growth does not. Real
//! performance budgets live in the perf harness (`benches/budgets.json`).

use std::hint::black_box;
use std::time::{Duration, Instant};

/// Timed runs per size, after one warm-up run of each.
const RUNS: usize = 7;

/// Asserts that `run` on the large input takes less than `limit` times as long
/// as on the small one (`inputs = [small, large]`), comparing medians of
/// interleaved runs, and returns the two medians. Size the work so one small
/// run takes at least about 100 µs, and a regression's large run a few seconds
/// at most.
#[track_caller]
pub fn assert_ratio_below<T, R>(
    what: &str,
    limit: f64,
    mut inputs: [T; 2],
    mut run: impl FnMut(&mut T) -> R,
) -> [Duration; 2] {
    let [small, large] = &mut inputs;
    black_box(run(small));
    black_box(run(large));
    let (mut s, mut l) = (Vec::with_capacity(RUNS), Vec::with_capacity(RUNS));
    for _ in 0..RUNS {
        s.push(time(|| run(small)));
        l.push(time(|| run(large)));
    }
    let (s, l) = (median(s), median(l));
    let ratio = l.as_secs_f64() / s.as_secs_f64();
    println!("{what}: {s:?} -> {l:?}, {ratio:.1}x (limit {limit}x)");
    assert!(
        ratio < limit,
        "{what}: {s:?} -> {l:?}, {ratio:.1}x (limit {limit}x)"
    );
    [s, l]
}

fn time<R>(f: impl FnOnce() -> R) -> Duration {
    let start = Instant::now();
    black_box(f());
    start.elapsed()
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort_unstable();
    times[times.len() / 2]
}
