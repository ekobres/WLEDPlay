//! PCM / Audio Sync runtime helpers shared by the binary event loop.

use crate::audio::Analyzer;
use crate::config::{AgcMode, Config};
use crate::web::SharedStatus;
use crate::wled::AudioSyncPacket;
use std::collections::VecDeque;
use std::io::Read;
use std::time::{Duration, Instant};

/// Silence packets sent after stream end before stopping UDP (SRS §34).
pub const IDLE_GRACE_PACKETS: u64 = 10;
/// Shairport gaps for up to ~1s at session start / resync.
pub const PCM_IDLE_START: Duration = Duration::from_millis(2000);
/// After playback is established, treat a gap as pause (stop lights sooner).
pub const PCM_IDLE_PLAY: Duration = Duration::from_millis(400);
/// Keep at most this much unread stdin PCM when not realtime-pacing (AirPlay).
pub const STDIN_KEEP_MS: u64 = 80;
/// Count an underrun only after PCM has been silent this long.
pub const PCM_UNDERRUN_AFTER: Duration = Duration::from_millis(80);
/// Max queued Audio Sync packets for live visual delay (5s at 50 Hz, plus slack).
pub const DELAY_QUEUE_MAX: usize = 300;

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

pub fn enqueue_packet(
    queue: &mut VecDeque<(Instant, AudioSyncPacket)>,
    packet: AudioSyncPacket,
    delay_ms: u64,
    now: Instant,
) {
    queue.push_back((now + Duration::from_millis(delay_ms), packet));
    while queue.len() > DELAY_QUEUE_MAX {
        queue.pop_front();
    }
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

pub fn drain_due_packets(
    queue: &mut VecDeque<(Instant, AudioSyncPacket)>,
    now: Instant,
) -> Vec<AudioSyncPacket> {
    let mut due = Vec::new();
    while queue.front().map(|(t, _)| now >= *t).unwrap_or(false) {
        due.push(queue.pop_front().unwrap().1);
    }
    due
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopAction {
    Continue,
    Exit,
}

pub struct SendLoop {
    pub udp_active: bool,
    pub idle_packets_sent: u64,
    pub announced_end: bool,
    pub underrun_open: bool,
    pub delay_q: VecDeque<(Instant, AudioSyncPacket)>,
}

impl Default for SendLoop {
    fn default() -> Self {
        Self {
            udp_active: false,
            idle_packets_sent: 0,
            announced_end: false,
            underrun_open: false,
            delay_q: VecDeque::new(),
        }
    }
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
            let pkt = rt.analyzer.build_packet();
            publish_audio(status, rt, cfg);
            enqueue_packet(&mut self.delay_q, pkt, delay_ms, now);
            LoopAction::Continue
        } else if self.udp_active {
            let pkt = rt.analyzer.silence_packet();
            publish_audio(status, rt, cfg);
            enqueue_packet(&mut self.delay_q, pkt, delay_ms, now);
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
        s.audio.peak = rt.analyzer.samples.sample_peak;
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
}

pub fn publish_audio(status: &SharedStatus, rt: &PcmRuntime, cfg: &Config) {
    let mut s = status.lock().unwrap();
    s.audio.sample_rate = crate::pcm::ANALYSIS_RATE;
    s.audio.channels = 1;
    if rt.stream_active {
        s.audio.rms = rt.analyzer.rms_proxy();
        s.audio.peak = rt.analyzer.samples.sample_peak;
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

pub fn mark_stream_ended(rt: &mut PcmRuntime, status: &SharedStatus) {
    if rt.stream_active {
        tracing::info!("PCM stream ended");
        rt.stream_active = false;
        rt.stream_since = None;
        rt.analyzer.set_stream_active(false);
        status.lock().unwrap().airplay.connected = false;
    }
    rt.stdin_closed = true;
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
    fn enqueue_drops_oldest_when_over_cap() {
        let mut q = VecDeque::new();
        let now = Instant::now();
        for _ in 0..DELAY_QUEUE_MAX + 5 {
            enqueue_packet(&mut q, AudioSyncPacket::silence(), 0, now);
        }
        assert_eq!(q.len(), DELAY_QUEUE_MAX);
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
    fn drain_due_packets_respects_time() {
        let mut q = VecDeque::new();
        let now = Instant::now();
        enqueue_packet(&mut q, AudioSyncPacket::silence(), 5_000, now);
        assert!(drain_due_packets(&mut q, now).is_empty());
        assert_eq!(
            drain_due_packets(&mut q, now + Duration::from_secs(6)).len(),
            1
        );
        assert!(q.is_empty());
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
        assert_eq!(
            sl.on_update(&mut rt, &status, &cfg, true, now),
            LoopAction::Continue
        );
        rt.last_pcm = Some(now);
        assert_eq!(
            sl.on_update(&mut rt, &status, &cfg, true, now),
            LoopAction::Continue
        );
        assert!(sl.udp_active);
        assert!(sl.delay_q.len() >= 1);

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
        let mut sl = SendLoop::default();
        sl.udp_active = true;
        sl.idle_packets_sent = 0;
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
        publish_audio(&status, &rt, &cfg);
        update_status(&status, &rt, "239.0.0.1:11988".into(), 4, 1, 7, &cfg, now);
        let s = status.lock().unwrap();
        assert!(s.airplay.connected);
        assert_eq!(s.counters.pcm_frames, 9);
        assert_eq!(s.counters.airplay_connections, 3);
        assert_eq!(s.wled.packets_per_second, 7.0);
        assert!(s.airplay.stream_duration_s >= 2.0);
    }

    #[test]
    fn mark_stream_helpers_are_idempotent() {
        let status = status();
        let mut rt = PcmRuntime::new(AgcMode::Off);
        let now = Instant::now();
        mark_stream_connected(&mut rt, &status, now);
        mark_stream_connected(&mut rt, &status, now);
        assert_eq!(rt.connections, 1);
        mark_stream_ended(&mut rt, &status);
        mark_stream_ended(&mut rt, &status);
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
