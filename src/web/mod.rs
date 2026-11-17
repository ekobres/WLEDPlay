mod operator;

pub use operator::{
    action_buttons_enabled, dest_lines, operator_hint, operator_ui, parse_send_delay_ms,
    parse_shairport_s, should_overwrite_fields, OperatorSnapshot,
};

use crate::clock::{self, ClockSession, ClockStatus};
use crate::config::{delay_ms_bounds, merge_controllers, WledSight};
use crate::control::{ConfigPatch, Control};
use crate::metadata::{self, SharedArtwork, TrackMetadata};
use crate::netif::InterfaceInfo;
use crate::wled::{measure_delays, MeasureInput};
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[derive(Clone)]
pub struct AppState {
    pub status: SharedStatus,
    pub artwork: SharedArtwork,
    pub control: Option<Control>,
    pub rescan: Arc<AtomicBool>,
    pub wled_rescan: Arc<AtomicBool>,
    pub wled_seen: Arc<Mutex<Vec<WledSight>>>,
    pub log_dir: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AirPlayPlayer {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub flags: String,
    #[serde(default)]
    pub igl: String,
    #[serde(default)]
    pub gcgl: String,
    #[serde(default)]
    pub target: bool,
    #[serde(default)]
    pub leader: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AirPlaySession {
    #[serde(default)]
    pub target_name: String,
    #[serde(default)]
    pub flags: String,
    #[serde(default)]
    pub igl: String,
    #[serde(default)]
    pub gcgl: String,
    #[serde(default)]
    pub gid: String,
    #[serde(default)]
    pub leader: String,
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default)]
    pub players: Vec<AirPlayPlayer>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RuntimeStatus {
    pub version: String,
    pub airplay: AirplayStatus,
    pub audio: AudioStatus,
    pub wled: WledStatus,
    pub metadata: TrackMetadataStatus,
    pub counters: CounterStatus,
    pub auto_join: AutoJoinStatus,
    pub session: AirPlaySession,
    pub airplay_devices: Vec<AirPlayDeviceInfo>,
    #[serde(skip)]
    pub clock_session: ClockSession,
    pub clock: ClockStatus,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AirplayStatus {
    pub connected: bool,
    pub receiver_name: String,
    pub connections: u64,
    pub stream_duration_s: f32,
    pub source: Option<String>,
    pub encoding: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AudioStatus {
    pub sample_rate: u32,
    pub channels: u16,
    pub rms: f32,
    pub peak: bool,
    pub dominant_frequency: f32,
    pub bands: [u8; 16],
    pub sync_offset_ms: i32,
    pub update_hz: u32,
    /// Last UDP Audio Sync packet actually sent (after send delay).
    pub tx_connected: bool,
    pub tx_rms: f32,
    pub tx_peak: bool,
    pub tx_dominant_frequency: f32,
    pub tx_bands: [u8; 16],
    /// WLED `multAgc` currently applied to samples/FFT (1.0 when AGC is off).
    pub agc_gain: f32,
    pub agc_on: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct WledStatus {
    pub packets_per_second: f32,
    pub destination: String,
    pub destinations: Vec<String>,
    pub multicast_enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TrackMetadataStatus {
    pub artist: Option<String>,
    pub album: Option<String>,
    pub title: Option<String>,
    pub has_artwork: bool,
    /// Bumps when the cover bytes change, so the UI can reload `/artwork`.
    pub artwork_rev: u64,
    pub position_s: Option<f32>,
    pub duration_s: Option<f32>,
    #[serde(skip)]
    pub position_base_s: Option<f32>,
    #[serde(skip)]
    pub progress_unix_ms: Option<u64>,
}

impl TrackMetadataStatus {
    fn has_track(&self) -> bool {
        self.title.is_some() || self.artist.is_some() || self.album.is_some()
    }

    pub fn set_from_track(&mut self, meta: &TrackMetadata) {
        let same_track =
            self.title == meta.title && self.artist == meta.artist && self.album == meta.album;
        self.artist = meta.artist.clone();
        self.album = meta.album.clone();
        self.title = meta.title.clone();
        self.has_artwork = meta.has_artwork;
        self.duration_s = meta.duration_s();
        // AirPlay 2 puts elapsed time in the now-playing plist and does not
        // write it to the metadata pipe. A later item (lead, artwork) must
        // not wipe a clock that is already running for this track.
        if let Some(pos) = meta.position_s() {
            self.position_base_s = Some(pos);
            self.progress_unix_ms = meta.progress_unix_ms;
        } else if !same_track {
            self.position_s = None;
            self.position_base_s = None;
            self.progress_unix_ms = None;
        }
    }

    pub fn refresh_position(&mut self, playing: bool) {
        if playing {
            if self.position_base_s.is_none() && self.has_track() {
                self.position_base_s = Some(0.0);
                self.progress_unix_ms = Some(metadata::unix_ms_now());
            } else if self.progress_unix_ms.is_none() && self.position_base_s.is_some() {
                self.progress_unix_ms = Some(metadata::unix_ms_now());
            }
            self.position_s = metadata::interpolated_position(
                self.position_base_s,
                self.progress_unix_ms,
                self.duration_s,
            );
            return;
        }
        if self.progress_unix_ms.is_some() {
            if let Some(pos) = metadata::interpolated_position(
                self.position_base_s,
                self.progress_unix_ms,
                self.duration_s,
            ) {
                self.position_s = Some(pos);
                self.position_base_s = Some(pos);
            }
            self.progress_unix_ms = None;
        } else if self.position_s.is_none() {
            self.position_s = self.position_base_s;
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CounterStatus {
    pub pcm_frames: u64,
    pub fft_frames: u64,
    pub packets_sent: u64,
    pub udp_errors: u64,
    pub pcm_underruns: u64,
    pub analysis_overruns: u64,
    pub airplay_connections: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct AutoJoinStatus {
    pub enabled: bool,
    pub target_id: String,
    pub target_name: String,
    pub state: String,
    pub label: String,
    pub target_available: bool,
    pub target_playing: bool,
    pub target_status: String,
    pub wledplay_busy: bool,
    pub pending_config_change: bool,
    pub discovery_active: bool,
    pub device_count: u32,
    pub last_error: String,
    pub self_id: String,
    pub discovery_error: String,
    pub snapshot_at: String,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub struct AirPlayDeviceInfo {
    pub id: String,
    pub name: String,
    pub label: String,
    pub model: String,
    pub address: String,
    pub available: bool,
    pub playing: bool,
}

pub type SharedStatus = Arc<Mutex<RuntimeStatus>>;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(ui))
        .route("/health", get(health))
        .route("/status", get(status_handler))
        .route("/levels", get(levels_handler))
        .route("/artwork", get(artwork_handler))
        .route("/config", get(config_get).patch(config_patch))
        .route("/interfaces", get(interfaces_handler))
        .route("/airplay/devices", get(devices_handler))
        .route("/airplay/rescan", post(rescan_handler))
        .route("/wled/controllers", get(wled_controllers))
        .route("/wled/rescan", post(wled_rescan_handler))
        .route("/wled/measure", post(wled_measure))
        .route("/logs", get(logs_handler))
        .route("/logs/download", get(logs_download))
        .route("/restart", post(restart_handler))
        .with_state(state)
}

pub fn bind_http(
    listen: &str,
    port: u16,
) -> Result<TcpListener, Box<dyn std::error::Error + Send + Sync>> {
    let addr: SocketAddr = format!("{listen}:{port}").parse()?;
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(128)?;
    Ok(TcpListener::from_std(socket.into())?)
}

pub fn map_serve_io(
    result: std::io::Result<()>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Ok(result?)
}

pub async fn serve_on_until(
    listener: TcpListener,
    state: AppState,
    shutdown: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = listener.local_addr()?;
    tracing::info!("HTTP status listening on http://{addr}");
    map_serve_io(
        axum::serve(listener, router(state))
            .with_graceful_shutdown(shutdown)
            .await,
    )
}

pub async fn serve(
    listen: &str,
    port: u16,
    state: AppState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    serve_on_until(
        bind_http(listen, port)?,
        state,
        Box::pin(std::future::pending()),
    )
    .await
}

async fn ui() -> Html<&'static str> {
    Html(operator_ui())
}

async fn health() -> &'static str {
    "ok"
}

async fn status_handler(State(state): State<AppState>) -> Json<RuntimeStatus> {
    let (session, connected) = {
        let s = state.status.lock().unwrap();
        (s.clock_session.clone(), s.airplay.connected)
    };
    let clock = clock::read_from_dir(std::path::Path::new("/dev/shm"), clock::monotonic_raw_ns())
        .with_playback(&session, connected);
    let s = {
        let mut s = state.status.lock().unwrap();
        s.clock = clock;
        let playing = s.airplay.connected;
        s.metadata.refresh_position(playing);
        s.clone()
    };
    Json(s)
}

/// Slim 50 Hz payload: live meters only (no metadata / counters / destinations).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LiveLevels {
    pub connected: bool,
    pub rms: f32,
    pub peak: bool,
    pub dominant_frequency: f32,
    pub bands: [u8; 16],
    pub update_hz: u32,
    pub tx_connected: bool,
    pub tx_rms: f32,
    pub tx_peak: bool,
    pub tx_dominant_frequency: f32,
    pub tx_bands: [u8; 16],
    pub agc_gain: f32,
    pub agc_on: bool,
}

pub fn live_levels(status: &RuntimeStatus) -> LiveLevels {
    LiveLevels {
        connected: status.airplay.connected,
        rms: status.audio.rms,
        peak: status.audio.peak,
        dominant_frequency: status.audio.dominant_frequency,
        bands: status.audio.bands,
        update_hz: status.audio.update_hz,
        tx_connected: status.audio.tx_connected,
        tx_rms: status.audio.tx_rms,
        tx_peak: status.audio.tx_peak,
        tx_dominant_frequency: status.audio.tx_dominant_frequency,
        tx_bands: status.audio.tx_bands,
        agc_gain: status.audio.agc_gain,
        agc_on: status.audio.agc_on,
    }
}

async fn levels_handler(State(state): State<AppState>) -> Json<LiveLevels> {
    Json(live_levels(&state.status.lock().unwrap()))
}

async fn artwork_handler(State(state): State<AppState>) -> Response {
    let art = state.artwork.lock().unwrap();
    match art.as_ref() {
        Some(a) => (
            [
                (header::CONTENT_TYPE, a.content_type.clone()),
                (header::CACHE_CONTROL, "no-store".into()),
            ],
            a.bytes.clone(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn config_get(State(state): State<AppState>) -> Response {
    match &state.control {
        Some(ctrl) => Json(ctrl.view()).into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

async fn config_patch(State(state): State<AppState>, Json(patch): Json<ConfigPatch>) -> Response {
    let Some(ctrl) = &state.control else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let level_change = patch.log_level.is_some();
    match ctrl.apply_patch(patch) {
        Ok(view) => {
            if level_change {
                let level = ctrl.config.lock().unwrap().logging.level.clone();
                let _ = crate::logctl::reload(&level);
            }
            Json(view).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct LogParams {
    source: Option<String>,
    tail: Option<bool>,
    offset: Option<u64>,
    before: Option<u64>,
    q: Option<String>,
    limit: Option<usize>,
}

#[derive(Serialize)]
struct LogBody {
    #[serde(flatten)]
    page: crate::logs::LogPage,
    level: String,
}

fn current_log_level(state: &AppState) -> String {
    match &state.control {
        Some(ctrl) => ctrl.config.lock().unwrap().logging.level.clone(),
        None => "info".into(),
    }
}

fn log_request(params: &LogParams) -> crate::logs::LogRequest {
    crate::logs::LogRequest {
        source: params.source.clone().unwrap_or_else(|| "wledplay".into()),
        tail: params.tail.unwrap_or(true),
        offset: params.offset,
        before: params.before,
        query: params.q.clone().unwrap_or_default(),
        limit: params.limit.unwrap_or(200),
    }
}

fn log_disposition(name: &str) -> header::HeaderValue {
    match name {
        "shairport.log" => {
            header::HeaderValue::from_static("attachment; filename=\"shairport.log\"")
        }
        "nqptp.log" => header::HeaderValue::from_static("attachment; filename=\"nqptp.log\""),
        "console.log" => header::HeaderValue::from_static("attachment; filename=\"console.log\""),
        _ => header::HeaderValue::from_static("attachment; filename=\"wledplay.log\""),
    }
}

async fn logs_handler(State(state): State<AppState>, Query(params): Query<LogParams>) -> Response {
    match crate::logs::read_logs(&state.log_dir, &log_request(&params)) {
        Ok(page) => Json(LogBody {
            page,
            level: current_log_level(&state),
        })
        .into_response(),
        Err(crate::logs::LogReadError::BadSource) => (
            StatusCode::BAD_REQUEST,
            "log source must be wledplay, shairport, nqptp, or console",
        )
            .into_response(),
        Err(crate::logs::LogReadError::Io(err)) => {
            (StatusCode::INTERNAL_SERVER_ERROR, err).into_response()
        }
    }
}

async fn logs_download(State(state): State<AppState>, Query(params): Query<LogParams>) -> Response {
    let source = params.source.unwrap_or_else(|| "wledplay".into());
    match crate::logs::read_file(&state.log_dir, &source) {
        Ok((name, bytes)) => {
            let mut response = bytes.into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("text/plain; charset=utf-8"),
            );
            response
                .headers_mut()
                .insert(header::CONTENT_DISPOSITION, log_disposition(&name));
            response
        }
        Err(crate::logs::LogFileError::BadSource) => (
            StatusCode::BAD_REQUEST,
            "log source must be wledplay, shairport, nqptp, or console",
        )
            .into_response(),
        Err(crate::logs::LogFileError::Missing) => StatusCode::NOT_FOUND.into_response(),
        Err(crate::logs::LogFileError::Read(err)) => {
            (StatusCode::INTERNAL_SERVER_ERROR, err).into_response()
        }
    }
}

async fn interfaces_handler(State(state): State<AppState>) -> Json<Vec<InterfaceInfo>> {
    Json(match &state.control {
        Some(ctrl) => ctrl.view().interfaces,
        None => vec![],
    })
}

async fn devices_handler(State(state): State<AppState>) -> Json<serde_json::Value> {
    let s = state.status.lock().unwrap();
    Json(serde_json::json!({
        "discovery_active": s.auto_join.discovery_active,
        "self_id": s.auto_join.self_id,
        "devices": s.airplay_devices,
    }))
}

async fn rescan_handler(State(state): State<AppState>) -> StatusCode {
    state.rescan.store(true, Ordering::Relaxed);
    StatusCode::ACCEPTED
}

async fn wled_rescan_handler(State(state): State<AppState>) -> StatusCode {
    state.wled_rescan.store(true, Ordering::Relaxed);
    StatusCode::ACCEPTED
}

#[derive(Debug, Deserialize)]
struct MeasureRequest {
    #[serde(default)]
    controllers: Vec<MeasureInput>,
}

#[derive(Debug, Serialize)]
struct MeasureReport {
    results: Vec<crate::wled::DelayEstimate>,
}

async fn wled_measure(
    State(state): State<AppState>,
    payload: Result<Json<MeasureRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(json) => json,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let (min_delay_ms, max_delay_ms) = match &state.control {
        Some(control) => {
            let cfg = control.config.lock().expect("config");
            delay_ms_bounds(
                cfg.audio.sync_offset_ms,
                cfg.audio.shairport_latency_offset_s,
            )
        }
        None => (0, 0),
    };
    let controllers = request.controllers;
    let results = tokio::task::spawn_blocking(move || {
        measure_delays(&controllers, min_delay_ms, max_delay_ms)
    })
    .await
    .unwrap_or_default();
    Json(MeasureReport { results }).into_response()
}

async fn wled_controllers(State(state): State<AppState>) -> Json<crate::config::WledTable> {
    let seen = state.wled_seen.lock().unwrap().clone();
    let table = match &state.control {
        Some(control) => {
            let cfg = control.config.lock().unwrap().clone();
            merge_controllers(&cfg, &seen)
        }
        None => crate::config::WledTable {
            controllers: Vec::new(),
            dismissed: Vec::new(),
            min_delay_ms: 0,
            max_delay_ms: 0,
        },
    };
    Json(table)
}

async fn restart_handler(State(state): State<AppState>) -> StatusCode {
    if let Some(ctrl) = &state.control {
        ctrl.request_restart();
        StatusCode::ACCEPTED
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::control::Control;
    use crate::metadata::{unix_ms_now, Artwork, TrackMetadata};
    use crate::wled::WledSender;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::atomic::AtomicBool;
    use tower::ServiceExt;

    fn bare_state() -> AppState {
        AppState {
            status: Arc::new(Mutex::new(RuntimeStatus {
                version: "0.3.0".into(),
                ..Default::default()
            })),
            artwork: Arc::new(Mutex::new(None)),
            control: None,
            rescan: Arc::new(AtomicBool::new(false)),
            wled_rescan: Arc::new(AtomicBool::new(false)),
            wled_seen: Arc::new(Mutex::new(Vec::new())),
            log_dir: std::env::temp_dir(),
        }
    }

    fn control_state() -> (AppState, Arc<AtomicBool>) {
        let cfg = Config::default();
        let sender = WledSender::new(
            IpAddr::V4(Ipv4Addr::new(239, 0, 0, 1)),
            11988,
            vec![],
            None,
            None,
        )
        .unwrap();
        let restart = Arc::new(AtomicBool::new(false));
        let control = Control {
            config: Arc::new(Mutex::new(cfg.clone())),
            sender: Arc::new(Mutex::new(sender)),
            config_path: None,
            applied_name: cfg.airplay.name.clone(),
            applied_http: cfg.network.http.clone(),
            applied_airplay: cfg.network.airplay.clone(),
            applied_unicast: cfg.network.unicast.clone(),
            applied_multicast: cfg.network.multicast.clone(),
            applied_shairport: cfg.audio.shairport_latency_offset_s,
            applied_multicast_address: cfg.wled.multicast_address.clone(),
            applied_port: cfg.wled.port,
            restart: restart.clone(),
        };
        (
            AppState {
                status: Arc::new(Mutex::new(RuntimeStatus::default())),
                artwork: Arc::new(Mutex::new(None)),
                control: Some(control),
                rescan: Arc::new(AtomicBool::new(false)),
                wled_rescan: Arc::new(AtomicBool::new(false)),
                wled_seen: Arc::new(Mutex::new(Vec::new())),
                log_dir: std::env::temp_dir(),
            },
            restart,
        )
    }

    async fn send(app: Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
        let res = app.oneshot(req).await.unwrap();
        let status = res.status();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        (status, body.to_vec())
    }

    #[tokio::test]
    async fn health_and_ui() {
        let app = router(bare_state());
        let (status, body) = send(
            app.clone(),
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"ok");

        let (status, body) = send(
            router(bare_state()),
            Request::builder().uri("/").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains("WLEDPlay"));
        assert!(html.contains("id=\"viz-toggle\""));
        assert!(html.contains("id=\"viz-source\""));
        assert!(html.contains("id=\"viz-decay\""));
        assert!(html.contains("id=\"beat\""));
        assert!(html.contains("id=\"freq\""));
        assert!(html.contains("id=\"agc\""));
        assert!(html.contains("id=\"trackbar\""));
        assert!(html.contains("role=\"switch\""));
        assert!(html.contains("id=\"ctl-cancel\""));
        assert!(html.contains("id=\"ctl-gain\""));
        assert!(html.contains("id=\"ctl-autojoin\""));
        assert!(html.contains("function isDirty()"));
    }

    #[test]
    fn track_progress_interpolates_and_freezes() {
        let mut md = TrackMetadataStatus::default();
        md.refresh_position(false);
        assert!(md.position_s.is_none());

        let track = TrackMetadata {
            rtp_start: Some(0),
            rtp_current: Some(22050),
            rtp_end: Some(44100),
            duration_ms: Some(60_000),
            progress_unix_ms: None,
            ..TrackMetadata::default()
        };
        md.set_from_track(&track);
        md.refresh_position(true);
        assert!((md.position_s.unwrap() - 0.5).abs() < 0.05);
        assert!(md.progress_unix_ms.is_some());

        md.progress_unix_ms = Some(unix_ms_now().saturating_sub(2000));
        md.refresh_position(true);
        assert!((md.position_s.unwrap() - 2.5).abs() < 0.3);

        md.refresh_position(false);
        let frozen = md.position_s.unwrap();
        assert!(md.progress_unix_ms.is_none());
        md.refresh_position(false);
        assert!((md.position_s.unwrap() - frozen).abs() < 1e-6);

        md.position_s = None;
        md.refresh_position(false);
        assert!((md.position_s.unwrap() - md.position_base_s.unwrap()).abs() < 1e-6);

        md.position_base_s = None;
        md.progress_unix_ms = Some(unix_ms_now());
        md.refresh_position(false);
        assert!(md.progress_unix_ms.is_none());

        md.refresh_position(true);
        assert!(md.position_s.is_none());
    }

    #[test]
    fn track_progress_runs_when_airplay_omits_elapsed() {
        let mut md = TrackMetadataStatus::default();
        let mut track = TrackMetadata {
            title: Some("Song".into()),
            artist: Some("Artist".into()),
            duration_ms: Some(180_000),
            ..TrackMetadata::default()
        };
        md.set_from_track(&track);
        assert!(md.position_base_s.is_none());
        assert!((md.duration_s.unwrap() - 180.0).abs() < 1e-3);

        md.refresh_position(true);
        assert!(md.position_s.unwrap() < 0.05);
        let running = md.progress_unix_ms;
        assert!(running.is_some());

        md.progress_unix_ms = Some(unix_ms_now().saturating_sub(1500));
        md.refresh_position(true);
        assert!((md.position_s.unwrap() - 1.5).abs() < 0.3);

        track.album = Some("Album".into());
        md.set_from_track(&track);
        assert!(md.progress_unix_ms.is_none());
        assert!(md.position_base_s.is_none());

        md.refresh_position(true);
        md.progress_unix_ms = Some(unix_ms_now().saturating_sub(2000));
        let kept = md.progress_unix_ms;
        track.duration_ms = Some(200_000);
        md.set_from_track(&track);
        assert_eq!(md.progress_unix_ms, kept);
        assert!((md.duration_s.unwrap() - 200.0).abs() < 1e-3);
        md.refresh_position(true);
        assert!((md.position_s.unwrap() - 2.0).abs() < 0.3);

        track.rtp_start = Some(0);
        track.rtp_current = Some(44_100);
        track.rtp_end = Some(88_200);
        track.progress_unix_ms = Some(unix_ms_now());
        md.set_from_track(&track);
        md.refresh_position(true);
        assert!((md.position_s.unwrap() - 1.0).abs() < 0.05);
    }

    #[tokio::test]
    async fn status_and_artwork() {
        let state = bare_state();
        *state.artwork.lock().unwrap() = Some(Artwork {
            bytes: vec![1, 2, 3],
            content_type: "image/png".into(),
        });
        let (status, body) = send(
            router(state.clone()),
            Request::builder()
                .uri("/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(String::from_utf8_lossy(&body).contains("0.3.0"));
        assert!(String::from_utf8_lossy(&body).contains("auto_join"));
        assert!(String::from_utf8_lossy(&body).contains("\"clock\""));
        {
            let mut s = state.status.lock().unwrap();
            s.airplay.encoding = "ALAC · 44100/16/2".into();
            s.airplay.source = Some("10.0.0.8".into());
            s.session = AirPlaySession {
                target_name: "Kitchen".into(),
                flags: "0x198404".into(),
                igl: "1".into(),
                gcgl: "0".into(),
                gid: "group".into(),
                leader: "Kitchen".into(),
                members: vec!["Bar".into()],
                players: vec![AirPlayPlayer {
                    name: "Bar".into(),
                    flags: "0x20004".into(),
                    igl: "0".into(),
                    gcgl: "1".into(),
                    target: false,
                    leader: false,
                }],
            };
        }
        let (status, body) = send(
            router(state.clone()),
            Request::builder()
                .uri("/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let published = String::from_utf8_lossy(&body);
        assert!(published.contains("\"encoding\":\"ALAC · 44100/16/2\""));
        assert!(published.contains("\"target_name\":\"Kitchen\""));
        assert!(published.contains("\"leader\":\"Kitchen\""));
        assert!(published.contains("\"members\":[\"Bar\"]"));
        assert!(published.contains("\"name\":\"Bar\""));
        assert!(published.contains("\"flags\":\"0x20004\""));
        assert!(published.contains("\"gcgl\":\"1\""));

        state.status.lock().unwrap().audio.rms = 0.42;
        state.status.lock().unwrap().audio.bands[0] = 200;
        state.status.lock().unwrap().airplay.connected = true;
        state.status.lock().unwrap().audio.tx_rms = 0.25;
        state.status.lock().unwrap().audio.tx_peak = true;
        state.status.lock().unwrap().audio.tx_dominant_frequency = 880.0;
        state.status.lock().unwrap().audio.tx_bands[2] = 90;
        state.status.lock().unwrap().audio.tx_connected = true;
        state.status.lock().unwrap().audio.agc_gain = 2.25;
        state.status.lock().unwrap().audio.agc_on = true;
        {
            let mut s = state.status.lock().unwrap();
            s.metadata.position_base_s = Some(1.0);
            s.metadata.duration_s = Some(180.0);
            s.metadata.progress_unix_ms = Some(unix_ms_now().saturating_sub(1500));
        }
        let (status, body) = send(
            router(state.clone()),
            Request::builder()
                .uri("/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let live: serde_json::Value =
            serde_json::from_str(&String::from_utf8_lossy(&body)).unwrap();
        assert!(live["metadata"]["position_s"].as_f64().unwrap() > 2.0);
        assert!(!String::from_utf8_lossy(&body).contains("position_base_s"));
        let (status, body) = send(
            router(state.clone()),
            Request::builder()
                .uri("/levels")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let levels: LiveLevels = serde_json::from_str(&String::from_utf8_lossy(&body)).unwrap();
        assert!(levels.connected);
        assert!((levels.rms - 0.42).abs() < 1e-6);
        assert_eq!(levels.bands[0], 200);
        assert!(levels.tx_connected);
        assert!(levels.tx_peak);
        assert!((levels.tx_rms - 0.25).abs() < 1e-6);
        assert!((levels.tx_dominant_frequency - 880.0).abs() < 1e-6);
        assert_eq!(levels.tx_bands[2], 90);
        assert!(levels.agc_on);
        assert!((levels.agc_gain - 2.25).abs() < 1e-6);
        assert!(!String::from_utf8_lossy(&body).contains("receiver_name"));

        let (status, body) = send(
            router(state),
            Request::builder()
                .uri("/artwork")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, vec![1, 2, 3]);

        let (status, _) = send(
            router(bare_state()),
            Request::builder()
                .uri("/artwork")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn config_and_restart_without_control() {
        let app = router(bare_state());
        let (status, _) = send(
            app.clone(),
            Request::builder()
                .uri("/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

        let (status, _) = send(
            router(bare_state()),
            Request::builder()
                .method("PATCH")
                .uri("/config")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

        let (status, _) = send(
            router(bare_state()),
            Request::builder()
                .method("POST")
                .uri("/restart")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

        let (status, body) = send(
            router(bare_state()),
            Request::builder()
                .uri("/interfaces")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"[]");
    }

    #[tokio::test]
    async fn config_patch_and_restart_with_control() {
        let (state, restart) = control_state();
        let (status, body) = send(
            router(state.clone()),
            Request::builder()
                .uri("/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let cfg_json = String::from_utf8_lossy(&body);
        assert!(cfg_json.contains("WLEDPlay"));
        assert!(cfg_json.contains("\"gain\":\"normal\""));
        assert!(cfg_json.contains("\"gain\":\"live\""));

        let (status, _) = send(
            router(state.clone()),
            Request::builder()
                .method("PATCH")
                .uri("/config")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"sync_offset_ms":100}"#))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = send(
            router(state.clone()),
            Request::builder()
                .method("PATCH")
                .uri("/config")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"gain":"vivid"}"#))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(String::from_utf8_lossy(&body).contains("vivid"));

        let (status, body) = send(
            router(state.clone()),
            Request::builder()
                .method("PATCH")
                .uri("/config")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"auto_join_enabled":true,"auto_join_target_id":"speaker-1","auto_join_target_name":"Kitchen"}"#,
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let patched = String::from_utf8_lossy(&body);
        assert!(patched.contains("speaker-1"));
        assert!(patched.contains("\"auto_join_enabled\":\"live\""));

        state.status.lock().unwrap().auto_join.discovery_active = true;
        state.status.lock().unwrap().auto_join.self_id = "self".into();
        state
            .status
            .lock()
            .unwrap()
            .airplay_devices
            .push(crate::web::AirPlayDeviceInfo {
                id: "speaker-1".into(),
                name: "Kitchen".into(),
                label: "Kitchen".into(),
                model: "HomePod".into(),
                address: "10.0.0.9".into(),
                available: true,
                playing: false,
            });
        let (status, body) = send(
            router(state.clone()),
            Request::builder()
                .uri("/airplay/devices")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let devices = String::from_utf8_lossy(&body);
        assert!(devices.contains("speaker-1"));
        assert!(devices.contains("\"discovery_active\":true"));

        let (status, _) = send(
            router(state.clone()),
            Request::builder()
                .method("PATCH")
                .uri("/config")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    "{{\"auto_join_target_id\":\"{}\"}}",
                    "x".repeat(129)
                )))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, _) = send(
            router(state.clone()),
            Request::builder()
                .method("PATCH")
                .uri("/config")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"airplay_name":""}"#))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, body) = send(
            router(state.clone()),
            Request::builder()
                .uri("/interfaces")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let _ = body;

        let (status, _) = send(
            router(state),
            Request::builder()
                .method("POST")
                .uri("/restart")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert!(restart.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn bind_http_and_serve_health() {
        let listener = bind_http("127.0.0.1", 0).unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_on_until(
            listener,
            bare_state(),
            Box::pin(std::future::pending()),
        ));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf).contains("ok"));

        assert!(bind_http("not-an-ip", 8080).is_err());

        let listener = bind_http("127.0.0.1", 0).unwrap();
        serve_on_until(listener, bare_state(), Box::pin(std::future::ready(())))
            .await
            .unwrap();

        let listener = bind_http("127.0.0.1", 0).unwrap();
        let handle = tokio::spawn(serve_on_until(
            listener,
            bare_state(),
            Box::pin(std::future::pending()),
        ));
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        handle.abort();
    }

    #[tokio::test]
    async fn serve_wrapper_answers() {
        let listener = bind_http("127.0.0.1", 0).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        tokio::spawn(serve("127.0.0.1", port, bare_state()));
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        let _ = tokio::net::TcpStream::connect(("127.0.0.1", port)).await;
        assert!(tokio::net::TcpStream::connect(("127.0.0.1", 1))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn serve_reports_bind_error() {
        let err = serve("not-an-ip", 9, bare_state()).await.unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn map_serve_io_ok_and_err() {
        map_serve_io(Ok(())).unwrap();
        assert!(map_serve_io(Err(std::io::Error::other("fail"))).is_err());
    }

    #[tokio::test]
    async fn rescan_sets_the_flag() {
        let state = bare_state();
        let flag = state.rescan.clone();
        let (status, _) = send(
            router(state),
            Request::builder()
                .method("POST")
                .uri("/airplay/rescan")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert!(flag.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[tokio::test]
    async fn wled_table_route_merges_rows_and_rescan_sets_the_flag() {
        let (state, _) = control_state();
        {
            let mut cfg = state.control.as_ref().unwrap().config.lock().unwrap();
            cfg.audio.sync_offset_ms = 1200;
            cfg.audio.shairport_latency_offset_s = -1.2;
            cfg.wled.controllers.push(crate::config::WledController {
                address: "192.0.2.10".into(),
                name: "Saved".into(),
                unicast: true,
                delay_ms: 1,
                manual: true,
                name_locked: true,
            });
        }
        state.wled_seen.lock().unwrap().push(WledSight {
            address: "192.0.2.11".into(),
            name: "Found".into(),
            online: true,
        });
        let (status, body) = send(
            router(state.clone()),
            Request::builder()
                .uri("/wled/controllers")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("192.0.2.10"));
        assert!(text.contains("192.0.2.11"));
        assert!(text.contains("\"unicast\":false") || text.contains("\"unicast\": false"));
        let flag = state.wled_rescan.clone();
        let (status, _) = send(
            router(state),
            Request::builder()
                .method("POST")
                .uri("/wled/rescan")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert!(flag.load(std::sync::atomic::Ordering::Relaxed));

        let (status, body) = send(
            router(bare_state()),
            Request::builder()
                .uri("/wled/controllers")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("\"controllers\":[]") || text.contains("\"controllers\": []"));
    }

    #[tokio::test]
    async fn wled_measure_fills_delay_and_rejects_a_bad_body() {
        let (state, _) = control_state();
        {
            let mut cfg = state.control.as_ref().unwrap().config.lock().unwrap();
            cfg.audio.sync_offset_ms = 1200;
            cfg.audio.shairport_latency_offset_s = -1.2;
        }
        let (status, _) = send(
            router(state.clone()),
            Request::builder()
                .method("POST")
                .uri("/wled/measure")
                .header("content-type", "application/json")
                .body(Body::from("nope"))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let server = std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut sock, _)) => {
                        use std::io::Write;
                        let body = r#"{"leds":{"count":2000,"fps":20}}"#;
                        let packet = format!(
                            "HTTP/1.0 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = sock.write_all(packet.as_bytes());
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
                }
            }
        });
        let payload = format!(
            "{{\"controllers\":[{{\"address\":\"{addr}\",\"delay_ms\":0}},{{\"address\":\"not-an-ip\",\"delay_ms\":4}}]}}"
        );
        let (status, body) = send(
            router(state),
            Request::builder()
                .method("POST")
                .uri("/wled/measure")
                .header("content-type", "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await;
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
        assert_eq!(status, StatusCode::OK);
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains(&addr.to_string()));
        assert!(text.contains("not an IPv4 address"));
        assert!(text.contains("\"ok\":false") || text.contains("\"ok\": false"));
        assert!(text.contains("\"ok\":true") || text.contains("\"ok\": true"));

        let (status, body) = send(
            router(bare_state()),
            Request::builder()
                .method("POST")
                .uri("/wled/measure")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"controllers":[{"address":"not-an-ip","delay_ms":4}]}"#,
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("not an IPv4 address"));
        assert!(text.contains("\"delay_ms\":4") || text.contains("\"delay_ms\": 4"));
    }

    #[test]
    fn log_download_names() {
        assert!(super::log_disposition("wledplay.log")
            .to_str()
            .unwrap()
            .contains("wledplay.log"));
        assert!(super::log_disposition("shairport.log")
            .to_str()
            .unwrap()
            .contains("shairport.log"));
        assert!(super::log_disposition("nqptp.log")
            .to_str()
            .unwrap()
            .contains("nqptp.log"));
        assert!(super::log_disposition("console.log")
            .to_str()
            .unwrap()
            .contains("console.log"));
        assert!(super::log_disposition("other")
            .to_str()
            .unwrap()
            .contains("wledplay.log"));
    }

    #[tokio::test]
    async fn logs_can_be_read_downloaded_and_the_level_changed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("wledplay.log"),
            "2026-11-17T10:00:00Z INFO ready\n2026-11-17T10:00:01Z ERROR down\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("shairport.log"), "0.1: error: late\n").unwrap();
        let (mut state, _) = control_state();
        let cfg_path = dir.path().join("cfg.yaml");
        std::fs::write(&cfg_path, "").unwrap();
        state.log_dir = dir.path().to_path_buf();
        state.control.as_mut().unwrap().config_path = Some(cfg_path.clone());

        let (status, body) = send(
            router(state.clone()),
            Request::builder().uri("/logs").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("ready"));
        assert!(text.contains("\"level\":\"info\"") || text.contains("\"level\": \"info\""));

        let (status, body) = send(
            router(state.clone()),
            Request::builder()
                .uri("/logs?source=wledplay&tail=false&offset=0&q=error&limit=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("ERROR"));
        assert!(!text.contains("ready"));

        let (status, _) = send(
            router(state.clone()),
            Request::builder()
                .uri("/logs?source=nope")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, _) = send(
            router(state.clone()),
            Request::builder()
                .uri("/logs?source=nqptp")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = send(
            router(state.clone()),
            Request::builder()
                .uri("/logs/download")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(String::from_utf8(body).unwrap().contains("ready"));

        let (status, body) = send(
            router(state.clone()),
            Request::builder()
                .uri("/logs/download?source=shairport")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(String::from_utf8(body).unwrap().contains("late"));

        let (status, _) = send(
            router(state.clone()),
            Request::builder()
                .uri("/logs/download?source=nqptp")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, _) = send(
            router(state.clone()),
            Request::builder()
                .uri("/logs/download?source=../secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, _) = send(
            router(bare_state()),
            Request::builder().uri("/logs").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = send(
            router(state.clone()),
            Request::builder()
                .method("PATCH")
                .uri("/config")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"log_level":"debug"}"#))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            state
                .control
                .as_ref()
                .unwrap()
                .config
                .lock()
                .unwrap()
                .logging
                .level,
            "debug"
        );
        assert!(std::fs::read_to_string(&cfg_path)
            .unwrap()
            .contains("debug"));

        let (status, _) = send(
            router(state.clone()),
            Request::builder()
                .method("PATCH")
                .uri("/config")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"log_level":"verbose"}"#))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let broken = dir.path().join("broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::create_dir(broken.join("console.log")).unwrap();
        let mut bad = bare_state();
        bad.log_dir = broken;
        let (status, _) = send(
            router(bad.clone()),
            Request::builder()
                .uri("/logs?source=console")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        let (status, _) = send(
            router(bad),
            Request::builder()
                .uri("/logs/download?source=console")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }
}
