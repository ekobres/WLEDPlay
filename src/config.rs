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
    GainSetting::default()
}
fn default_multicast_enabled() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            airplay: AirplayConfig::default(),
            audio: AudioConfig::default(),
            wled: WledConfig::default(),
            logging: LoggingConfig::default(),
            web: WebConfig::default(),
            pcm: PcmFormat::default(),
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
            if let Ok(p) = v.parse::<i32>() {
                self.audio.sync_offset_ms = p.max(0);
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
        write_config_yaml(path, serde_yaml::to_string(self).map_err(display_err))
    }

    pub fn path_writable(path: &Path) -> bool {
        if File::options().write(true).open(path).is_ok() {
            return true;
        }
        let Some(dir) = path.parent() else {
            return false;
        };
        match std::fs::metadata(dir) {
            Ok(meta) => meta.is_dir() && !meta.permissions().readonly(),
            Err(_) => false,
        }
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

pub fn display_err(err: impl std::fmt::Display) -> String {
    err.to_string()
}

pub fn write_config_yaml(path: &Path, yaml: Result<String, String>) -> Result<(), String> {
    let body = yaml.map_err(|e| format!("serialize config: {e}"))?;
    std::fs::write(
        path,
        format!("# WLEDPlay configuration (written by the control UI)\n{body}"),
    )
    .map_err(|e| format!("write {}: {e}", path.display()))
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

    #[test]
    fn parse_destination_rejects_empty_and_invalid() {
        assert!(parse_destination("", 11988).is_none());
        assert!(parse_destination("   ", 11988).is_none());
        assert!(parse_destination("not-an-ip", 11988).is_none());
    }

    #[test]
    fn parse_destination_accepts_socket_addr_and_ip() {
        assert_eq!(
            parse_destination("10.0.0.1:5000", 11988),
            Some("10.0.0.1:5000".parse().unwrap())
        );
        assert_eq!(
            parse_destination("10.0.0.1", 11988),
            Some("10.0.0.1:11988".parse().unwrap())
        );
    }

    #[test]
    fn agc_mode_from_gain_setting() {
        let mut cfg = Config::default();
        assert_eq!(cfg.agc_mode(), AgcMode::Normal);
        cfg.audio.gain = GainSetting::Fixed(128);
        assert_eq!(cfg.agc_mode(), AgcMode::Off);
        cfg.audio.gain = GainSetting::Agc("vivid".into());
        assert_eq!(cfg.agc_mode(), AgcMode::Normal);
    }

    #[test]
    fn multicast_ip_falls_back_on_invalid() {
        let mut cfg = Config::default();
        cfg.wled.multicast_address = "not-multicast".into();
        assert_eq!(cfg.multicast_ip(), IpAddr::V4(Ipv4Addr::new(239, 0, 0, 1)));
    }

    #[test]
    fn yaml_roundtrip_preserves_network_and_wled() {
        let mut cfg = Config::default();
        cfg.network.http = "eth0".into();
        cfg.network.airplay = "eth1".into();
        cfg.wled.destinations = vec!["10.11.1.124".into()];
        cfg.audio.shairport_latency_offset_s = -1.2;
        let yaml = serde_yaml::to_string(&cfg).unwrap();
        let parsed: Config = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.network.http, "eth0");
        assert_eq!(parsed.network.airplay, "eth1");
        assert_eq!(parsed.wled.destinations, vec!["10.11.1.124"]);
        assert!((parsed.audio.shairport_latency_offset_s - -1.2).abs() < 1e-9);
    }

    #[test]
    fn apply_env_overrides_selected_fields() {
        let _g = env_lock();
        let keys = [
            "WLEDPLAY_AIRPLAY_NAME",
            "WLEDPLAY_DESTINATION",
            "WLEDPLAY_MULTICAST_ENABLED",
        ];
        for key in keys {
            std::env::remove_var(key);
        }

        std::env::set_var("WLEDPLAY_AIRPLAY_NAME", "EnvName");
        std::env::set_var("WLEDPLAY_DESTINATION", "10.1.2.3, 10.1.2.4");
        std::env::set_var("WLEDPLAY_MULTICAST_ENABLED", "false");

        let mut cfg = Config::default();
        cfg.apply_env();

        assert_eq!(cfg.airplay.name, "EnvName");
        assert_eq!(
            cfg.wled.destinations,
            vec!["10.1.2.3".to_string(), "10.1.2.4".to_string()]
        );
        assert!(!cfg.wled.multicast_enabled);

        for key in keys {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn pcm_format_bytes_per_sample() {
        assert_eq!(PcmFormat::default().bytes_per_sample(), 2);
        let mut pcm = PcmFormat::default();
        pcm.format = "s32le".into();
        assert_eq!(pcm.bytes_per_sample(), 4);
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn load_missing_empty_and_invalid_yaml() {
        let _g = env_lock();
        let missing = std::path::Path::new("/no/such/wledplay-config.yaml");
        let cfg = Config::load(Some(missing));
        assert_eq!(cfg.airplay.name, "WLEDPlay");

        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.yaml");
        std::fs::write(&empty, "").unwrap();
        let cfg = Config::load(Some(&empty));
        assert_eq!(cfg.web.port, 8080);

        let bad = dir.path().join("bad.yaml");
        std::fs::write(&bad, "airplay: [unterminated").unwrap();
        let cfg = Config::load(Some(&bad));
        assert_eq!(cfg.airplay.name, "WLEDPlay");

        let cfg = Config::load(None);
        assert_eq!(cfg.wled.port, 11988);
        assert!(matches!(GainSetting::default(), GainSetting::Auto));
    }

    #[test]
    fn apply_env_covers_remaining_keys() {
        let _g = env_lock();
        let keys = [
            "WLEDPLAY_AIRPLAY_NAME",
            "WLEDPLAY_MULTICAST_ADDRESS",
            "WLEDPLAY_PORT",
            "WLEDPLAY_SYNC_OFFSET_MS",
            "WLEDPLAY_LOG_LEVEL",
            "WLEDPLAY_DESTINATION",
            "WLEDPLAY_HTTP_INTERFACE",
            "WLEDPLAY_AIRPLAY_INTERFACE",
            "WLEDPLAY_UNICAST_INTERFACE",
            "WLEDPLAY_MULTICAST_INTERFACE",
            "WLEDPLAY_WEB_LISTEN",
            "WLEDPLAY_WEB_PORT",
            "WLEDPLAY_METADATA_PIPE",
            "WLEDPLAY_SHAIRPORT_LATENCY_OFFSET",
            "WLEDPLAY_MULTICAST_ENABLED",
        ];
        for key in keys {
            std::env::remove_var(key);
        }

        std::env::set_var("WLEDPLAY_MULTICAST_ADDRESS", "239.9.9.9");
        std::env::set_var("WLEDPLAY_PORT", "not-a-port");
        std::env::set_var("WLEDPLAY_SYNC_OFFSET_MS", "xyz");
        std::env::set_var("WLEDPLAY_LOG_LEVEL", "debug");
        std::env::set_var("WLEDPLAY_DESTINATION", "");
        std::env::set_var("WLEDPLAY_HTTP_INTERFACE", "");
        std::env::set_var("WLEDPLAY_AIRPLAY_INTERFACE", "eth1");
        std::env::set_var("WLEDPLAY_UNICAST_INTERFACE", "eth2");
        std::env::set_var("WLEDPLAY_MULTICAST_INTERFACE", "eth3");
        std::env::set_var("WLEDPLAY_WEB_LISTEN", "127.0.0.1");
        std::env::set_var("WLEDPLAY_WEB_PORT", "9090");
        std::env::set_var("WLEDPLAY_METADATA_PIPE", "/tmp/m");
        std::env::set_var("WLEDPLAY_SHAIRPORT_LATENCY_OFFSET", "");
        std::env::set_var("WLEDPLAY_MULTICAST_ENABLED", "0");

        let mut cfg = Config::default();
        cfg.apply_env();
        assert_eq!(cfg.wled.multicast_address, "239.9.9.9");
        assert_eq!(cfg.wled.port, 11988);
        assert_eq!(cfg.audio.sync_offset_ms, 0);
        assert_eq!(cfg.logging.level, "debug");
        assert!(cfg.wled.destinations.is_empty());
        assert!(cfg.network.http.is_empty());
        assert_eq!(cfg.network.airplay, "eth1");
        assert_eq!(cfg.network.unicast, "eth2");
        assert_eq!(cfg.network.multicast, "eth3");
        assert_eq!(cfg.web.listen, "127.0.0.1");
        assert_eq!(cfg.web.port, 9090);
        assert_eq!(cfg.metadata.pipe, "/tmp/m");
        assert!(!cfg.wled.multicast_enabled);

        std::env::set_var("WLEDPLAY_PORT", "12121");
        std::env::set_var("WLEDPLAY_SYNC_OFFSET_MS", "-25");
        std::env::set_var("WLEDPLAY_DESTINATION", "10.0.0.8");
        std::env::set_var("WLEDPLAY_HTTP_INTERFACE", "eth0");
        std::env::set_var("WLEDPLAY_SHAIRPORT_LATENCY_OFFSET", "not-f64");
        std::env::set_var("WLEDPLAY_WEB_PORT", "nope");
        std::env::set_var("WLEDPLAY_MULTICAST_ENABLED", "true");
        cfg.apply_env();
        assert_eq!(cfg.wled.port, 12121);
        assert_eq!(cfg.audio.sync_offset_ms, 0);
        assert_eq!(cfg.wled.destinations, vec!["10.0.0.8".to_string()]);
        assert_eq!(cfg.network.http, "eth0");
        assert_eq!(cfg.web.port, 9090);
        assert!(cfg.wled.multicast_enabled);

        std::env::set_var("WLEDPLAY_SHAIRPORT_LATENCY_OFFSET", "-1.25");
        cfg.apply_env();
        assert!((cfg.audio.shairport_latency_offset_s - -1.25).abs() < 1e-9);

        std::env::set_var("WLEDPLAY_AIRPLAY_INTERFACE", "");
        std::env::set_var("WLEDPLAY_UNICAST_INTERFACE", "");
        std::env::set_var("WLEDPLAY_MULTICAST_INTERFACE", "");
        std::env::set_var("WLEDPLAY_WEB_LISTEN", "");
        std::env::set_var("WLEDPLAY_METADATA_PIPE", "");
        let airplay = cfg.network.airplay.clone();
        cfg.apply_env();
        assert_eq!(cfg.network.airplay, airplay);

        for key in keys {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn save_and_writable_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.yaml");
        Config::default().save(&path).unwrap();
        assert!(Config::path_writable(&path));

        let missing = dir.path().join("missing.yaml");
        assert!(Config::path_writable(&missing));

        let err = Config::default().save(std::path::Path::new("/no/such/wledplay-dir/x.yaml"));
        assert!(err.is_err());
        assert!(!Config::path_writable(std::path::Path::new(
            "/no/such/wledplay-dir/x.yaml"
        )));
        assert!(!Config::path_writable(std::path::Path::new("/")));

        let err = write_config_yaml(&path, Err("boom".into()));
        assert!(err.unwrap_err().contains("serialize config"));
        assert_eq!(display_err("x"), "x");
    }

    #[test]
    fn env_lock_recovers_from_poison() {
        let _ = std::panic::catch_unwind(|| {
            let _g = env_lock();
            panic!("poison env lock");
        });
        let _g = env_lock();
    }
}
