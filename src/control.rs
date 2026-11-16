use crate::config::{AgcMode, Config};
use crate::netif::{self, InterfaceInfo};
use crate::wled::WledSender;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Control {
    pub config: Arc<Mutex<Config>>,
    pub sender: Arc<Mutex<WledSender>>,
    pub config_path: Option<PathBuf>,
    pub applied_name: String,
    pub applied_http: String,
    pub applied_airplay: String,
    pub applied_unicast: String,
    pub applied_multicast: String,
    pub applied_shairport: f64,
    /// Multicast group and UDP port take effect on the next process start.
    pub applied_multicast_address: String,
    pub applied_port: u16,
    pub restart: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConfigPatch {
    pub airplay_name: Option<String>,
    pub sync_offset_ms: Option<i32>,
    pub gain: Option<String>,
    pub shairport_latency_offset_s: Option<f64>,
    pub multicast_enabled: Option<bool>,
    pub multicast_address: Option<String>,
    pub port: Option<u16>,
    pub destinations: Option<Vec<String>>,
    pub http_interface: Option<String>,
    pub airplay_interface: Option<String>,
    pub unicast_interface: Option<String>,
    pub multicast_interface: Option<String>,
    pub auto_join_enabled: Option<bool>,
    pub auto_join_target_id: Option<String>,
    pub auto_join_target_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigView {
    pub values: ConfigValues,
    pub fields: ConfigFields,
    pub interfaces: Vec<InterfaceInfo>,
    pub persisted: bool,
    pub persist_error: Option<String>,
    pub needs_restart: bool,
    pub config_writable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigValues {
    pub airplay_name: String,
    pub sync_offset_ms: i32,
    pub gain: AgcMode,
    pub shairport_latency_offset_s: f64,
    /// Send delay (ms) that aligns UDP with presentation for the current Shairport lead.
    pub on_time_send_delay_ms: u32,
    pub multicast_enabled: bool,
    pub multicast_address: String,
    pub port: u16,
    pub destinations: Vec<String>,
    pub http_interface: String,
    pub airplay_interface: String,
    pub unicast_interface: String,
    pub multicast_interface: String,
    pub auto_join_enabled: bool,
    pub auto_join_target_id: String,
    pub auto_join_target_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigFields {
    pub sync_offset_ms: &'static str,
    pub gain: &'static str,
    pub multicast_enabled: &'static str,
    pub destinations: &'static str,
    pub airplay_name: &'static str,
    pub http_interface: &'static str,
    pub airplay_interface: &'static str,
    pub unicast_interface: &'static str,
    pub multicast_interface: &'static str,
    pub shairport_latency_offset_s: &'static str,
    pub multicast_address: &'static str,
    pub port: &'static str,
    pub auto_join_enabled: &'static str,
    pub auto_join_target_id: &'static str,
    pub auto_join_target_name: &'static str,
}

impl ConfigFields {
    fn standard() -> Self {
        Self {
            sync_offset_ms: "live",
            gain: "live",
            multicast_enabled: "live",
            destinations: "live",
            airplay_name: "restart",
            http_interface: "restart",
            airplay_interface: "restart",
            unicast_interface: "restart",
            multicast_interface: "restart",
            shairport_latency_offset_s: "restart",
            multicast_address: "restart",
            port: "restart",
            auto_join_enabled: "live",
            auto_join_target_id: "live",
            auto_join_target_name: "live",
        }
    }
}

impl Control {
    pub fn view(&self) -> ConfigView {
        let cfg = self.config.lock().unwrap().clone();
        let writable = self
            .config_path
            .as_deref()
            .map(Config::path_writable)
            .unwrap_or(false);
        ConfigView {
            values: values_from(&cfg),
            fields: ConfigFields::standard(),
            interfaces: netif::list_interfaces(),
            persisted: false,
            persist_error: None,
            needs_restart: self.needs_restart(&cfg),
            config_writable: writable,
        }
    }

    pub fn apply_patch(&self, patch: ConfigPatch) -> Result<ConfigView, String> {
        let mut cfg = self.config.lock().unwrap().clone();
        apply_patch_to(&mut cfg, patch)?;

        {
            let mut live = self.config.lock().unwrap();
            let mut sender = self.sender.lock().unwrap();
            sender.set_multicast_enabled(cfg.wled.multicast_enabled);
            let mut pinned = cfg.clone();
            pinned.wled.port = self.applied_port;
            sender.set_unicast(pinned.unicast_sockets());
            *live = cfg.clone();
        }

        let mut persisted = false;
        let mut persist_error = None;
        if let Some(path) = &self.config_path {
            match cfg.save(path) {
                Ok(()) => persisted = true,
                Err(e) => persist_error = Some(e),
            }
        }

        let writable = self
            .config_path
            .as_deref()
            .map(Config::path_writable)
            .unwrap_or(false);
        Ok(ConfigView {
            values: values_from(&cfg),
            fields: ConfigFields::standard(),
            interfaces: netif::list_interfaces(),
            persisted,
            persist_error,
            needs_restart: self.needs_restart(&cfg),
            config_writable: writable,
        })
    }

    pub fn request_restart(&self) {
        self.restart.store(true, Ordering::SeqCst);
    }

    fn needs_restart(&self, cfg: &Config) -> bool {
        cfg.airplay.name != self.applied_name
            || cfg.network.http != self.applied_http
            || cfg.network.airplay != self.applied_airplay
            || cfg.network.unicast != self.applied_unicast
            || cfg.network.multicast != self.applied_multicast
            || (cfg.audio.shairport_latency_offset_s - self.applied_shairport).abs() > 0.0001
            || cfg.wled.multicast_address != self.applied_multicast_address
            || cfg.wled.port != self.applied_port
    }
}

fn apply_patch_to(cfg: &mut Config, patch: ConfigPatch) -> Result<(), String> {
    if let Some(name) = &patch.airplay_name {
        let name = name.trim();
        if name.is_empty() || name.len() > 64 {
            return Err("airplay_name must be 1–64 characters".into());
        }
    }
    if let Some(ms) = patch.sync_offset_ms {
        if !(0..=5_000).contains(&ms) {
            return Err("sync_offset_ms must be between 0 and 5000".into());
        }
    }
    if let Some(gain) = &patch.gain {
        if parse_gain(gain).is_none() {
            return Err("gain must be off, normal, vivid, or lazy".into());
        }
    }
    if let Some(off) = patch.shairport_latency_offset_s {
        if !(-5.0..=2.0).contains(&off) {
            return Err("shairport_latency_offset_s must be between -5 and 2".into());
        }
    }
    if let Some(addr) = &patch.multicast_address {
        let parsed: std::net::IpAddr = addr
            .parse()
            .map_err(|_| "multicast_address is not a valid IP".to_string())?;
        if let std::net::IpAddr::V4(v4) = parsed {
            if !v4.is_multicast() {
                return Err("multicast_address must be a multicast IPv4".into());
            }
        } else {
            return Err("multicast_address must be IPv4".into());
        }
    }
    if let Some(port) = patch.port {
        if port == 0 {
            return Err("port must be 1–65535".into());
        }
    }
    let dest_port = patch.port.unwrap_or(cfg.wled.port);
    if let Some(dests) = &patch.destinations {
        for d in dests {
            if crate::config::parse_destination(d, dest_port).is_none() && !d.trim().is_empty() {
                return Err(format!("invalid destination: {d}"));
            }
        }
    }
    if let Some(id) = &patch.auto_join_target_id {
        if id.trim().len() > 128 {
            return Err("auto_join_target_id must be at most 128 characters".into());
        }
    }
    if let Some(name) = &patch.auto_join_target_name {
        if name.trim().len() > 128 {
            return Err("auto_join_target_name must be at most 128 characters".into());
        }
    }
    validate_iface(&patch.http_interface)?;
    validate_iface(&patch.airplay_interface)?;
    validate_iface(&patch.unicast_interface)?;
    validate_iface(&patch.multicast_interface)?;

    if let Some(name) = patch.airplay_name {
        cfg.airplay.name = name.trim().to_string();
    }
    if let Some(ms) = patch.sync_offset_ms {
        cfg.audio.sync_offset_ms = ms;
    }
    if let Some(gain) = patch.gain {
        cfg.audio.gain = parse_gain(&gain).expect("validated");
    }
    if let Some(off) = patch.shairport_latency_offset_s {
        cfg.audio.shairport_latency_offset_s = off;
    }
    if let Some(on) = patch.multicast_enabled {
        cfg.wled.multicast_enabled = on;
    }
    if let Some(addr) = patch.multicast_address {
        cfg.wled.multicast_address = addr;
    }
    if let Some(port) = patch.port {
        cfg.wled.port = port;
    }
    if let Some(dests) = patch.destinations {
        cfg.wled.destinations = dests
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    if let Some(iface) = patch.http_interface {
        cfg.network.http = iface.trim().to_string();
    }
    if let Some(iface) = patch.airplay_interface {
        cfg.network.airplay = iface.trim().to_string();
    }
    if let Some(iface) = patch.unicast_interface {
        cfg.network.unicast = iface.trim().to_string();
    }
    if let Some(iface) = patch.multicast_interface {
        cfg.network.multicast = iface.trim().to_string();
    }
    if let Some(enabled) = patch.auto_join_enabled {
        cfg.auto_join.enabled = enabled;
    }
    if let Some(id) = patch.auto_join_target_id {
        cfg.auto_join.target_id = id.trim().to_string();
    }
    if let Some(name) = patch.auto_join_target_name {
        cfg.auto_join.target_name = name.trim().to_string();
    }
    Ok(())
}

fn validate_iface(name: &Option<String>) -> Result<(), String> {
    if let Some(iface) = name {
        let iface = iface.trim();
        if !iface.is_empty() {
            netif::resolve_interface(iface)?;
        }
    }
    Ok(())
}

fn parse_gain(raw: &str) -> Option<AgcMode> {
    AgcMode::parse(raw).or_else(|| raw.trim().parse::<i64>().ok().map(AgcMode::from_sound_agc))
}

/// Presentation-aligned send delay for the current Shairport lead (`|offset|` in ms).
pub fn on_time_send_delay_ms(shairport_s: f64) -> u32 {
    (-shairport_s * 1000.0).round().max(0.0) as u32
}

fn values_from(cfg: &Config) -> ConfigValues {
    let shairport = cfg.audio.shairport_latency_offset_s;
    ConfigValues {
        airplay_name: cfg.airplay.name.clone(),
        sync_offset_ms: cfg.audio.sync_offset_ms,
        gain: cfg.audio.gain,
        shairport_latency_offset_s: shairport,
        on_time_send_delay_ms: on_time_send_delay_ms(shairport),
        multicast_enabled: cfg.wled.multicast_enabled,
        multicast_address: cfg.wled.multicast_address.clone(),
        port: cfg.wled.port,
        destinations: cfg.wled.destinations.clone(),
        http_interface: cfg.network.http.clone(),
        airplay_interface: cfg.network.airplay.clone(),
        unicast_interface: cfg.network.unicast.clone(),
        multicast_interface: cfg.network.multicast.clone(),
        auto_join_enabled: cfg.auto_join.enabled,
        auto_join_target_id: cfg.auto_join.target_id.clone(),
        auto_join_target_name: cfg.auto_join.target_name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{AgcMode, Config};

    #[test]
    fn on_time_send_delay_ms_matches_shairport_lead() {
        assert_eq!(super::on_time_send_delay_ms(-1.2), 1200);
        assert_eq!(super::on_time_send_delay_ms(-2.0), 2000);
        assert_eq!(super::on_time_send_delay_ms(0.0), 0);
        assert_eq!(super::on_time_send_delay_ms(0.5), 0);
    }

    #[test]
    fn sync_and_shairport_patches_are_independent() {
        let mut cfg = Config::default();
        super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                sync_offset_ms: Some(250),
                shairport_latency_offset_s: Some(-1.2),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(cfg.audio.sync_offset_ms, 250);
        assert!((cfg.audio.shairport_latency_offset_s - -1.2).abs() < 1e-9);

        super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                sync_offset_ms: Some(80),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(cfg.audio.sync_offset_ms, 80);
        assert!((cfg.audio.shairport_latency_offset_s - -1.2).abs() < 1e-9);

        super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                gain: Some("vivid".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(cfg.audio.gain, AgcMode::Vivid);
        super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                gain: Some("2".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(cfg.audio.gain, AgcMode::Vivid);
        super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                gain: Some("auto".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(cfg.audio.gain, AgcMode::Normal);
        super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                gain: Some("off".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(cfg.audio.gain, AgcMode::Off);
        super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                gain: Some("0".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(cfg.audio.gain, AgcMode::Off);
        assert!(super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                gain: Some("nope".into()),
                ..Default::default()
            }
        )
        .is_err());
        assert_eq!(cfg.audio.gain, AgcMode::Off);
    }

    #[test]
    fn values_from_includes_on_time_send_delay() {
        let mut cfg = Config::default();
        cfg.audio.sync_offset_ms = 1200;
        cfg.audio.shairport_latency_offset_s = -1.2;
        let values = super::values_from(&cfg);
        assert_eq!(values.sync_offset_ms, 1200);
        assert_eq!(values.on_time_send_delay_ms, 1200);
        assert_eq!(values.gain, AgcMode::Normal);
    }

    #[test]
    fn invalid_patch_leaves_previous_settings() {
        let mut cfg = Config::default();
        cfg.airplay.name = "KeepMe".into();
        let err = super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                airplay_name: Some("NewName".into()),
                destinations: Some(vec!["not-an-ip".into()]),
                ..Default::default()
            },
        );
        assert!(err.is_err());
        assert_eq!(err.unwrap_err(), "invalid destination: not-an-ip");
        assert_eq!(cfg.airplay.name, "KeepMe");
    }

    #[test]
    fn patch_sets_four_nics_independently() {
        let mut cfg = Config::default();
        super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                http_interface: Some("".into()),
                airplay_interface: Some("".into()),
                unicast_interface: Some("".into()),
                multicast_interface: Some("".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(cfg.network.http.is_empty());
        assert!(cfg.network.airplay.is_empty());
        assert!(cfg.network.unicast.is_empty());
        assert!(cfg.network.multicast.is_empty());
    }

    #[test]
    fn rejects_empty_airplay_name() {
        let mut cfg = Config::default();
        let err = super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                airplay_name: Some("   ".into()),
                ..Default::default()
            },
        );
        assert!(err.is_err());
    }

    #[test]
    fn rejects_non_multicast_address() {
        let mut cfg = Config::default();
        let err = super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                multicast_address: Some("10.0.0.1".into()),
                ..Default::default()
            },
        );
        assert!(err.is_err());
    }

    #[test]
    fn rejects_port_zero() {
        let mut cfg = Config::default();
        let err = super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                port: Some(0),
                ..Default::default()
            },
        );
        assert!(err.is_err());
    }

    #[test]
    fn rejects_unknown_network_interface() {
        let mut cfg = Config::default();
        let err = super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                unicast_interface: Some("wledplay-nonexistent-nic-xyz".into()),
                ..Default::default()
            },
        );
        assert!(err.is_err());
    }

    #[test]
    fn rejects_sync_offset_out_of_range() {
        let mut cfg = Config::default();
        assert!(super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                sync_offset_ms: Some(5001),
                ..Default::default()
            }
        )
        .is_err());
        assert!(super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                sync_offset_ms: Some(-1),
                ..Default::default()
            }
        )
        .is_err());
    }

    #[test]
    fn needs_restart_when_applied_settings_diverge() {
        use crate::wled::WledSender;
        use std::net::{IpAddr, Ipv4Addr};
        use std::sync::atomic::AtomicBool;
        use std::sync::{Arc, Mutex};

        let cfg = Config::default();
        let sender = WledSender::new(
            IpAddr::V4(Ipv4Addr::new(239, 0, 0, 1)),
            11988,
            vec![],
            None,
            None,
        )
        .unwrap();
        let control = super::Control {
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
            restart: Arc::new(AtomicBool::new(false)),
        };
        assert!(!control.view().needs_restart);

        let mut changed = cfg.clone();
        changed.network.http = "lo0".into();
        assert!(control.needs_restart(&changed));
        control.request_restart();
        assert!(control.restart.load(std::sync::atomic::Ordering::SeqCst));

        let view = control.view();
        assert!(!view.config_writable);
        assert_eq!(view.values.airplay_name, cfg.airplay.name);
    }

    fn test_control(path: Option<std::path::PathBuf>) -> super::Control {
        use crate::wled::WledSender;
        use std::net::{IpAddr, Ipv4Addr};
        use std::sync::atomic::AtomicBool;
        use std::sync::{Arc, Mutex};

        let cfg = Config::default();
        let sender = WledSender::new(
            IpAddr::V4(Ipv4Addr::new(239, 0, 0, 1)),
            11988,
            vec![],
            None,
            None,
        )
        .unwrap();
        super::Control {
            config: Arc::new(Mutex::new(cfg.clone())),
            sender: Arc::new(Mutex::new(sender)),
            config_path: path,
            applied_name: cfg.airplay.name.clone(),
            applied_http: cfg.network.http.clone(),
            applied_airplay: cfg.network.airplay.clone(),
            applied_unicast: cfg.network.unicast.clone(),
            applied_multicast: cfg.network.multicast.clone(),
            applied_shairport: cfg.audio.shairport_latency_offset_s,
            applied_multicast_address: cfg.wled.multicast_address.clone(),
            applied_port: cfg.wled.port,
            restart: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn port_and_multicast_group_wait_for_restart() {
        let control = test_control(None);
        let seeded = control
            .apply_patch(super::ConfigPatch {
                destinations: Some(vec!["192.0.2.10".into()]),
                ..Default::default()
            })
            .unwrap();
        assert!(!seeded.needs_restart);
        let before = {
            let sender = control.sender.lock().unwrap();
            (sender.multicast_target(), sender.unicast_targets().to_vec())
        };
        assert_eq!(before.0, "239.0.0.1:11988".parse().unwrap());
        assert_eq!(before.1, vec!["192.0.2.10:11988".parse().unwrap()]);

        let port_only = control
            .apply_patch(super::ConfigPatch {
                port: Some(12001),
                ..Default::default()
            })
            .unwrap();
        assert!(port_only.needs_restart);
        assert_eq!(port_only.values.port, 12001);
        let after_port = {
            let sender = control.sender.lock().unwrap();
            (sender.multicast_target(), sender.unicast_targets().to_vec())
        };
        assert_eq!(after_port, before);

        let group_only = control
            .apply_patch(super::ConfigPatch {
                port: Some(11988),
                multicast_address: Some("239.1.1.1".into()),
                ..Default::default()
            })
            .unwrap();
        assert!(group_only.needs_restart);
        assert_eq!(group_only.values.multicast_address, "239.1.1.1");
        assert_eq!(group_only.values.port, 11988);
        let after_group = {
            let sender = control.sender.lock().unwrap();
            (sender.multicast_target(), sender.unicast_targets().to_vec())
        };
        assert_eq!(after_group, before);
    }

    #[test]
    fn apply_patch_persists_and_rejects_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.yaml");
        std::fs::write(&path, "").unwrap();
        let control = test_control(Some(path.clone()));

        let view = control
            .apply_patch(super::ConfigPatch {
                airplay_name: Some("Living Room".into()),
                sync_offset_ms: Some(80),
                gain: Some("lazy".into()),
                shairport_latency_offset_s: Some(-0.5),
                multicast_enabled: Some(false),
                multicast_address: Some("239.1.1.1".into()),
                port: Some(12001),
                destinations: Some(vec!["10.0.0.8".into(), "  ".into()]),
                http_interface: Some("".into()),
                airplay_interface: Some("".into()),
                unicast_interface: Some("".into()),
                multicast_interface: Some("".into()),
                auto_join_enabled: Some(true),
                auto_join_target_id: Some("  aa-bb  ".into()),
                auto_join_target_name: Some(" Kitchen ".into()),
            })
            .unwrap();
        assert!(view.persisted);
        assert_eq!(view.values.airplay_name, "Living Room");
        assert_eq!(view.values.gain, AgcMode::Lazy);
        assert_eq!(view.fields.gain, "live");
        assert_eq!(view.fields.auto_join_enabled, "live");
        assert_eq!(view.fields.auto_join_target_id, "live");
        assert!(view.values.auto_join_enabled);
        assert_eq!(view.values.auto_join_target_id, "aa-bb");
        assert_eq!(view.values.auto_join_target_name, "Kitchen");
        assert_eq!(view.values.port, 12001);
        assert_eq!(view.values.destinations, vec!["10.0.0.8".to_string()]);
        assert!(!view.values.multicast_enabled);
        assert!(view.needs_restart);

        let err = control
            .apply_patch(super::ConfigPatch {
                airplay_name: Some("x".repeat(65)),
                ..Default::default()
            })
            .unwrap_err();
        assert!(err.contains("airplay_name"));

        let mut cfg = Config::default();
        assert!(super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                sync_offset_ms: Some(5001),
                ..Default::default()
            }
        )
        .is_err());
        assert!(super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                sync_offset_ms: Some(-1),
                ..Default::default()
            }
        )
        .is_err());
        assert!(super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                shairport_latency_offset_s: Some(3.0),
                ..Default::default()
            }
        )
        .is_err());
        assert!(super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                multicast_address: Some("nope".into()),
                ..Default::default()
            }
        )
        .is_err());
        assert!(super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                multicast_address: Some("ff02::1".into()),
                ..Default::default()
            }
        )
        .is_err());
        assert!(super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                auto_join_target_id: Some("x".repeat(129)),
                ..Default::default()
            }
        )
        .is_err());
        assert!(super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                auto_join_target_name: Some("n".repeat(129)),
                ..Default::default()
            }
        )
        .is_err());
        super::apply_patch_to(
            &mut cfg,
            super::ConfigPatch {
                auto_join_enabled: Some(true),
                auto_join_target_id: Some("".into()),
                auto_join_target_name: Some("".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(cfg.auto_join.enabled);
        assert!(cfg.auto_join.target_id.is_empty());
        assert!(!control.needs_restart(&cfg));
    }

    #[test]
    fn apply_patch_save_error_is_reported() {
        let control = test_control(Some(std::path::PathBuf::from(
            "/no/such/wledplay-dir/cfg.yaml",
        )));
        let view = control
            .apply_patch(super::ConfigPatch {
                sync_offset_ms: Some(10),
                ..Default::default()
            })
            .unwrap();
        assert!(!view.persisted);
        assert!(view.persist_error.is_some());
        assert!(!view.config_writable);
    }
}
