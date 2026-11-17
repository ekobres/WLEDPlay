//! Once-a-second host and send-timing sample for the Statistics card.
//!
//! CPU and memory prefer the container cgroup when this process is the
//! cgroup root (a container). Otherwise they are this process. Send lateness
//! is how long the latest due packet waited past its slot.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HostUsage {
    pub cpu_usec: Option<u64>,
    pub memory_bytes: Option<u64>,
    pub container: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HostReading {
    pub cpu_percent: Option<f32>,
    pub memory_bytes: Option<u64>,
    pub container: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PerfSnapshot {
    pub cpu_percent: Option<f32>,
    pub memory_bytes: Option<u64>,
    pub container: bool,
    pub send_late_ms: u32,
    pub send_jitter_ms: u32,
    pub delay_drops: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HostSampler {
    prev_cpu_usec: Option<u64>,
    prev_at: Option<Instant>,
}

impl HostSampler {
    pub fn sample(&mut self, now: Instant) -> HostReading {
        self.sample_with(now, read_host_usage())
    }

    pub fn sample_with(&mut self, now: Instant, usage: HostUsage) -> HostReading {
        let cpu_percent = match (self.prev_cpu_usec, self.prev_at, usage.cpu_usec) {
            (Some(prev), Some(at), Some(next)) => {
                cpu_percent(prev, next, now.saturating_duration_since(at))
            }
            _ => None,
        };
        if usage.cpu_usec.is_some() {
            self.prev_cpu_usec = usage.cpu_usec;
            self.prev_at = Some(now);
        }
        HostReading {
            cpu_percent,
            memory_bytes: usage.memory_bytes,
            container: usage.container,
        }
    }
}

/// Latest lateness, and the spread of lateness, across one status interval.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SendSpread {
    count: u32,
    min_ms: u32,
    max_ms: u32,
    last_ms: u32,
}

impl SendSpread {
    pub fn record(&mut self, late: Duration) {
        let ms = u32::try_from(late.as_millis()).unwrap_or(u32::MAX);
        if self.count == 0 {
            self.min_ms = ms;
            self.max_ms = ms;
        } else {
            self.min_ms = self.min_ms.min(ms);
            self.max_ms = self.max_ms.max(ms);
        }
        self.last_ms = ms;
        self.count = self.count.saturating_add(1);
    }

    pub fn take(&mut self) -> (u32, u32) {
        if self.count == 0 {
            return (0, 0);
        }
        let out = (self.last_ms, self.max_ms.saturating_sub(self.min_ms));
        *self = Self::default();
        out
    }
}

pub fn cpu_percent(prev_usec: u64, next_usec: u64, elapsed: Duration) -> Option<f32> {
    let wall = u64::try_from(elapsed.as_micros()).unwrap_or(0);
    let wall = std::num::NonZeroU64::new(wall)?;
    let used = next_usec.saturating_sub(prev_usec);
    Some((used as f32) * 100.0 / (wall.get() as f32))
}

pub fn cgroup_v2_path(text: &str) -> Option<&str> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("0::") {
            return if rest.is_empty() { None } else { Some(rest) };
        }
    }
    None
}

pub fn use_cgroup(text: &str) -> bool {
    cgroup_v2_path(text) == Some("/")
}

pub fn cgroup_usage_usec(cpu_stat: &str) -> Option<u64> {
    for line in cpu_stat.lines() {
        let mut parts = line.split_whitespace();
        if parts.next()? == "usage_usec" {
            return parts.next()?.parse().ok();
        }
    }
    None
}

pub fn cgroup_memory_bytes(text: &str) -> Option<u64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    text.parse().ok()
}

pub fn rss_bytes(statm: &str, page_size: u64) -> Option<u64> {
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    pages.checked_mul(page_size)
}

pub fn page_size_or(n: i64) -> u64 {
    match u64::try_from(n) {
        Ok(v) if v > 0 => v,
        _ => 4096,
    }
}

pub fn timeval_usec(sec: i64, usec: i32) -> u64 {
    let secs = u64::try_from(sec).unwrap_or(0);
    let usecs = u64::try_from(usec).unwrap_or(0);
    secs.saturating_mul(1_000_000).saturating_add(usecs)
}

pub fn cpu_usec_from_rusage(
    rc: i32,
    user_sec: i64,
    user_usec: i32,
    sys_sec: i64,
    sys_usec: i32,
) -> Option<u64> {
    if rc != 0 {
        return None;
    }
    Some(timeval_usec(user_sec, user_usec).saturating_add(timeval_usec(sys_sec, sys_usec)))
}

pub fn resolve_host(
    cgroup_text: Option<&str>,
    cgroup: Option<HostUsage>,
    process: HostUsage,
) -> HostUsage {
    if cgroup_text.is_some_and(use_cgroup) {
        if let Some(cgroup) = cgroup {
            return cgroup;
        }
    }
    process
}

pub fn select_cgroup(text: Option<&str>, dir: &Path) -> Option<HostUsage> {
    if text.is_some_and(use_cgroup) {
        read_cgroup_files(dir)
    } else {
        None
    }
}

pub fn read_cgroup_files(dir: &Path) -> Option<HostUsage> {
    let cpu = fs::read_to_string(dir.join("cpu.stat")).ok()?;
    let mem = fs::read_to_string(dir.join("memory.current")).ok()?;
    Some(HostUsage {
        cpu_usec: cgroup_usage_usec(&cpu),
        memory_bytes: cgroup_memory_bytes(&mem),
        container: true,
    })
}

pub fn rss_from_file(path: &Path, page_size: u64) -> Option<u64> {
    let text = fs::read_to_string(path).ok()?;
    rss_bytes(&text, page_size)
}

fn page_size() -> u64 {
    page_size_or(unsafe { libc::sysconf(libc::_SC_PAGESIZE) })
}

fn as_i64<T: Into<i64>>(v: T) -> i64 {
    v.into()
}

fn usec_i32(v: i64) -> i32 {
    i32::try_from(v).unwrap_or(0)
}

fn rusage_cpu_usec() -> Option<u64> {
    let mut usage = unsafe { std::mem::zeroed::<libc::rusage>() };
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    cpu_usec_from_rusage(
        rc,
        as_i64(usage.ru_utime.tv_sec),
        usec_i32(as_i64(usage.ru_utime.tv_usec)),
        as_i64(usage.ru_stime.tv_sec),
        usec_i32(as_i64(usage.ru_stime.tv_usec)),
    )
}

fn read_process_usage() -> HostUsage {
    HostUsage {
        cpu_usec: rusage_cpu_usec(),
        memory_bytes: rss_from_file(Path::new("/proc/self/statm"), page_size()),
        container: false,
    }
}

pub fn read_host_usage() -> HostUsage {
    let text = fs::read_to_string("/proc/self/cgroup").ok();
    let cgroup = select_cgroup(text.as_deref(), Path::new("/sys/fs/cgroup"));
    resolve_host(text.as_deref(), cgroup, read_process_usage())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn cpu_percent_is_one_core_over_the_interval() {
        assert_eq!(
            cpu_percent(1_000, 1_500, Duration::from_millis(1)),
            Some(50.0)
        );
        assert_eq!(cpu_percent(50, 10, Duration::from_millis(1)), Some(0.0));
        assert!(cpu_percent(0, 1, Duration::ZERO).is_none());
        assert!(cpu_percent(0, 1, Duration::from_nanos(1)).is_none());
    }

    #[test]
    fn cgroup_text_parses_the_container_root_only() {
        assert_eq!(cgroup_v2_path("0::/\n"), Some("/"));
        assert_eq!(
            cgroup_v2_path("0::/system.slice/app\n"),
            Some("/system.slice/app")
        );
        assert_eq!(cgroup_v2_path("0::\n"), None);
        assert_eq!(cgroup_v2_path("1:name=systemd:/\n"), None);
        assert!(use_cgroup("0::/\n"));
        assert!(!use_cgroup("0::/user.slice\n"));
        assert_eq!(cgroup_usage_usec("user_usec 1\nusage_usec 42\n"), Some(42));
        assert_eq!(cgroup_usage_usec("usage_usec nope\n"), None);
        assert_eq!(cgroup_usage_usec("nr_periods 1\n"), None);
        assert_eq!(cgroup_memory_bytes(" 8192\n"), Some(8192));
        assert_eq!(cgroup_memory_bytes("   "), None);
        assert_eq!(cgroup_memory_bytes("nope"), None);
    }

    #[test]
    fn rss_uses_resident_pages() {
        assert_eq!(rss_bytes("10 4 1 2 0 1 0", 4096), Some(16384));
        assert_eq!(rss_bytes("10", 4096), None);
        assert_eq!(rss_bytes("10 nope", 4096), None);
        assert_eq!(rss_bytes("10 4", 0), Some(0));
        assert_eq!(page_size_or(4096), 4096);
        assert_eq!(page_size_or(0), 4096);
        assert_eq!(page_size_or(-1), 4096);
        assert!(page_size() >= 4096);
    }

    #[test]
    fn rusage_times_add_and_reject_a_failed_call() {
        assert_eq!(timeval_usec(2, 5), 2_000_005);
        assert_eq!(timeval_usec(-1, -5), 0);
        assert_eq!(cpu_usec_from_rusage(0, 1, 0, 0, 250), Some(1_000_250));
        assert_eq!(cpu_usec_from_rusage(-1, 1, 0, 0, 0), None);
        assert!(rusage_cpu_usec().is_some());
    }

    #[test]
    fn container_sample_wins_only_at_the_cgroup_root() {
        let container = HostUsage {
            cpu_usec: Some(10),
            memory_bytes: Some(20),
            container: true,
        };
        let process = HostUsage {
            cpu_usec: Some(1),
            memory_bytes: Some(2),
            container: false,
        };
        assert_eq!(
            resolve_host(Some("0::/\n"), Some(container), process),
            container
        );
        assert_eq!(resolve_host(Some("0::/\n"), None, process), process);
        assert_eq!(
            resolve_host(Some("0::/user.slice\n"), Some(container), process),
            process
        );
        assert_eq!(resolve_host(None, Some(container), process), process);
    }

    #[test]
    fn cgroup_and_statm_files_round_trip() {
        let dir = std::env::temp_dir().join(format!("wled-perf-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("cpu.stat"), "usage_usec 77\n").unwrap();
        fs::write(dir.join("memory.current"), "4096\n").unwrap();
        let usage = read_cgroup_files(&dir).unwrap();
        assert_eq!(usage.cpu_usec, Some(77));
        assert_eq!(usage.memory_bytes, Some(4096));
        assert!(usage.container);
        fs::write(dir.join("cpu.stat"), "usage_usec bad\n").unwrap();
        assert!(read_cgroup_files(&dir).unwrap().cpu_usec.is_none());
        let statm = dir.join("statm");
        fs::write(&statm, "8 2 1 1 0 1 0\n").unwrap();
        assert_eq!(rss_from_file(&statm, 4096), Some(8192));
        assert!(rss_from_file(dir.join("missing").as_path(), 4096).is_none());
        fs::remove_file(dir.join("memory.current")).unwrap();
        assert!(read_cgroup_files(&dir).is_none());
        assert!(read_cgroup_files(dir.join("missing").as_path()).is_none());
        assert!(select_cgroup(Some("0::/\n"), &dir).is_none());
        fs::write(dir.join("memory.current"), "8\n").unwrap();
        assert!(select_cgroup(Some("0::/\n"), &dir).is_some());
        assert!(select_cgroup(None, &dir).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sampler_needs_two_readings() {
        let mut sampler = HostSampler::default();
        let t0 = Instant::now();
        let first = sampler.sample_with(
            t0,
            HostUsage {
                cpu_usec: Some(1_000),
                memory_bytes: Some(10),
                container: false,
            },
        );
        assert!(first.cpu_percent.is_none());
        assert_eq!(first.memory_bytes, Some(10));
        let second = sampler.sample_with(
            t0 + Duration::from_millis(100),
            HostUsage {
                cpu_usec: Some(51_000),
                memory_bytes: Some(12),
                container: true,
            },
        );
        assert!((second.cpu_percent.unwrap() - 50.0).abs() < 0.01);
        assert!(second.container);
        let skipped = sampler.sample_with(
            t0 + Duration::from_millis(200),
            HostUsage {
                cpu_usec: None,
                memory_bytes: None,
                container: false,
            },
        );
        assert!(skipped.cpu_percent.is_none());
        assert!(!skipped.container);
        let _ = sampler.sample(Instant::now());
        let _ = read_host_usage();
        assert_eq!(as_i64(12_i32), 12);
        assert_eq!(usec_i32(12), 12);
        assert_eq!(usec_i32(i64::from(i32::MAX) + 1), 0);
    }

    #[test]
    fn send_spread_keeps_the_latest_and_the_range() {
        let mut spread = SendSpread::default();
        assert_eq!(spread.take(), (0, 0));
        spread.record(Duration::from_millis(5));
        spread.record(Duration::from_millis(12));
        spread.record(Duration::from_secs(5_000_000));
        assert_eq!(spread.take(), (u32::MAX, u32::MAX - 5));
        assert_eq!(spread.take(), (0, 0));
        spread.record(Duration::from_millis(3));
        assert_eq!(spread.take(), (3, 0));
    }
}
