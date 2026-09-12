use axum::extract::State;
use axum::{routing::get, Json, Router};
use serde::Serialize;
use socket2::{Domain, Protocol, Socket, Type};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[derive(Debug, Clone, Default, Serialize)]
pub struct RuntimeStatus {
    pub version: String,
    pub airplay: AirplayStatus,
    pub audio: AudioStatus,
    pub wled: WledStatus,
    pub counters: CounterStatus,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AirplayStatus {
    pub connected: bool,
    pub receiver_name: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AudioStatus {
    pub sample_rate: u32,
    pub channels: u16,
    pub rms: f32,
    pub peak: bool,
    pub dominant_frequency: f32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct WledStatus {
    pub packets_per_second: f32,
    pub destination: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CounterStatus {
    pub pcm_frames: u64,
    pub fft_frames: u64,
    pub packets_sent: u64,
    pub udp_errors: u64,
}

pub type SharedStatus = Arc<Mutex<RuntimeStatus>>;

pub async fn serve(
    listen: &str,
    port: u16,
    status: SharedStatus,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = Router::new()
        .route("/health", get(health))
        .route("/status", get(status_handler))
        .with_state(status);

    let addr: SocketAddr = format!("{listen}:{port}").parse()?;
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.bind(&addr.into())?;
    socket.listen(128)?;
    let listener = TcpListener::from_std(socket.into())?;
    tracing::info!("HTTP status listening on http://{listen}:{port}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

async fn status_handler(State(status): State<SharedStatus>) -> Json<RuntimeStatus> {
    Json(status.lock().unwrap().clone())
}
