use crate::clock;
use crate::config::{Config, PcmFormat};
use crate::control::Control;
use crate::metadata::{self, SharedArtwork, TrackMetadata};
use crate::netif::InterfaceBind;
use crate::pcm::PcmDecoder;
use crate::runtime::{
    self, event_loop_wait, http_listen, mark_stream_connected, mark_stream_ended, next_due_at,
    open_pcm_input, path_is_fifo, pcm_file_pump, pop_due_packet, record_transmitted,
    resolve_optional_nic, start_ms, stdin_available_bytes, step_send_deadline, update_status,
    LoopAction, PcmRuntime, PcmSessionGap, SendLoop,
};
use crate::web::{self, AirplayStatus, AppState, RuntimeStatus, SharedStatus};
use crate::wled::WledSender;
use clap::Parser;
use std::fs::File;
use std::io::{self, BufRead, BufReader, IsTerminal, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tracing_subscriber::EnvFilter;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser, Debug)]
#[command(
    name = "wledplay",
    version,
    about = "AirPlay-to-WLED audio-reactive bridge"
)]
pub struct Cli {
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// `stdin` or a PCM file / FIFO path
    #[arg(long, default_value = "stdin")]
    pub input: String,

    #[arg(long, default_value = "239.0.0.1")]
    pub multicast: String,

    #[arg(long, default_value_t = 11988)]
    pub port: u16,

    #[arg(long, default_value_t = 44100)]
    pub pcm_rate: u32,

    #[arg(long, default_value_t = 2)]
    pub pcm_channels: u16,

    #[arg(long, default_value = "s16le")]
    pub pcm_format: String,

    #[arg(long)]
    pub no_web: bool,

    /// Unicast WLED destination(s); repeat for multiple devices
    #[arg(long = "destination", value_name = "IP")]
    pub destinations: Vec<String>,

    /// Pace stdin PCM to real-time (required for `cat file.pcm` tests)
    #[arg(long, default_value_t = true)]
    pub realtime: bool,

    /// Disable pacing (AirPlay / Shairport already emit PCM in real time)
    #[arg(long)]
    pub no_realtime: bool,

    /// Bind the HTTP UI to this host NIC. Env: WLEDPLAY_HTTP_INTERFACE
    #[arg(long, env = "WLEDPLAY_HTTP_INTERFACE")]
    pub http_interface: Option<String>,

    /// Bind Shairport / Avahi (mDNS, AirPlay) to this host NIC. Env: WLEDPLAY_AIRPLAY_INTERFACE
    #[arg(long, env = "WLEDPLAY_AIRPLAY_INTERFACE")]
    pub airplay_interface: Option<String>,

    /// Bind WLED unicast UDP to this host NIC. Env: WLEDPLAY_UNICAST_INTERFACE
    #[arg(long, env = "WLEDPLAY_UNICAST_INTERFACE")]
    pub unicast_interface: Option<String>,

    /// Bind WLED multicast UDP to this host NIC. Env: WLEDPLAY_MULTICAST_INTERFACE
    #[arg(long, env = "WLEDPLAY_MULTICAST_INTERFACE")]
    pub multicast_interface: Option<String>,

    /// Shairport metadata FIFO. Env: WLEDPLAY_METADATA_PIPE
    #[arg(long, env = "WLEDPLAY_METADATA_PIPE", value_name = "PATH")]
    pub metadata_pipe: Option<PathBuf>,
}

pub fn apply_cli(cfg: &mut Config, cli: &Cli) {
    if cli.multicast != "239.0.0.1" {
        cfg.wled.multicast_address = cli.multicast.clone();
    }
    if cli.port != 11988 {
        cfg.wled.port = cli.port;
    }
    cfg.pcm.rate = cli.pcm_rate;
    cfg.pcm.channels = cli.pcm_channels;
    cfg.pcm.format = cli.pcm_format.clone();
    if !cli.destinations.is_empty() {
        cfg.wled.destinations = cli.destinations.clone();
    }
    if let Some(iface) = cli.http_interface.as_ref().filter(|s| !s.is_empty()) {
        cfg.network.http = iface.clone();
    }
    if let Some(iface) = cli.airplay_interface.as_ref().filter(|s| !s.is_empty()) {
        cfg.network.airplay = iface.clone();
    }
    if let Some(iface) = cli.unicast_interface.as_ref().filter(|s| !s.is_empty()) {
        cfg.network.unicast = iface.clone();
    }
    if let Some(iface) = cli.multicast_interface.as_ref().filter(|s| !s.is_empty()) {
        cfg.network.multicast = iface.clone();
    }
    if let Some(pipe) = cli
        .metadata_pipe
        .as_ref()
        .filter(|p| !p.as_os_str().is_empty())
    {
        cfg.metadata.pipe = pipe.display().to_string();
    }
}

pub fn pace_realtime(cli: &Cli) -> bool {
    cli.realtime && !cli.no_realtime
}

pub fn log_pcm_skip(skipped: usize, bytes_per_second: u64) {
    let ms = skipped as u64 * 1000 / bytes_per_second.max(1);
    tracing::info!("dropped {skipped} bytes of PCM backlog (~{ms} ms)");
}

pub fn log_pcm_input(path: Option<&PathBuf>) {
    if let Some(path) = path {
        tracing::info!("PCM input: {}", path.display());
    }
}

pub fn pcm_input_path(input: &str) -> Option<PathBuf> {
    if input == "stdin" {
        None
    } else {
        Some(PathBuf::from(input))
    }
}

pub fn finite_pcm_input(path: Option<&PathBuf>, stdin_is_pipe: bool) -> bool {
    match path {
        Some(p) => !path_is_fifo(p),
        None => stdin_is_pipe,
    }
}

pub fn instant_or_now(ts: Option<Instant>) -> Instant {
    ts.unwrap_or_else(Instant::now)
}

pub fn should_log_skip(last: Instant, now: Instant, interval: Duration) -> bool {
    now.saturating_duration_since(last) >= interval
}

pub fn pace_wait(target: Duration, elapsed: Duration) -> Option<Duration> {
    if target > elapsed {
        Some(target - elapsed)
    } else {
        None
    }
}

pub fn socket_bind(bind: Option<&InterfaceBind>) -> Option<(&str, std::net::Ipv4Addr)> {
    bind.map(|b| (b.name.as_str(), b.ipv4))
}

pub fn log_web_error(e: impl std::fmt::Display) {
    tracing::error!("web server error: {e}");
}

pub async fn run_with_cli(cli: Cli) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config_path = cli.config.clone();
    let mut cfg = Config::load(cli.config.as_deref());
    apply_cli(&mut cfg, &cli);

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(&cfg.logging.level)),
        )
        .try_init()
        .ok();

    let pace_realtime = pace_realtime(&cli);
    log_startup(&cfg, pace_realtime);

    let http_bind = resolve_optional_nic("HTTP", &cfg.network.http)?;
    let _airplay_bind = resolve_optional_nic("AirPlay/mDNS", &cfg.network.airplay)?;
    let unicast_bind = resolve_optional_nic("WLED unicast", &cfg.network.unicast)?;
    let multicast_bind = resolve_optional_nic("WLED multicast", &cfg.network.multicast)?;

    let status: SharedStatus = Arc::new(Mutex::new(RuntimeStatus {
        version: VERSION.into(),
        airplay: AirplayStatus {
            connected: false,
            receiver_name: cfg.airplay.name.clone(),
            ..Default::default()
        },
        ..Default::default()
    }));
    let artwork: SharedArtwork = Arc::new(Mutex::new(None));

    let unicast = cfg.unicast_sockets();
    let mut sender = WledSender::new(
        cfg.multicast_ip(),
        cfg.wled.port,
        unicast,
        socket_bind(unicast_bind.as_ref()),
        socket_bind(multicast_bind.as_ref()),
    )
    .expect("WLED UDP sockets");
    sender.set_multicast_enabled(cfg.wled.multicast_enabled);
    let sender = Arc::new(Mutex::new(sender));
    let live_cfg = Arc::new(Mutex::new(cfg.clone()));
    let restart = Arc::new(AtomicBool::new(false));
    let control = Control {
        config: live_cfg.clone(),
        sender: sender.clone(),
        config_path: config_path.clone(),
        applied_name: cfg.airplay.name.clone(),
        applied_http: cfg.network.http.clone(),
        applied_airplay: cfg.network.airplay.clone(),
        applied_unicast: cfg.network.unicast.clone(),
        applied_multicast: cfg.network.multicast.clone(),
        applied_shairport: cfg.audio.shairport_latency_offset_s,
        restart: restart.clone(),
    };

    if cfg.web.enabled && !cli.no_web {
        let listen = http_listen(http_bind.as_ref(), &cfg.web.listen);
        let port = cfg.web.port;
        let state = AppState {
            status: status.clone(),
            artwork: artwork.clone(),
            control: Some(control),
        };
        tokio::spawn(async move {
            web::serve(&listen, port, state)
                .await
                .map_err(log_web_error)
                .ok();
        });
    }

    if !cfg.metadata.pipe.is_empty() {
        spawn_metadata_reader(cfg.metadata.pipe.clone(), status.clone(), artwork.clone());
    }

    let update_interval = Duration::from_millis(1000 / cfg.audio.update_hz.max(1) as u64);
    let stdin_is_pipe = !io::stdin().is_terminal();
    let pcm_path = pcm_input_path(&cli.input);
    let finite_input = finite_pcm_input(pcm_path.as_ref(), stdin_is_pipe);
    log_pcm_input(pcm_path.as_ref());

    tracing::info!("Waiting for PCM stream...");

    let runtime = Arc::new(Mutex::new(PcmRuntime::new(cfg.agc_mode())));
    spawn_pcm_reader(
        runtime.clone(),
        status.clone(),
        cfg.pcm.clone(),
        pace_realtime,
        pcm_path,
    );

    let mut last_send = instant_or_now(Instant::now().checked_sub(update_interval));
    let mut last_status = instant_or_now(Instant::now().checked_sub(Duration::from_secs(1)));
    let mut packets_last_sec = 0u64;
    let mut loop_state = SendLoop::default();
    let auto_join = crate::auto_join::from_env();

    loop {
        let action = event_loop_tick(
            &restart,
            &runtime,
            &live_cfg,
            &sender,
            &status,
            &mut loop_state,
            &mut last_send,
            &mut last_status,
            &mut packets_last_sec,
            update_interval,
            finite_input,
            Instant::now(),
            &auto_join,
        );
        if action == LoopAction::Exit {
            break;
        }
        let wait = event_loop_wait(
            Instant::now(),
            last_send,
            last_status,
            update_interval,
            next_due_at(&loop_state.delay_q),
        );
        tokio::time::sleep(wait).await;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn event_loop_tick(
    restart: &AtomicBool,
    runtime: &Mutex<PcmRuntime>,
    live_cfg: &Mutex<Config>,
    sender: &Mutex<WledSender>,
    status: &SharedStatus,
    loop_state: &mut SendLoop,
    last_send: &mut Instant,
    last_status: &mut Instant,
    packets_last_sec: &mut u64,
    update_interval: Duration,
    finite_input: bool,
    now: Instant,
    auto_join: &crate::auto_join::AutoJoinHub,
) -> LoopAction {
    if restart.load(Ordering::Relaxed) {
        tracing::info!("Restart requested; exiting so the supervisor can relaunch");
        return LoopAction::Exit;
    }
    if now.saturating_duration_since(*last_send) >= update_interval {
        let mut rt = runtime.lock().unwrap();
        let live = live_cfg.lock().unwrap();
        let action = loop_state.on_update(&mut rt, status, &live, finite_input, now);
        drop(live);
        drop(rt);
        *last_send = step_send_deadline(*last_send, update_interval, now);
        if action == LoopAction::Exit {
            return LoopAction::Exit;
        }
    }

    {
        let mut snd = sender.lock().unwrap();
        while let Some(pkt) = pop_due_packet(&mut loop_state.delay_q, now) {
            let _ = snd.send(&pkt);
            record_transmitted(status, &pkt);
            *packets_last_sec += 1;
        }
    }

    if now.saturating_duration_since(*last_status) >= Duration::from_secs(1) {
        let rt = runtime.lock().unwrap();
        let live = live_cfg.lock().unwrap();
        let snd = sender.lock().unwrap();
        update_status(
            status,
            &rt,
            snd.destination(),
            snd.packets_sent(),
            snd.udp_errors(),
            *packets_last_sec,
            &live,
            now,
        );
        *packets_last_sec = 0;
        *last_status = now;
    }

    let (raw_busy, auto, receiver_name) = {
        let rt = runtime.lock().unwrap();
        let live = live_cfg.lock().unwrap();
        (
            rt.stream_active,
            live.auto_join.clone(),
            live.airplay.name.clone(),
        )
    };
    auto_join.drive(raw_busy, &auto, &receiver_name, now, status);

    LoopAction::Continue
}

fn log_startup(cfg: &Config, pace_realtime: bool) {
    tracing::info!("WLEDPlay {}", VERSION);
    tracing::info!("AirPlay receiver: {}", cfg.airplay.name);
    let layout = if cfg.pcm.channels == 1 {
        "mono"
    } else {
        "stereo"
    };
    tracing::info!("PCM: {} Hz / {} / {}", cfg.pcm.rate, layout, cfg.pcm.format);
    tracing::info!("Analyzer: initialized");
    tracing::info!("WLED Audio Sync: protocol v2");
    let dest = format!("{}:{}", cfg.wled.multicast_address, cfg.wled.port);
    tracing::info!("Destination: {dest}");
    let unicast = cfg.unicast_sockets();
    if !cfg.wled.destinations.is_empty() {
        tracing::info!("Unicast destinations: {:?}", cfg.wled.destinations);
        if unicast.is_empty() {
            tracing::warn!("configured destinations did not parse as IP or IP:port; only multicast will be sent");
        } else {
            tracing::info!("Resolved unicast sockets: {:?}", unicast);
        }
    }
    tracing::info!("Update rate: {} Hz", cfg.audio.update_hz);
    if pace_realtime {
        tracing::info!("Realtime PCM pacing: enabled");
    }
}

pub fn spawn_pcm_reader(
    runtime: Arc<Mutex<PcmRuntime>>,
    status: SharedStatus,
    pcm: PcmFormat,
    pace_realtime: bool,
    input_path: Option<PathBuf>,
) {
    thread::Builder::new()
        .name("pcm-input".into())
        .spawn(move || {
            if let Some(path) = input_path {
                let fifo = path_is_fifo(&path);
                let held = std::cell::RefCell::new(None);
                let runtime_read = runtime.clone();
                let status_read = status.clone();
                let pcm_read = pcm.clone();
                pcm_file_pump(
                    &mut || {
                        *held.borrow_mut() = Some(open_pcm_input(&path)?);
                        Ok(())
                    },
                    &mut || {
                        read_pcm(
                            held.borrow_mut().as_mut().unwrap(),
                            false,
                            &|| 0,
                            runtime_read.clone(),
                            status_read.clone(),
                            pcm_read.clone(),
                            pace_realtime,
                            fifo,
                        )
                    },
                    &mut |e| {
                        tracing::error!("PCM input {}: {e}", path.display());
                        runtime.lock().unwrap().stdin_closed = true;
                    },
                );
            } else {
                let mut stdin = io::stdin();
                read_pcm(
                    &mut stdin,
                    true,
                    &stdin_available_bytes,
                    runtime,
                    status,
                    pcm,
                    pace_realtime,
                    false,
                );
            }
        })
        .expect("pcm reader thread");
}

#[allow(clippy::too_many_arguments)]
pub fn read_pcm(
    reader: &mut dyn Read,
    discard_backlog_ok: bool,
    available: &dyn Fn() -> u64,
    runtime: Arc<Mutex<PcmRuntime>>,
    status: SharedStatus,
    pcm: PcmFormat,
    pace_realtime: bool,
    reopenable: bool,
) -> bool {
    let persistent = discard_backlog_ok || reopenable;
    let bytes_per_second = pcm.rate as u64 * pcm.channels as u64 * pcm.bytes_per_sample() as u64;
    let mut decoder = PcmDecoder::new(pcm.rate, pcm.channels, pcm.clone());
    let mut read_buf = [0u8; 4096];
    let mut playback_clock = Instant::now();
    let mut bytes_played: u64 = 0;
    let mut last_skip_log = instant_or_now(Instant::now().checked_sub(Duration::from_secs(2)));
    let origin = Instant::now();

    loop {
        if discard_backlog_ok && !pace_realtime && bytes_per_second > 0 {
            let keep = runtime::stdin_keep_bytes(bytes_per_second);
            if let Some(skipped) = runtime::discard_reader_backlog(reader, available(), keep) {
                runtime.lock().unwrap().analysis_overruns += 1;
                if should_log_skip(last_skip_log, Instant::now(), Duration::from_secs(2)) {
                    log_pcm_skip(skipped, bytes_per_second);
                    last_skip_log = Instant::now();
                }
            }
        }
        match reader.read(&mut read_buf) {
            Ok(0) => {
                let mut rt = runtime.lock().unwrap();
                mark_stream_ended(&mut rt, &status, persistent);
                drop(rt);
                match runtime::pcm_session_gap(persistent, reopenable) {
                    PcmSessionGap::Exit => return false,
                    PcmSessionGap::Reopen => return true,
                    PcmSessionGap::Wait(d) => {
                        thread::sleep(d);
                        decoder = PcmDecoder::new(pcm.rate, pcm.channels, pcm.clone());
                        playback_clock = Instant::now();
                        bytes_played = 0;
                        continue;
                    }
                }
            }
            Ok(n) => {
                let now_ms = start_ms(origin, Instant::now());
                let mut samples = Vec::new();
                let frames = decoder.decode_bytes(&read_buf[..n], &mut |sample| {
                    samples.push(sample);
                });
                {
                    let mut rt = runtime.lock().unwrap();
                    if !rt.stream_active {
                        mark_stream_connected(&mut rt, &status, Instant::now());
                        playback_clock = Instant::now();
                        bytes_played = 0;
                    }
                    rt.pcm_frames += frames as u64;
                    rt.last_pcm = Some(Instant::now());
                }
                {
                    let mut rt = runtime.lock().unwrap();
                    for sample in samples {
                        rt.analyzer.ingest_mono_sample(sample, now_ms);
                    }
                }

                if pace_realtime && bytes_per_second > 0 {
                    bytes_played += n as u64;
                    let target =
                        Duration::from_secs_f64(bytes_played as f64 / bytes_per_second as f64);
                    let elapsed = playback_clock.elapsed();
                    thread::sleep(pace_wait(target, elapsed).unwrap_or_default());
                }
            }
            Err(e) => {
                tracing::error!("PCM read error: {e}");
                runtime.lock().unwrap().stdin_closed = true;
                return false;
            }
        }
    }
}

pub fn spawn_metadata_reader(path: String, status: SharedStatus, artwork: SharedArtwork) {
    thread::Builder::new()
        .name("metadata".into())
        .spawn(move || {
            tracing::info!("Metadata pipe: {path}");
            read_metadata_pipe(&path, &status, &artwork, None);
        })
        .expect("metadata reader thread");
}

/// `max_cycles` limits reopen attempts (tests). `None` runs until the process exits.
pub fn read_metadata_pipe(
    path: &str,
    status: &SharedStatus,
    artwork: &SharedArtwork,
    max_cycles: Option<u32>,
) {
    let mut cycles = 0u32;
    loop {
        if max_cycles.is_some_and(|n| cycles >= n) {
            break;
        }
        cycles += 1;
        let file = match File::open(path) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("metadata pipe {path}: {e}");
                thread::sleep(metadata_retry_delay(max_cycles));
                continue;
            }
        };
        let mut reader = BufReader::new(file);
        consume_metadata_reader(&mut reader, status, artwork);
    }
}

fn log_track(meta: &TrackMetadata) {
    let artist = meta.artist.as_deref().unwrap_or("?");
    let title = meta.title.as_deref().unwrap_or("?");
    tracing::info!("Track: {artist} — {title}");
}

fn log_clip(source: Option<&str>) {
    if let Some(src) = source {
        tracing::info!("AirPlay source: {src}");
    }
}

fn metadata_retry_delay(max_cycles: Option<u32>) -> Duration {
    if max_cycles.is_some() {
        Duration::ZERO
    } else {
        Duration::from_secs(2)
    }
}

pub fn consume_metadata_reader(
    reader: &mut dyn BufRead,
    status: &SharedStatus,
    artwork: &SharedArtwork,
) {
    let mut raw = String::new();
    let mut line = String::new();
    let mut meta = TrackMetadata::default();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                apply_metadata_line(&mut raw, &line, &mut meta, artwork, status);
            }
            Err(e) => {
                tracing::warn!("metadata read: {e}");
                break;
            }
        }
    }
}

pub fn apply_metadata_line(
    raw: &mut String,
    line: &str,
    meta: &mut TrackMetadata,
    artwork: &SharedArtwork,
    status: &SharedStatus,
) {
    raw.push_str(line);
    let mut session = status.lock().unwrap().clock_session.clone();
    for item in metadata::drain_items(raw) {
        clock::fold_metadata(&item, &mut session);
        let mut art = artwork.lock().unwrap();
        if metadata::apply_item(meta, &mut art, &item) {
            if item.code == "minm" || item.code == "asar" {
                log_track(meta);
            }
            if item.code == "clip" {
                log_clip(meta.source.as_deref());
            }
        } else {
            tracing::debug!("ignored metadata {}/{}", item.type_code, item.code);
        }
    }
    let mut s = status.lock().unwrap();
    s.clock_session = session;
    s.metadata.set_from_track(meta);
    let playing = s.airplay.connected;
    s.metadata.refresh_position(playing);
    s.airplay.source = meta.source.clone();
    s.airplay.encoding = metadata::stream_encoding(
        meta.stream_type.as_deref(),
        meta.stream_description.as_deref(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgcMode;
    use crate::web::{AirplayStatus, RuntimeStatus};
    use std::io::Cursor;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn status() -> SharedStatus {
        Arc::new(Mutex::new(RuntimeStatus {
            airplay: AirplayStatus::default(),
            ..Default::default()
        }))
    }

    #[test]
    fn apply_cli_overrides_and_skips_empty() {
        let cli = Cli::try_parse_from([
            "wledplay",
            "--multicast",
            "239.1.2.3",
            "--port",
            "12000",
            "--pcm-rate",
            "48000",
            "--pcm-channels",
            "1",
            "--pcm-format",
            "s32le",
            "--destination",
            "10.1.2.3",
            "--http-interface",
            "eth0",
            "--airplay-interface",
            "eth1",
            "--unicast-interface",
            "eth2",
            "--multicast-interface",
            "eth3",
            "--metadata-pipe",
            "/tmp/meta",
        ])
        .unwrap();
        let mut cfg = Config::default();
        apply_cli(&mut cfg, &cli);
        assert_eq!(cfg.wled.multicast_address, "239.1.2.3");
        assert_eq!(cfg.wled.port, 12000);
        assert_eq!(cfg.pcm.rate, 48000);
        assert_eq!(cfg.pcm.channels, 1);
        assert_eq!(cfg.pcm.format, "s32le");
        assert_eq!(cfg.wled.destinations, vec!["10.1.2.3"]);
        assert_eq!(cfg.network.http, "eth0");
        assert_eq!(cfg.network.airplay, "eth1");
        assert_eq!(cfg.network.unicast, "eth2");
        assert_eq!(cfg.network.multicast, "eth3");
        assert_eq!(cfg.metadata.pipe, "/tmp/meta");

        let skip = Cli::try_parse_from([
            "wledplay",
            "--http-interface",
            "",
            "--airplay-interface",
            "",
            "--unicast-interface",
            "",
            "--multicast-interface",
            "",
        ])
        .unwrap();
        let before = cfg.network.clone();
        apply_cli(&mut cfg, &skip);
        assert_eq!(cfg.network.http, before.http);
    }

    #[test]
    fn pace_and_input_helpers() {
        let realtime = Cli::try_parse_from(["wledplay"]).unwrap();
        assert!(pace_realtime(&realtime));
        let live = Cli::try_parse_from(["wledplay", "--no-realtime"]).unwrap();
        assert!(!pace_realtime(&live));
        assert!(pcm_input_path("stdin").is_none());
        assert_eq!(
            pcm_input_path("/tmp/a.pcm"),
            Some(PathBuf::from("/tmp/a.pcm"))
        );
        assert!(finite_pcm_input(Some(&PathBuf::from("/tmp/a.pcm")), false));
        assert!(finite_pcm_input(None, true));
        assert!(!finite_pcm_input(None, false));
        #[cfg(unix)]
        {
            let dir = tempfile::tempdir().unwrap();
            let fifo = dir.path().join("a.fifo");
            let cstr = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(cstr.as_ptr(), 0o644) }, 0);
            assert!(!finite_pcm_input(Some(&fifo), true));
        }
        let now = Instant::now();
        assert_eq!(instant_or_now(Some(now)), now);
        let fallback = instant_or_now(None);
        assert!(fallback.elapsed() < Duration::from_secs(1));
        assert!(should_log_skip(
            instant_or_now(now.checked_sub(Duration::from_secs(3))),
            now,
            Duration::from_secs(2)
        ));
        assert!(!should_log_skip(now, now, Duration::from_secs(2)));
        assert_eq!(
            pace_wait(Duration::from_millis(10), Duration::from_millis(3)),
            Some(Duration::from_millis(7))
        );
        assert!(pace_wait(Duration::from_millis(3), Duration::from_millis(10)).is_none());
        let bind = InterfaceBind {
            name: "eth0".into(),
            ipv4: std::net::Ipv4Addr::new(10, 0, 0, 1),
        };
        assert_eq!(socket_bind(Some(&bind)).map(|(n, _)| n), Some("eth0"));
        assert!(socket_bind(None).is_none());
        log_web_error("boom");
    }

    #[test]
    fn log_startup_covers_unicast_and_invalid_destinations() {
        let mut cfg = Config::default();
        cfg.pcm.channels = 1;
        log_startup(&cfg, true);
        cfg.pcm.channels = 2;
        cfg.wled.destinations = vec!["10.0.0.1".into()];
        log_startup(&cfg, false);
        cfg.wled.destinations = vec!["not-an-ip".into()];
        log_startup(&cfg, false);
    }

    #[test]
    fn read_pcm_persistent_reopens_after_eof() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct ReconnectOnce {
            reads: AtomicUsize,
            sample: [u8; 2],
        }
        impl io::Read for ReconnectOnce {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                match self.reads.fetch_add(1, Ordering::SeqCst) {
                    0 => {
                        buf[..2].copy_from_slice(&self.sample);
                        Ok(2)
                    }
                    1 => Ok(0),
                    2 => {
                        buf[..2].copy_from_slice(&self.sample);
                        Ok(2)
                    }
                    _ => Err(io::Error::new(io::ErrorKind::Interrupted, "done")),
                }
            }
        }

        let rt = Arc::new(Mutex::new(PcmRuntime::new(AgcMode::Off)));
        let status = status();
        let pcm = PcmFormat {
            rate: 22050,
            channels: 1,
            format: "s16le".into(),
        };
        read_pcm(
            &mut ReconnectOnce {
                reads: AtomicUsize::new(0),
                sample: 1000i16.to_le_bytes(),
            },
            true,
            &|| 0,
            rt.clone(),
            status,
            pcm,
            false,
            false,
        );
        let guard = rt.lock().unwrap();
        assert_eq!(guard.connections, 2);
        assert!(guard.stdin_closed);
    }

    #[test]
    fn read_pcm_from_cursor_connects_and_ends() {
        let rt = Arc::new(Mutex::new(PcmRuntime::new(AgcMode::Off)));
        let status = status();
        let pcm = PcmFormat {
            rate: 22050,
            channels: 1,
            format: "s16le".into(),
        };
        let mut bytes = Vec::new();
        for _ in 0..3000 {
            bytes.extend_from_slice(&1000i16.to_le_bytes());
        }
        read_pcm(
            &mut Cursor::new(bytes),
            false,
            &|| 0,
            rt.clone(),
            status,
            pcm,
            false,
            false,
        );
        let guard = rt.lock().unwrap();
        assert!(guard.pcm_frames > 0);
        assert!(guard.stdin_closed);
        assert!(!guard.stream_active);
    }

    #[test]
    fn read_pcm_reopenable_fifo_returns_on_eof() {
        let rt = Arc::new(Mutex::new(PcmRuntime::new(AgcMode::Off)));
        let status = status();
        let pcm = PcmFormat {
            rate: 22050,
            channels: 1,
            format: "s16le".into(),
        };
        let again = read_pcm(
            &mut Cursor::new(1000i16.to_le_bytes().to_vec()),
            false,
            &|| 0,
            rt.clone(),
            status,
            pcm,
            false,
            true,
        );
        assert!(again);
        let guard = rt.lock().unwrap();
        assert!(!guard.stdin_closed);
        assert_eq!(guard.connections, 1);
        assert!(!guard.stream_active);
    }

    struct EofThenClose<R: Read> {
        inner: R,
        eofs: u8,
    }
    impl<R: Read> Read for EofThenClose<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.inner.read(buf)?;
            if n > 0 {
                return Ok(n);
            }
            self.eofs = self.eofs.saturating_add(1);
            if self.eofs >= 2 {
                Err(io::Error::other("pipe closed"))
            } else {
                Ok(0)
            }
        }
    }

    #[test]
    fn read_pcm_discards_backlog_and_handles_errors() {
        let rt = Arc::new(Mutex::new(PcmRuntime::new(AgcMode::Off)));
        let status = status();
        let pcm = PcmFormat::default();
        read_pcm(
            &mut EofThenClose {
                inner: Cursor::new(vec![0u8; 16]),
                eofs: 0,
            },
            true,
            &|| 50_000,
            rt.clone(),
            status.clone(),
            pcm.clone(),
            false,
            false,
        );
        assert!(rt.lock().unwrap().analysis_overruns >= 1);

        let rt = Arc::new(Mutex::new(PcmRuntime::new(AgcMode::Off)));
        read_pcm(
            &mut EofThenClose {
                inner: Cursor::new(vec![0u8; 80_000]),
                eofs: 0,
            },
            true,
            &|| 80_000,
            rt.clone(),
            status.clone(),
            PcmFormat::default(),
            false,
            false,
        );
        assert!(rt.lock().unwrap().analysis_overruns >= 1);

        struct Boom;
        impl Read for Boom {
            fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("fail"))
            }
        }
        let rt = Arc::new(Mutex::new(PcmRuntime::new(AgcMode::Off)));
        read_pcm(
            &mut Boom,
            false,
            &|| 0,
            rt.clone(),
            status,
            pcm,
            false,
            false,
        );
        assert!(rt.lock().unwrap().stdin_closed);
    }

    #[test]
    fn apply_metadata_line_updates_status() {
        let status = status();
        let artwork = Arc::new(Mutex::new(None));
        let mut meta = TrackMetadata::default();
        let mut raw = String::new();
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"Hey Jude");
        let line = format!(
            "<item><type>636f7265</type><code>6d696e6d</code><data encoding=\"base64\">{b64}</data></item>\n"
        );
        apply_metadata_line(&mut raw, &line, &mut meta, &artwork, &status);
        assert_eq!(
            status.lock().unwrap().metadata.title.as_deref(),
            Some("Hey Jude")
        );

        let clip = format!(
            "<item><type>73736e63</type><code>636c6970</code><data encoding=\"base64\">{}</data></item>\n",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"iPhone")
        );
        apply_metadata_line(&mut raw, &clip, &mut meta, &artwork, &status);
        assert_eq!(
            status.lock().unwrap().airplay.source.as_deref(),
            Some("iPhone")
        );

        let asar = format!(
            "<item><type>636f7265</type><code>61736172</code><data encoding=\"base64\">{}</data></item>\n",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"Artist")
        );
        apply_metadata_line(&mut raw, &asar, &mut meta, &artwork, &status);
        let empty_clip = "<item><type>73736e63</type><code>636c6970</code></item>\n";
        apply_metadata_line(&mut raw, empty_clip, &mut meta, &artwork, &status);
        apply_metadata_line(
            &mut raw,
            "<item><type>636f7265</type><code>61626364</code></item>\n",
            &mut meta,
            &artwork,
            &status,
        );
        let mut empty_meta = TrackMetadata::default();
        let empty_minm = "<item><type>636f7265</type><code>6d696e6d</code><data encoding=\"base64\"></data></item>\n";
        apply_metadata_line(
            &mut String::new(),
            empty_minm,
            &mut empty_meta,
            &artwork,
            &status,
        );
        let prgr = format!(
            "<item><type>73736e63</type><code>70726772</code><data encoding=\"base64\">{}</data></item>\n",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"0/22050/44100")
        );
        apply_metadata_line(&mut raw, &prgr, &mut meta, &artwork, &status);
        {
            let st = status.lock().unwrap();
            assert!((st.metadata.position_s.unwrap() - 0.5).abs() < 1e-3);
            assert!((st.metadata.duration_s.unwrap() - 1.0).abs() < 1e-3);
        }
        let lead = format!(
            "<item><type>73736e63</type><code>6c656164</code><data encoding=\"base64\">{}</data></item>\n",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"-9700000000")
        );
        apply_metadata_line(&mut raw, &lead, &mut meta, &artwork, &status);
        assert!(status.lock().unwrap().clock_session.lead_ms.unwrap() < -9_000.0);
        let styp = format!(
            "<item><type>73736e63</type><code>73747970</code><data encoding=\"base64\">{}</data></item>\n",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"ALAC")
        );
        let sdsc = format!(
            "<item><type>73736e63</type><code>73647363</code><data encoding=\"base64\">{}</data></item>\n",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"44100/16/2")
        );
        apply_metadata_line(&mut raw, &styp, &mut meta, &artwork, &status);
        apply_metadata_line(&mut raw, &sdsc, &mut meta, &artwork, &status);
        assert_eq!(status.lock().unwrap().airplay.encoding, "ALAC · 44100/16/2");
        let pend = "<item><type>73736e63</type><code>70656e64</code></item>\n";
        apply_metadata_line(&mut raw, pend, &mut meta, &artwork, &status);
        assert!(status.lock().unwrap().clock_session.lead_ms.is_none());
        assert!(status.lock().unwrap().airplay.encoding.is_empty());
    }

    #[test]
    fn consume_metadata_reader_handles_eof_and_error() {
        let status = status();
        let artwork = Arc::new(Mutex::new(None));
        let xml = "<item><type>636f7265</type><code>61736172</code><data encoding=\"base64\">QQ==</data></item>\n";
        consume_metadata_reader(&mut Cursor::new(xml), &status, &artwork);
        assert_eq!(status.lock().unwrap().metadata.artist.as_deref(), Some("A"));

        struct Boom;
        impl Read for Boom {
            fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("fail"))
            }
        }
        impl BufRead for Boom {
            fn fill_buf(&mut self) -> io::Result<&[u8]> {
                Err(io::Error::other("fail"))
            }
            fn consume(&mut self, _amt: usize) {}
        }
        let mut boom = Boom;
        let _ = boom.read(&mut [0u8; 1]);
        boom.consume(0);
        consume_metadata_reader(&mut boom, &status, &artwork);
    }

    #[test]
    fn read_metadata_pipe_missing_file_stops_when_limited() {
        let status = status();
        let artwork = Arc::new(Mutex::new(None));
        read_metadata_pipe("/no/such/wledplay-meta", &status, &artwork, Some(2));
        assert_eq!(metadata_retry_delay(Some(1)), Duration::ZERO);
        assert_eq!(metadata_retry_delay(None), Duration::from_secs(2));
    }

    #[test]
    fn spawn_pcm_reader_missing_file_marks_closed() {
        let rt = Arc::new(Mutex::new(PcmRuntime::new(AgcMode::Off)));
        let status = status();
        spawn_pcm_reader(
            rt.clone(),
            status,
            PcmFormat::default(),
            false,
            Some(PathBuf::from("/no/such/wledplay.pcm")),
        );
        for _ in 0..50 {
            if rt.lock().unwrap().stdin_closed {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert!(rt.lock().unwrap().stdin_closed);
    }

    #[test]
    fn spawn_pcm_reader_reads_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.pcm");
        std::fs::write(&path, [0u8; 64]).unwrap();
        let rt = Arc::new(Mutex::new(PcmRuntime::new(AgcMode::Off)));
        let status = status();
        spawn_pcm_reader(rt.clone(), status, PcmFormat::default(), false, Some(path));
        for _ in 0..50 {
            if rt.lock().unwrap().stdin_closed {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert!(rt.lock().unwrap().stdin_closed);
    }

    #[test]
    fn read_pcm_paces_when_realtime() {
        let rt = Arc::new(Mutex::new(PcmRuntime::new(AgcMode::Off)));
        let status = status();
        let pcm = PcmFormat {
            rate: 8_000,
            channels: 1,
            format: "s16le".into(),
        };
        let mut bytes = Vec::new();
        for _ in 0..16 {
            bytes.extend_from_slice(&1i16.to_le_bytes());
        }
        let start = Instant::now();
        read_pcm(
            &mut Cursor::new(bytes),
            false,
            &|| 0,
            rt,
            status,
            pcm,
            true,
            false,
        );
        assert!(start.elapsed() >= Duration::from_millis(1));
    }

    #[test]
    fn read_pcm_realtime_skips_sleep_when_already_on_time() {
        let rt = Arc::new(Mutex::new(PcmRuntime::new(AgcMode::Off)));
        let status = status();
        let pcm = PcmFormat {
            rate: 192_000,
            channels: 2,
            format: "s32le".into(),
        };
        let bytes = vec![0u8; 8];
        read_pcm(
            &mut Cursor::new(bytes),
            false,
            &|| 0,
            rt.clone(),
            status,
            pcm,
            true,
            false,
        );
        assert!(rt.lock().unwrap().stdin_closed);
    }

    #[test]
    fn apply_cli_skips_empty_metadata_pipe() {
        let mut cli = Cli::try_parse_from(["wledplay"]).unwrap();
        cli.metadata_pipe = Some(PathBuf::new());
        let mut cfg = Config::default();
        cfg.metadata.pipe = "keep".into();
        apply_cli(&mut cfg, &cli);
        assert_eq!(cfg.metadata.pipe, "keep");
    }

    #[test]
    fn spawn_metadata_reader_starts_thread() {
        let status = status();
        let artwork = Arc::new(Mutex::new(None));
        spawn_metadata_reader("/no/such/wledplay-meta".into(), status, artwork);
        thread::sleep(Duration::from_millis(5));
    }

    #[tokio::test]
    async fn run_with_cli_reads_pcm_file_and_exits() {
        let dir = tempfile::tempdir().unwrap();
        let pcm = dir.path().join("t.pcm");
        let mut bytes = Vec::new();
        for _ in 0..1024 {
            bytes.extend_from_slice(&0i16.to_le_bytes());
        }
        std::fs::write(&pcm, &bytes).unwrap();

        let cli = Cli::try_parse_from([
            "wledplay",
            "--no-web",
            "--no-realtime",
            "--input",
            pcm.to_str().unwrap(),
            "--destination",
            "127.0.0.1",
            "--pcm-rate",
            "22050",
            "--pcm-channels",
            "1",
        ])
        .unwrap();
        run_with_cli(cli).await.unwrap();
    }

    #[tokio::test]
    async fn run_with_cli_rejects_unknown_nic() {
        for (flag, needle) in [
            ("--http-interface", "HTTP"),
            ("--airplay-interface", "AirPlay/mDNS"),
            ("--unicast-interface", "WLED unicast"),
            ("--multicast-interface", "WLED multicast"),
        ] {
            let err = run_with_cli(
                Cli::try_parse_from(["wledplay", "--no-web", flag, "wledplay-no-such-nic"])
                    .unwrap(),
            )
            .await
            .unwrap_err();
            assert!(err.to_string().contains(needle));
        }
    }

    #[tokio::test]
    async fn run_with_cli_covers_startup_and_web_error() {
        let dir = tempfile::tempdir().unwrap();
        let pcm = dir.path().join("t.pcm");
        std::fs::write(&pcm, [0u8; 32]).unwrap();
        let cfg_path = dir.path().join("cfg.yaml");
        std::fs::write(
            &cfg_path,
            "wled:\n  destinations: [\"not-an-ip\"]\nweb:\n  enabled: true\n  listen: 127.0.0.1\n  port: 1\naudio:\n  update_hz: 0\n",
        )
        .unwrap();
        let cli = Cli::try_parse_from([
            "wledplay",
            "--config",
            cfg_path.to_str().unwrap(),
            "--no-realtime",
            "--input",
            pcm.to_str().unwrap(),
            "--pcm-channels",
            "2",
        ])
        .unwrap();
        let _ = run_with_cli(cli).await;
    }

    #[test]
    fn read_metadata_pipe_reads_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta");
        let xml = "<item><type>636f7265</type><code>6d696e6d</code><data encoding=\"base64\">QQ==</data></item>\n";
        std::fs::write(&path, xml).unwrap();
        let status = status();
        let artwork = Arc::new(Mutex::new(None));
        read_metadata_pipe(path.to_str().unwrap(), &status, &artwork, Some(1));
        assert_eq!(status.lock().unwrap().metadata.title.as_deref(), Some("A"));
    }

    #[test]
    fn event_loop_tick_restart_send_and_status() {
        let cfg = Config::default();
        let runtime = Mutex::new(PcmRuntime::new(AgcMode::Off));
        let live_cfg = Mutex::new(cfg.clone());
        let sender = Mutex::new(
            WledSender::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::new(239, 0, 0, 1)),
                11988,
                vec!["127.0.0.1:9".parse().unwrap()],
                None,
                None,
            )
            .unwrap(),
        );
        sender.lock().unwrap().set_multicast_enabled(false);
        let status = status();
        let restart = AtomicBool::new(true);
        let mut loop_state = SendLoop::default();
        let now = Instant::now();
        let mut last_send = now;
        let mut last_status = now;
        let mut packets = 0u64;
        let hub = crate::auto_join::AutoJoinHub::inactive();
        assert_eq!(
            event_loop_tick(
                &restart,
                &runtime,
                &live_cfg,
                &sender,
                &status,
                &mut loop_state,
                &mut last_send,
                &mut last_status,
                &mut packets,
                Duration::from_millis(20),
                true,
                now,
                &hub,
            ),
            LoopAction::Exit
        );

        restart.store(false, Ordering::Relaxed);
        runtime.lock().unwrap().stdin_closed = true;
        let mut last_send = now - Duration::from_millis(50);
        assert_eq!(
            event_loop_tick(
                &restart,
                &runtime,
                &live_cfg,
                &sender,
                &status,
                &mut loop_state,
                &mut last_send,
                &mut last_status,
                &mut packets,
                Duration::from_millis(20),
                true,
                now,
                &hub,
            ),
            LoopAction::Exit
        );

        runtime.lock().unwrap().stdin_closed = false;
        runtime.lock().unwrap().stream_active = true;
        runtime.lock().unwrap().last_pcm = Some(now);
        let mut loop_state = SendLoop::default();
        crate::runtime::enqueue_packet(
            &mut loop_state.delay_q,
            crate::wled::AudioSyncPacket::silence(),
            0,
            now,
        );
        let mut last_send = now;
        let mut last_status = now - Duration::from_secs(2);
        let mut packets = 0;
        assert_eq!(
            event_loop_tick(
                &restart,
                &runtime,
                &live_cfg,
                &sender,
                &status,
                &mut loop_state,
                &mut last_send,
                &mut last_status,
                &mut packets,
                Duration::from_millis(20),
                false,
                now,
                &hub,
            ),
            LoopAction::Continue
        );
        assert!(sender.lock().unwrap().packets_sent() >= 1);
        assert!(!status.lock().unwrap().audio.tx_connected);

        let mut live = crate::wled::AudioSyncPacket::silence();
        live.sample_smth = 90.0;
        live.fft_result[1] = 40;
        crate::runtime::enqueue_packet(&mut loop_state.delay_q, live, 0, now);

        let mut last_send = now;
        let mut last_status = now;
        assert_eq!(
            event_loop_tick(
                &restart,
                &runtime,
                &live_cfg,
                &sender,
                &status,
                &mut loop_state,
                &mut last_send,
                &mut last_status,
                &mut packets,
                Duration::from_secs(60),
                false,
                now,
                &hub,
            ),
            LoopAction::Continue
        );
        assert!(status.lock().unwrap().audio.tx_connected);
        assert_eq!(status.lock().unwrap().audio.tx_bands[1], 40);

        let mut last_send = now - Duration::from_millis(50);
        let mut last_status = now;
        runtime.lock().unwrap().stream_active = true;
        runtime.lock().unwrap().last_pcm = Some(now);
        assert_eq!(
            event_loop_tick(
                &restart,
                &runtime,
                &live_cfg,
                &sender,
                &status,
                &mut loop_state,
                &mut last_send,
                &mut last_status,
                &mut packets,
                Duration::from_millis(20),
                false,
                now,
                &hub,
            ),
            LoopAction::Continue
        );
    }

    #[test]
    fn spawn_pcm_reader_stdin_sees_eof() {
        let rt = Arc::new(Mutex::new(PcmRuntime::new(AgcMode::Off)));
        let status = status();
        spawn_pcm_reader(rt.clone(), status, PcmFormat::default(), false, None);
        thread::sleep(Duration::from_millis(20));
    }

    #[cfg(unix)]
    #[test]
    fn spawn_pcm_reader_fifo_opens_without_busy_spin() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pcm.fifo");
        let cstr = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cstr.as_ptr(), 0o644) }, 0);
        let rt = Arc::new(Mutex::new(PcmRuntime::new(AgcMode::Off)));
        spawn_pcm_reader(
            rt.clone(),
            status(),
            PcmFormat::default(),
            false,
            Some(fifo),
        );
        thread::sleep(Duration::from_millis(20));
        assert!(!rt.lock().unwrap().stdin_closed);
    }

    #[tokio::test]
    async fn run_with_cli_metadata_pipe() {
        let dir = tempfile::tempdir().unwrap();
        let pcm = dir.path().join("t.pcm");
        std::fs::write(&pcm, [0u8; 16]).unwrap();
        let meta = dir.path().join("meta");
        std::fs::write(&meta, "").unwrap();
        let cli = Cli::try_parse_from([
            "wledplay",
            "--no-web",
            "--no-realtime",
            "--input",
            pcm.to_str().unwrap(),
            "--metadata-pipe",
            meta.to_str().unwrap(),
            "--pcm-channels",
            "1",
        ])
        .unwrap();
        run_with_cli(cli).await.unwrap();
    }

    #[test]
    fn log_pcm_input_covers_both_branches() {
        log_pcm_input(None);
        log_pcm_input(Some(&PathBuf::from("/tmp/x.pcm")));
        log_pcm_skip(80, 8000);
        log_track(&TrackMetadata::default());
        log_clip(None);
        log_clip(Some("iPhone"));
    }
}
