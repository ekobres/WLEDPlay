use serde::{Deserialize, Serialize};
use std::fs::File;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub airplay: AirplayConfig,
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub wled: WledConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub web: WebConfig,
    #[serde(default)]
    pub pcm: PcmFormat,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub metadata: MetadataConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetadataConfig {
    /// Shairport Sync metadata FIFO. Empty = disabled.
    #[serde(default)]
    pub pipe: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// HTTP control UI. Empty = `web.listen` (default all interfaces).
    #[serde(default)]
    pub http: String,
    /// Shairport Sync + Avahi (mDNS / AirPlay). Empty = all interfaces.
    #[serde(default)]
    pub airplay: String,
    /// WLED Audio Sync unicast UDP. Empty = kernel default route.
    #[serde(default)]
    pub unicast: String,
    /// WLED Audio Sync multicast (`239.0.0.1`). Empty = kernel default route.
    #[serde(default)]
    pub multicast: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirplayConfig {
    #[serde(default = "default_name")]
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    #[serde(default)]
    pub sync_offset_ms: i32,
    /// Shairport `audio_backend_latency_offset_in_seconds` (negative = earlier PCM).
    #[serde(default)]
    pub shairport_latency_offset_s: f64,
    #[serde(default = "default_gain")]
    pub gain: GainSetting,
    #[serde(default)]
    pub update_hz: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GainSetting {
    Auto,
    Fixed(u8),
    Agc(String),
}

impl Default for GainSetting {
    fn default() -> Self {
        GainSetting::Auto
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgcMode {
    Off,
    Normal,
    Vivid,
    Lazy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WledConfig {
    #[serde(default = "default_multicast")]
    pub multicast_address: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub destinations: Vec<String>,
    #[serde(default = "default_multicast_enabled")]
    pub multicast_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    #[serde(default = "default_web_enabled")]
    pub enabled: bool,
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default = "default_web_port")]
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PcmFormat {
    #[serde(default = "default_pcm_rate")]
    pub rate: u32,
    #[serde(default = "default_pcm_channels")]
    pub channels: u16,
    #[serde(default = "default_pcm_format")]
    pub format: String,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sync_offset_ms: 0,
            shairport_latency_offset_s: 0.0,
            gain: default_gain(),
            update_hz: 50,
        }
    }
}

impl Default for AirplayConfig {
    fn default() -> Self {
        Self {
            name: default_name(),
        }
    }
}

impl Default for WledConfig {
    fn default() -> Self {
        Self {
            multicast_address: default_multicast(),
            port: default_port(),
            destinations: vec![],
            multicast_enabled: true,
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
        }
    }
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: default_web_enabled(),
            listen: default_listen(),
            port: default_web_port(),
        }
    }
}

impl Default for PcmFormat {
    fn default() -> Self {
        Self {
            rate: default_pcm_rate(),
            channels: default_pcm_channels(),
            format: default_pcm_format(),
        }
    }
}

impl PcmFormat {
    pub fn bytes_per_sample(&self) -> usize {
        if self.format.contains("32") {
            4
        } else {
            2
        }
    }
}

fn default_name() -> String {
    "WLEDPlay".into()
}
fn default_multicast() -> String {
    "239.0.0.1".into()
}
fn default_port() -> u16 {
    11988
}
fn default_log_level() -> String {
    "info".into()
}
fn default_web_enabled() -> bool {
    true
}
fn default_listen() -> String {
    "0.0.0.0".into()
}
fn default_web_port() -> u16 {
    8080
}
fn default_pcm_rate() -> u32 {
    44100
}
fn default_pcm_channels() -> u16 {
    2
}
fn default_pcm_format() -> String {
    "s16le".into()
}
fn default_gain() -> GainSetting {
    GainSetting::Auto
}
fn default_multicast_enabled() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            airplay: AirplayConfig {
                name: default_name(),
            },
            audio: AudioConfig {
                sync_offset_ms: 0,
                shairport_latency_offset_s: 0.0,
                gain: default_gain(),
                update_hz: 50,
            },
            wled: WledConfig {
                multicast_address: default_multicast(),
                port: default_port(),
                destinations: vec![],
                multicast_enabled: true,
            },
            logging: LoggingConfig {
                level: default_log_level(),
            },
            web: WebConfig {
                enabled: default_web_enabled(),
                listen: default_listen(),
                port: default_web_port(),
            },
            pcm: PcmFormat {
                rate: default_pcm_rate(),
                channels: default_pcm_channels(),
                format: default_pcm_format(),
            },
            network: NetworkConfig::default(),
            metadata: MetadataConfig::default(),
        }
    }
}

impl Config {
    pub fn load(path: Option<&Path>) -> Self {
        let mut cfg = if let Some(p) = path {
            let data = std::fs::read_to_string(p).unwrap_or_else(|e| {
                tracing::warn!("config {} not readable ({e}), using defaults", p.display());
                String::new()
            });
            if data.is_empty() {
                Config::default()
            } else {
                serde_yaml::from_str(&data).unwrap_or_else(|e| {
                    tracing::warn!("config parse error: {e}, using defaults");
                    Config::default()
                })
            }
        } else {
            Config::default()
        };
        cfg.apply_env();
        cfg
    }

    pub fn apply_env(&mut self) {
        if let Ok(v) = std::env::var("WLEDPLAY_AIRPLAY_NAME") {
            self.airplay.name = v;
        }
        if let Ok(v) = std::env::var("WLEDPLAY_MULTICAST_ADDRESS") {
            self.wled.multicast_address = v;
        }
        if let Ok(v) = std::env::var("WLEDPLAY_PORT") {
            if let Ok(p) = v.parse() {
                self.wled.port = p;
            }
        }
        if let Ok(v) = std::env::var("WLEDPLAY_SYNC_OFFSET_MS") {
            if let Ok(p) = v.parse() {
                self.audio.sync_offset_ms = p;
            }
        }
        if let Ok(v) = std::env::var("WLEDPLAY_LOG_LEVEL") {
            self.logging.level = v;
        }
        if let Ok(v) = std::env::var("WLEDPLAY_DESTINATION") {
            if !v.is_empty() {
                self.wled.destinations = v.split(',').map(|s| s.trim().to_string()).collect();
            }
        }
        if let Ok(v) = std::env::var("WLEDPLAY_HTTP_INTERFACE") {
            if !v.is_empty() {
                self.network.http = v;
            }
        }
        if let Ok(v) = std::env::var("WLEDPLAY_AIRPLAY_INTERFACE") {
            if !v.is_empty() {
                self.network.airplay = v;
            }
        }
        if let Ok(v) = std::env::var("WLEDPLAY_UNICAST_INTERFACE") {
            if !v.is_empty() {
                self.network.unicast = v;
            }
        }
        if let Ok(v) = std::env::var("WLEDPLAY_MULTICAST_INTERFACE") {
            if !v.is_empty() {
                self.network.multicast = v;
            }
        }
        if let Ok(v) = std::env::var("WLEDPLAY_WEB_LISTEN") {
            if !v.is_empty() {
                self.web.listen = v;
            }
        }
        if let Ok(v) = std::env::var("WLEDPLAY_WEB_PORT") {
            if let Ok(p) = v.parse() {
                self.web.port = p;
            }
        }
        if let Ok(v) = std::env::var("WLEDPLAY_METADATA_PIPE") {
            if !v.is_empty() {
                self.metadata.pipe = v;
            }
        }
        if let Ok(v) = std::env::var("WLEDPLAY_SHAIRPORT_LATENCY_OFFSET") {
            if !v.is_empty() {
                if let Ok(p) = v.parse() {
                    self.audio.shairport_latency_offset_s = p;
                }
            }
        }
        if let Ok(v) = std::env::var("WLEDPLAY_MULTICAST_ENABLED") {
            self.wled.multicast_enabled = v != "0" && !v.eq_ignore_ascii_case("false");
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let body = serde_yaml::to_string(self).map_err(|e| format!("serialize config: {e}"))?;
        std::fs::write(
            path,
            format!("# WLEDPlay configuration (written by the control UI)\n{body}"),
        )
        .map_err(|e| format!("write {}: {e}", path.display()))
    }

    pub fn path_writable(path: &Path) -> bool {
        File::options().write(true).open(path).is_ok()
            || path
                .parent()
                .map(|dir| File::options().write(true).open(dir).is_ok())
                .unwrap_or(false)
    }

    pub fn agc_mode(&self) -> AgcMode {
        match &self.audio.gain {
            GainSetting::Auto | GainSetting::Agc(_) => AgcMode::Normal,
            GainSetting::Fixed(_) => AgcMode::Off,
        }
    }

    pub fn multicast_ip(&self) -> IpAddr {
        self.wled
            .multicast_address
            .parse()
            .unwrap_or(IpAddr::V4(Ipv4Addr::new(239, 0, 0, 1)))
    }

    pub fn unicast_sockets(&self) -> Vec<SocketAddr> {
        self.wled
            .destinations
            .iter()
            .filter_map(|s| parse_destination(s, self.wled.port))
            .collect()
    }
}

pub fn parse_destination(s: &str, default_port: u16) -> Option<SocketAddr> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return Some(addr);
    }
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Some(SocketAddr::new(ip, default_port));
    }
    tracing::warn!("ignoring invalid WLED destination: {s}");
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ip_only_destination_with_default_port() {
        let mut cfg = Config::default();
        cfg.wled.port = 11988;
        cfg.wled.destinations = vec!["10.11.1.124".into()];
        assert_eq!(
            cfg.unicast_sockets(),
            vec!["10.11.1.124:11988".parse().unwrap()]
        );
    }

    #[test]
    fn parses_host_port_destination() {
        let mut cfg = Config::default();
        cfg.wled.port = 11988;
        cfg.wled.destinations = vec!["10.11.1.124:12000".into()];
        assert_eq!(
            cfg.unicast_sockets(),
            vec!["10.11.1.124:12000".parse().unwrap()]
        );
    }
}
