//! What a scenario measures and how it is reported (plan T2.9 metric
//! definitions): nearest-rank percentiles over frame and event samples, frame
//! intervals, the process clock (first paint counts from process start) and
//! the one-object result `--json` prints.
//!
//! Percentiles use the nearest-rank definition, like `benches/run-perf.ts`:
//! the p-th percentile is the smallest sample with at least p% of the sample
//! at or below it, so the p95 of 20 stops is the 19th smallest.

use std::time::{Duration, Instant, SystemTime};

use serde_json::{Map, Value, json};

/// A frame slower than this missed a 60 Hz frame (OQ-P6 counts them).
pub const SLOW_FRAME_MS: f64 = 16.7;

/// The nearest-rank `p`-th percentile of `values` (`None` when empty).
pub fn percentile(values: &[f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    Some(sorted[rank.min(sorted.len()) - 1])
}

/// Summary statistics over one kind of sample (frame times, intervals, …).
pub struct Series<'a> {
    values: &'a [f64],
}

impl<'a> Series<'a> {
    pub fn new(values: &'a [f64]) -> Series<'a> {
        Series { values }
    }

    pub fn p95(&self) -> Option<f64> {
        percentile(self.values, 95.0)
    }

    pub fn p99(&self) -> Option<f64> {
        percentile(self.values, 99.0)
    }

    pub fn max(&self) -> Option<f64> {
        self.values.iter().copied().reduce(f64::max)
    }

    /// Samples strictly above `limit`.
    pub fn over(&self, limit: f64) -> usize {
        self.values.iter().filter(|v| **v > limit).count()
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }
}

/// Gaps between consecutive instants, in milliseconds.
pub fn intervals_ms(times: &[Instant]) -> Vec<f64> {
    times
        .windows(2)
        .map(|w| ms(w[1].saturating_duration_since(w[0])))
        .collect()
}

/// `d` in milliseconds, rounded to microseconds.
pub fn ms(d: Duration) -> f64 {
    round3(d.as_secs_f64() * 1_000.0)
}

/// `v` rounded to three decimals.
pub fn round3(v: f64) -> f64 {
    (v * 1_000.0).round() / 1_000.0
}

/// When the kernel started this process (wall clock), from
/// `proc_pidinfo(PROC_PIDTBSDINFO)`; `None` if it cannot say.
pub fn process_start() -> Option<SystemTime> {
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

/// This process's clock: when it started and when `main` began, both as
/// [`Instant`]s so every measurement uses one monotonic clock.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    pub process_start: Instant,
    pub main: Instant,
    /// `false` when the kernel's start time was unavailable and
    /// `process_start` is `main`.
    pub from_kernel: bool,
}

impl Clock {
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

/// This process's peak resident set size so far (`getrusage`), in MB.
pub fn max_rss_mb() -> Option<f64> {
    // SAFETY: `rusage` is plain old data that `getrusage` fills in.
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) != 0 {
            return None;
        }
        // macOS reports bytes (Linux reports KiB).
        Some(round3(usage.ru_maxrss as f64 / (1024.0 * 1024.0)))
    }
}

/// One scenario run's result: the object `polygloss-perf --json` prints and
/// `benches/run-perf.ts` reads (`Polygloss --perf-scenario` prints the same
/// shape). `metrics` are the named numbers (null when not measured),
/// `samples` the raw series behind them, `info` context.
#[derive(Debug, Clone)]
pub struct ScenarioResult {
    scenario: String,
    corpus: String,
    layout: String,
    metrics: Map<String, Value>,
    samples: Map<String, Value>,
    info: Map<String, Value>,
}

impl ScenarioResult {
    pub fn new(scenario: &str, corpus: &str, layout: &str) -> ScenarioResult {
        ScenarioResult {
            scenario: scenario.to_owned(),
            corpus: corpus.to_owned(),
            layout: layout.to_owned(),
            metrics: Map::new(),
            samples: Map::new(),
            info: Map::new(),
        }
    }

    pub fn metric(&mut self, name: &str, value: Option<f64>) {
        self.metrics
            .insert(name.to_owned(), json!(value.map(round3)));
    }

    pub fn samples(&mut self, name: &str, values: Vec<f64>) {
        self.samples.insert(name.to_owned(), json!(values));
    }

    pub fn info(&mut self, name: &str, value: Value) {
        self.info.insert(name.to_owned(), value);
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

    /// The result as one line of JSON.
    pub fn to_json_line(&self) -> String {
        self.to_json().to_string()
    }

    /// A short human summary: the metrics, one per line.
    pub fn to_human(&self) -> String {
        let mut out = format!("{} · {} · {}\n", self.scenario, self.corpus, self.layout);
        for (name, value) in &self.metrics {
            out.push_str(&format!("  {name:<24} {value}\n"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant, SystemTime};

    use super::*;

    #[test]
    fn p95_of_a_known_sample() {
        let hundred: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&hundred, 95.0), Some(95.0));
        let reversed: Vec<f64> = hundred.iter().rev().copied().collect();
        assert_eq!(percentile(&reversed, 95.0), Some(95.0));
        assert_eq!(percentile(&hundred, 50.0), Some(50.0));
        assert_eq!(percentile(&hundred, 100.0), Some(100.0));
        // 20 stops: the 19th smallest (the same as run-perf.ts).
        let twenty: Vec<f64> = (1..=20).map(|i| f64::from(i) * 10.0).collect();
        assert_eq!(percentile(&twenty, 95.0), Some(190.0));
        assert_eq!(percentile(&[7.0], 95.0), Some(7.0));
        assert_eq!(percentile(&[], 95.0), None);
    }

    #[test]
    fn frame_series_summary_counts_frames_over_16_7_ms() {
        let mut frames = vec![2.0; 97];
        frames.extend([16.7, 16.8, 30.0]);
        let s = Series::new(&frames);
        assert_eq!(s.p95(), Some(2.0));
        assert_eq!(s.p99(), Some(16.8));
        assert_eq!(s.max(), Some(30.0));
        // Strictly over 16.7 ms: a 60 Hz frame budget missed.
        assert_eq!(s.over(SLOW_FRAME_MS), 2);
        assert_eq!(s.len(), 100);
        assert_eq!(Series::new(&[]).max(), None);
    }

    #[test]
    fn intervals_are_the_gaps_between_frames() {
        let t0 = Instant::now();
        let times = [
            t0,
            t0 + Duration::from_micros(8_333),
            t0 + Duration::from_micros(16_666),
            t0 + Duration::from_millis(50),
        ];
        let gaps = intervals_ms(&times);
        assert_eq!(gaps.len(), 3);
        assert!((gaps[0] - 8.333).abs() < 1e-9);
        assert!((gaps[2] - 33.334).abs() < 1e-9);
        assert!(intervals_ms(&times[..1]).is_empty());
    }

    #[test]
    fn durations_become_rounded_milliseconds() {
        assert_eq!(ms(Duration::from_micros(1_234_567)), 1234.567);
        assert_eq!(ms(Duration::from_nanos(2_000_400)), 2.0);
        assert_eq!(round3(8.33349), 8.333);
    }

    #[test]
    fn process_start_is_before_now_and_recent() {
        let start = process_start().expect("proc_pidinfo works on macOS");
        let now = SystemTime::now();
        let age = now.duration_since(start).expect("start is in the past");
        // This test process started moments ago (a generous bound for slow CI).
        assert!(age < Duration::from_secs(600), "age {age:?}");
    }

    #[test]
    fn process_clock_puts_process_start_before_main() {
        let clock = Clock::now();
        assert!(clock.process_start <= clock.main);
        assert!(clock.main - clock.process_start < Duration::from_secs(600));
    }

    #[test]
    fn max_rss_is_reported_in_megabytes() {
        let mb = max_rss_mb().expect("getrusage works");
        // A test binary linking GPUI is at least a few MB and far below 64 GB.
        assert!(mb > 1.0 && mb < 65_536.0, "{mb}");
    }

    #[test]
    fn result_json_has_the_card_shape() {
        let mut result = ScenarioResult::new("scroll", "typical", "split");
        result.metric("scroll_p95_ms", Some(2.25));
        result.metric("highlight_ms", None);
        result.samples("frame_ms", vec![1.0, 2.5]);
        result.info("files", serde_json::json!(30));
        let json = result.to_json();
        assert_eq!(
            json,
            serde_json::json!({
                "scenario": "scroll",
                "corpus": "typical",
                "layout": "split",
                "metrics": { "scroll_p95_ms": 2.25, "highlight_ms": null },
                "samples": { "frame_ms": [1.0, 2.5] },
                "info": { "files": 30 },
            })
        );
        // One line, so `--json` prints exactly one object.
        assert!(!result.to_json_line().contains('\n'));
        let human = result.to_human();
        assert!(human.contains("scroll_p95_ms"), "{human}");
        assert!(human.contains("2.25"), "{human}");
        assert!(human.contains("null"), "{human}");
    }
}
