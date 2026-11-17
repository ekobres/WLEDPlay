//! Operator log files written by the container entrypoint.
//!
//! `wledplay.log`, `shairport.log`, and `nqptp.log` are each process.
//! `console.log` is the combined container output. Shairport's stdout stays
//! the PCM FIFO; only its stderr is in `shairport.log`.

use serde::Serialize;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_READ: u64 = 8 * 1024 * 1024;
const MAX_LIMIT: usize = 500;

#[derive(Debug, Clone)]
pub struct LogRequest {
    pub source: String,
    pub tail: bool,
    pub offset: Option<u64>,
    pub before: Option<u64>,
    pub query: String,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LogLine {
    pub offset: u64,
    pub text: String,
    pub level: String,
    pub time: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LogPage {
    pub source: String,
    pub file: String,
    pub size: u64,
    pub offset: u64,
    pub next_offset: u64,
    pub latest_time: String,
    pub missing: bool,
    pub reset: bool,
    pub at_start: bool,
    pub lines: Vec<LogLine>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LogReadError {
    BadSource,
    Io(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum LogFileError {
    BadSource,
    Missing,
    Read(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawLine {
    offset: u64,
    end: u64,
    text: String,
}

pub fn file_name(source: &str) -> Option<&'static str> {
    match source {
        "wledplay" => Some("wledplay.log"),
        "shairport" => Some("shairport.log"),
        "nqptp" => Some("nqptp.log"),
        "console" => Some("console.log"),
        _ => None,
    }
}

pub fn normalize_level(raw: &str) -> Result<&'static str, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "error" => Ok("error"),
        "warn" | "warning" => Ok("warn"),
        "info" => Ok("info"),
        "debug" => Ok("debug"),
        "trace" => Ok("trace"),
        _ => Err("log level must be error, warn, info, debug, or trace".into()),
    }
}

pub fn log_dir_from(value: Option<&str>) -> PathBuf {
    match value.map(str::trim).filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from("/var/log/wledplay"),
    }
}

pub fn log_dir() -> PathBuf {
    log_dir_from(std::env::var("WLEDPLAY_LOG_DIR").ok().as_deref())
}

pub fn line_level(text: &str) -> &'static str {
    let lower = text.to_ascii_lowercase();
    if has_level(&lower, "fatal") || has_level(&lower, "critical") || has_level(&lower, "error") {
        return "error";
    }
    if has_level(&lower, "warning") || has_level(&lower, "warn") {
        return "warn";
    }
    if has_level(&lower, "info") {
        return "info";
    }
    if has_level(&lower, "debug") {
        return "debug";
    }
    if has_level(&lower, "trace") {
        return "trace";
    }
    ""
}

fn starts_level(text: &str, word: &str) -> bool {
    let Some(after) = text.strip_prefix(word) else {
        return false;
    };
    after.is_empty() || after.starts_with([' ', ':'])
}

fn has_level(lower: &str, word: &str) -> bool {
    if starts_level(lower, word) {
        return true;
    }
    let needle = format!(" {word}");
    let mut rest = lower;
    while let Some(at) = rest.find(&needle) {
        let after = &rest[at + needle.len()..];
        if after.is_empty() || after.starts_with([' ', ':']) {
            return true;
        }
        rest = &rest[at + 1..];
    }
    false
}

pub fn line_time(text: &str) -> String {
    let bytes = text.as_bytes();
    if bytes.len() >= 19
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
        && (bytes[10] == b'T' || bytes[10] == b' ')
        && bytes[11..13].iter().all(u8::is_ascii_digit)
        && bytes[13] == b':'
        && bytes[14..16].iter().all(u8::is_ascii_digit)
        && bytes[16] == b':'
        && bytes[17..19].iter().all(u8::is_ascii_digit)
    {
        let mut stamp = text[..19].to_string();
        stamp.replace_range(10..11, " ");
        return stamp;
    }
    String::new()
}

pub fn format_unix(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let tod = (secs % 86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    let hour = tod / 3600;
    let minute = (tod % 3600) / 60;
    let second = tod % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let mut year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        year += 1;
    }
    (year as i32, month as u32, day)
}

fn format_system(time: SystemTime) -> String {
    match time.duration_since(UNIX_EPOCH) {
        Ok(elapsed) => format_unix(elapsed.as_secs()),
        Err(_) => String::new(),
    }
}

fn io_err(err: impl std::fmt::Display) -> String {
    err.to_string()
}

fn align_window(start: u64, mut buf: Vec<u8>) -> (u64, Vec<u8>, bool) {
    let ends_nl = buf.last().copied() == Some(b'\n');
    if start > 0 {
        if let Some(index) = buf.iter().position(|byte| *byte == b'\n') {
            let drop_n = index + 1;
            buf.drain(..drop_n);
            return (start + drop_n as u64, buf, ends_nl);
        }
    }
    (start, buf, ends_nl)
}

fn read_tail_bytes(path: &Path, size: u64, max_read: u64) -> Result<(u64, Vec<u8>, bool), String> {
    if size <= max_read {
        let buf = std::fs::read(path).map_err(io_err)?;
        let ends_nl = buf.last().copied() == Some(b'\n');
        return Ok((0, buf, ends_nl));
    }
    let start = size - max_read;
    let mut file = File::open(path).map_err(io_err)?;
    file.seek(SeekFrom::Start(start)).map_err(io_err)?;
    let mut buf = vec![0u8; max_read as usize];
    let n = file.read(&mut buf).map_err(io_err)?;
    buf.truncate(n);
    let (start, buf, ends_nl) = align_window(start, buf);
    Ok((start, buf, ends_nl))
}

fn split_lines(base: u64, buf: &[u8]) -> Vec<RawLine> {
    let mut lines = Vec::new();
    let mut index = 0usize;
    while index < buf.len() {
        let start = index;
        while index < buf.len() && buf[index] != b'\n' {
            index += 1;
        }
        let text_end = index;
        if index < buf.len() {
            index += 1;
        }
        let mut text = String::from_utf8_lossy(&buf[start..text_end]).into_owned();
        if text.ends_with('\r') {
            text.pop();
        }
        if text.is_empty() {
            continue;
        }
        lines.push(RawLine {
            offset: base + start as u64,
            end: base + index as u64,
            text,
        });
    }
    lines
}

fn clamp_limit(limit: usize) -> usize {
    limit.clamp(1, MAX_LIMIT)
}

fn latest_stamp(lines: &[RawLine], mtime: &str) -> String {
    for line in lines.iter().rev() {
        let stamp = line_time(&line.text);
        if !stamp.is_empty() {
            return stamp;
        }
    }
    mtime.to_string()
}

fn file_mtime(path: &Path) -> String {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map(format_system)
        .unwrap_or_default()
}

fn next_cursor(lines: &[&RawLine], size: u64, ends_nl: bool) -> u64 {
    match lines.last() {
        None => size,
        Some(last) if !ends_nl && last.end == size => last.offset,
        Some(last) => last.end,
    }
}

struct Chosen<'a> {
    lines: Vec<&'a RawLine>,
    at_start: bool,
    reset: bool,
}

fn select_lines<'a>(all: &'a [RawLine], req: &LogRequest, size: u64, limit: usize) -> Chosen<'a> {
    let query = req.query.trim();
    if !query.is_empty() {
        let needle = query.to_ascii_lowercase();
        let matched: Vec<&RawLine> = all
            .iter()
            .filter(|line| line.text.to_ascii_lowercase().contains(&needle))
            .collect();
        let start = matched.len().saturating_sub(limit);
        return Chosen {
            at_start: start == 0,
            reset: false,
            lines: matched[start..].to_vec(),
        };
    }
    if let Some(before) = req.before {
        let eligible: Vec<&RawLine> = all.iter().filter(|line| line.end <= before).collect();
        let start = eligible.len().saturating_sub(limit);
        return Chosen {
            at_start: start == 0,
            reset: false,
            lines: eligible[start..].to_vec(),
        };
    }
    if let Some(offset) = req.offset.filter(|_| !req.tail) {
        if offset > size {
            return Chosen {
                lines: Vec::new(),
                at_start: true,
                reset: true,
            };
        }
        let eligible: Vec<&RawLine> = all.iter().filter(|line| line.offset >= offset).collect();
        let end = eligible.len().min(limit);
        let at_start = match (all.first(), eligible.first()) {
            (Some(first), Some(picked)) => first.offset == picked.offset,
            (None, _) => true,
            (Some(_), None) => false,
        };
        return Chosen {
            lines: eligible[..end].to_vec(),
            at_start,
            reset: false,
        };
    }
    let start = all.len().saturating_sub(limit);
    Chosen {
        at_start: start == 0,
        reset: false,
        lines: all[start..].iter().collect(),
    }
}

pub fn read_logs(dir: &Path, req: &LogRequest) -> Result<LogPage, LogReadError> {
    read_logs_max(dir, req, MAX_READ)
}

fn read_logs_max(dir: &Path, req: &LogRequest, max_read: u64) -> Result<LogPage, LogReadError> {
    let Some(name) = file_name(&req.source) else {
        return Err(LogReadError::BadSource);
    };
    let path = dir.join(name);
    let meta = match std::fs::metadata(&path) {
        Ok(meta) => meta,
        Err(_) => {
            return Ok(empty_page(&req.source, name, true));
        }
    };
    let size = meta.len();
    if size == 0 {
        let mut page = empty_page(&req.source, name, false);
        page.latest_time = file_mtime(&path);
        return Ok(page);
    }
    let (base, buf, ends_nl) = read_tail_bytes(&path, size, max_read).map_err(LogReadError::Io)?;
    let raw = split_lines(base, &buf);
    let limit = clamp_limit(req.limit);
    let chosen = select_lines(&raw, req, size, limit);
    let lines = chosen
        .lines
        .iter()
        .map(|line| LogLine {
            offset: line.offset,
            level: line_level(&line.text).to_string(),
            time: line_time(&line.text),
            text: line.text.clone(),
        })
        .collect();
    let offset = chosen.lines.first().map(|line| line.offset).unwrap_or(0);
    Ok(LogPage {
        source: req.source.clone(),
        file: name.to_string(),
        size,
        offset,
        next_offset: next_cursor(&chosen.lines, size, ends_nl),
        latest_time: latest_stamp(&raw, &file_mtime(&path)),
        missing: false,
        reset: chosen.reset,
        at_start: chosen.at_start,
        lines,
    })
}

fn empty_page(source: &str, file: &str, missing: bool) -> LogPage {
    LogPage {
        source: source.to_string(),
        file: file.to_string(),
        size: 0,
        offset: 0,
        next_offset: 0,
        latest_time: String::new(),
        missing,
        reset: false,
        at_start: true,
        lines: Vec::new(),
    }
}

pub fn read_file(dir: &Path, source: &str) -> Result<(String, Vec<u8>), LogFileError> {
    read_file_max(dir, source, MAX_READ)
}

fn read_file_max(
    dir: &Path,
    source: &str,
    max_read: u64,
) -> Result<(String, Vec<u8>), LogFileError> {
    let name = file_name(source).ok_or(LogFileError::BadSource)?;
    let path = dir.join(name);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(LogFileError::Missing);
        }
        Err(err) => return Err(LogFileError::Read(err.to_string())),
    };
    if bytes.len() as u64 > max_read {
        let start = bytes.len() - max_read as usize;
        return Ok((name.to_string(), bytes[start..].to_vec()));
    }
    Ok((name.to_string(), bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn req(source: &str) -> LogRequest {
        LogRequest {
            source: source.into(),
            tail: true,
            offset: None,
            before: None,
            query: String::new(),
            limit: 200,
        }
    }

    #[test]
    fn file_name_and_level_and_dir() {
        assert_eq!(file_name("wledplay"), Some("wledplay.log"));
        assert_eq!(file_name("shairport"), Some("shairport.log"));
        assert_eq!(file_name("nqptp"), Some("nqptp.log"));
        assert_eq!(file_name("console"), Some("console.log"));
        assert_eq!(file_name("other"), None);
        assert_eq!(normalize_level(" Debug "), Ok("debug"));
        assert_eq!(normalize_level("warning"), Ok("warn"));
        assert_eq!(normalize_level("error"), Ok("error"));
        assert_eq!(normalize_level("info"), Ok("info"));
        assert_eq!(normalize_level("trace"), Ok("trace"));
        assert!(normalize_level("verbose").is_err());
        assert_eq!(
            log_dir_from(Some(" /tmp/logs ")),
            PathBuf::from("/tmp/logs")
        );
        assert_eq!(log_dir_from(Some("  ")), PathBuf::from("/var/log/wledplay"));
        assert_eq!(log_dir_from(None), PathBuf::from("/var/log/wledplay"));
        let _ = log_dir();
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(200), 200);
        assert_eq!(clamp_limit(900), 500);
    }

    #[test]
    fn line_level_and_time() {
        assert_eq!(line_level("error: disk"), "error");
        assert_eq!(line_level("fatal: down"), "error");
        assert_eq!(line_level("critical: down"), "error");
        assert_eq!(line_level("2026-11-17T15:32:01Z  INFO ready"), "info");
        assert_eq!(line_level("warning: hot"), "warn");
        assert_eq!(line_level(" warn: hot"), "warn");
        assert_eq!(line_level("debug: sample"), "debug");
        assert_eq!(line_level("trace: hop"), "trace");
        assert_eq!(line_level("errors are not a level"), "");
        assert_eq!(line_level("traceback: no"), "");
        assert_eq!(line_level("information"), "");
        assert_eq!(line_level("error"), "error");
        assert_eq!(line_level("see error"), "error");
        assert_eq!(line_level(" infoX info "), "info");
        let early = civil_from_days(-719_469);
        assert!((1..=12).contains(&early.1));
        assert!(early.2 >= 1);
        assert_eq!(
            line_time("2026-11-17T15:32:01.123Z INFO"),
            "2026-11-17 15:32:01"
        );
        assert_eq!(
            line_time("2026-11-17 15:32:01 hello"),
            "2026-11-17 15:32:01"
        );
        assert_eq!(line_time("2026-11-17X15:32:01"), "");
        assert_eq!(line_time("short"), "");
        assert_eq!(format_unix(0), "1970-01-01 00:00:00");
        assert_eq!(format_unix(1582934400), "2020-02-29 00:00:00");
        assert_eq!(format_unix(1700000000), "2023-11-14 22:13:20");
        assert_eq!(format_system(UNIX_EPOCH), "1970-01-01 00:00:00");
        let before = UNIX_EPOCH.checked_sub(Duration::from_secs(1)).unwrap();
        assert_eq!(format_system(before), "");
        assert_eq!(file_mtime(Path::new("/no/such/wledplay-log")), "");
    }

    #[test]
    fn align_and_split_and_cursor() {
        let (start, buf, ends) = align_window(0, b"keep\n".to_vec());
        assert_eq!(start, 0);
        assert_eq!(buf, b"keep\n");
        assert!(ends);
        let (start, buf, ends) = align_window(5, b"BBBB\nCCCC\n".to_vec());
        assert_eq!(start, 10);
        assert_eq!(buf, b"CCCC\n");
        assert!(ends);
        let (start, buf, ends) = align_window(4, b"ABCDEFGH".to_vec());
        assert_eq!(start, 4);
        assert_eq!(buf, b"ABCDEFGH");
        assert!(!ends);
        let lines = split_lines(0, b"error: a\r\n\ninfo: b\npartial");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].text, "error: a");
        assert_eq!(lines[1].text, "info: b");
        assert_eq!(lines[2].text, "partial");
        assert_eq!(lines[2].end, lines[2].offset + "partial".len() as u64);
        let broken = split_lines(0, &[0xff, b'A']);
        assert!(broken[0].text.contains('\u{fffd}'));
        assert_eq!(next_cursor(&[], 12, true), 12);
        let last = RawLine {
            offset: 3,
            end: 8,
            text: "abc".into(),
        };
        assert_eq!(next_cursor(&[&last], 8, false), 3);
        assert_eq!(next_cursor(&[&last], 8, true), 8);
    }

    #[test]
    fn reads_tail_follow_search_and_download() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wledplay.log");
        let body = "\
2026-11-17T10:00:00Z INFO one
2026-11-17T10:00:01Z ERROR two
2026-11-17T10:00:02Z WARN three
plain
";
        std::fs::write(&path, body).unwrap();
        let page = read_logs(dir.path(), &req("wledplay")).unwrap();
        assert_eq!(page.lines.len(), 4);
        assert_eq!(page.lines[1].level, "error");
        assert_eq!(page.lines[2].level, "warn");
        assert_eq!(page.lines[0].time, "2026-11-17 10:00:00");
        assert_eq!(page.latest_time, "2026-11-17 10:00:02");
        assert!(page.at_start);
        assert!(!page.missing);
        assert_eq!(page.next_offset, page.size);

        let mut follow = req("wledplay");
        follow.tail = false;
        follow.offset = Some(page.lines[2].offset);
        let page = read_logs(dir.path(), &follow).unwrap();
        assert_eq!(page.lines.len(), 2);
        assert_eq!(page.lines[0].text, "2026-11-17T10:00:02Z WARN three");
        assert!(!page.reset);

        follow.offset = Some(page.size + 5);
        let page = read_logs(dir.path(), &follow).unwrap();
        assert!(page.reset);
        assert!(page.lines.is_empty());

        follow.offset = Some(std::fs::metadata(&path).unwrap().len());
        let page = read_logs(dir.path(), &follow).unwrap();
        assert!(!page.reset);
        assert!(page.lines.is_empty());

        let mut search = req("wledplay");
        search.query = "  error  ".into();
        search.limit = 0;
        let page = read_logs(dir.path(), &search).unwrap();
        assert_eq!(page.lines.len(), 1);
        assert!(page.lines[0].text.contains("ERROR"));

        let mut before = req("wledplay");
        before.before = Some(0);
        before.tail = false;
        let page = read_logs(dir.path(), &before).unwrap();
        assert!(page.lines.is_empty());
        assert!(page.at_start);

        let full = read_logs(dir.path(), &req("wledplay")).unwrap();
        before.before = Some(full.lines[2].offset);
        before.limit = 1;
        let page = read_logs(dir.path(), &before).unwrap();
        assert_eq!(page.lines.len(), 1);
        assert!(page.lines[0].text.contains("ERROR"));

        let mut many = String::new();
        for i in 0..6 {
            many.push_str(&format!("info: {i}\n"));
        }
        std::fs::write(&path, &many).unwrap();
        let mut limited = req("wledplay");
        limited.limit = 2;
        let page = read_logs(dir.path(), &limited).unwrap();
        assert_eq!(page.lines.len(), 2);
        assert!(page.lines[0].text.contains('4'));
        assert!(!page.at_start);

        std::fs::write(&path, "\n\n").unwrap();
        let mut blanks = req("wledplay");
        blanks.tail = false;
        blanks.offset = Some(0);
        let page = read_logs(dir.path(), &blanks).unwrap();
        assert!(page.lines.is_empty());
        assert!(page.at_start);

        std::fs::write(&path, "hello").unwrap();
        let page = read_logs(dir.path(), &req("wledplay")).unwrap();
        assert_eq!(page.lines[0].text, "hello");
        assert_eq!(page.next_offset, 0);
        assert!(!page.latest_time.is_empty());

        std::fs::write(&path, "").unwrap();
        let page = read_logs(dir.path(), &req("wledplay")).unwrap();
        assert!(!page.missing);
        assert!(page.lines.is_empty());

        let missing = read_logs(dir.path(), &req("nqptp")).unwrap();
        assert!(missing.missing);

        let err = read_logs(dir.path(), &req("nope")).unwrap_err();
        assert_eq!(err, LogReadError::BadSource);

        let (name, bytes) = read_file(dir.path(), "wledplay").unwrap();
        assert_eq!(name, "wledplay.log");
        assert!(bytes.is_empty());
        assert_eq!(
            read_file(dir.path(), "nqptp").unwrap_err(),
            LogFileError::Missing
        );
        assert_eq!(
            read_file(dir.path(), "side").unwrap_err(),
            LogFileError::BadSource
        );

        let big = dir.path().join("console.log");
        std::fs::write(&big, b"abcdefghij").unwrap();
        let (name, bytes) = read_file_max(dir.path(), "console", 4).unwrap();
        assert_eq!(name, "console.log");
        assert_eq!(bytes, b"ghij");

        let window = dir.path().join("shairport.log");
        std::fs::write(&window, b"AAAA\nBBBB\nCCCC\n").unwrap();
        let page = read_logs_max(dir.path(), &req("shairport"), 10).unwrap();
        assert_eq!(page.lines.len(), 1);
        assert_eq!(page.lines[0].text, "CCCC");

        std::fs::write(&window, vec![b'X'; 20]).unwrap();
        let page = read_logs_max(dir.path(), &req("shairport"), 8).unwrap();
        assert_eq!(page.lines[0].text, "XXXXXXXX");

        let as_dir = dir.path().join("nqptp.log");
        std::fs::create_dir(&as_dir).unwrap();
        assert!(read_logs(dir.path(), &req("nqptp")).is_err());
        assert!(matches!(
            read_file(dir.path(), "nqptp"),
            Err(LogFileError::Read(_))
        ));
        let _ = read_tail_bytes(&as_dir, 10_000, 8);
    }
}
