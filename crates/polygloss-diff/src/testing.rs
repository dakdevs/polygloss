//! Test support (feature `test-support`): machine-independent complexity
//! checks for the workspace's tests. Never enabled by shipping binaries.
//!
//! A wall-clock limit fails on a slow or loaded machine (CI's shared runners)
//! and passes a regression on a fast one. These checks time the same work at
//! two sizes in one process, interleaved, and compare the fastest runs: the
//! machine's speed cancels out, the growth does not. They time the calling
//! thread's CPU time, which a descheduled thread does not accrue: in wall time
//! a run spanning several scheduler quanta absorbs contention that a shorter
//! one escapes, so the ratio would grow with load. Noise left in CPU time (an
//! efficiency core, a cold cache) only adds, so the fastest run is the
//! estimate. Real performance budgets live in the perf harness
//! (`benches/budgets.json`).

use std::hint::black_box;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Timed runs per size, after one warm-up run of each.
const RUNS: usize = 7;

/// Asserts that `run` on the large input takes less than `limit` times the
/// calling thread's CPU time it takes on the small one (`inputs = [small,
/// large]`), comparing the fastest of interleaved runs, and returns the two
/// fastest times. For single-threaded work only: CPU time on other threads is not
/// counted. Size the work so one small run takes at least about 100 µs, and a
/// regression's large run a few seconds at most.
#[track_caller]
pub fn assert_ratio_below<T, R>(
    what: &str,
    limit: f64,
    inputs: [T; 2],
    run: impl FnMut(&mut T) -> R,
) -> [Duration; 2] {
    ratio_below(thread_cpu_time, what, limit, inputs, run)
}

/// [`assert_ratio_below`] in wall-clock time, for work whose behaviour is
/// defined in wall time (a time budget, a cancel from another thread). Load
/// can still inflate the large runs, so keep both sizes' runs bounded and
/// short.
#[track_caller]
pub fn assert_wall_ratio_below<T, R>(
    what: &str,
    limit: f64,
    inputs: [T; 2],
    run: impl FnMut(&mut T) -> R,
) -> [Duration; 2] {
    ratio_below(wall_time, what, limit, inputs, run)
}

#[track_caller]
fn ratio_below<T, R>(
    clock: fn() -> Duration,
    what: &str,
    limit: f64,
    mut inputs: [T; 2],
    mut run: impl FnMut(&mut T) -> R,
) -> [Duration; 2] {
    let [small, large] = &mut inputs;
    black_box(run(small));
    black_box(run(large));
    let time = |f: &mut dyn FnMut()| {
        let start = clock();
        f();
        clock() - start
    };
    let (mut s, mut l) = (Vec::with_capacity(RUNS), Vec::with_capacity(RUNS));
    for _ in 0..RUNS {
        s.push(time(&mut || drop(black_box(run(small)))));
        l.push(time(&mut || drop(black_box(run(large)))));
    }
    let (s, l) = (fastest(s), fastest(l));
    let ratio = l.as_secs_f64() / s.as_secs_f64();
    println!("{what}: {s:?} -> {l:?}, {ratio:.1}x (limit {limit}x)");
    assert!(
        ratio < limit,
        "{what}: {s:?} -> {l:?}, {ratio:.1}x (limit {limit}x)"
    );
    [s, l]
}

/// CPU time the calling thread has run, user and system.
#[allow(unsafe_code)]
fn thread_cpu_time() -> Duration {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec for the call's duration.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    assert_eq!(rc, 0, "clock_gettime(CLOCK_THREAD_CPUTIME_ID) failed");
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

/// Wall-clock time since the first call.
fn wall_time() -> Duration {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed()
}

fn fastest(times: Vec<Duration>) -> Duration {
    times.into_iter().min().unwrap()
}
