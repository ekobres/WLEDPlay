//! Relative send offsets from each controller's LED time and a short HTTP sample.
//!
//! The fastest controller stays on the base send delay. Slower ones are moved
//! earlier by the difference. The operator table stores the result only on
//! Control Save.

use crate::config::parse_destination;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

const MEASURE_LIMIT: usize = 32;
const MEASURE_SAMPLES: usize = 3;
const MEASURE_TIMEOUT: Duration = Duration::from_millis(400);
const HYSTERESIS_MS: i32 = 10;
/// WS2812-class show time, microseconds per LED, converted to milliseconds below.
const SHOW_US_PER_LED: u64 = 30;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct MeasureInput {
    pub address: String,
    #[serde(default)]
    pub delay_ms: i32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DelayEstimate {
    pub address: String,
    pub delay_ms: i32,
    pub led_ms: Option<i32>,
    pub network_ms: Option<i32>,
    pub ok: bool,
    pub error: String,
}

#[derive(Debug)]
struct InfoSample {
    led_count: Option<u32>,
    fps: Option<u32>,
    rtt_ms: u32,
}

struct Observation {
    address: String,
    current_delay_ms: i32,
    led_count: Option<u32>,
    fps: Option<u32>,
    rtts_ms: Vec<u32>,
    error: String,
}

impl Observation {
    fn failed(address: &str, delay_ms: i32, error: impl Into<String>) -> Self {
        Self {
            address: address.to_string(),
            current_delay_ms: delay_ms,
            led_count: None,
            fps: None,
            rtts_ms: Vec::new(),
            error: error.into(),
        }
    }

    fn from_results(address: &str, delay_ms: i32, results: &[Result<InfoSample, String>]) -> Self {
        let mut count = None;
        let mut fps = None;
        let mut rtts = Vec::new();
        let mut error = if results.is_empty() {
            "no response".to_string()
        } else {
            String::new()
        };
        for result in results {
            match result {
                Ok(sample) => {
                    rtts.push(sample.rtt_ms);
                    if sample.led_count.is_some() {
                        count = sample.led_count;
                    }
                    if sample.fps.is_some() {
                        fps = sample.fps;
                    }
                }
                Err(err) => error = err.clone(),
            }
        }
        Self {
            address: address.to_string(),
            current_delay_ms: delay_ms,
            led_count: count,
            fps,
            rtts_ms: rtts,
            error,
        }
    }
}

/// Show time in milliseconds, rounded. 300 LEDs is 9 ms.
fn led_show_ms(count: u32) -> i32 {
    ((u64::from(count) * SHOW_US_PER_LED + 500) / 1000) as i32
}

/// Loop time from `/json/info`. Frame period already includes the LED show.
pub fn processing_ms(count: Option<u32>, fps: Option<u32>) -> Option<i32> {
    let show = count.map(led_show_ms);
    let frame = fps.and_then(|fps| {
        let step = (1000 + fps / 2).checked_div(fps)?;
        Some(step as i32)
    });
    match (show, frame) {
        (None, None) => None,
        (Some(show), None) => Some(show),
        (None, Some(frame)) => Some(frame),
        (Some(show), Some(frame)) => Some(show.max(frame)),
    }
}

fn median_u32(samples: &[u32]) -> Option<u32> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        Some(sorted[mid])
    } else {
        let sum = u64::from(sorted[mid - 1]) + u64::from(sorted[mid]);
        Some((sum / 2) as u32)
    }
}

fn one_way_ms(rtt_ms: u32) -> i32 {
    (rtt_ms / 2) as i32
}

fn elapsed_ms(started: Instant) -> u32 {
    u32::try_from(started.elapsed().as_millis().min(u128::from(u32::MAX))).expect("rtt fits u32")
}

fn json_u32(value: &Value) -> Option<u32> {
    if let Some(n) = value.as_u64() {
        return u32::try_from(n).ok();
    }
    let n = value.as_f64()?;
    if n.is_finite() && n >= 0.0 && n <= f64::from(u32::MAX) {
        Some(n.floor() as u32)
    } else {
        None
    }
}

fn read_leds(body: &str) -> (Option<u32>, Option<u32>) {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return (None, None);
    };
    let Some(leds) = value.get("leds") else {
        return (None, None);
    };
    let count = leds.get("count").and_then(json_u32);
    let fps = leds.get("fps").and_then(json_u32);
    (count, fps)
}

fn parse_http_response(buf: &[u8]) -> Result<(u16, String), String> {
    if buf.is_empty() {
        return Err("no response".into());
    }
    let text = String::from_utf8_lossy(buf);
    let (head, body) = text.split_once("\r\n\r\n").ok_or("truncated response")?;
    let status = head.lines().next().unwrap_or("");
    let code = status
        .split_whitespace()
        .nth(1)
        .ok_or("bad response")?
        .parse::<u16>()
        .map_err(|_| "bad response".to_string())?;
    Ok((code, body.to_string()))
}

fn ipv4_socket(address: &str) -> Result<SocketAddr, String> {
    match parse_destination(address.trim(), 80) {
        Some(SocketAddr::V4(addr)) => Ok(SocketAddr::V4(addr)),
        _ => Err("not an IPv4 address".into()),
    }
}

fn fetch_info(addr: SocketAddr, timeout: Duration) -> Result<InfoSample, String> {
    let started = Instant::now();
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|err| err.to_string())?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let request = format!("GET /json/info HTTP/1.0\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    let _ = stream.write_all(request.as_bytes());
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let rtt_ms = elapsed_ms(started);
    let (code, body) = parse_http_response(&buf)?;
    if code != 200 {
        return Err(format!("http {code}"));
    }
    let (led_count, fps) = read_leds(&body);
    Ok(InfoSample {
        led_count,
        fps,
        rtt_ms,
    })
}

fn probe_one(input: &MeasureInput, timeout: Duration, samples: usize) -> Observation {
    let addr = match ipv4_socket(&input.address) {
        Ok(addr) => addr,
        Err(err) => return Observation::failed(&input.address, input.delay_ms, err),
    };
    let mut results = Vec::with_capacity(samples);
    for _ in 0..samples {
        results.push(fetch_info(addr, timeout));
    }
    Observation::from_results(&input.address, input.delay_ms, &results)
}

fn estimates_from(observations: &[Observation], min_ms: i32, max_ms: i32) -> Vec<DelayEstimate> {
    let totals: Vec<Option<i32>> = observations.iter().map(observation_total).collect();
    let fastest = totals.iter().filter_map(|total| *total).min();
    observations
        .iter()
        .zip(totals)
        .map(|(obs, total)| estimate_one(obs, total, fastest, min_ms, max_ms))
        .collect()
}

fn observation_total(obs: &Observation) -> Option<i32> {
    let network = median_u32(&obs.rtts_ms).map(one_way_ms);
    let led = processing_ms(obs.led_count, obs.fps);
    match (led, network) {
        (None, None) => None,
        (Some(led), None) => Some(led),
        (None, Some(network)) => Some(network),
        (Some(led), Some(network)) => Some(led + network),
    }
}

fn estimate_one(
    obs: &Observation,
    total: Option<i32>,
    fastest: Option<i32>,
    min_ms: i32,
    max_ms: i32,
) -> DelayEstimate {
    let (Some(total), Some(fastest)) = (total, fastest) else {
        return DelayEstimate {
            address: obs.address.clone(),
            delay_ms: obs.current_delay_ms,
            led_ms: processing_ms(obs.led_count, obs.fps),
            network_ms: median_u32(&obs.rtts_ms).map(one_way_ms),
            ok: false,
            error: if obs.error.is_empty() {
                "no response".to_string()
            } else {
                obs.error.clone()
            },
        };
    };
    let raw = fastest - total;
    let clamped = raw.clamp(min_ms, max_ms);
    let delay_ms = if (clamped - obs.current_delay_ms).abs() <= HYSTERESIS_MS {
        obs.current_delay_ms
    } else {
        clamped
    };
    DelayEstimate {
        address: obs.address.clone(),
        delay_ms,
        led_ms: processing_ms(obs.led_count, obs.fps),
        network_ms: median_u32(&obs.rtts_ms).map(one_way_ms),
        ok: true,
        error: String::new(),
    }
}

pub fn measure_delays(
    inputs: &[MeasureInput],
    min_delay_ms: i32,
    max_delay_ms: i32,
) -> Vec<DelayEstimate> {
    measure_delays_with(
        inputs,
        min_delay_ms,
        max_delay_ms,
        MEASURE_TIMEOUT,
        MEASURE_SAMPLES,
    )
}

fn measure_delays_with(
    inputs: &[MeasureInput],
    min_delay_ms: i32,
    max_delay_ms: i32,
    timeout: Duration,
    samples: usize,
) -> Vec<DelayEstimate> {
    let inputs = &inputs[..inputs.len().min(MEASURE_LIMIT)];
    if inputs.is_empty() {
        return Vec::new();
    }
    thread::scope(|scope| {
        let handles: Vec<_> = inputs
            .iter()
            .map(|input| scope.spawn(|| probe_one(input, timeout, samples)))
            .collect();
        let observations: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("probe"))
            .collect();
        estimates_from(&observations, min_delay_ms, max_delay_ms)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn input(address: &str, delay_ms: i32) -> MeasureInput {
        MeasureInput {
            address: address.to_string(),
            delay_ms,
        }
    }

    fn obs(
        address: &str,
        delay_ms: i32,
        count: Option<u32>,
        fps: Option<u32>,
        rtts: &[u32],
        error: &str,
    ) -> Observation {
        Observation {
            address: address.to_string(),
            current_delay_ms: delay_ms,
            led_count: count,
            fps,
            rtts_ms: rtts.to_vec(),
            error: error.to_string(),
        }
    }

    #[test]
    fn show_time_rounds_to_the_nearest_millisecond() {
        assert_eq!(led_show_ms(0), 0);
        assert_eq!(led_show_ms(16), 0);
        assert_eq!(led_show_ms(17), 1);
        assert_eq!(led_show_ms(300), 9);
    }

    #[test]
    fn processing_uses_show_time_or_frame_period() {
        assert_eq!(processing_ms(None, None), None);
        assert_eq!(processing_ms(Some(300), None), Some(9));
        assert_eq!(processing_ms(None, Some(0)), None);
        assert_eq!(processing_ms(None, Some(40)), Some(25));
        assert_eq!(processing_ms(Some(300), Some(40)), Some(25));
        assert_eq!(processing_ms(Some(2000), Some(40)), Some(60));
    }

    #[test]
    fn median_and_one_way_delay() {
        assert_eq!(median_u32(&[]), None);
        assert_eq!(median_u32(&[9]), Some(9));
        assert_eq!(median_u32(&[1, 9]), Some(5));
        assert_eq!(median_u32(&[9, 1, 4]), Some(4));
        assert_eq!(one_way_ms(0), 0);
        assert_eq!(one_way_ms(11), 5);
    }

    #[test]
    fn json_numbers_and_led_block() {
        assert_eq!(json_u32(&Value::from(12_u64)), Some(12));
        assert_eq!(json_u32(&Value::from(u64::MAX)), None);
        assert_eq!(json_u32(&serde_json::json!(40.9)), Some(40));
        assert_eq!(json_u32(&serde_json::json!(-1.0)), None);
        assert_eq!(json_u32(&serde_json::json!(4_294_967_296.0)), None);
        assert_eq!(json_u32(&Value::from("30")), None);
        assert_eq!(
            read_leds(r#"{"leds":{"count":300,"fps":42}}"#),
            (Some(300), Some(42))
        );
        assert_eq!(read_leds(r#"{"leds":{"fps":10}}"#), (None, Some(10)));
        assert_eq!(read_leds(r#"{"ver":"0.16.0"}"#), (None, None));
        assert_eq!(read_leds("not-json"), (None, None));
        assert_eq!(read_leds(r#"{"leds":"nope"}"#), (None, None));
    }

    #[test]
    fn http_status_line() {
        assert_eq!(parse_http_response(b"").unwrap_err(), "no response");
        assert_eq!(
            parse_http_response(b"HTTP/1.0 200 OK").unwrap_err(),
            "truncated response"
        );
        assert_eq!(
            parse_http_response(b"\r\n\r\n").unwrap_err(),
            "bad response"
        );
        assert_eq!(
            parse_http_response(b"HTTP/1.0\r\n\r\n").unwrap_err(),
            "bad response"
        );
        assert_eq!(
            parse_http_response(b"HTTP/1.0 abc\r\n\r\n").unwrap_err(),
            "bad response"
        );
        let (code, body) = parse_http_response(b"HTTP/1.1 200 OK\r\n\r\n{\"leds\":{}}").unwrap();
        assert_eq!(code, 200);
        assert_eq!(body, "{\"leds\":{}}");
    }

    #[test]
    fn ipv4_addresses_only() {
        assert_eq!(
            ipv4_socket("192.0.2.10").unwrap(),
            "192.0.2.10:80".parse().unwrap()
        );
        assert_eq!(
            ipv4_socket(" 192.0.2.10:8080 ").unwrap(),
            "192.0.2.10:8080".parse().unwrap()
        );
        assert_eq!(ipv4_socket("not-an-ip").unwrap_err(), "not an IPv4 address");
        assert_eq!(ipv4_socket("").unwrap_err(), "not an IPv4 address");
        assert_eq!(
            ipv4_socket("2001:db8::1").unwrap_err(),
            "not an IPv4 address"
        );
    }

    #[test]
    fn samples_combine_and_a_miss_keeps_the_error() {
        let mixed = Observation::from_results(
            "192.0.2.10",
            4,
            &[
                Err("http 500".into()),
                Ok(InfoSample {
                    led_count: Some(300),
                    fps: None,
                    rtt_ms: 8,
                }),
                Ok(InfoSample {
                    led_count: None,
                    fps: Some(40),
                    rtt_ms: 12,
                }),
            ],
        );
        assert_eq!(mixed.led_count, Some(300));
        assert_eq!(mixed.fps, Some(40));
        assert_eq!(mixed.rtts_ms, vec![8, 12]);
        let empty = Observation::from_results("192.0.2.11", 0, &[]);
        assert_eq!(empty.error, "no response");
        assert!(empty.rtts_ms.is_empty());
    }

    #[test]
    fn slower_controller_is_sent_early_and_small_changes_stay() {
        let rows = [
            obs("fast", 0, Some(30), Some(60), &[2], ""),
            obs("slow", 0, Some(2000), Some(20), &[2], ""),
            obs("close", -5, Some(30), Some(60), &[2], ""),
            obs("held", 0, Some(30), Some(60), &[80], ""),
            obs("down", 0, None, None, &[], "no response"),
            obs("broken", 3, None, None, &[], "http 500"),
        ];
        let out = estimates_from(&rows, -1200, 3800);
        assert_eq!(out[0].delay_ms, 0);
        assert_eq!(out[0].led_ms, Some(17));
        assert_eq!(out[1].delay_ms, -43);
        assert_eq!(out[2].delay_ms, -5);
        assert_eq!(out[3].delay_ms, -39);
        assert!(!out[4].ok);
        assert_eq!(out[4].delay_ms, 0);
        assert_eq!(out[4].error, "no response");
        assert!(!out[5].ok);
        assert_eq!(out[5].error, "http 500");
        assert_eq!(out[5].delay_ms, 3);

        let within = estimates_from(&rows[..2], -10, 0);
        assert_eq!(within[0].delay_ms, 0);
        assert_eq!(within[1].delay_ms, 0);
        let clamped = estimates_from(&rows[..2], -11, 0);
        assert_eq!(clamped[0].delay_ms, 0);
        assert_eq!(clamped[1].delay_ms, -11);

        let quiet = estimates_from(&[obs("only", 15, None, None, &[], "")], -1200, 3800);
        assert!(!quiet[0].ok);
        assert_eq!(quiet[0].error, "no response");
        assert_eq!(quiet[0].delay_ms, 15);
    }

    #[test]
    fn ten_milliseconds_is_left_alone() {
        let rows = [
            obs("base", 0, None, None, &[0], ""),
            obs("ten", 0, None, None, &[20], ""),
            obs("eleven", 0, None, None, &[22], ""),
        ];
        let out = estimates_from(&rows, -1200, 3800);
        assert_eq!(out[0].delay_ms, 0);
        assert_eq!(out[1].delay_ms, 0);
        assert_eq!(out[2].delay_ms, -11);
    }

    #[test]
    fn network_only_sample_still_compares() {
        let rows = [
            obs("near", 0, None, None, &[4], ""),
            obs("far", 0, None, None, &[40], ""),
            obs("wire", 0, Some(2000), None, &[], ""),
        ];
        let out = estimates_from(&rows, -1200, 3800);
        assert_eq!(out[0].delay_ms, 0);
        assert_eq!(out[0].led_ms, None);
        assert_eq!(out[0].network_ms, Some(2));
        assert_eq!(out[1].delay_ms, -18);
        assert!(out[1].error.is_empty());
        assert_eq!(out[2].delay_ms, -58);
        assert_eq!(out[2].led_ms, Some(60));
        assert_eq!(out[2].network_ms, None);
    }

    fn serve(status: &str, body: &str) -> (SocketAddr, Arc<AtomicBool>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let status = status.to_string();
        let body = body.to_string();
        let handle = thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut sock, _)) => {
                        let mut buf = [0_u8; 512];
                        let _ = sock.read(buf.as_mut_slice());
                        let packet = format!(
                            "HTTP/1.0 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = sock.write_all(packet.as_bytes());
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        (addr, stop, handle)
    }

    #[test]
    fn fetch_reads_json_info_and_reports_failures() {
        let (addr, stop, handle) = serve("200 OK", r#"{"leds":{"count":300,"fps":42.2}}"#);
        let sample = fetch_info(addr, Duration::from_millis(500)).unwrap();
        assert_eq!(sample.led_count, Some(300));
        assert_eq!(sample.fps, Some(42));
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();

        let (addr, stop, handle) = serve("503 Service Unavailable", "busy");
        let err = fetch_info(addr, Duration::from_millis(500)).unwrap_err();
        assert_eq!(err, "http 503");
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let refused = listener.local_addr().unwrap();
        drop(listener);
        assert!(fetch_info(refused, Duration::from_millis(200)).is_err());

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stalled = listener.local_addr().unwrap();
        let stall = thread::spawn(move || {
            let (sock, _) = listener.accept().expect("stall accept");
            thread::sleep(Duration::from_millis(300));
            drop(sock);
        });
        let err = fetch_info(stalled, Duration::from_millis(50)).unwrap_err();
        assert_eq!(err, "no response");
        stall.join().unwrap();
    }

    #[test]
    fn probe_rejects_bad_addresses_and_zero_samples() {
        let bad = probe_one(&input("not-an-ip", 7), Duration::from_millis(50), 2);
        assert_eq!(bad.error, "not an IPv4 address");
        assert_eq!(bad.current_delay_ms, 7);
        let none = probe_one(&input("192.0.2.10", 3), Duration::from_millis(200), 0);
        assert!(none.rtts_ms.is_empty());
        assert_eq!(none.error, "no response");
    }

    #[test]
    fn measure_limits_the_list_and_offsets_live_controllers() {
        assert!(measure_delays(&[], -1200, 3800).is_empty());
        let too_many: Vec<_> = (0..33).map(|i| input(&format!("bad-{i}"), 0)).collect();
        let limited = measure_delays_with(&too_many, -1200, 3800, Duration::from_millis(50), 1);
        assert_eq!(limited.len(), 32);
        assert!(limited.iter().all(|row| !row.ok));

        let (fast, stop_fast, fast_thread) = serve("200 OK", r#"{"leds":{"count":30,"fps":60}}"#);
        let (slow, stop_slow, slow_thread) = serve("200 OK", r#"{"leds":{"count":2000,"fps":20}}"#);
        let rows = measure_delays_with(
            &[input(&fast.to_string(), 0), input(&slow.to_string(), 0)],
            -1200,
            3800,
            Duration::from_millis(500),
            3,
        );
        stop_fast.store(true, Ordering::Relaxed);
        stop_slow.store(true, Ordering::Relaxed);
        fast_thread.join().unwrap();
        slow_thread.join().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.ok && row.error.is_empty()));
        assert_eq!(rows[0].delay_ms, 0);
        assert!(rows[1].delay_ms < -20);
    }
}
