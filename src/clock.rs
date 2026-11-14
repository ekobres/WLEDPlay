//! NQPTP shared-memory clock, plus the first-frame lead Shairport reports.
//!
//! The raw local-to-master offset is an epoch difference and is not shown.
//! Sample age is how long since the last follow-up. Step is the last grandmaster
//! change. First-frame lead is presentation time minus now when audio starts:
//! negative means that frame was already late.

use crate::metadata::MetaItem;
use serde::Serialize;
use std::path::Path;

pub const NQPTP_VERSION: u16 = 11;
const SHM_BYTES: usize = 88;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClockSession {
    pub first_frame_lead_ms: Option<f64>,
    pub discontinuity_frames: Option<i32>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ClockStatus {
    pub present: bool,
    pub consistent: bool,
    pub version: u16,
    pub version_ok: bool,
    pub master_id: String,
    pub sample_age_ms: Option<f64>,
    pub master_age_s: Option<f64>,
    pub step_ms: Option<f64>,
    pub first_frame_lead_ms: Option<f64>,
    pub discontinuity_frames: Option<i32>,
    pub summary: String,
    pub detail: String,
    pub level: String,
}

impl Default for ClockStatus {
    fn default() -> Self {
        Self::absent()
    }
}

impl ClockStatus {
    pub fn absent() -> Self {
        Self {
            present: false,
            consistent: false,
            version: 0,
            version_ok: false,
            master_id: String::new(),
            sample_age_ms: None,
            master_age_s: None,
            step_ms: None,
            first_frame_lead_ms: None,
            discontinuity_frames: None,
            summary: "not running".into(),
            detail: "NQPTP has not published a clock".into(),
            level: "idle".into(),
        }
    }

    pub fn with_playback(mut self, session: &ClockSession, stream_active: bool) -> Self {
        self.first_frame_lead_ms = session.first_frame_lead_ms;
        self.discontinuity_frames = session.discontinuity_frames;
        self.finish(stream_active);
        self
    }

    fn finish(&mut self, stream_active: bool) {
        if !self.present {
            if let Some(lead) = self.first_frame_lead_ms {
                let phrase = frame_phrase(lead);
                self.summary = phrase.clone();
                if lead.abs() >= 1_000.0 {
                    self.level = "bad".into();
                } else if lead.abs() >= 50.0 {
                    self.level = "warn".into();
                }
                self.detail = format!("{phrase} · {}", self.detail);
            }
            if let Some(gap) = self.discontinuity_frames {
                self.detail = format!("{} · gap {gap} frames", self.detail);
            }
            return;
        }
        if self.version == 0 {
            self.summary = "waiting".into();
            self.detail = "NQPTP clock record is not initialised".into();
            self.level = "idle".into();
            return;
        }
        if !self.version_ok {
            self.summary = format!("version {}", self.version);
            self.detail = format!("expected NQPTP interface version {NQPTP_VERSION}");
            self.level = "bad".into();
            return;
        }
        if !self.consistent {
            self.summary = "unstable".into();
            self.detail = "the two NQPTP clock records do not match".into();
            self.level = "bad".into();
            return;
        }
        if self.master_id.is_empty() {
            self.summary = "no master".into();
            self.detail = "no PTP grandmaster is selected".into();
            self.level = if stream_active { "bad" } else { "idle" }.into();
            return;
        }

        let step_bad = self.step_ms.is_some_and(|ms| ms.abs() >= 1_000.0);
        let age_bad = self
            .sample_age_ms
            .is_some_and(|ms| ms >= 10_000.0 || ms <= -20.0);
        let lead_bad = self
            .first_frame_lead_ms
            .is_some_and(|ms| ms.abs() >= 1_000.0);
        // A few milliseconds of grandmaster movement is the discrepancy worth showing.
        let step_warn = self.step_ms.is_some_and(|ms| ms.abs() >= 5.0);
        let age_warn = self
            .sample_age_ms
            .is_some_and(|ms| ms >= 2_000.0 || ms <= -2.0);
        let lead_warn = self.first_frame_lead_ms.is_some_and(|ms| ms.abs() >= 50.0);

        self.level = if step_bad || age_bad || lead_bad {
            "bad"
        } else if step_warn || age_warn || lead_warn {
            "warn"
        } else {
            "ok"
        }
        .into();

        self.summary = if step_bad {
            format!("last step {}", format_ms(self.step_ms.unwrap_or(0.0)))
        } else if age_bad {
            format!(
                "sample {} old",
                format_ms(self.sample_age_ms.unwrap_or(0.0))
            )
        } else if lead_bad {
            frame_phrase(self.first_frame_lead_ms.unwrap_or(0.0))
        } else if age_warn {
            format!(
                "sample {} old",
                format_ms(self.sample_age_ms.unwrap_or(0.0))
            )
        } else if step_warn {
            format!("last step {}", format_ms(self.step_ms.unwrap_or(0.0)))
        } else if lead_warn {
            frame_phrase(self.first_frame_lead_ms.unwrap_or(0.0))
        } else if let Some(age) = self.sample_age_ms {
            format!("sample {} old", format_ms(age))
        } else {
            "locked".into()
        };

        let mut parts = Vec::new();
        parts.push(format!("master {}", self.master_id));
        if let Some(held) = self.master_age_s {
            parts.push(format_held(held));
        }
        if let Some(step) = self.step_ms {
            parts.push(format!("last step {}", format_ms(step)));
        }
        if let Some(age) = self.sample_age_ms {
            parts.push(format!("sample {} old", format_ms(age)));
        }
        if let Some(lead) = self.first_frame_lead_ms {
            parts.push(frame_phrase(lead));
        }
        if let Some(gap) = self.discontinuity_frames {
            parts.push(format!("gap {gap} frames"));
        }
        self.detail = parts.join(" · ");
    }
}

pub fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    monotonic_from(rc, ts.tv_sec, ts.tv_nsec)
}

pub fn monotonic_from(rc: i32, sec: i64, nsec: i64) -> u64 {
    if rc != 0 {
        return 0;
    }
    (sec.max(0) as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(nsec.max(0) as u64)
}

pub fn read_from_dir(dir: &Path, now_ns: u64) -> ClockStatus {
    let preferred = dir.join("nqptp");
    if preferred.is_file() {
        return read_file(&preferred, now_ns);
    }
    let mut names = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .collect::<Vec<_>>(),
        Err(_) => return ClockStatus::absent(),
    };
    names.retain(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("nqptp"))
    });
    names.sort();
    let mut fallback = None;
    for path in names {
        let status = read_file(&path, now_ns);
        if !status.master_id.is_empty() {
            return status;
        }
        if fallback.is_none() && status.present {
            fallback = Some(status);
        }
    }
    fallback.unwrap_or_else(ClockStatus::absent)
}

pub fn read_file(path: &Path, now_ns: u64) -> ClockStatus {
    match std::fs::read(path) {
        Ok(bytes) => parse_shm(&bytes, now_ns),
        Err(_) => ClockStatus::absent(),
    }
}

pub fn parse_shm(bytes: &[u8], now_ns: u64) -> ClockStatus {
    if bytes.len() < SHM_BYTES {
        let mut status = ClockStatus::absent();
        status.summary = "incomplete".into();
        status.detail = "NQPTP clock record is too short".into();
        return status;
    }
    let version = u16::from_le_bytes([bytes[0], bytes[1]]);
    let mut status = ClockStatus {
        present: true,
        consistent: bytes[8..48] == bytes[48..88],
        version,
        version_ok: version == NQPTP_VERSION,
        ..ClockStatus::absent()
    };
    status.present = true;
    if version == 0 || version != NQPTP_VERSION || !status.consistent {
        status.finish(false);
        return status;
    }
    let master = read_u64(bytes, 8);
    if master != 0 {
        status.master_id = format!("{master:016x}");
        status.sample_age_ms = age_ms(now_ns, read_u64(bytes, 16));
        status.master_age_s = age_s(now_ns, read_u64(bytes, 40));
        status.step_ms =
            Some(i64::from_le_bytes(bytes[32..40].try_into().unwrap()) as f64 / 1_000_000.0);
    }
    status.finish(false);
    status
}

pub fn fold_metadata(item: &MetaItem, now_ns: u64, session: &mut ClockSession) {
    if item.type_code != "ssnc" {
        return;
    }
    match item.code.as_str() {
        "pend" | "pfls" => *session = ClockSession::default(),
        "pffr" => {
            if let Some(lead) = first_frame_lead_ms(&item.data, now_ns) {
                session.first_frame_lead_ms = Some(lead);
            }
        }
        "pdis" => {
            if let Some(gap) = discontinuity_frames(&item.data) {
                session.discontinuity_frames = Some(gap);
            }
        }
        _ => {}
    }
}

fn first_frame_lead_ms(data: &[u8], now_ns: u64) -> Option<f64> {
    let text = std::str::from_utf8(data).ok()?.trim();
    let play: u64 = text.split('/').nth(1)?.trim().parse().ok()?;
    Some((play as i128 - now_ns as i128) as f64 / 1_000_000.0)
}

fn discontinuity_frames(data: &[u8]) -> Option<i32> {
    let text = std::str::from_utf8(data).ok()?.trim();
    text.split('/').nth(1)?.trim().parse().ok()
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn age_ms(now_ns: u64, sample_ns: u64) -> Option<f64> {
    if sample_ns == 0 {
        return None;
    }
    Some((now_ns as i128 - sample_ns as i128) as f64 / 1_000_000.0)
}

fn age_s(now_ns: u64, start_ns: u64) -> Option<f64> {
    if start_ns == 0 || now_ns < start_ns {
        return None;
    }
    Some((now_ns - start_ns) as f64 / 1_000_000_000.0)
}

fn format_ms(ms: f64) -> String {
    let sign = if ms < 0.0 { "-" } else { "" };
    let magnitude = ms.abs();
    if magnitude >= 1_000.0 {
        format!("{sign}{:.1} s", magnitude / 1_000.0)
    } else if magnitude >= 100.0 {
        format!("{sign}{:.0} ms", magnitude)
    } else {
        format!("{sign}{:.1} ms", magnitude)
    }
}

fn frame_phrase(lead_ms: f64) -> String {
    if lead_ms < 0.0 {
        format!("first frame {} late", format_ms(-lead_ms))
    } else {
        format!("first frame {} early", format_ms(lead_ms))
    }
}

fn format_held(seconds: f64) -> String {
    if seconds >= 120.0 {
        format!("held {:.0} min", seconds / 60.0)
    } else {
        format!("held {:.0} s", seconds.max(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn record(master: u64, local: u64, step_ns: i64, start: u64) -> [u8; 40] {
        let mut bytes = [0u8; 40];
        bytes[0..8].copy_from_slice(&master.to_le_bytes());
        bytes[8..16].copy_from_slice(&local.to_le_bytes());
        bytes[24..32].copy_from_slice(&step_ns.to_le_bytes());
        bytes[32..40].copy_from_slice(&start.to_le_bytes());
        bytes
    }

    fn shm(version: u16, main: &[u8], secondary: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0u8; SHM_BYTES];
        bytes[0..2].copy_from_slice(&version.to_le_bytes());
        bytes[8..48].copy_from_slice(main);
        bytes[48..88].copy_from_slice(secondary);
        bytes
    }

    #[test]
    fn absent_short_waiting_and_version_mismatch() {
        let absent = parse_shm(&[], 0);
        assert_eq!(absent.summary, "incomplete");
        assert!(!absent.present);
        let waiting = parse_shm(&shm(0, &[0; 40], &[0; 40]), 0);
        assert_eq!(waiting.summary, "waiting");
        assert_eq!(waiting.level, "idle");
        let wrong = parse_shm(&shm(7, &[0; 40], &[0; 40]), 0);
        assert_eq!(wrong.summary, "version 7");
        assert_eq!(wrong.level, "bad");
    }

    #[test]
    fn unstable_and_masterless_reads() {
        let main = record(1, 1, 0, 1);
        let status = parse_shm(&shm(NQPTP_VERSION, &main, &[0; 40]), 1);
        assert_eq!(status.summary, "unstable");
        let quiet = parse_shm(&shm(NQPTP_VERSION, &[0; 40], &[0; 40]), 0)
            .with_playback(&ClockSession::default(), false);
        assert_eq!(quiet.summary, "no master");
        assert_eq!(quiet.level, "idle");
        let live = parse_shm(&shm(NQPTP_VERSION, &[0; 40], &[0; 40]), 0)
            .with_playback(&ClockSession::default(), true);
        assert_eq!(live.level, "bad");
    }

    #[test]
    fn healthy_stale_and_jumped_clocks() {
        let now = 200_000_000_000u64;
        let main = record(0xe02b, now - 18_000_000, 100_000, now - 30_000_000_000);
        let ok = parse_shm(&shm(NQPTP_VERSION, &main, &main), now);
        assert_eq!(ok.level, "ok");
        assert_eq!(ok.master_id, "000000000000e02b");
        assert!(ok.summary.starts_with("sample"));
        assert!(ok.detail.contains("held 30 s"));
        assert!(ok.detail.contains("last step"));

        let stale = record(0xe02b, now - 12_000_000_000, 0, now);
        let stale = parse_shm(&shm(NQPTP_VERSION, &stale, &stale), now);
        assert_eq!(stale.level, "bad");
        assert!(stale.summary.starts_with("sample"));

        let jumped = record(0xe02b, now - 10_000_000, -9_700_000_000, 0);
        let jumped = parse_shm(&shm(NQPTP_VERSION, &jumped, &jumped), now);
        assert_eq!(jumped.level, "bad");
        assert!(jumped.summary.starts_with("last step"));
        assert!(jumped.summary.contains("9.7 s"));
        assert!(jumped.detail.contains("sample"));

        let warned = record(0xe02b, now - 2_500_000_000, 25_000_000, now - 1_000_000_000);
        let warned = parse_shm(&shm(NQPTP_VERSION, &warned, &warned), now);
        assert_eq!(warned.level, "warn");
        assert!(warned.summary.starts_with("sample"));

        let nudged = record(0xe02b, now - 18_000_000, 250_000_000, now - 1_000_000_000);
        let nudged = parse_shm(&shm(NQPTP_VERSION, &nudged, &nudged), now);
        assert_eq!(nudged.level, "warn");
        assert!(nudged.summary.starts_with("last step"));
        assert!(nudged.summary.contains("250 ms"));

        let backwards = record(0xe02b, now + 50_000_000, 0, now + 1);
        let backwards = parse_shm(&shm(NQPTP_VERSION, &backwards, &backwards), now);
        assert_eq!(backwards.level, "bad");
        assert!(backwards.master_age_s.is_none());
    }

    #[test]
    fn playback_lead_and_gap_join_the_reading() {
        let now = 200_000_000_000u64;
        let main = record(0xe02b, now - 5_000_000, 0, now - 150_000_000_000);
        let clock = parse_shm(&shm(NQPTP_VERSION, &main, &main), now).with_playback(
            &ClockSession {
                first_frame_lead_ms: Some(-9_700.0),
                discontinuity_frames: Some(1024),
            },
            true,
        );
        assert_eq!(clock.level, "bad");
        assert!(clock.summary.contains("9.7 s") || clock.detail.contains("late"));
        assert!(clock.detail.contains("late"));
        assert!(clock.detail.contains("gap 1024 frames"));
        assert!(clock.detail.contains("held 2 min"));

        let early = parse_shm(&shm(NQPTP_VERSION, &main, &main), now).with_playback(
            &ClockSession {
                first_frame_lead_ms: Some(80.0),
                discontinuity_frames: None,
            },
            false,
        );
        assert!(early.detail.contains("early") || early.summary.contains("early"));
        assert_eq!(early.level, "warn");
    }

    #[test]
    fn metadata_notes_lead_gap_and_stop() {
        let mut session = ClockSession::default();
        fold_metadata(
            &MetaItem {
                type_code: "core".into(),
                code: "minm".into(),
                data: vec![],
            },
            0,
            &mut session,
        );
        fold_metadata(
            &MetaItem {
                type_code: "ssnc".into(),
                code: "pffr".into(),
                data: b"10/5000000".to_vec(),
            },
            1_000_000,
            &mut session,
        );
        assert!(session.first_frame_lead_ms.unwrap() > 0.0);
        fold_metadata(
            &MetaItem {
                type_code: "ssnc".into(),
                code: "pffr".into(),
                data: vec![0xff],
            },
            0,
            &mut session,
        );
        fold_metadata(
            &MetaItem {
                type_code: "ssnc".into(),
                code: "pffr".into(),
                data: b"not-a-lead".to_vec(),
            },
            0,
            &mut session,
        );
        fold_metadata(
            &MetaItem {
                type_code: "ssnc".into(),
                code: "pdis".into(),
                data: vec![0xff],
            },
            0,
            &mut session,
        );
        assert!(session.first_frame_lead_ms.is_some());
        fold_metadata(
            &MetaItem {
                type_code: "ssnc".into(),
                code: "pdis".into(),
                data: b"10/-48000".to_vec(),
            },
            0,
            &mut session,
        );
        assert_eq!(session.discontinuity_frames, Some(-48000));
        fold_metadata(
            &MetaItem {
                type_code: "ssnc".into(),
                code: "pdis".into(),
                data: b"nope".to_vec(),
            },
            0,
            &mut session,
        );
        fold_metadata(
            &MetaItem {
                type_code: "ssnc".into(),
                code: "pend".into(),
                data: vec![],
            },
            0,
            &mut session,
        );
        assert!(session.first_frame_lead_ms.is_none());
        session.discontinuity_frames = Some(1);
        fold_metadata(
            &MetaItem {
                type_code: "ssnc".into(),
                code: "pfls".into(),
                data: vec![],
            },
            0,
            &mut session,
        );
        assert!(session.discontinuity_frames.is_none());
        fold_metadata(
            &MetaItem {
                type_code: "ssnc".into(),
                code: "clip".into(),
                data: vec![],
            },
            0,
            &mut session,
        );
    }

    #[test]
    fn directory_prefers_the_live_master() {
        let dir = std::env::temp_dir().join(format!("wledplay-clock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_from_dir(&dir, 0).summary, "not running");
        assert_eq!(
            read_from_dir(Path::new("/no/such/wledplay-clock"), 0).summary,
            "not running"
        );
        assert_eq!(read_file(&dir.join("missing"), 0).summary, "not running");
        let late = ClockStatus::absent().with_playback(
            &ClockSession {
                first_frame_lead_ms: Some(-9_700.0),
                discontinuity_frames: Some(512),
            },
            true,
        );
        assert_eq!(late.level, "bad");
        assert!(late.summary.contains("late"));
        assert!(late.detail.contains("gap 512 frames"));
        let quiet = ClockStatus::absent().with_playback(
            &ClockSession {
                first_frame_lead_ms: Some(4.0),
                discontinuity_frames: None,
            },
            false,
        );
        assert_eq!(quiet.level, "idle");
        let caution = ClockStatus::absent().with_playback(
            &ClockSession {
                first_frame_lead_ms: Some(-80.0),
                discontinuity_frames: None,
            },
            false,
        );
        assert_eq!(caution.level, "warn");
        assert!(caution.summary.contains("late"));

        let now = 8_000_000_000u64;
        let idle = record(0, 0, 0, 0);
        let live = record(0xabc, now - 1_000_000, 0, now - 1_000_000_000);
        fs::write(dir.join("nqptp-b"), shm(NQPTP_VERSION, &live, &live)).unwrap();
        fs::write(dir.join("nqptp-a"), shm(NQPTP_VERSION, &idle, &idle)).unwrap();
        fs::write(dir.join("other"), b"nope").unwrap();
        let picked = read_from_dir(&dir, now);
        assert_eq!(picked.master_id, "0000000000000abc");

        fs::write(dir.join("nqptp"), shm(NQPTP_VERSION, &idle, &idle)).unwrap();
        let preferred = read_from_dir(&dir, now);
        assert!(preferred.master_id.is_empty());
        assert_eq!(preferred.summary, "no master");

        fs::write(dir.join("notes.txt"), b"ignore").unwrap();
        let only = dir.join("siblings");
        fs::create_dir_all(&only).unwrap();
        fs::write(only.join("nqptp-z"), shm(NQPTP_VERSION, &idle, &idle)).unwrap();
        assert_eq!(read_from_dir(&only, now).summary, "no master");
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&only);
        assert!(monotonic_ns() > 0 || monotonic_ns() == 0);
        assert_eq!(monotonic_from(-1, 5, 5), 0);
        assert_eq!(monotonic_from(0, -1, -1), 0);
        assert_eq!(monotonic_from(0, 2, 3), 2_000_000_003);
    }

    #[test]
    fn locked_without_a_sample_still_names_the_master() {
        let main = record(4, 0, 0, 0);
        let status = parse_shm(&shm(NQPTP_VERSION, &main, &main), 10);
        assert_eq!(status.summary, "locked");
        assert!(status.sample_age_ms.is_none());
        assert_eq!(status.level, "ok");
    }
}
