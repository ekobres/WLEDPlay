//! Live WLEDPlay log level. `RUST_LOG` still wins at process start.
//!
//! Timestamps are local civil time with a numeric offset (`-05:00`, `+00:00`).

use std::fmt;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::EnvFilter;

/// Wall clock for tracing lines. The offset is read when the line is written.
#[derive(Clone, Copy, Debug)]
pub struct LocalTimer;

impl FormatTime for LocalTimer {
    fn format_time(&self, w: &mut Writer<'_>) -> fmt::Result {
        w.write_str(&format_local_stamp())
    }
}

/// `±HH:MM` for a seconds-east-of-UTC offset. Zero is `+00:00`.
pub fn format_offset(seconds_east: i32) -> String {
    let sign = if seconds_east < 0 { '-' } else { '+' };
    let abs = seconds_east.unsigned_abs();
    let hours = abs / 3600;
    let mins = (abs % 3600) / 60;
    format!("{sign}{hours:02}:{mins:02}")
}

fn offset_seconds(east: i64) -> i32 {
    i32::try_from(east).unwrap_or(0)
}

fn secs_from_elapsed(secs: u64) -> i64 {
    i64::try_from(secs).unwrap_or(i64::MAX)
}

fn micros_since_epoch(now: SystemTime) -> (i64, u32) {
    match now.duration_since(UNIX_EPOCH) {
        Ok(elapsed) => (
            secs_from_elapsed(elapsed.as_secs()),
            elapsed.subsec_micros(),
        ),
        Err(_) => (0, 0),
    }
}

fn civil_local(secs: i64) -> Option<libc::tm> {
    let t: libc::time_t = secs;
    let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
    // SAFETY: `t` is a valid time_t and `tm` is a writable libc::tm.
    // localtime_r is thread-safe and writes `tm` only when it returns non-null.
    let ptr = unsafe { libc::localtime_r(&t, &mut tm) };
    if ptr.is_null() {
        None
    } else {
        Some(tm)
    }
}

pub fn format_local_from_tm(tm: &libc::tm, micros: u32) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}{}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        micros,
        format_offset(offset_seconds(tm.tm_gmtoff))
    )
}

fn format_from_civil(tm: Option<libc::tm>, micros: u32) -> String {
    match tm {
        Some(tm) => format_local_from_tm(&tm, micros),
        None => format!("1970-01-01T00:00:00.{micros:06}+00:00"),
    }
}

pub fn format_unix_local(secs: i64, micros: u32) -> String {
    format_from_civil(civil_local(secs), micros)
}

pub fn format_local_stamp() -> String {
    let (secs, micros) = micros_since_epoch(SystemTime::now());
    format_unix_local(secs, micros)
}

type ReloadFn = Box<dyn Fn(EnvFilter) -> Result<(), String> + Send + Sync>;

static RELOAD: OnceLock<ReloadFn> = OnceLock::new();

pub fn filter_for(level: &str) -> EnvFilter {
    EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"))
}

fn flatten_reload(result: Result<(), tracing_subscriber::reload::Error>) -> Result<(), String> {
    match result {
        Ok(()) => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

fn dispatch(handle: Option<&ReloadFn>, level: &str) -> Result<(), String> {
    let filter = EnvFilter::try_new(level).map_err(|err| err.to_string())?;
    match handle {
        Some(reload) => reload(filter),
        None => Ok(()),
    }
}

pub fn install(level: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| filter_for(level));
    let builder = tracing_subscriber::fmt()
        .with_timer(LocalTimer)
        .with_env_filter(filter)
        .with_ansi(false)
        .with_filter_reloading();
    let handle = builder.reload_handle();
    builder.try_init().ok();
    let _ = RELOAD.set(Box::new(move |filter| {
        flatten_reload(handle.reload(filter))
    }));
}

pub fn reload(level: &str) -> Result<(), String> {
    dispatch(RELOAD.get(), level)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_falls_back_and_reload_reports_both_outcomes() {
        assert!(filter_for("debug").to_string().contains("debug"));
        assert!(filter_for("!!!").to_string().contains("info"));
        assert!(dispatch(None, "info").is_ok());
        assert!(dispatch(None, "!!!").is_err());
        let ok: ReloadFn = Box::new(|_| Ok(()));
        assert!(dispatch(Some(&ok), "debug").is_ok());
        let fail: ReloadFn = Box::new(|_| Err("down".into()));
        assert_eq!(dispatch(Some(&fail), "info").unwrap_err(), "down");
        assert!(flatten_reload(Ok(())).is_ok());
        let builder = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new("info"))
            .with_ansi(false)
            .with_filter_reloading();
        let handle = builder.reload_handle();
        drop(builder);
        let err = flatten_reload(handle.reload(EnvFilter::new("debug"))).unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn install_stores_a_handle_that_reload_can_call() {
        install("info");
        let _ = reload("debug");
        install("info");
        let _ = reload("!!!");
    }

    #[test]
    fn offset_and_civil_stamp_use_a_numeric_zone() {
        assert_eq!(format_offset(-18_000), "-05:00");
        assert_eq!(format_offset(0), "+00:00");
        assert_eq!(format_offset(19_800), "+05:30");
        assert_eq!(offset_seconds(i64::MAX), 0);
        assert_eq!(offset_seconds(-18_000), -18_000);

        let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
        tm.tm_year = 126;
        tm.tm_mon = 10;
        tm.tm_mday = 17;
        tm.tm_hour = 7;
        tm.tm_min = 21;
        tm.tm_sec = 1;
        tm.tm_gmtoff = -18_000;
        assert_eq!(
            format_local_from_tm(&tm, 123_456),
            "2026-11-17T07:21:01.123456-05:00"
        );

        let before = SystemTime::UNIX_EPOCH
            .checked_sub(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(micros_since_epoch(before), (0, 0));
        let (secs, micros) = micros_since_epoch(SystemTime::UNIX_EPOCH);
        assert_eq!((secs, micros), (0, 0));
        assert_eq!(secs_from_elapsed(0), 0);
        assert_eq!(secs_from_elapsed(u64::MAX), i64::MAX);
    }

    #[test]
    fn live_stamp_is_local_and_has_no_zulu_marker() {
        let stamp = format_local_stamp();
        assert_eq!(stamp.len(), 32, "{stamp}");
        assert!(!stamp.contains('Z'), "{stamp}");
        assert_eq!(&stamp[4..5], "-");
        assert_eq!(&stamp[10..11], "T");
        assert_eq!(&stamp[19..20], ".");
        let sign = stamp.as_bytes()[26];
        assert!(sign == b'+' || sign == b'-', "{stamp}");
        let mut buf = String::new();
        let mut writer = Writer::new(&mut buf);
        LocalTimer.format_time(&mut writer).unwrap();
        assert_eq!(buf.len(), 32);
        assert!(!buf.contains('Z'));
    }

    #[test]
    fn unrepresentable_unix_time_falls_back_without_z() {
        assert_eq!(
            format_from_civil(None, 7),
            "1970-01-01T00:00:00.000007+00:00"
        );
        let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
        tm.tm_year = 70;
        tm.tm_mday = 1;
        let rendered = format_from_civil(Some(tm), 0);
        assert!(rendered.starts_with("1970-01-01T00:00:00.000000"));
        assert!(!rendered.contains('Z'));

        for secs in [i64::MAX, i64::MIN] {
            let stamp = format_unix_local(secs, 1);
            assert!(!stamp.contains('Z'), "{stamp}");
            assert_eq!(stamp.len(), 32, "{stamp}");
        }
        assert!(!format_unix_local(0, 0).contains('Z'));
    }
}
