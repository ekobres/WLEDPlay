use crate::config::Config;
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
    pub restart: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConfigPatch {
    pub airplay_name: Option<String>,
    pub visual_delay_ms: Option<i32>,
    pub shairport_latency_offset_s: Option<f64>,
    pub multicast_enabled: Option<bool>,
    pub multicast_address: Option<String>,
    pub port: Option<u16>,
    pub destinations: Option<Vec<String>>,
    pub http_interface: Option<String>,
    pub airplay_interface: Option<String>,
    pub unicast_interface: Option<String>,
    pub multicast_interface: Option<String>,
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
    pub visual_delay_ms: i32,
    pub shairport_latency_offset_s: f64,
    pub multicast_enabled: bool,
    pub multicast_address: String,
    pub port: u16,
    pub destinations: Vec<String>,
    pub http_interface: String,
    pub airplay_interface: String,
    pub unicast_interface: String,
    pub multicast_interface: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigFields {
    pub visual_delay_ms: &'static str,
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
}

impl ConfigFields {
    fn standard() -> Self {
        Self {
            visual_delay_ms: "live",
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
            sender.set_unicast(cfg.unicast_sockets());
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
    }
}

fn apply_patch_to(cfg: &mut Config, patch: ConfigPatch) -> Result<(), String> {
    if let Some(name) = &patch.airplay_name {
        let name = name.trim();
        if name.is_empty() || name.len() > 64 {
            return Err("airplay_name must be 1–64 characters".into());
        }
    }
    if let Some(ms) = patch.visual_delay_ms {
        if !(-5_000..=5_000).contains(&ms) {
            return Err("visual_delay_ms must be between -5000 and 5000".into());
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
    validate_iface(&patch.http_interface)?;
    validate_iface(&patch.airplay_interface)?;
    validate_iface(&patch.unicast_interface)?;
    validate_iface(&patch.multicast_interface)?;

    if let Some(name) = patch.airplay_name {
        cfg.airplay.name = name.trim().to_string();
    }
    if let Some(ms) = patch.visual_delay_ms {
        apply_visual_delay(cfg, ms);
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

/// Map the unified operator delay onto live send offset and/or Shairport offset.
/// Extra lateness is applied immediately (`sync_offset_ms`). Going earlier than
/// the current Shairport compensation requires a restart.
fn apply_visual_delay(cfg: &mut Config, ms: i32) {
    let shairport_ms = (cfg.audio.shairport_latency_offset_s * 1000.0).round() as i32;
    let live = ms - shairport_ms;
    if live >= 0 {
        cfg.audio.sync_offset_ms = live;
    } else {
        cfg.audio.sync_offset_ms = 0;
        cfg.audio.shairport_latency_offset_s = f64::from(ms) / 1000.0;
    }
}

fn visual_delay_ms(cfg: &Config) -> i32 {
    cfg.audio.sync_offset_ms + (cfg.audio.shairport_latency_offset_s * 1000.0).round() as i32
}

fn values_from(cfg: &Config) -> ConfigValues {
    let visual_delay_ms = visual_delay_ms(cfg);
    ConfigValues {
        airplay_name: cfg.airplay.name.clone(),
        visual_delay_ms,
        shairport_latency_offset_s: cfg.audio.shairport_latency_offset_s,
        multicast_enabled: cfg.wled.multicast_enabled,
        multicast_address: cfg.wled.multicast_address.clone(),
        port: cfg.wled.port,
        destinations: cfg.wled.destinations.clone(),
        http_interface: cfg.network.http.clone(),
        airplay_interface: cfg.network.airplay.clone(),
        unicast_interface: cfg.network.unicast.clone(),
        multicast_interface: cfg.network.multicast.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::apply_visual_delay;
    use crate::config::Config;

    #[test]
    fn positive_delay_is_send_offset() {
        let mut cfg = Config::default();
        apply_visual_delay(&mut cfg, 250);
        assert_eq!(cfg.audio.sync_offset_ms, 250);
        assert_eq!(cfg.audio.shairport_latency_offset_s, 0.0);
    }

    #[test]
    fn negative_delay_sets_shairport_offset() {
        let mut cfg = Config::default();
        apply_visual_delay(&mut cfg, -1200);
        assert_eq!(cfg.audio.sync_offset_ms, 0);
        assert!((cfg.audio.shairport_latency_offset_s - -1.2).abs() < 1e-9);
    }

    #[test]
    fn later_than_current_shairport_is_live_send_offset() {
        let mut cfg = Config::default();
        cfg.audio.shairport_latency_offset_s = -1.2;
        apply_visual_delay(&mut cfg, -1000);
        assert_eq!(cfg.audio.sync_offset_ms, 200);
        assert!((cfg.audio.shairport_latency_offset_s - -1.2).abs() < 1e-9);
        assert_eq!(super::visual_delay_ms(&cfg), -1000);
    }

    #[test]
    fn earlier_than_current_shairport_updates_offset() {
        let mut cfg = Config::default();
        cfg.audio.shairport_latency_offset_s = -1.2;
        apply_visual_delay(&mut cfg, -1400);
        assert_eq!(cfg.audio.sync_offset_ms, 0);
        assert!((cfg.audio.shairport_latency_offset_s - -1.4).abs() < 1e-9);
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
}
