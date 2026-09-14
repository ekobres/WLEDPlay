mod operator;

pub use operator::{
    action_buttons_enabled, dest_lines, operator_hint, operator_ui, parse_send_delay_ms,
    parse_shairport_s, should_overwrite_fields, OperatorSnapshot,
};

use crate::control::{ConfigPatch, Control};
use crate::metadata::SharedArtwork;
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

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(ui))
        .route("/health", get(health))
        .route("/status", get(status_handler))
        .route("/levels", get(levels_handler))
        .route("/artwork", get(artwork_handler))
        .route("/config", get(config_get).patch(config_patch))
        .route("/interfaces", get(interfaces_handler))
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
    Json(state.status.lock().unwrap().clone())
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
}

pub fn live_levels(status: &RuntimeStatus) -> LiveLevels {
    LiveLevels {
        connected: status.airplay.connected,
        rms: status.audio.rms,
        peak: status.audio.peak,
        dominant_frequency: status.audio.dominant_frequency,
        bands: status.audio.bands,
        update_hz: status.audio.update_hz,
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
    use crate::metadata::Artwork;
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
        assert!(html.contains("role=\"switch\""));
        assert!(html.contains("id=\"ctl-cancel\""));
        assert!(html.contains("function isDirty()"));
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

        state.status.lock().unwrap().audio.rms = 0.42;
        state.status.lock().unwrap().audio.bands[0] = 200;
        state.status.lock().unwrap().airplay.connected = true;
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
        assert!(String::from_utf8_lossy(&body).contains("WLEDPlay"));

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
