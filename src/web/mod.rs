mod operator;

pub use operator::{
    action_buttons_enabled, dest_lines, operator_hint, operator_ui, parse_send_delay_ms,
    parse_shairport_s, should_overwrite_fields, OperatorSnapshot,
};

use crate::clock::{self, ClockSession, ClockStatus};
use crate::control::{ConfigPatch, Control};
use crate::metadata::{self, SharedArtwork, TrackMetadata};
use crate::netif::InterfaceInfo;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[derive(Clone)]
pub struct AppState {
    pub status: SharedStatus,
    pub artwork: SharedArtwork,
    pub control: Option<Control>,
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
    pub position_s: Option<f32>,
    pub duration_s: Option<f32>,
    #[serde(skip)]
    pub position_base_s: Option<f32>,
    #[serde(skip)]
    pub progress_unix_ms: Option<u64>,
}

impl TrackMetadataStatus {
    pub fn set_from_track(&mut self, meta: &TrackMetadata) {
        self.artist = meta.artist.clone();
        self.album = meta.album.clone();
        self.title = meta.title.clone();
        self.has_artwork = meta.has_artwork;
        self.duration_s = meta.duration_s();
        self.position_base_s = meta.position_s();
        self.progress_unix_ms = meta.progress_unix_ms;
    }

    pub fn refresh_position(&mut self, playing: bool) {
        if playing {
            if self.progress_unix_ms.is_none() && self.position_base_s.is_some() {
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
    match ctrl.apply_patch(patch) {
        Ok(view) => Json(view).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
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
                version: "0.1.0".into(),
                ..Default::default()
            })),
            artwork: Arc::new(Mutex::new(None)),
            control: None,
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
            restart: restart.clone(),
        };
        (
            AppState {
                status: Arc::new(Mutex::new(RuntimeStatus::default())),
                artwork: Arc::new(Mutex::new(None)),
                control: Some(control),
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

        let mut track = TrackMetadata::default();
        track.rtp_start = Some(0);
        track.rtp_current = Some(22050);
        track.rtp_end = Some(44100);
        track.duration_ms = Some(60_000);
        track.progress_unix_ms = None;
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
        assert!(String::from_utf8_lossy(&body).contains("0.1.0"));
        assert!(String::from_utf8_lossy(&body).contains("auto_join"));
        assert!(String::from_utf8_lossy(&body).contains("\"clock\""));

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
}
