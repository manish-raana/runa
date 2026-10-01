//! Resource samples and lifecycle events, recorded by each supervisor so
//! `runa metrics`, `runa status --json` and UIs can show history.
//!
//! Both are JSONL files under `~/.runa/metrics/`, capped so they never grow
//! without bound.

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
/// One hour of samples.
const MAX_SAMPLES: usize = 720;
const MAX_EVENTS: usize = 200;
const PS_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Sample {
    /// Unix seconds.
    pub t: u64,
    /// CPU use of the whole process group, in percent of one core.
    pub cpu: f64,
    pub rss_kb: u64,
    /// Processes in the group.
    pub procs: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// The command was spawned.
    Start,
    /// The command exited on its own.
    Exit,
    /// A restart was requested (`runa restart`).
    Restart,
    /// The supervisor was stopped.
    Stop,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Event {
    pub t: u64,
    pub event: EventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
}

impl Event {
    pub fn new(event: EventKind, pid: Option<i32>, code: Option<i32>) -> Self {
        Self {
            t: unix_now(),
            event,
            pid,
            code,
        }
    }
}

pub fn samples_path(run_dir: &Path, name: &str) -> PathBuf {
    run_dir
        .join("metrics")
        .join(format!("{name}.samples.jsonl"))
}

pub fn events_path(run_dir: &Path, name: &str) -> PathBuf {
    run_dir.join("metrics").join(format!("{name}.events.jsonl"))
}

/// An append-only JSONL file that keeps roughly its last `max` lines.
pub struct JsonlLog {
    path: PathBuf,
    max: usize,
    lines: usize,
}

impl JsonlLog {
    /// Starts an empty log at `path`, replacing any previous one.
    pub fn create(path: PathBuf, max: usize) -> Self {
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::remove_file(&path);
        Self {
            path,
            max,
            lines: 0,
        }
    }

    pub fn append<T: Serialize>(&mut self, record: &T) {
        let Ok(line) = serde_json::to_string(record) else {
            return;
        };
        let written = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut file| writeln!(file, "{line}"));
        if written.is_err() {
            return;
        }
        self.lines += 1;
        // Compact at twice the cap so the rewrite is rare.
        if self.lines > self.max * 2 {
            self.compact();
        }
    }

    fn compact(&mut self) {
        let Ok(content) = fs::read_to_string(&self.path) else {
            return;
        };
        let lines: Vec<&str> = content.lines().collect();
        let keep = &lines[lines.len().saturating_sub(self.max)..];
        let tmp = self.path.with_extension("jsonl.tmp");
        if fs::write(&tmp, keep.join("\n") + "\n").is_ok() && fs::rename(&tmp, &self.path).is_ok() {
            self.lines = keep.len();
        }
    }
}

pub fn sample_log(run_dir: &Path, name: &str) -> JsonlLog {
    JsonlLog::create(samples_path(run_dir, name), MAX_SAMPLES)
}

pub fn event_log(run_dir: &Path, name: &str) -> JsonlLog {
    JsonlLog::create(events_path(run_dir, name), MAX_EVENTS)
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> Vec<T> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Samples taken at or after `since` (unix seconds), oldest first.
pub fn read_samples(run_dir: &Path, name: &str, since: Option<u64>) -> Vec<Sample> {
    let mut samples: Vec<Sample> = read_jsonl(&samples_path(run_dir, name));
    if let Some(since) = since {
        samples.retain(|s| s.t >= since);
    }
    samples
}

pub fn read_events(run_dir: &Path, name: &str, since: Option<u64>) -> Vec<Event> {
    let mut events: Vec<Event> = read_jsonl(&events_path(run_dir, name));
    if let Some(since) = since {
        events.retain(|e| e.t >= since);
    }
    events
}

/// The newest sample, if it is recent enough to describe the process now.
pub fn current_sample(run_dir: &Path, name: &str) -> Option<Sample> {
    let latest = read_samples(run_dir, name, None).pop()?;
    let max_age = SAMPLE_INTERVAL.as_secs() * 3;
    (unix_now().saturating_sub(latest.t) <= max_age).then_some(latest)
}

/// Resource use summed over one process group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroupUsage {
    /// Cumulative CPU time in seconds.
    pub cpu_secs: f64,
    pub rss_kb: u64,
    pub procs: u32,
}

/// Sums `ps -A -o pgid=,rss=,time=` rows belonging to `pgid`.
pub fn parse_ps_usage(output: &str, pgid: i32) -> Option<GroupUsage> {
    let mut usage = GroupUsage {
        cpu_secs: 0.0,
        rss_kb: 0,
        procs: 0,
    };
    for line in output.lines() {
        let mut fields = line.split_whitespace();
        let (Some(group), Some(rss), Some(time)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if group.parse::<i32>().ok() != Some(pgid) {
            continue;
        }
        usage.rss_kb += rss.parse::<u64>().unwrap_or(0);
        usage.cpu_secs += parse_cpu_time(time).unwrap_or(0.0);
        usage.procs += 1;
    }
    (usage.procs > 0).then_some(usage)
}

/// Parses ps `time`: `mm:ss.cc` on macOS, `[dd-]hh:mm:ss` on Linux.
pub fn parse_cpu_time(value: &str) -> Option<f64> {
    let (days, clock) = match value.split_once('-') {
        Some((days, clock)) => (days.parse::<f64>().ok()?, clock),
        None => (0.0, value),
    };
    let mut secs = 0.0;
    let mut unit = 1.0;
    for part in clock.rsplit(':') {
        secs += part.parse::<f64>().ok()? * unit;
        unit *= 60.0;
    }
    Some(days * 86_400.0 + secs)
}

async fn group_usage(pgid: i32) -> Option<GroupUsage> {
    let output = tokio::time::timeout(
        PS_TIMEOUT,
        tokio::process::Command::new("ps")
            .args(["-A", "-o", "pgid=,rss=,time="])
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    parse_ps_usage(&String::from_utf8_lossy(&output.stdout), pgid)
}

/// Turns cumulative CPU time into a percentage between two readings.
#[derive(Default)]
pub struct CpuTracker {
    prev: Option<(i32, f64, Instant)>,
}

impl CpuTracker {
    /// `None` for the first reading of a group, which only sets the baseline.
    pub fn update(&mut self, pgid: i32, cpu_secs: f64, now: Instant) -> Option<f64> {
        let prev = self.prev.replace((pgid, cpu_secs, now));
        let (prev_pgid, prev_secs, prev_at) = prev?;
        let elapsed = now.duration_since(prev_at).as_secs_f64();
        if prev_pgid != pgid || elapsed <= 0.0 {
            return None;
        }
        // Exited group members take their CPU time with them: clamp at zero.
        let percent = ((cpu_secs - prev_secs) / elapsed * 100.0).max(0.0);
        Some((percent * 10.0).round() / 10.0)
    }
}

/// Samples the group led by `child` (0 when no child is running) until
/// the task is aborted.
pub async fn sample_loop(mut log: JsonlLog, child: Arc<AtomicI32>) {
    let mut interval = tokio::time::interval(SAMPLE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut cpu = CpuTracker::default();
    loop {
        interval.tick().await;
        let pgid = child.load(Ordering::Relaxed);
        if pgid <= 1 {
            continue;
        }
        let Some(usage) = group_usage(pgid).await else {
            continue;
        };
        if let Some(percent) = cpu.update(pgid, usage.cpu_secs, Instant::now()) {
            log.append(&Sample {
                t: unix_now(),
                cpu: percent,
                rss_kb: usage.rss_kb,
                procs: usage.procs,
            });
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_macos_and_linux_cpu_times() {
        assert_eq!(parse_cpu_time("0:01.50"), Some(1.5));
        assert_eq!(parse_cpu_time("12:34.56"), Some(754.56));
        assert_eq!(parse_cpu_time("01:02:03"), Some(3723.0));
        assert_eq!(parse_cpu_time("2-00:00:01"), Some(172_801.0));
        assert_eq!(parse_cpu_time("garbage"), None);
    }

    #[test]
    fn sums_only_the_matching_process_group() {
        let output = "  400  1000   0:01.00\n  400  2000   0:02.50\n  401  9999   9:00.00\n";

        let usage = parse_ps_usage(output, 400).unwrap();

        assert_eq!(usage.procs, 2);
        assert_eq!(usage.rss_kb, 3000);
        assert_eq!(usage.cpu_secs, 3.5);
        assert_eq!(parse_ps_usage(output, 402), None);
    }

    #[test]
    fn cpu_tracker_needs_a_baseline_and_resets_on_new_group() {
        let mut tracker = CpuTracker::default();
        let start = Instant::now();

        assert_eq!(tracker.update(400, 1.0, start), None);
        assert_eq!(
            tracker.update(400, 1.5, start + Duration::from_secs(5)),
            Some(10.0)
        );
        // A restarted child has a new group: no delta across groups.
        assert_eq!(
            tracker.update(500, 0.1, start + Duration::from_secs(10)),
            None
        );
        // CPU time lost to exited members never goes negative.
        assert_eq!(
            tracker.update(500, 0.0, start + Duration::from_secs(15)),
            Some(0.0)
        );
    }

    #[test]
    fn jsonl_log_keeps_the_newest_records() {
        let dir = std::env::temp_dir().join(format!("runa-metrics-{}", std::process::id()));
        let path = dir.join("p.samples.jsonl");
        let mut log = JsonlLog::create(path.clone(), 3);

        for t in 0..10 {
            log.append(&Sample {
                t,
                cpu: 0.0,
                rss_kb: 0,
                procs: 1,
            });
        }

        let kept: Vec<u64> = read_jsonl::<Sample>(&path).iter().map(|s| s.t).collect();
        assert!(kept.len() <= 6, "log was not compacted: {kept:?}");
        assert_eq!(kept.last(), Some(&9));
        let _ = fs::remove_dir_all(dir);
    }
}
