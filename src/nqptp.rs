//! Tell stock NQPTP which clock to follow.
//!
//! Shairport Sync 5.6.3 builds this list during AirPlay 2 group setup and then
//! leaves it unsent until the audio stream exists. The sender's address is
//! already on the metadata pipe (`ssnc`/`conn`) at that moment. NQPTP's control
//! port is the same socket Shairport uses later: `/nqptp T <ip>` then `/nqptp B`,
//! including the trailing NUL, to UDP localhost:9000. Stock NQPTP follows the
//! first address only, so the message carries the one client address.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const SMI: &str = "/nqptp";
const CONTROL_PORT: u16 = 9000;
/// `conn` and `clip` for the same session arrive back to back. A second `T`
/// would drop the clock record that was just created.
const RESEND_QUIET: Duration = Duration::from_secs(2);

static TARGET_OVERRIDE: Mutex<Option<SocketAddr>> = Mutex::new(None);
static LAST_ARM: Mutex<Option<(String, Instant)>> = Mutex::new(None);

pub fn control_packet(body: &str) -> Vec<u8> {
    let mut bytes = format!("{SMI} {body}").into_bytes();
    bytes.push(0);
    bytes
}

pub fn arm_packets(ip: &str) -> Option<(Vec<u8>, Vec<u8>)> {
    let ip = ip.trim().trim_matches(['[', ']']);
    if ip.parse::<IpAddr>().is_err() {
        return None;
    }
    Some((control_packet(&format!("T {ip}")), control_packet("B")))
}

pub fn should_send(last: &Option<(String, Instant)>, ip: &str, now: Instant) -> bool {
    match last {
        Some((prev, at)) if prev == ip && now.duration_since(*at) < RESEND_QUIET => false,
        _ => true,
    }
}

pub fn udp_send(bind_to: SocketAddr, target: SocketAddr, bytes: &[u8]) -> bool {
    match UdpSocket::bind(bind_to) {
        Ok(sock) => sock.send_to(bytes, target).is_ok(),
        Err(_) => false,
    }
}

fn target_addr() -> SocketAddr {
    TARGET_OVERRIDE
        .lock()
        .unwrap()
        .unwrap_or(SocketAddr::from((Ipv4Addr::LOCALHOST, CONTROL_PORT)))
}

pub fn arm_new_client(ip: &str) -> bool {
    let Some((timing, play)) = arm_packets(ip) else {
        return false;
    };
    let ip = ip.trim().trim_matches(['[', ']']).to_string();
    let now = Instant::now();
    let mut last = LAST_ARM.lock().unwrap();
    if !should_send(&last, &ip, now) {
        return false;
    }
    let bind_to = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    let target = target_addr();
    let sent = udp_send(bind_to, target, &timing) && udp_send(bind_to, target, &play);
    if !sent {
        return false;
    }
    *last = Some((ip, now));
    true
}

/// `ssnc`/`conn` is emitted when group setup starts. `clip` repeats the address.
pub fn note_client(type_code: &str, code: &str, data: &[u8]) -> bool {
    if type_code != "ssnc" || (code != "conn" && code != "clip") {
        return false;
    }
    let armed = arm_new_client(&String::from_utf8_lossy(data));
    if armed {
        tracing::info!("NQPTP following {}", String::from_utf8_lossy(data).trim());
    }
    armed
}

#[cfg(test)]
fn set_target_override(addr: Option<SocketAddr>) {
    *TARGET_OVERRIDE.lock().unwrap() = addr;
}

#[cfg(test)]
fn clear_last_arm() {
    *LAST_ARM.lock().unwrap() = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn packets_match_the_nqptp_control_port() {
        assert_eq!(control_packet("B"), b"/nqptp B\0");
        let (timing, play) = arm_packets("10.3.1.204").unwrap();
        assert_eq!(timing, b"/nqptp T 10.3.1.204\0");
        assert_eq!(play, b"/nqptp B\0");
        let (v6, _) = arm_packets("  [::1] ").unwrap();
        assert_eq!(v6, b"/nqptp T ::1\0");
        assert!(arm_packets("iPhone").is_none());
        assert!(arm_packets("").is_none());
        assert!(arm_packets("10.3.1.999").is_none());
    }

    #[test]
    fn quiet_window_suppresses_only_the_same_address() {
        let now = Instant::now();
        let recent = now.checked_sub(Duration::from_millis(50)).unwrap();
        let stale = now.checked_sub(Duration::from_secs(3)).unwrap();
        let same = Some(("10.3.1.204".to_string(), recent));
        assert!(!should_send(&same, "10.3.1.204", now));
        assert!(should_send(&same, "10.3.1.205", now));
        assert!(should_send(
            &Some(("10.3.1.204".to_string(), stale)),
            "10.3.1.204",
            now
        ));
        assert!(should_send(&None, "10.3.1.204", now));
    }

    #[test]
    fn udp_send_reports_bind_and_address_family_errors() {
        let held = UdpSocket::bind("127.0.0.1:0").unwrap();
        let occupied = held.local_addr().unwrap();
        assert!(!udp_send(occupied, "127.0.0.1:9".parse().unwrap(), b"x"));
        assert!(!udp_send(
            "127.0.0.1:0".parse().unwrap(),
            "[::1]:9".parse().unwrap(),
            b"x"
        ));
    }

    #[test]
    fn note_client_arms_once_per_address_then_uses_the_default_port() {
        clear_last_arm();
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        set_target_override(Some(sock.local_addr().unwrap()));
        assert!(note_client("ssnc", "conn", b"10.9.8.7"));
        assert!(!note_client("ssnc", "clip", b"10.9.8.7"));
        assert!(!note_client("core", "minm", b"10.9.8.7"));
        assert!(!note_client("ssnc", "clip", b"iPhone"));
        sock.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let mut buf = [0u8; 64];
        let (n, _) = sock.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"/nqptp T 10.9.8.7\0");
        let (n, _) = sock.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"/nqptp B\0");
        assert!(sock.recv_from(&mut buf).is_err());

        set_target_override(Some("[::1]:9".parse().unwrap()));
        assert!(!note_client("ssnc", "conn", b"10.9.8.8"));

        set_target_override(None);
        assert!(note_client("ssnc", "conn", b"192.0.2.20"));
        clear_last_arm();
        set_target_override(None);
    }
}
