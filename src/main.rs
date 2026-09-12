use clap::Parser;
use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tracing_subscriber::EnvFilter;
use wledplay::audio::Analyzer;
use wledplay::config::Config;
use wledplay::pcm::{self, PcmDecoder};
use wledplay::web::{self, AirplayStatus, RuntimeStatus, SharedStatus};
use wledplay::wled::WledSender;

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Silence packets sent after stream end before stopping UDP (SRS §34).
const IDLE_GRACE_PACKETS: u64 = 10;
/// Shairport gaps for up to ~1s at session start / resync.
const PCM_IDLE_START: Duration = Duration::from_millis(2000);
/// After playback is established, treat a gap as pause (stop lights sooner).
const PCM_IDLE_PLAY: Duration = Duration::from_millis(400);
/// Keep at most this much unread stdin PCM when not realtime-pacing (AirPlay).
const STDIN_KEEP_MS: u64 = 80;

#[derive(Parser, Debug)]
#[command(name = "wledplay", about = "AirPlay-to-WLED audio-reactive bridge")]
struct Cli {
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    #[arg(long, default_value = "stdin")]
    input: String,

    #[arg(long, default_value = "239.0.0.1")]
    multicast: String,

    #[arg(long, default_value_t = 11988)]
    port: u16,

    #[arg(long, default_value_t = 44100)]
    pcm_rate: u32,

    #[arg(long, default_value_t = 2)]
    pcm_channels: u16,

    #[arg(long, default_value = "s16le")]
    pcm_format: String,

    #[arg(long)]
    no_web: bool,

    /// Unicast WLED destination(s); repeat for multiple devices
    #[arg(long = "destination", value_name = "IP")]
    destinations: Vec<String>,

    /// Pace stdin PCM to real-time (required for `cat file.pcm` tests)
    #[arg(long, default_value_t = true)]
    realtime: bool,

    /// Disable pacing (AirPlay / Shairport already emit PCM in real time)
    #[arg(long)]
    no_realtime: bool,

    /// Bind AirPlay/UDP to this host NIC (e.g. eth1). Env: WLEDPLAY_INTERFACE
    #[arg(long, env = "WLEDPLAY_INTERFACE")]
    interface: Option<String>,
}

struct PcmRuntime {
    analyzer: Analyzer,
    stream_active: bool,
    pcm_frames: u64,
    last_pcm: Option<Instant>,
    stream_since: Option<Instant>,
    stdin_closed: bool,
    connections: u64,
    pcm_underruns: u64,
    analysis_overruns: u64,
}

fn pcm_idle_timeout(rt: &PcmRuntime) -> Duration {
    match rt.stream_since {
        Some(t) if t.elapsed() >= PCM_IDLE_START => PCM_IDLE_PLAY,
        _ => PCM_IDLE_START,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();
    let mut cfg = Config::load(cli.config.as_deref());

    if cli.multicast != "239.0.0.1" {
        cfg.wled.multicast_address = cli.multicast;
    }
    if cli.port != 11988 {
        cfg.wled.port = cli.port;
    }
    cfg.pcm.rate = cli.pcm_rate;
    cfg.pcm.channels = cli.pcm_channels;
    cfg.pcm.format = cli.pcm_format;
    if !cli.destinations.is_empty() {
        cfg.wled.destinations = cli.destinations;
    }
    if let Some(iface) = cli.interface.filter(|s| !s.is_empty()) {
        cfg.network.interface = iface;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(&cfg.logging.level)),
        )
        .init();

    let pace_realtime = cli.realtime && !cli.no_realtime;

    tracing::info!("WLEDPlay {}", VERSION);
    tracing::info!("AirPlay receiver: {}", cfg.airplay.name);
    tracing::info!(
        "PCM: {} Hz / {} / {}",
        cfg.pcm.rate,
        if cfg.pcm.channels == 1 {
            "mono"
        } else {
            "stereo"
        },
        cfg.pcm.format
    );
    tracing::info!("Analyzer: initialized");
    tracing::info!("WLED Audio Sync: protocol v2");
    tracing::info!(
        "Destination: {}:{}",
        cfg.wled.multicast_address,
        cfg.wled.port
    );
    let unicast = cfg.unicast_sockets();
    if !cfg.wled.destinations.is_empty() {
        tracing::info!("Unicast destinations: {:?}", cfg.wled.destinations);
        if unicast.is_empty() {
            tracing::warn!(
                "configured destinations did not parse as IP or IP:port; only multicast will be sent"
            );
        } else {
            tracing::info!("Resolved unicast sockets: {:?}", unicast);
        }
    }
    tracing::info!("Update rate: {} Hz", cfg.audio.update_hz);
    if pace_realtime {
        tracing::info!("Realtime PCM pacing: enabled");
    }

    let iface_bind = if cfg.network.interface.is_empty() {
        None
    } else {
        match wledplay::netif::resolve_interface(&cfg.network.interface) {
            Ok(b) => {
                tracing::info!("Network interface: {} ({})", b.name, b.ipv4);
                Some(b)
            }
            Err(e) => {
                tracing::error!("{e}");
                std::process::exit(1);
            }
        }
    };

    let status: SharedStatus = Arc::new(Mutex::new(RuntimeStatus {
        version: VERSION.into(),
        airplay: AirplayStatus {
            connected: false,
            receiver_name: cfg.airplay.name.clone(),
            ..Default::default()
        },
        ..Default::default()
    }));

    if cfg.web.enabled && !cli.no_web {
        let listen = cfg.web.listen.clone();
        let port = cfg.web.port;
        let st = status.clone();
        tokio::spawn(async move {
            if let Err(e) = web::serve(&listen, port, st).await {
                tracing::error!("web server error: {e}");
            }
        });
    }

    let mut sender = WledSender::new(
        cfg.multicast_ip(),
        cfg.wled.port,
        unicast,
        iface_bind.as_ref().map(|b| b.ipv4),
        iface_bind.as_ref().map(|b| b.name.as_str()),
    )?;

    let update_interval = Duration::from_millis(1000 / cfg.audio.update_hz.max(1) as u64);
    let stdin_is_pipe = !io::stdin().is_terminal();

    if cli.input != "stdin" {
        tracing::error!("Only --input stdin is implemented; use the AirPlay runner or Docker");
        std::process::exit(1);
    }

    tracing::info!("Waiting for PCM stream...");

    let runtime = Arc::new(Mutex::new(PcmRuntime {
        analyzer: Analyzer::new(cfg.agc_mode()),
        stream_active: false,
        pcm_frames: 0,
        last_pcm: None,
        stream_since: None,
        stdin_closed: false,
        connections: 0,
        pcm_underruns: 0,
        analysis_overruns: 0,
    }));

    spawn_pcm_reader(
        runtime.clone(),
        status.clone(),
        cfg.pcm.clone(),
        pace_realtime,
    );

    let mut last_send = Instant::now();
    let mut last_status = Instant::now();
    let mut packets_last_sec = 0u64;
    let mut udp_active = false;
    let mut idle_packets_sent = 0u64;
    let mut announced_end = false;

    loop {
        let now = Instant::now();
        if last_send.elapsed() >= update_interval {
            let mut rt = runtime.lock().unwrap();
            if rt.stream_active {
                if let Some(t) = rt.last_pcm {
                    if t.elapsed() >= pcm_idle_timeout(&rt) {
                        tracing::info!("PCM stream ended");
                        rt.stream_active = false;
                        rt.stream_since = None;
                        rt.analyzer.set_stream_active(false);
                        status.lock().unwrap().airplay.connected = false;
                        idle_packets_sent = 0;
                        announced_end = true;
                    } else if t.elapsed() > update_interval {
                        rt.pcm_underruns += 1;
                    }
                }
            }

            if rt.stream_active {
                udp_active = true;
                idle_packets_sent = 0;
                announced_end = false;
                if cfg.audio.sync_offset_ms > 0 {
                    drop(rt);
                    thread::sleep(Duration::from_millis(cfg.audio.sync_offset_ms as u64));
                    rt = runtime.lock().unwrap();
                }
                let pkt = rt.analyzer.build_packet();
                drop(rt);
                sender.send(&pkt)?;
                packets_last_sec += 1;
            } else if udp_active {
                let pkt = rt.analyzer.silence_packet();
                let closed = rt.stdin_closed;
                drop(rt);
                sender.send(&pkt)?;
                packets_last_sec += 1;
                idle_packets_sent += 1;
                if idle_packets_sent >= IDLE_GRACE_PACKETS {
                    udp_active = false;
                    tracing::info!("Audio Sync transmission stopped (stream idle)");
                    if stdin_is_pipe && closed {
                        tracing::info!("Input closed; exiting");
                        break;
                    }
                }
            } else {
                let closed = rt.stdin_closed;
                drop(rt);
                if stdin_is_pipe && closed {
                    if !announced_end {
                        tracing::info!("Input closed; exiting");
                    }
                    break;
                }
            }

            last_send = now;
        }

        if last_status.elapsed() >= Duration::from_secs(1) {
            let rt = runtime.lock().unwrap();
            update_status(
                &status,
                &rt,
                &sender,
                packets_last_sec,
                &cfg,
            );
            packets_last_sec = 0;
            last_status = Instant::now();
        }

        thread::sleep(Duration::from_millis(2));
    }

    Ok(())
}

fn discard_stdin_backlog(reader: &mut impl Read, keep_bytes: u64) -> Option<usize> {
    let avail = stdin_available_bytes();
    if avail <= keep_bytes {
        return None;
    }
    let mut left = (avail - keep_bytes) as usize;
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

fn stdin_available_bytes() -> u64 {
    #[cfg(unix)]
    {
        let mut nbytes: libc::c_int = 0;
        let ret = unsafe { libc::ioctl(libc::STDIN_FILENO, libc::FIONREAD, &mut nbytes) };
        if ret == 0 && nbytes > 0 {
            nbytes as u64
        } else {
            0
        }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

fn spawn_pcm_reader(
    runtime: Arc<Mutex<PcmRuntime>>,
    status: SharedStatus,
    pcm: wledplay::config::PcmFormat,
    pace_realtime: bool,
) {
    thread::Builder::new()
        .name("pcm-stdin".into())
        .spawn(move || {
            let bytes_per_second = pcm.rate as u64
                * pcm.channels as u64
                * pcm.bytes_per_sample() as u64;
            let mut decoder = PcmDecoder::new(pcm.rate, pcm.channels, pcm);
            let stdin = io::stdin();
            let mut reader = stdin.lock();
            let mut read_buf = [0u8; 4096];
            let mut playback_clock = Instant::now();
            let mut bytes_played: u64 = 0;
            let mut last_skip_log = Instant::now();

            loop {
                if !pace_realtime && bytes_per_second > 0 {
                    let keep = (bytes_per_second * STDIN_KEEP_MS / 1000).max(4096);
                    if let Some(skipped) = discard_stdin_backlog(&mut reader, keep) {
                        runtime.lock().unwrap().analysis_overruns += 1;
                        if last_skip_log.elapsed() >= Duration::from_secs(2) {
                            tracing::info!(
                                "dropped {} bytes of PCM backlog (~{} ms)",
                                skipped,
                                skipped as u64 * 1000 / bytes_per_second
                            );
                            last_skip_log = Instant::now();
                        }
                    }
                }
                match reader.read(&mut read_buf) {
                    Ok(0) => {
                        let mut rt = runtime.lock().unwrap();
                        if rt.stream_active {
                            tracing::info!("PCM stream ended");
                            rt.stream_active = false;
                            rt.stream_since = None;
                            rt.analyzer.set_stream_active(false);
                            status.lock().unwrap().airplay.connected = false;
                        }
                        rt.stdin_closed = true;
                        break;
                    }
                    Ok(n) => {
                        let now_ms = start_ms();
                        let mut cursor = std::io::Cursor::new(&read_buf[..n]);
                        let mut rt = runtime.lock().unwrap();
                        if !rt.stream_active {
                            tracing::info!("PCM stream connected");
                            rt.stream_active = true;
                            rt.stream_since = Some(Instant::now());
                            rt.connections += 1;
                            rt.analyzer.set_stream_active(true);
                            status.lock().unwrap().airplay.connected = true;
                            playback_clock = Instant::now();
                            bytes_played = 0;
                        }
                        match decoder.decode_chunk(&mut cursor, |sample| {
                            rt.analyzer.ingest_mono_sample(sample, now_ms);
                        }) {
                            Ok(frames) => rt.pcm_frames += frames as u64,
                            Err(e) => {
                                tracing::error!("PCM decode error: {e}");
                                rt.stdin_closed = true;
                                break;
                            }
                        }
                        rt.last_pcm = Some(Instant::now());
                        drop(rt);

                        if pace_realtime && bytes_per_second > 0 {
                            bytes_played += n as u64;
                            let target = Duration::from_secs_f64(
                                bytes_played as f64 / bytes_per_second as f64,
                            );
                            let elapsed = playback_clock.elapsed();
                            if target > elapsed {
                                thread::sleep(target - elapsed);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("stdin read error: {e}");
                        runtime.lock().unwrap().stdin_closed = true;
                        break;
                    }
                }
            }
        })
        .expect("pcm reader thread");
}

fn start_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_millis() as u64
}

fn update_status(
    status: &SharedStatus,
    rt: &PcmRuntime,
    sender: &WledSender,
    packets_per_sec: u64,
    cfg: &Config,
) {
    let mut s = status.lock().unwrap();
    s.audio.sample_rate = pcm::ANALYSIS_RATE;
    s.audio.channels = 1;
    s.audio.rms = rt.analyzer.rms_proxy();
    s.audio.peak = rt.analyzer.samples.sample_peak;
    s.audio.dominant_frequency = rt.analyzer.dominant_frequency();
    s.audio.bands = rt.analyzer.fft_bands();
    s.audio.sync_offset_ms = cfg.audio.sync_offset_ms;
    s.wled.packets_per_second = packets_per_sec as f32;
    s.wled.destination = sender.destination();
    s.wled.destinations = cfg.wled.destinations.clone();
    s.counters.pcm_frames = rt.pcm_frames;
    s.counters.fft_frames = rt.analyzer.fft_frames();
    s.counters.packets_sent = sender.packets_sent();
    s.counters.udp_errors = sender.udp_errors();
    s.counters.pcm_underruns = rt.pcm_underruns;
    s.counters.analysis_overruns = rt.analysis_overruns;
    s.counters.airplay_connections = rt.connections;
    s.airplay.receiver_name = cfg.airplay.name.clone();
    s.airplay.connections = rt.connections;
    s.airplay.stream_duration_s = rt
        .stream_since
        .map(|t| t.elapsed().as_secs_f32())
        .unwrap_or(0.0);
}
