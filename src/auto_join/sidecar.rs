//! Localhost pyatv sidecar client. The audio thread never blocks on this I/O.

use super::{
    detected_count, device_choices, AirPlayDevice, AutoJoinService, JoinRequest, JoinResult,
    TickInput,
};
use crate::config::AutoJoinConfig;
use crate::web::{AirPlaySession, AutoJoinStatus, SharedStatus};
use serde::Deserialize;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const RECV_SLICE: Duration = Duration::from_millis(50);
const SCAN_EVERY: Duration = Duration::from_secs(2);
const HTTP_TIMEOUT: Duration = Duration::from_secs(2);
const JOIN_HTTP_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Default)]
struct Discovery {
    devices: Vec<AirPlayDevice>,
    self_id: String,
    discovery_active: bool,
    error: String,
    session: AirPlaySession,
}

#[derive(Debug, Clone, Default)]
struct Watch {
    receiver_name: String,
    probe: String,
}

enum WorkerMsg {
    Stop,
    Join(JoinRequest),
}

#[derive(Debug, Default, Deserialize)]
struct ScanBody {
    #[serde(default)]
    self_id: String,
    #[serde(default)]
    discovery_active: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    devices: Vec<AirPlayDevice>,
    #[serde(default)]
    session: AirPlaySession,
}

#[derive(Debug, Deserialize)]
struct JoinBody {
    ok: bool,
    #[serde(default)]
    error: String,
}

pub struct AutoJoinHub {
    epoch: Instant,
    svc: Mutex<AutoJoinService>,
    discovery: Arc<Mutex<Discovery>>,
    watch: Arc<Mutex<Watch>>,
    results: Arc<Mutex<Vec<JoinResult>>>,
    cmd_tx: Option<Sender<WorkerMsg>>,
    worker: Option<JoinHandle<()>>,
}

impl AutoJoinHub {
    pub fn inactive() -> Self {
        Self {
            epoch: Instant::now(),
            svc: Mutex::new(AutoJoinService::new()),
            discovery: Arc::new(Mutex::new(Discovery::default())),
            watch: Arc::new(Mutex::new(Watch::default())),
            results: Arc::new(Mutex::new(Vec::new())),
            cmd_tx: None,
            worker: None,
        }
    }

    pub fn drive(
        &self,
        raw_busy: bool,
        auto: &AutoJoinConfig,
        receiver_name: &str,
        now: Instant,
        status: &SharedStatus,
    ) {
        {
            let mut watch = self.watch.lock().unwrap();
            watch.receiver_name = receiver_name.to_string();
            watch.probe = auto.target_id.clone();
        }
        let now_ms = now.saturating_duration_since(self.epoch).as_millis() as u64;
        let discovery = self.discovery.lock().unwrap().clone();
        let results = {
            let mut queued = self.results.lock().unwrap();
            std::mem::take(&mut *queued)
        };
        let tick_with = |svc: &mut AutoJoinService, join_results: &[JoinResult]| {
            svc.tick(TickInput {
                now_ms,
                raw_busy,
                enabled: auto.enabled,
                target_id: &auto.target_id,
                target_name: &auto.target_name,
                self_id: &discovery.self_id,
                devices: &discovery.devices,
                discovery_active: discovery.discovery_active,
                discovery_error: &discovery.error,
                can_join: self.cmd_tx.is_some(),
                join_results,
            })
        };
        let mut output = {
            let mut svc = self.svc.lock().unwrap();
            tick_with(&mut svc, &results)
        };
        if let Some(req) = output.join.take() {
            let tx = self.cmd_tx.as_ref().expect("join without worker");
            if tx.send(WorkerMsg::Join(req)).is_err() {
                let mut svc = self.svc.lock().unwrap();
                svc.abandon_join(now_ms);
                output = tick_with(&mut svc, &[]);
            }
        }
        let choices = device_choices(
            &discovery.devices,
            &discovery.self_id,
            receiver_name,
            &auto.target_id,
            &auto.target_name,
        );
        let mut view: AutoJoinStatus = output.view;
        view.device_count = detected_count(&discovery.devices, &discovery.self_id, receiver_name);
        let mut status = status.lock().unwrap();
        status.auto_join = view;
        status.session = discovery.session;
        status.airplay_devices = choices;
    }
}

impl Drop for AutoJoinHub {
    fn drop(&mut self) {
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(WorkerMsg::Stop);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn from_env() -> AutoJoinHub {
    match std::env::var("WLEDPLAY_AUTOJOIN_URL") {
        Ok(url) if !url.trim().is_empty() => {
            if parse_http_base(url.trim()).is_err() {
                tracing::warn!("ignoring invalid WLEDPLAY_AUTOJOIN_URL");
                AutoJoinHub::inactive()
            } else {
                spawn(url.trim(), SCAN_EVERY)
            }
        }
        _ => AutoJoinHub::inactive(),
    }
}

fn spawn(base: &str, scan_every: Duration) -> AutoJoinHub {
    let discovery = Arc::new(Mutex::new(Discovery::default()));
    let watch = Arc::new(Mutex::new(Watch::default()));
    let results = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = mpsc::channel();
    let base = base.to_string();
    let discovery_w = discovery.clone();
    let watch_w = watch.clone();
    let results_w = results.clone();
    let worker = thread::Builder::new()
        .name("auto-join".into())
        .spawn(move || serve_loop(base, scan_every, rx, discovery_w, watch_w, results_w))
        .expect("auto join thread");
    AutoJoinHub {
        epoch: Instant::now(),
        svc: Mutex::new(AutoJoinService::new()),
        discovery,
        watch,
        results,
        cmd_tx: Some(tx),
        worker: Some(worker),
    }
}

fn serve_loop(
    base: String,
    scan_every: Duration,
    rx: Receiver<WorkerMsg>,
    discovery: Arc<Mutex<Discovery>>,
    watch: Arc<Mutex<Watch>>,
    results: Arc<Mutex<Vec<JoinResult>>>,
) {
    let mut next_scan = Instant::now();
    loop {
        match rx.recv_timeout(RECV_SLICE) {
            Ok(WorkerMsg::Stop) => return,
            Err(RecvTimeoutError::Disconnected) => return,
            Ok(WorkerMsg::Join(req)) => {
                let result = perform_join(&base, req);
                results.lock().unwrap().push(result);
            }
            Err(RecvTimeoutError::Timeout) => {
                if Instant::now() >= next_scan {
                    let (name, probe) = {
                        let watch = watch.lock().unwrap();
                        (watch.receiver_name.clone(), watch.probe.clone())
                    };
                    let found = fetch_scan(&base, &name, &probe);
                    apply_scan(&mut discovery.lock().unwrap(), found);
                    next_scan = Instant::now() + scan_every;
                }
            }
        }
    }
}

fn perform_join(base: &str, req: JoinRequest) -> JoinResult {
    let outcome = post_join(base, &req);
    JoinResult {
        revision: req.revision,
        generation: req.generation,
        ok: outcome.is_ok(),
        error: outcome.err().unwrap_or_default(),
    }
}

fn apply_scan(dest: &mut Discovery, found: Result<ScanBody, String>) {
    match found {
        Ok(scan) => {
            if dest.error != scan.error {
                if !scan.error.is_empty() {
                    tracing::warn!("Auto Join discovery: {}", scan.error);
                }
                dest.error = scan.error;
            }
            dest.devices = scan.devices;
            dest.self_id = scan.self_id;
            dest.discovery_active = scan.discovery_active;
            dest.session = scan.session;
        }
        Err(err) => {
            dest.discovery_active = false;
            if dest.error != err {
                tracing::warn!("Auto Join discovery: {err}");
                dest.error = err;
            }
        }
    }
}

pub(crate) fn parse_http_base(base: &str) -> Result<(String, u16), String> {
    let rest = base
        .trim()
        .strip_prefix("http://")
        .ok_or("auto join url must be http://host:port")?;
    let rest = rest.trim_end_matches('/');
    if rest.is_empty() || rest.contains('/') {
        return Err("auto join url must be http://host:port".into());
    }
    let (host, port) = rest
        .rsplit_once(':')
        .ok_or("auto join url must be http://host:port")?;
    if host.is_empty() || host.contains(':') {
        return Err("auto join url must be http://host:port".into());
    }
    let port: u16 = port
        .parse()
        .map_err(|_| "auto join url must be http://host:port")?;
    if port == 0 {
        return Err("auto join url must be http://host:port".into());
    }
    Ok((host.to_string(), port))
}

fn fetch_scan(base: &str, name: &str, probe: &str) -> Result<ScanBody, String> {
    let path = format!(
        "/v1/scan?name={}&probe={}",
        query_escape(name),
        query_escape(probe)
    );
    let (code, body) = http_exchange(base, "GET", &path, None, HTTP_TIMEOUT)?;
    if !(200..300).contains(&code) {
        return Err(if body.is_empty() {
            format!("scan http {code}")
        } else {
            body
        });
    }
    serde_json::from_str(&body).map_err(|err| err.to_string())
}

fn post_join(base: &str, req: &JoinRequest) -> Result<(), String> {
    let payload = serde_json::json!({
        "target_id": req.target_id,
        "receiver_id": req.receiver_id,
    })
    .to_string();
    let (code, body) = http_exchange(
        base,
        "POST",
        "/v1/join",
        Some(payload.as_bytes()),
        JOIN_HTTP_TIMEOUT,
    )?;
    if !(200..300).contains(&code) {
        if body.is_empty() {
            return Err(format!("join http {code}"));
        }
        if let Ok(parsed) = serde_json::from_str::<JoinBody>(&body) {
            if !parsed.error.is_empty() {
                return Err(parsed.error);
            }
        }
        return Err(body);
    }
    let parsed: JoinBody = serde_json::from_str(&body).map_err(|err| err.to_string())?;
    if parsed.ok {
        Ok(())
    } else {
        Err(if parsed.error.is_empty() {
            "join failed".into()
        } else {
            parsed.error
        })
    }
}

fn query_escape(raw: &str) -> String {
    let mut out = String::new();
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn http_exchange(
    base: &str,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    timeout: Duration,
) -> Result<(u16, String), String> {
    let (host, port) = parse_http_base(base)?;
    let addr = SocketAddr::new(
        host.parse()
            .map_err(|_| "auto join url must be http://host:port".to_string())?,
        port,
    );
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|err| err.to_string())?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let body = body.unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.0\r\nHost: {host}:{port}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .expect("auto join request");
    if !body.is_empty() {
        stream.write_all(body).expect("auto join body");
    }
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    if buf.is_empty() {
        return Err("empty auto join response".into());
    }
    parse_http(&buf)
}

fn parse_http(buf: &[u8]) -> Result<(u16, String), String> {
    let text = String::from_utf8_lossy(buf);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or("truncated auto join response")?;
    let status = head.lines().next().unwrap_or("");
    let code = status
        .split_whitespace()
        .nth(1)
        .ok_or("bad auto join status")?
        .parse::<u16>()
        .map_err(|_| "bad auto join status")?;
    Ok((code, body.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AutoJoinConfig;
    use crate::web::RuntimeStatus;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    fn serve(status: &str, body: &str) -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let status = status.to_string();
        let body = body.to_string();
        let handle = thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf);
                let raw = format!(
                    "{status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(raw.as_bytes());
            }
        });
        (port, handle)
    }

    #[test]
    fn parse_base_and_query_and_http() {
        assert!(parse_http_base("https://127.0.0.1:1").is_err());
        assert!(parse_http_base("http://127.0.0.1").is_err());
        assert!(parse_http_base("http://:80").is_err());
        assert!(parse_http_base("http://127.0.0.1:0").is_err());
        assert!(parse_http_base("http://127.0.0.1:nope").is_err());
        assert!(parse_http_base("http://127.0.0.1:80/extra").is_err());
        assert!(parse_http_base("http://[::1]:80").is_err());
        assert_eq!(
            parse_http_base("http://127.0.0.1:8091/").unwrap(),
            ("127.0.0.1".into(), 8091)
        );
        assert_eq!(query_escape("a b"), "a%20b");
        assert_eq!(query_escape("Az_9.-"), "Az_9.-");
        assert!(parse_http("nope".as_bytes()).is_err());
        assert!(parse_http(b"HTTP/1.0\r\n\r\n").is_err());
        assert!(parse_http(b"HTTP/1.0 nope\r\n\r\n").is_err());
        assert_eq!(
            parse_http(b"HTTP/1.1 204 Empty\r\n\r\nhi").unwrap(),
            (204, "hi".into())
        );
        assert!(http_exchange("http://127.0.0.1:1", "GET", "/", None, HTTP_TIMEOUT).is_err());
        assert!(http_exchange("http://not a host:80", "GET", "/", None, HTTP_TIMEOUT).is_err());

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 128];
            let _ = sock.read(&mut buf);
        });
        let empty = http_exchange(
            &format!("http://127.0.0.1:{port}"),
            "GET",
            "/v1/scan",
            None,
            HTTP_TIMEOUT,
        );
        assert!(empty.is_err());
        let _ = handle.join();
    }

    #[test]
    fn scan_and_join_http_branches() {
        let (port, handle) = serve(
            "HTTP/1.1 200 OK",
            r#"{"self_id":"self","discovery_active":true,"error":"","devices":[],"session":{"target_name":"Kitchen","flags":"0x198404","igl":"1","gcgl":"0","gid":"group","leader":"Kitchen","members":["Bar"],"players":[{"name":"Bar","flags":"0x20004","igl":"0","gcgl":"1","target":true,"leader":false}]}}"#,
        );
        let base = format!("http://127.0.0.1:{port}");
        let scan = fetch_scan(&base, "WLED Play", "aa").unwrap();
        assert_eq!(scan.self_id, "self");
        assert!(scan.discovery_active);
        assert_eq!(scan.session.leader, "Kitchen");
        assert_eq!(scan.session.flags, "0x198404");
        assert_eq!(scan.session.members, vec!["Bar".to_string()]);
        assert_eq!(scan.session.players.len(), 1);
        assert_eq!(scan.session.players[0].name, "Bar");
        assert!(scan.session.players[0].target);
        assert!(!scan.session.players[0].leader);
        assert_eq!(scan.session.players[0].flags, "0x20004");
        let _ = handle.join();

        let (port, handle) = serve("HTTP/1.1 500 ERR", "");
        let err = fetch_scan(&format!("http://127.0.0.1:{port}"), "n", "").unwrap_err();
        assert!(err.contains("500"));
        let _ = handle.join();

        let (port, handle) = serve("HTTP/1.1 502 ERR", "nope");
        let err = fetch_scan(&format!("http://127.0.0.1:{port}"), "n", "").unwrap_err();
        assert_eq!(err, "nope");
        let _ = handle.join();

        let (port, handle) = serve("HTTP/1.1 200 OK", "not-json");
        assert!(fetch_scan(&format!("http://127.0.0.1:{port}"), "n", "").is_err());
        let _ = handle.join();

        let req = JoinRequest {
            target_id: "aa".into(),
            receiver_id: "self".into(),
            revision: 1,
            generation: 2,
        };
        let (port, handle) = serve("HTTP/1.1 200 OK", r#"{"ok":true,"error":""}"#);
        assert!(post_join(&format!("http://127.0.0.1:{port}"), &req).is_ok());
        let _ = handle.join();

        let (port, handle) = serve("HTTP/1.1 200 OK", r#"{"ok":false,"error":""}"#);
        assert_eq!(
            post_join(&format!("http://127.0.0.1:{port}"), &req).unwrap_err(),
            "join failed"
        );
        let _ = handle.join();

        let (port, handle) = serve(
            "HTTP/1.1 200 OK",
            r#"{"ok":false,"error":"group modification not supported"}"#,
        );
        assert!(post_join(&format!("http://127.0.0.1:{port}"), &req)
            .unwrap_err()
            .contains("not supported"));
        let _ = handle.join();

        let (port, handle) = serve("HTTP/1.1 502 ERR", "");
        assert!(post_join(&format!("http://127.0.0.1:{port}"), &req)
            .unwrap_err()
            .contains("502"));
        let _ = handle.join();

        let (port, handle) = serve(
            "HTTP/1.1 502 ERR",
            r#"{"ok":false,"error":"unable to identify group leader"}"#,
        );
        assert!(post_join(&format!("http://127.0.0.1:{port}"), &req)
            .unwrap_err()
            .contains("group leader"));
        let _ = handle.join();

        let (port, handle) = serve("HTTP/1.1 502 ERR", r#"{"ok":false,"error":""}"#);
        assert!(post_join(&format!("http://127.0.0.1:{port}"), &req)
            .unwrap_err()
            .contains("ok"));
        let _ = handle.join();

        let (port, handle) = serve("HTTP/1.1 200 OK", "not-json");
        assert!(post_join(&format!("http://127.0.0.1:{port}"), &req).is_err());
        let _ = handle.join();

        let (port, handle) = serve("HTTP/1.1 502 ERR", "plain");
        assert_eq!(
            post_join(&format!("http://127.0.0.1:{port}"), &req).unwrap_err(),
            "plain"
        );
        let _ = handle.join();

        let _ = format!("{:?}", Discovery::default());
        let _ = format!("{:?}", Watch::default().clone());
        let _ = format!(
            "{:?}",
            ScanBody {
                self_id: String::new(),
                discovery_active: false,
                error: String::new(),
                devices: vec![],
                session: AirPlaySession::default(),
            }
        );
        let _ = format!(
            "{:?}",
            JoinBody {
                ok: false,
                error: String::new(),
            }
        );
    }

    #[test]
    fn apply_scan_logs_changes_only() {
        let mut dest = Discovery::default();
        apply_scan(
            &mut dest,
            Ok(ScanBody {
                self_id: "self".into(),
                discovery_active: true,
                error: String::new(),
                devices: vec![],
                session: AirPlaySession {
                    target_name: "Kitchen".into(),
                    flags: "0x198404".into(),
                    igl: "1".into(),
                    gcgl: "0".into(),
                    gid: "group".into(),
                    leader: "Kitchen".into(),
                    members: vec!["Bar".into()],
                    players: vec![],
                },
            }),
        );
        assert!(dest.discovery_active);
        assert_eq!(dest.session.leader, "Kitchen");
        assert_eq!(dest.session.members, vec!["Bar".to_string()]);
        apply_scan(
            &mut dest,
            Ok(ScanBody {
                self_id: "self".into(),
                discovery_active: true,
                error: "pairing required".into(),
                devices: vec![],
                session: AirPlaySession::default(),
            }),
        );
        assert_eq!(dest.error, "pairing required");
        assert!(dest.session.leader.is_empty());
        apply_scan(
            &mut dest,
            Ok(ScanBody {
                self_id: "self".into(),
                discovery_active: true,
                error: String::new(),
                devices: vec![],
                session: AirPlaySession::default(),
            }),
        );
        assert!(dest.error.is_empty());
        apply_scan(
            &mut dest,
            Ok(ScanBody {
                self_id: "self".into(),
                discovery_active: true,
                error: "pairing required".into(),
                devices: vec![],
                session: AirPlaySession {
                    leader: "Kept".into(),
                    ..AirPlaySession::default()
                },
            }),
        );
        apply_scan(&mut dest, Err("down".into()));
        assert_eq!(dest.session.leader, "Kept");
        assert!(!dest.discovery_active);
        assert_eq!(dest.error, "down");
        apply_scan(&mut dest, Err("down".into()));
        assert_eq!(dest.error, "down");
    }

    #[test]
    fn inactive_hub_updates_status_without_joining() {
        let hub = AutoJoinHub::inactive();
        let status = Arc::new(Mutex::new(RuntimeStatus::default()));
        let auto = AutoJoinConfig {
            enabled: true,
            target_id: "aa".into(),
            target_name: "Kitchen".into(),
        };
        hub.discovery.lock().unwrap().session = AirPlaySession {
            target_name: "Kitchen".into(),
            leader: "Kitchen".into(),
            members: vec!["Bar".into()],
            ..AirPlaySession::default()
        };
        hub.drive(false, &auto, "WLEDPlay", Instant::now(), &status);
        let view = status.lock().unwrap().auto_join.clone();
        assert_eq!(status.lock().unwrap().session.leader, "Kitchen");
        assert_eq!(
            status.lock().unwrap().session.members,
            vec!["Bar".to_string()]
        );
        assert_eq!(view.target_id, "aa");
        assert!(!view.discovery_active);
        assert!(status
            .lock()
            .unwrap()
            .airplay_devices
            .iter()
            .any(|d| d.label.contains("unavailable")));
    }

    #[test]
    fn worker_scans_joins_and_stops() {
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_s = hits.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_s = stop.clone();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            while !stop_s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut sock, _)) => {
                        let _ = sock.set_read_timeout(Some(Duration::from_millis(200)));
                        let mut buf = vec![0u8; 2048];
                        let n = sock.read(&mut buf).unwrap_or(0);
                        let text = String::from_utf8_lossy(&buf[..n]);
                        hits_s.fetch_add(1, Ordering::SeqCst);
                        let body = if text.starts_with("POST") {
                            r#"{"ok":false,"error":"group modification not supported"}"#
                        } else {
                            r#"{"self_id":"self","discovery_active":true,"error":"","devices":[{"id":"aa","name":"Kitchen","model":"HomePod","address":"10.0.0.9","available":true,"playing":true,"airplay2":true}]}"#
                        };
                        let raw = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = sock.write_all(raw.as_bytes());
                    }
                    Err(_) => thread::sleep(Duration::from_millis(10)),
                }
            }
        });
        let hub = spawn(
            &format!("http://127.0.0.1:{port}"),
            Duration::from_millis(20),
        );
        let status = Arc::new(Mutex::new(RuntimeStatus::default()));
        let auto = AutoJoinConfig {
            enabled: true,
            target_id: "aa".into(),
            target_name: "Kitchen".into(),
        };
        let start = Instant::now();
        while status.lock().unwrap().auto_join.self_id.is_empty()
            && start.elapsed() < Duration::from_secs(2)
        {
            hub.drive(false, &auto, "WLEDPlay", Instant::now(), &status);
            thread::sleep(Duration::from_millis(30));
        }
        assert_eq!(status.lock().unwrap().auto_join.self_id, "self");
        let mut saw_result = false;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            let ready = status
                .lock()
                .unwrap()
                .auto_join
                .last_error
                .contains("not supported");
            if ready {
                saw_result = true;
                break;
            }
            hub.drive(false, &auto, "WLEDPlay", Instant::now(), &status);
        }
        assert!(saw_result);
        assert!(hits.load(Ordering::SeqCst) >= 1);
        drop(hub);
        stop.store(true, Ordering::SeqCst);
        let _ = server.join();
    }

    #[test]
    fn dead_worker_abandons_join() {
        let hub = spawn("http://127.0.0.1:1", Duration::from_secs(30));
        if let Some(tx) = &hub.cmd_tx {
            let _ = tx.send(WorkerMsg::Stop);
        }
        if let Some(handle) = hub.worker.as_ref() {
            let _ = handle.thread().id();
        }
        thread::sleep(Duration::from_millis(80));
        hub.discovery.lock().unwrap().self_id = "self".into();
        hub.discovery.lock().unwrap().devices = vec![AirPlayDevice {
            id: "aa".into(),
            name: "Kitchen".into(),
            model: String::new(),
            address: String::new(),
            available: true,
            playing: true,
            paused: false,
            airplay2: true,
            phone_owned: false,
        }];
        hub.discovery.lock().unwrap().discovery_active = true;
        let status = Arc::new(Mutex::new(RuntimeStatus::default()));
        let auto = AutoJoinConfig {
            enabled: true,
            target_id: "aa".into(),
            target_name: "Kitchen".into(),
        };
        hub.drive(false, &auto, "WLEDPlay", Instant::now(), &status);
        let err = status.lock().unwrap().auto_join.last_error.clone();
        assert!(err.contains("sidecar command not sent"), "{err}");
        hub.svc.lock().unwrap().abandon_join(0);
    }

    #[test]
    fn serve_loop_exits_when_disconnected() {
        let (tx, rx) = mpsc::channel();
        drop(tx);
        serve_loop(
            "http://127.0.0.1:1".into(),
            Duration::from_secs(30),
            rx,
            Arc::new(Mutex::new(Discovery::default())),
            Arc::new(Mutex::new(Watch::default())),
            Arc::new(Mutex::new(Vec::new())),
        );
    }

    #[test]
    fn from_env_respects_url() {
        let _guard = ENV.lock().unwrap();
        std::env::remove_var("WLEDPLAY_AUTOJOIN_URL");
        assert!(from_env().cmd_tx.is_none());
        std::env::set_var("WLEDPLAY_AUTOJOIN_URL", "   ");
        assert!(from_env().cmd_tx.is_none());
        std::env::set_var("WLEDPLAY_AUTOJOIN_URL", "https://nope");
        assert!(from_env().cmd_tx.is_none());
        std::env::set_var("WLEDPLAY_AUTOJOIN_URL", "http://127.0.0.1:1");
        let hub = from_env();
        assert!(hub.cmd_tx.is_some());
        drop(hub);
        std::env::remove_var("WLEDPLAY_AUTOJOIN_URL");
    }

    static ENV: Mutex<()> = Mutex::new(());
}
