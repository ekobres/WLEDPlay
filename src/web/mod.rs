use crate::control::{ConfigPatch, Control};
use crate::metadata::SharedArtwork;
use crate::netif::InterfaceInfo;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
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

pub type SharedStatus = Arc<Mutex<RuntimeStatus>>;

pub async fn serve(
    listen: &str,
    port: u16,
    state: AppState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = Router::new()
        .route("/", get(ui))
        .route("/health", get(health))
        .route("/status", get(status_handler))
        .route("/artwork", get(artwork_handler))
        .route("/config", get(config_get).patch(config_patch))
        .route("/interfaces", get(interfaces_handler))
        .route("/restart", post(restart_handler))
        .with_state(state);

    let addr: SocketAddr = format!("{listen}:{port}").parse()?;
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(128)?;
    let listener = TcpListener::from_std(socket.into())?;
    tracing::info!("HTTP status listening on http://{listen}:{port}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn ui() -> Html<&'static str> {
    Html(include_str!("index.html"))
}

async fn health() -> &'static str {
    "ok"
}

async fn status_handler(State(state): State<AppState>) -> Json<RuntimeStatus> {
    Json(state.status.lock().unwrap().clone())
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

async fn restart_handler(State(state): State<AppState>) -> StatusCode {
    if let Some(ctrl) = &state.control {
        ctrl.request_restart();
        StatusCode::ACCEPTED
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}
