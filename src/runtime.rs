//! PCM / Audio Sync runtime helpers shared by the binary event loop.

use crate::audio::Analyzer;
use crate::config::{AgcMode, Config};
use crate::web::{AudioStatus, SharedStatus};
use crate::wled::{AudioSyncPacket, PacketTarget};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;

/// Silence packets sent after stream end before stopping UDP (SRS §34).
pub const IDLE_GRACE_PACKETS: u64 = 10;
/// Shairport gaps for up to ~1s at session start / resync.
pub const PCM_IDLE_START: Duration = Duration::from_millis(2000);
/// After playback is established, treat a gap as pause (stop lights sooner).
pub const PCM_IDLE_PLAY: Duration = Duration::from_millis(400);
/// Keep at most this much unread stdin PCM when not realtime-pacing (AirPlay).
pub const STDIN_KEEP_MS: u64 = 80;
/// Poll interval while a persistent non-FIFO stdin has no writer (AirPlay session gap).
/// Must not busy-spin: a pegged core lets the pipe fill and wrecks delay calibration.
/// FIFO paths open RDWR so `read` blocks in the kernel instead.
pub const PCM_FIFO_IDLE_POLL: Duration = Duration::from_millis(20);
/// `/status` refresh cadence used to schedule the event loop.
pub const STATUS_INTERVAL: Duration = Duration::from_secs(1);
/// Cap idle sleeps so `POST /restart` is noticed without a 500 Hz poll.
pub const EVENT_LOOP_WAKE_CAP: Duration = Duration::from_millis(50);
/// Count an underrun only after PCM has been silent this long.
pub const PCM_UNDERRUN_AFTER: Duration = Duration::from_millis(80);
/// Max queued departures. Each entry is one send time, shared by every
/// controller with that offset. 8192 covers a 5s hold at 50 Hz with many
/// different offsets. Overflow drops the newest entry and keeps the packets
/// that are about to be sent.
pub const DELAY_QUEUE_MAX: usize = 8192;

pub struct PcmRuntime {
    pub analyzer: Analyzer,
    pub stream_active: bool,
    pub pcm_frames: u64,
    pub last_pcm: Option<Instant>,
    pub stream_since: Option<Instant>,
    pub stdin_closed: bool,
    pub connections: u64,
    pub pcm_underruns: u64,
    pub analysis_overruns: u64,
}

impl PcmRuntime {
    pub fn new(agc: AgcMode) -> Self {
        Self {
            analyzer: Analyzer::new(agc),
            stream_active: false,
            pcm_frames: 0,
            last_pcm: None,
            stream_since: None,
            stdin_closed: false,
            connections: 0,
            pcm_underruns: 0,
            analysis_overruns: 0,
        }
    }
}

pub fn pcm_idle_timeout(stream_since: Option<Instant>, now: Instant) -> Duration {
    match stream_since {
        Some(t) if now.saturating_duration_since(t) >= PCM_IDLE_START => PCM_IDLE_PLAY,
        _ => PCM_IDLE_START,
    }
}

/// Count one underrun per contiguous PCM gap, not once per send tick.
pub fn note_pcm_gap(open: &mut bool, starved: bool) -> bool {
    if starved {
        if !*open {
            *open = true;
            return true;
        }
    } else {
        *open = false;
    }
    false
}

pub type DelayItem = (Instant, AudioSyncPacket, Vec<PacketTarget>);

pub fn enqueue_packet(
    queue: &mut VecDeque<DelayItem>,
    packet: AudioSyncPacket,
    delay_ms: u64,
    now: Instant,
) {
    enqueue_packet_at(queue, packet, now + Duration::from_millis(delay_ms));
}

pub fn enqueue_packet_at(
    queue: &mut VecDeque<DelayItem>,
    packet: AudioSyncPacket,
    due: Instant,
) {
    insert_due(queue, (due, packet, vec![PacketTarget::Fanout]));
}

pub fn enqueue_departure(
    queue: &mut VecDeque<DelayItem>,
    packet: AudioSyncPacket,
    base: Instant,
    multicast: bool,
    unicast: &[(SocketAddr, i32)],
) {
    let mut groups: Vec<(i32, Vec<PacketTarget>)> = Vec::new();
    if multicast {
        groups.push((0, vec![PacketTarget::Multicast]));
    }
    for (addr, offset_ms) in unicast {
        if let Some((_, targets)) = groups.iter_mut().find(|(offset, _)| *offset == *offset_ms) {
            targets.push(PacketTarget::Unicast(*addr));
        } else {
            groups.push((*offset_ms, vec![PacketTarget::Unicast(*addr)]));
        }
    }
    for (offset_ms, targets) in groups {
        insert_due(
            queue,
            (shift_due_ms(base, offset_ms), packet.clone(), targets),
        );
    }
}

fn shift_due_ms(base: Instant, offset_ms: i32) -> Instant {
    if offset_ms == 0 {
        return base;
    }
    let step = Duration::from_millis(u64::from(offset_ms.unsigned_abs()));
    if offset_ms > 0 {
        base.checked_add(step).unwrap_or(base)
    } else {
        base.checked_sub(step).unwrap_or(base)
    }
}

fn insert_due(queue: &mut VecDeque<DelayItem>, item: DelayItem) {
    let pos = queue
        .iter()
        .position(|(due, _, _)| *due > item.0)
        .unwrap_or(queue.len());
    queue.insert(pos, item);
    while queue.len() > DELAY_QUEUE_MAX {
        queue.pop_back();
    }
}

/// Send time for the next window. Windows from one wake are spaced by
/// `interval`, and the following wake continues after the last of them.
/// A schedule more than one interval late, or more than four ahead, starts
/// again at `now + delay` instead of flushing every overdue slot at once.
pub fn next_departure(
    scheduled: Option<Instant>,
    now: Instant,
    delay: Duration,
    interval: Duration,
) -> Instant {
    let anchor = now + delay;
    let Some(next) = scheduled else {
        return anchor;
    };
    if interval.is_zero() {
        return anchor;
    }
    if anchor.saturating_duration_since(next) > interval {
        return anchor;
    }
    if next.saturating_duration_since(anchor) > interval.saturating_mul(4) {
        return anchor;
    }
    next
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcmSessionGap {
    Exit,
    Reopen,
    Wait(Duration),
}

/// After PCM EOF, either exit, reopen a FIFO (kernel-block on `open`/`read`), or poll stdin.
pub fn pcm_session_gap(persistent: bool, reopenable: bool) -> PcmSessionGap {
    if !persistent {
        PcmSessionGap::Exit
    } else if reopenable {
        PcmSessionGap::Reopen
    } else {
        PcmSessionGap::Wait(PCM_FIFO_IDLE_POLL)
    }
}

/// Drive a file/FIFO PCM source: reopen while `read` returns true (EOF on a FIFO).
pub fn pcm_file_pump(
    open: &mut dyn FnMut() -> std::io::Result<()>,
    read: &mut dyn FnMut() -> bool,
    on_open_err: &mut dyn FnMut(std::io::Error),
) {
    loop {
        match open() {
            Ok(()) => {
                if !read() {
                    return;
                }
            }
            Err(e) => {
                on_open_err(e);
                return;
            }
        }
    }
}

pub fn path_is_fifo(path: &Path) -> bool {
    #[cfg(unix)]
    {
        std::fs::metadata(path)
            .map(|m| m.file_type().is_fifo())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

/// Open a PCM file, or a FIFO as RDWR so an empty pipe blocks in the kernel instead of EOF.
pub fn open_pcm_input(path: &Path) -> std::io::Result<File> {
    if path_is_fifo(path) {
        OpenOptions::new().read(true).write(true).open(path)
    } else {
        File::open(path)
    }
}

pub fn stdin_keep_bytes(bytes_per_second: u64) -> u64 {
    (bytes_per_second * STDIN_KEEP_MS / 1000).max(4096)
}

pub fn discard_reader_backlog(
    reader: &mut dyn Read,
    available: u64,
    keep_bytes: u64,
) -> Option<usize> {
    if available <= keep_bytes {
        return None;
    }
    let mut left = (available - keep_bytes) as usize;
    let skipped = left;
    let mut buf = [0u8; 8192];
    while left > 0 {
        let chunk = left.min(buf.len());
        let n = match reader.read(&mut buf[..chunk]) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        left = left.saturating_sub(n);
    }
    Some(skipped.saturating_sub(left))
}

pub fn ioctl_available_bytes(ret: i32, nbytes: i32) -> u64 {
    if ret == 0 && nbytes > 0 {
        nbytes as u64
    } else {
        0
    }
}

pub fn stdin_available_bytes() -> u64 {
    #[cfg(unix)]
    {
        let mut nbytes: libc::c_int = 0;
        let ret = unsafe { libc::ioctl(libc::STDIN_FILENO, libc::FIONREAD, &mut nbytes) };
        ioctl_available_bytes(ret, nbytes)
    }
    #[cfg(not(unix))]
    {
        0
    }
}

pub fn step_send_deadline(last_send: Instant, interval: Duration, now: Instant) -> Instant {
    let next = last_send + interval;
    if now.saturating_duration_since(next) >= interval {
        now
    } else {
        next
    }
}

pub fn pop_due_packet(
    queue: &mut VecDeque<DelayItem>,
    now: Instant,
) -> Option<(AudioSyncPacket, Vec<PacketTarget>)> {
    if queue.front().map(|(t, _, _)| now >= *t).unwrap_or(false) {
        let (_, packet, target) = queue.pop_front().unwrap();
        Some((packet, target))
    } else {
        None
    }
}

pub fn next_due_at(queue: &VecDeque<DelayItem>) -> Option<Instant> {
    queue.front().map(|(t, _, _)| *t)
}

/// Sleep until the next send tick, delayed packet, or status refresh (capped for restart).
pub fn event_loop_wait(
    now: Instant,
    last_send: Instant,
    last_status: Instant,
    update_interval: Duration,
    next_due: Option<Instant>,
) -> Duration {
    let next_send = if last_send > now {
        last_send
    } else {
        last_send + update_interval
    };
    let next_status = last_status + STATUS_INTERVAL;
    let mut wake = next_send.min(next_status);
    if let Some(due) = next_due {
        if due < wake {
            wake = due;
        }
    }
    let cap = now + EVENT_LOOP_WAKE_CAP;
    if wake > cap {
        wake = cap;
    }
    wake.saturating_duration_since(now)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopAction {
    Continue,
    Exit,
}

#[derive(Default)]
pub struct SendLoop {
    pub udp_active: bool,
    pub idle_packets_sent: u64,
    pub announced_end: bool,
    pub underrun_open: bool,
    pub delay_q: VecDeque<DelayItem>,
    /// Next free send time on the monotonic `Instant` clock. Only the event
    /// loop reserves slots, and it advances this while it holds the PCM lock.
    pub next_due: Option<Instant>,
}

impl SendLoop {
    pub fn on_update(
        &mut self,
        rt: &mut PcmRuntime,
        status: &SharedStatus,
        cfg: &Config,
        finite_input: bool,
        now: Instant,
    ) -> LoopAction {
        rt.analyzer.set_agc(cfg.agc_mode());
        if rt.stream_active {
            if let Some(t) = rt.last_pcm {
                if now.saturating_duration_since(t) >= pcm_idle_timeout(rt.stream_since, now) {
                    tracing::info!("PCM stream ended");
                    rt.stream_active = false;
                    rt.stream_since = None;
                    rt.analyzer.set_stream_active(false);
                    status.lock().unwrap().airplay.connected = false;
                    self.idle_packets_sent = 0;
                    self.announced_end = true;
                    self.underrun_open = false;
                    self.next_due = None;
                }
            }
        }

        let delay_ms = cfg.audio.sync_offset_ms.max(0) as u64;
        if rt.stream_active {
            self.udp_active = true;
            self.idle_packets_sent = 0;
            self.announced_end = false;
            let starved = rt
                .last_pcm
                .map(|t| now.saturating_duration_since(t) >= PCM_UNDERRUN_AFTER)
                .unwrap_or(false);
            if note_pcm_gap(&mut self.underrun_open, starved) {
                rt.pcm_underruns += 1;
            }
            let hz = cfg.audio.update_hz.max(1);
            let interval_ms = 1000 / u64::from(hz);
            let interval = Duration::from_millis(interval_ms);
            let hop = Analyzer::departure_hop(hz);
            let mut due = next_departure(
                self.next_due,
                now,
                Duration::from_millis(delay_ms),
                interval,
            );
            let mut slot = 0u64;
            while slot < 4 {
                let Some(pkt) = rt.analyzer.take_departure(hop) else {
                    break;
                };
                publish_audio(status, rt, cfg, pkt.sample_peak != 0);
                enqueue_departure(
                    &mut self.delay_q,
                    pkt,
                    due,
                    cfg.wled.multicast_enabled,
                    &crate::config::unicast_delay_targets(cfg),
                );
                due += interval;
                slot += 1;
            }
            if slot > 0 {
                self.next_due = Some(due);
            }
            LoopAction::Continue
        } else if self.udp_active {
            let pkt = rt.analyzer.silence_packet();
            publish_audio(status, rt, cfg, false);
            enqueue_departure(
                &mut self.delay_q,
                pkt,
                now + Duration::from_millis(delay_ms),
                cfg.wled.multicast_enabled,
                &crate::config::unicast_delay_targets(cfg),
            );
            self.idle_packets_sent += 1;
            if self.idle_packets_sent >= IDLE_GRACE_PACKETS {
                self.udp_active = false;
                tracing::info!("Audio Sync transmission stopped (stream idle)");
            }
            LoopAction::Continue
        } else {
            let closed = rt.stdin_closed;
            if finite_input && closed && self.delay_q.is_empty() {
                if !self.announced_end {
                    tracing::info!("Input closed; exiting");
                }
                LoopAction::Exit
            } else {
                LoopAction::Continue
            }
        }
    }
}

pub fn start_ms(start: Instant, now: Instant) -> u64 {
    now.saturating_duration_since(start).as_millis() as u64
}

#[allow(clippy::too_many_arguments)]
pub fn update_status(
    status: &SharedStatus,
    rt: &PcmRuntime,
    destination: String,
    packets_sent: u64,
    udp_errors: u64,
    packets_per_sec: u64,
    cfg: &Config,
    now: Instant,
) {
    let mut s = status.lock().unwrap();
    s.audio.sample_rate = crate::pcm::ANALYSIS_RATE;
    s.audio.channels = 1;
    if rt.stream_active {
        s.audio.rms = rt.analyzer.rms_proxy();
        s.audio.dominant_frequency = rt.analyzer.dominant_frequency();
        s.audio.bands = rt.analyzer.fft_bands();
    } else {
        s.audio.rms = 0.0;
        s.audio.peak = false;
        s.audio.dominant_frequency = 0.0;
        s.audio.bands = [0; 16];
    }
    s.audio.sync_offset_ms = cfg.audio.sync_offset_ms;
    s.audio.update_hz = cfg.audio.update_hz;
    s.audio.agc_gain = rt.analyzer.agc_gain();
    s.audio.agc_on = rt.analyzer.agc_enabled();
    s.wled.packets_per_second = packets_per_sec as f32;
    s.wled.destination = destination;
    s.wled.destinations = cfg.wled.destinations.clone();
    s.wled.multicast_enabled = cfg.wled.multicast_enabled;
    s.counters.pcm_frames = rt.pcm_frames;
    s.counters.fft_frames = rt.analyzer.fft_frames();
    s.counters.packets_sent = packets_sent;
    s.counters.udp_errors = udp_errors;
    s.counters.pcm_underruns = rt.pcm_underruns;
    s.counters.analysis_overruns = rt.analysis_overruns;
    s.counters.airplay_connections = rt.connections;
    s.airplay.receiver_name = cfg.airplay.name.clone();
    s.airplay.connections = rt.connections;
    s.airplay.stream_duration_s = rt
        .stream_since
        .map(|t| now.saturating_duration_since(t).as_secs_f32())
        .unwrap_or(0.0);
    s.metadata.refresh_position(rt.stream_active);
}

pub fn apply_transmitted_packet(audio: &mut AudioStatus, pkt: &AudioSyncPacket) {
    audio.tx_rms = (pkt.sample_smth / 255.0).clamp(0.0, 1.0);
    audio.tx_peak = pkt.sample_peak != 0;
    audio.tx_dominant_frequency = pkt.fft_major_peak;
    audio.tx_bands = pkt.fft_result;
    audio.tx_connected = pkt.sample_smth > 1.0 || pkt.fft_result.iter().any(|&b| b > 0);
}

pub fn record_transmitted(status: &SharedStatus, pkt: &AudioSyncPacket) {
    apply_transmitted_packet(&mut status.lock().unwrap().audio, pkt);
}

pub fn publish_audio(status: &SharedStatus, rt: &PcmRuntime, cfg: &Config, packet_peak: bool) {
    let mut s = status.lock().unwrap();
    s.audio.sample_rate = crate::pcm::ANALYSIS_RATE;
    s.audio.channels = 1;
    if rt.stream_active {
        s.audio.rms = rt.analyzer.rms_proxy();
        s.audio.peak = packet_peak;
        s.audio.dominant_frequency = rt.analyzer.dominant_frequency();
        s.audio.bands = rt.analyzer.fft_bands();
    } else {
        s.audio.rms = 0.0;
        s.audio.peak = false;
        s.audio.dominant_frequency = 0.0;
        s.audio.bands = [0; 16];
    }
    s.audio.sync_offset_ms = cfg.audio.sync_offset_ms;
    s.audio.update_hz = cfg.audio.update_hz;
    s.audio.agc_gain = rt.analyzer.agc_gain();
    s.audio.agc_on = rt.analyzer.agc_enabled();
    s.airplay.connected = rt.stream_active;
}

pub fn mark_stream_connected(rt: &mut PcmRuntime, status: &SharedStatus, now: Instant) {
    if rt.stream_active {
        return;
    }
    tracing::info!("PCM stream connected");
    rt.stream_active = true;
    rt.stream_since = Some(now);
    rt.connections += 1;
    rt.analyzer.set_stream_active(true);
    status.lock().unwrap().airplay.connected = true;
}

pub fn mark_stream_ended(rt: &mut PcmRuntime, status: &SharedStatus, keep_input_open: bool) {
    if rt.stream_active {
        tracing::info!("PCM stream ended");
        rt.stream_active = false;
        rt.stream_since = None;
        rt.analyzer.set_stream_active(false);
        status.lock().unwrap().airplay.connected = false;
    }
    if !keep_input_open {
        rt.stdin_closed = true;
    }
}

pub fn http_listen(http_bind: Option<&crate::netif::InterfaceBind>, fallback: &str) -> String {
    http_bind
        .map(|b| b.ipv4.to_string())
        .unwrap_or_else(|| fallback.to_string())
}

pub fn resolve_optional_nic(
    role: &str,
    name: &str,
) -> Result<Option<crate::netif::InterfaceBind>, String> {
    describe_nic_bind(role, crate::netif::optional_bind(name))
}

pub(crate) fn describe_nic_bind(
    role: &str,
    bind: Result<Option<crate::netif::InterfaceBind>, String>,
) -> Result<Option<crate::netif::InterfaceBind>, String> {
    match bind {
        Ok(Some(b)) => {
            tracing::info!("{role} interface: {} ({})", b.name, b.ipv4);
            Ok(Some(b))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(format!("{role}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::web::{AirplayStatus, RuntimeStatus};
    use crate::wled::AudioSyncPacket;
    use std::io::Cursor;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    fn status() -> SharedStatus {
        Arc::new(Mutex::new(RuntimeStatus {
            airplay: AirplayStatus {
                receiver_name: "t".into(),
                ..Default::default()
            },
            ..Default::default()
        }))
    }

    #[test]
    fn counts_one_underrun_per_gap() {
        let mut open = false;
        assert!(note_pcm_gap(&mut open, true));
        assert!(!note_pcm_gap(&mut open, true));
        assert!(!note_pcm_gap(&mut open, false));
        assert!(note_pcm_gap(&mut open, true));
    }

    #[test]
    fn idle_timeout_shortens_after_startup_window() {
        let now = Instant::now();
        assert_eq!(pcm_idle_timeout(None, now), PCM_IDLE_START);
        assert_eq!(
            pcm_idle_timeout(Some(now - Duration::from_millis(100)), now),
            PCM_IDLE_START
        );
        assert_eq!(
            pcm_idle_timeout(Some(now - Duration::from_secs(3)), now),
            PCM_IDLE_PLAY
        );
    }

    #[test]
    fn pcm_session_gap_exit_wait_or_reopen() {
        assert_eq!(pcm_session_gap(false, false), PcmSessionGap::Exit);
        assert_eq!(pcm_session_gap(false, true), PcmSessionGap::Exit);
        assert_eq!(
            pcm_session_gap(true, false),
            PcmSessionGap::Wait(PCM_FIFO_IDLE_POLL)
        );
        assert_eq!(pcm_session_gap(true, true), PcmSessionGap::Reopen);
    }

    #[test]
    fn pcm_file_pump_reopens_then_stops() {
        let mut opens = 0u8;
        let mut reads = 0u8;
        let mut errs = 0u8;
        pcm_file_pump(
            &mut || {
                opens += 1;
                if opens <= 2 {
                    Ok(())
                } else {
                    Err(std::io::Error::other("gone"))
                }
            },
            &mut || {
                reads += 1;
                true
            },
            &mut |_| {
                errs += 1;
            },
        );
        assert_eq!(opens, 3);
        assert_eq!(reads, 2);
        assert_eq!(errs, 1);

        let mut opens = 0u8;
        pcm_file_pump(
            &mut || {
                opens += 1;
                Err(std::io::Error::other("missing"))
            },
            &mut || true,
            &mut |_| {},
        );
        assert_eq!(opens, 1);
    }

    #[test]
    fn path_is_fifo_false_for_missing_and_regular_files() {
        assert!(!path_is_fifo(Path::new("/no/such/wledplay.pcm")));
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.pcm");
        std::fs::write(&file, [0u8; 4]).unwrap();
        assert!(!path_is_fifo(&file));
        assert!(open_pcm_input(&file).is_ok());
        assert!(open_pcm_input(Path::new("/no/such/wledplay.pcm")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn open_pcm_fifo_uses_rdwr() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("audio.fifo");
        let cstr = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cstr.as_ptr(), 0o644) }, 0);
        assert!(path_is_fifo(&fifo));
        let f = open_pcm_input(&fifo).unwrap();
        drop(f);
    }

    #[test]
    fn stdin_keep_bytes_is_80ms_or_4k() {
        assert_eq!(stdin_keep_bytes(0), 4096);
        assert_eq!(stdin_keep_bytes(192_000), 15_360);
    }

    #[test]
    fn enqueue_drops_the_newest_when_over_cap() {
        let mut q = VecDeque::new();
        let now = Instant::now();
        for _ in 0..DELAY_QUEUE_MAX {
            enqueue_packet(&mut q, AudioSyncPacket::silence(), 0, now);
        }
        enqueue_packet(&mut q, AudioSyncPacket::silence(), 1_000, now);
        assert_eq!(q.len(), DELAY_QUEUE_MAX);
        assert!(q.iter().all(|(due, _, _)| *due == now));
    }

    #[test]
    fn discard_backlog_skips_when_under_keep() {
        let mut cur = Cursor::new(vec![1u8; 16]);
        assert!(discard_reader_backlog(&mut cur, 16, 32).is_none());
    }

    #[test]
    fn discard_backlog_reads_excess() {
        let mut cur = Cursor::new(vec![7u8; 100]);
        let skipped = discard_reader_backlog(&mut cur, 100, 20).unwrap();
        assert_eq!(skipped, 80);
        assert_eq!(cur.position(), 80);
    }

    #[test]
    fn discard_backlog_stops_on_eof() {
        let mut cur = Cursor::new(vec![1u8; 4]);
        let skipped = discard_reader_backlog(&mut cur, 10_000, 0).unwrap();
        assert_eq!(skipped, 4);
    }

    #[test]
    fn discard_backlog_stops_on_read_error() {
        struct Boom;
        impl Read for Boom {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("boom"))
            }
        }
        let skipped = discard_reader_backlog(&mut Boom, 100, 0).unwrap();
        assert_eq!(skipped, 0);
    }

    #[test]
    fn ioctl_available_bytes_branches() {
        assert_eq!(ioctl_available_bytes(0, 12), 12);
        assert_eq!(ioctl_available_bytes(0, 0), 0);
        assert_eq!(ioctl_available_bytes(-1, 12), 0);
        let _ = stdin_available_bytes();
    }

    #[test]
    fn step_send_deadline_resets_when_far_behind() {
        let now = Instant::now();
        let interval = Duration::from_millis(20);
        let last = now - Duration::from_millis(80);
        assert_eq!(step_send_deadline(last, interval, now), now);
        let last = now - Duration::from_millis(10);
        assert_eq!(step_send_deadline(last, interval, now), last + interval);
    }

    #[test]
    fn pop_due_packet_respects_time() {
        let mut q = VecDeque::new();
        let now = Instant::now();
        enqueue_packet(&mut q, AudioSyncPacket::silence(), 5_000, now);
        assert!(pop_due_packet(&mut q, now).is_none());
        assert_eq!(next_due_at(&q), Some(now + Duration::from_millis(5_000)));
        assert!(pop_due_packet(&mut q, now + Duration::from_secs(6)).is_some());
        assert!(q.is_empty());
        assert!(next_due_at(&q).is_none());
        assert!(pop_due_packet(&mut q, now).is_none());
    }

    #[test]
    fn event_loop_wait_uses_soonest_deadline_and_cap() {
        let now = Instant::now();
        let interval = Duration::from_millis(20);
        assert_eq!(event_loop_wait(now, now, now, interval, None), interval);
        assert_eq!(
            event_loop_wait(now, now + Duration::from_millis(15), now, interval, None),
            Duration::from_millis(15)
        );
        assert_eq!(
            event_loop_wait(
                now,
                now + Duration::from_millis(20),
                now,
                interval,
                Some(now + Duration::from_millis(5))
            ),
            Duration::from_millis(5)
        );
        assert_eq!(
            event_loop_wait(now, now + Duration::from_secs(5), now, interval, None),
            EVENT_LOOP_WAKE_CAP
        );
        assert_eq!(
            event_loop_wait(
                now,
                now + Duration::from_secs(5),
                now - STATUS_INTERVAL,
                interval,
                None
            ),
            Duration::ZERO
        );
        assert_eq!(
            event_loop_wait(
                now,
                now,
                now,
                interval,
                Some(now - Duration::from_millis(1))
            ),
            Duration::ZERO
        );
    }

    #[test]
    fn send_loop_emits_then_idles_then_exits() {
        let status = status();
        let cfg = Config::default();
        let mut rt = PcmRuntime::new(AgcMode::Off);
        let mut sl = SendLoop::default();
        let now = Instant::now();

        rt.stream_active = true;
        rt.last_pcm = None;
        assert_eq!(rt.analyzer.samples.sound_agc, AgcMode::Off);
        assert_eq!(
            sl.on_update(&mut rt, &status, &cfg, true, now),
            LoopAction::Continue
        );
        assert_eq!(rt.analyzer.samples.sound_agc, AgcMode::Normal);
        rt.last_pcm = Some(now);
        assert_eq!(
            sl.on_update(&mut rt, &status, &cfg, true, now),
            LoopAction::Continue
        );
        assert!(sl.udp_active);
        assert!(sl.delay_q.is_empty());
        for i in 0..512 {
            rt.analyzer.ingest_mono_sample(0.0, i as u64);
        }
        assert_eq!(
            sl.on_update(&mut rt, &status, &cfg, true, now),
            LoopAction::Continue
        );
        assert_eq!(sl.delay_q.len(), 1);

        rt.last_pcm = Some(now - Duration::from_secs(5));
        rt.stream_since = Some(now - Duration::from_secs(5));
        assert_eq!(
            sl.on_update(&mut rt, &status, &cfg, true, now),
            LoopAction::Continue
        );
        assert!(!rt.stream_active);
        assert!(sl.announced_end);

        rt.stdin_closed = true;
        sl.delay_q.clear();
        sl.idle_packets_sent = IDLE_GRACE_PACKETS - 1;
        sl.udp_active = true;
        assert_eq!(
            sl.on_update(&mut rt, &status, &cfg, true, now),
            LoopAction::Continue
        );
        assert!(!sl.udp_active);
        sl.delay_q.clear();
        assert_eq!(
            sl.on_update(&mut rt, &status, &cfg, true, now),
            LoopAction::Exit
        );
    }

    #[test]
    fn send_loop_staggers_one_window_per_departure() {
        let status = status();
        let cfg = Config::default();
        let mut rt = PcmRuntime::new(AgcMode::Off);
        let mut sl = SendLoop::default();
        let now = Instant::now();
        rt.stream_active = true;
        rt.last_pcm = Some(now);
        for i in 0..(512 + 441) {
            let t = i as f32 / 22050.0;
            let s = (t * 440.0 * 2.0 * std::f32::consts::PI).sin() * 8_000.0;
            rt.analyzer.ingest_mono_sample(s, i as u64);
        }
        sl.on_update(&mut rt, &status, &cfg, false, now);
        assert_eq!(sl.delay_q.len(), 2);
        let gap = sl.delay_q[1].0.saturating_duration_since(sl.delay_q[0].0);
        assert_eq!(gap, Duration::from_millis(20));
    }

    #[test]
    fn departure_queues_each_destination_on_its_own_deadline() {
        let mut q = VecDeque::new();
        let base = Instant::now() + Duration::from_secs(2);
        let later: std::net::SocketAddr = "192.0.2.10:11988".parse().unwrap();
        let earlier: std::net::SocketAddr = "192.0.2.11:11988".parse().unwrap();
        enqueue_departure(
            &mut q,
            AudioSyncPacket::silence(),
            base,
            true,
            &[(later, 15), (earlier, -7)],
        );
        assert_eq!(q.len(), 3);
        assert_eq!(q[0].0, base - Duration::from_millis(7));
        assert!(matches!(q[0].2.as_slice(), [PacketTarget::Unicast(addr)] if *addr == earlier));
        assert_eq!(q[1].0, base);
        assert!(matches!(q[1].2.as_slice(), [PacketTarget::Multicast]));
        assert_eq!(q[2].0, base + Duration::from_millis(15));
        assert!(matches!(q[2].2.as_slice(), [PacketTarget::Unicast(addr)] if *addr == later));
        enqueue_departure(
            &mut q,
            AudioSyncPacket::silence(),
            base,
            false,
            &[(later, 0)],
        );
        assert!(q.iter().any(|item| item.0 == base && matches!(item.2.as_slice(), [PacketTarget::Unicast(_)])));
    }

    #[test]
    fn shared_send_time_is_one_queue_entry() {
        let mut q = VecDeque::new();
        let base = Instant::now() + Duration::from_millis(1200);
        let targets: Vec<(std::net::SocketAddr, i32)> = (1..=8)
            .map(|n| {
                (
                    format!("192.0.2.{n}:11988").parse().unwrap(),
                    0,
                )
            })
            .collect();
        enqueue_departure(
            &mut q,
            AudioSyncPacket::silence(),
            base,
            true,
            &targets,
        );
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].0, base);
        assert_eq!(q[0].2.len(), 9);
        assert!(matches!(q[0].2[0], PacketTarget::Multicast));
    }

    #[test]
    fn send_loop_second_window_stays_one_slot_later() {
        let status = status();
        let cfg = Config::default();
        let mut rt = PcmRuntime::new(AgcMode::Off);
        let mut sl = SendLoop::default();
        let now = Instant::now();
        rt.stream_active = true;
        rt.last_pcm = Some(now);
        for i in 0..(512 + 441) {
            let t = i as f32 / 22050.0;
            let s = (t * 440.0 * 2.0 * std::f32::consts::PI).sin() * 8_000.0;
            rt.analyzer.ingest_mono_sample(s, i as u64);
        }
        sl.on_update(&mut rt, &status, &cfg, false, now);
        sl.on_update(
            &mut rt,
            &status,
            &cfg,
            false,
            now + Duration::from_millis(20),
        );
        assert_eq!(sl.delay_q.len(), 2);
    }

    #[test]
    fn send_loop_keeps_a_later_window_off_the_next_wake() {
        let status = status();
        let cfg = Config::default();
        let mut rt = PcmRuntime::new(AgcMode::Off);
        let mut sl = SendLoop::default();
        let now = Instant::now();
        rt.stream_active = true;
        rt.last_pcm = Some(now);
        for i in 0..(512 + 441) {
            rt.analyzer.ingest_mono_sample(0.2, i as u64);
        }
        sl.on_update(&mut rt, &status, &cfg, false, now);
        assert_eq!(sl.delay_q.len(), 2);
        for i in (512 + 441)..(512 + 441 + 441) {
            rt.analyzer.ingest_mono_sample(0.2, i as u64);
        }
        sl.on_update(
            &mut rt,
            &status,
            &cfg,
            false,
            now + Duration::from_millis(20),
        );
        assert_eq!(sl.delay_q.len(), 3);
        let first = sl.delay_q[0].0;
        assert_eq!(sl.delay_q[1].0, first + Duration::from_millis(20));
        assert_eq!(sl.delay_q[2].0, first + Duration::from_millis(40));
    }

    #[test]
    fn next_departure_holds_phase_and_drops_a_stale_grid() {
        let now = Instant::now();
        let delay = Duration::from_millis(1180);
        let interval = Duration::from_millis(20);
        let anchor = now + delay;
        assert_eq!(next_departure(None, now, delay, interval), anchor);
        assert_eq!(next_departure(Some(anchor), now, delay, interval), anchor);
        assert_eq!(
            next_departure(Some(anchor + interval), now, delay, interval),
            anchor + interval
        );
        assert_eq!(
            next_departure(
                Some(anchor - Duration::from_millis(5)),
                now,
                delay,
                interval
            ),
            anchor - Duration::from_millis(5)
        );
        assert_eq!(
            next_departure(Some(anchor + interval * 4), now, delay, interval),
            anchor + interval * 4
        );
        assert_eq!(
            next_departure(
                Some(anchor - interval - Duration::from_millis(1)),
                now,
                delay,
                interval
            ),
            anchor
        );
        assert_eq!(
            next_departure(Some(anchor + interval * 5), now, delay, interval),
            anchor
        );
        assert_eq!(
            next_departure(Some(anchor + interval), now, delay, Duration::ZERO),
            anchor
        );
    }

    #[test]
    fn send_loop_counts_underrun_when_starved() {
        let status = status();
        let cfg = Config::default();
        let mut rt = PcmRuntime::new(AgcMode::Off);
        let mut sl = SendLoop::default();
        let now = Instant::now();
        rt.stream_active = true;
        rt.last_pcm = Some(now - PCM_UNDERRUN_AFTER - Duration::from_millis(5));
        sl.on_update(&mut rt, &status, &cfg, false, now);
        assert_eq!(rt.pcm_underruns, 1);
        sl.on_update(&mut rt, &status, &cfg, false, now);
        assert_eq!(rt.pcm_underruns, 1);
    }

    #[test]
    fn send_loop_exits_when_already_idle_and_input_closed() {
        let status = status();
        let cfg = Config::default();
        let mut rt = PcmRuntime::new(AgcMode::Off);
        let mut sl = SendLoop::default();
        rt.stdin_closed = true;
        sl.announced_end = false;
        assert_eq!(
            sl.on_update(&mut rt, &status, &cfg, true, Instant::now()),
            LoopAction::Exit
        );

        sl.announced_end = true;
        assert_eq!(
            sl.on_update(&mut rt, &status, &cfg, true, Instant::now()),
            LoopAction::Exit
        );
        assert_eq!(
            sl.on_update(&mut rt, &status, &cfg, false, Instant::now()),
            LoopAction::Continue
        );
    }

    #[test]
    fn send_loop_grace_continues_until_packet_budget() {
        let status = status();
        let cfg = Config::default();
        let mut rt = PcmRuntime::new(AgcMode::Off);
        let mut sl = SendLoop {
            udp_active: true,
            ..SendLoop::default()
        };
        assert_eq!(
            sl.on_update(&mut rt, &status, &cfg, true, Instant::now()),
            LoopAction::Continue
        );
        assert!(sl.udp_active);
        assert_eq!(sl.idle_packets_sent, 1);
    }

    #[test]
    fn status_and_publish_cover_active_and_idle() {
        let status = status();
        let mut cfg = Config::default();
        cfg.audio.sync_offset_ms = 12;
        cfg.wled.destinations = vec!["10.0.0.1".into()];
        let mut rt = PcmRuntime::new(AgcMode::Off);
        let now = Instant::now();
        update_status(&status, &rt, "none".into(), 0, 0, 0, &cfg, now);
        assert_eq!(status.lock().unwrap().audio.rms, 0.0);
        assert_eq!(status.lock().unwrap().airplay.stream_duration_s, 0.0);

        rt.stream_active = true;
        rt.stream_since = Some(now - Duration::from_secs(2));
        rt.pcm_frames = 9;
        rt.connections = 3;
        publish_audio(&status, &rt, &cfg, false);
        update_status(&status, &rt, "239.0.0.1:11988".into(), 4, 1, 7, &cfg, now);
        {
            let s = status.lock().unwrap();
            assert!(s.airplay.connected);
            assert_eq!(s.counters.pcm_frames, 9);
            assert_eq!(s.counters.airplay_connections, 3);
            assert_eq!(s.wled.packets_per_second, 7.0);
            assert!(s.airplay.stream_duration_s >= 2.0);
            assert!(!s.audio.agc_on);
            assert!((s.audio.agc_gain - 1.0).abs() < 1e-6);
        }

        rt.analyzer.set_agc(AgcMode::Vivid);
        rt.analyzer.samples.mult_agc = 2.5;
        publish_audio(&status, &rt, &cfg, false);
        let a = status.lock().unwrap().audio.clone();
        assert!(a.agc_on);
        assert!((a.agc_gain - 2.5).abs() < 1e-6);
    }

    #[test]
    fn transmitted_meters_follow_last_udp_packet() {
        let status = status();
        record_transmitted(&status, &AudioSyncPacket::silence());
        {
            let a = &status.lock().unwrap().audio;
            assert!(!a.tx_connected);
            assert!(!a.tx_peak);
            assert_eq!(a.tx_bands, [0; 16]);
        }

        let mut pkt = AudioSyncPacket::silence();
        pkt.sample_smth = 128.0;
        pkt.sample_peak = 1;
        pkt.fft_major_peak = 440.0;
        pkt.fft_result[3] = 200;
        record_transmitted(&status, &pkt);
        let a = status.lock().unwrap().audio.clone();
        assert!(a.tx_connected);
        assert!(a.tx_peak);
        assert!((a.tx_rms - 128.0 / 255.0).abs() < 1e-6);
        assert!((a.tx_dominant_frequency - 440.0).abs() < 1e-6);
        assert_eq!(a.tx_bands[3], 200);

        let mut smth_only = AudioSyncPacket::silence();
        smth_only.sample_smth = 2.0;
        apply_transmitted_packet(&mut status.lock().unwrap().audio, &smth_only);
        assert!(status.lock().unwrap().audio.tx_connected);
        assert!(!status.lock().unwrap().audio.tx_peak);

        let mut bands_only = AudioSyncPacket::silence();
        bands_only.fft_result[0] = 1;
        apply_transmitted_packet(&mut status.lock().unwrap().audio, &bands_only);
        assert!(status.lock().unwrap().audio.tx_connected);

        apply_transmitted_packet(
            &mut status.lock().unwrap().audio,
            &AudioSyncPacket::silence(),
        );
        assert!(!status.lock().unwrap().audio.tx_connected);
    }

    #[test]
    fn publish_audio_peak_follows_packet_not_hold_flag() {
        let status = status();
        let mut cfg = Config::default();
        cfg.audio.sync_offset_ms = 0;
        let mut rt = PcmRuntime::new(AgcMode::Off);
        rt.stream_active = true;
        rt.analyzer.samples.sample_peak = true;
        publish_audio(&status, &rt, &cfg, false);
        assert!(!status.lock().unwrap().audio.peak);
        publish_audio(&status, &rt, &cfg, true);
        assert!(status.lock().unwrap().audio.peak);
        rt.stream_active = false;
        publish_audio(&status, &rt, &cfg, true);
        assert!(!status.lock().unwrap().audio.peak);
    }

    #[test]
    fn mark_stream_end_keeps_stdin_open_for_persistent_input() {
        let status = status();
        let mut rt = PcmRuntime::new(AgcMode::Off);
        mark_stream_connected(&mut rt, &status, Instant::now());
        mark_stream_ended(&mut rt, &status, true);
        assert!(!rt.stdin_closed);
        assert!(!rt.stream_active);
    }

    #[test]
    fn mark_stream_helpers_are_idempotent() {
        let status = status();
        let mut rt = PcmRuntime::new(AgcMode::Off);
        let now = Instant::now();
        mark_stream_connected(&mut rt, &status, now);
        mark_stream_connected(&mut rt, &status, now);
        assert_eq!(rt.connections, 1);
        mark_stream_ended(&mut rt, &status, false);
        mark_stream_ended(&mut rt, &status, false);
        assert!(rt.stdin_closed);
        assert!(!rt.stream_active);
    }

    #[test]
    fn http_listen_prefers_nic_ipv4() {
        let bind = crate::netif::InterfaceBind {
            name: "eth0".into(),
            ipv4: std::net::Ipv4Addr::new(10, 0, 0, 8),
        };
        assert_eq!(http_listen(Some(&bind), "0.0.0.0"), "10.0.0.8");
        assert_eq!(http_listen(None, "0.0.0.0"), "0.0.0.0");
    }

    #[test]
    fn resolve_optional_nic_empty_and_unknown() {
        assert!(resolve_optional_nic("HTTP", "").unwrap().is_none());
        assert!(resolve_optional_nic("HTTP", "wledplay-no-such-nic").is_err());
        let bind = crate::netif::InterfaceBind {
            name: "eth0".into(),
            ipv4: std::net::Ipv4Addr::new(10, 0, 0, 8),
        };
        assert_eq!(
            describe_nic_bind("HTTP", Ok(Some(bind)))
                .unwrap()
                .unwrap()
                .name,
            "eth0"
        );
    }

    #[test]
    fn start_ms_counts_elapsed() {
        let start = Instant::now();
        let now = start + Duration::from_millis(40);
        assert_eq!(start_ms(start, now), 40);
    }
}
