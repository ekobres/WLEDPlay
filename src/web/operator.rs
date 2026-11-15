use crate::config::AgcMode;
use crate::control::ConfigValues;

/// Embedded operator UI (switch + pending Save/Cancel).
pub fn operator_ui() -> &'static str {
    include_str!("index.html")
}

/// Live form values the operator page compares against the last saved config.
#[derive(Debug, Clone)]
pub struct OperatorSnapshot {
    pub sync_offset_ms: i32,
    pub shairport_latency_offset_s: f64,
    pub gain: AgcMode,
    pub airplay_name: String,
    pub http_interface: String,
    pub airplay_interface: String,
    pub unicast_interface: String,
    pub multicast_interface: String,
    pub multicast_enabled: bool,
    pub destinations: Vec<String>,
    pub auto_join_enabled: bool,
    pub auto_join_target_id: String,
    pub auto_join_target_name: String,
}

pub fn dest_lines(text: &str) -> Vec<String> {
    text.split('\n')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn parse_send_delay_ms(raw: &str) -> i32 {
    raw.trim()
        .parse::<f64>()
        .ok()
        .map(|n| n.max(0.0) as i32)
        .unwrap_or(0)
}

pub fn parse_shairport_s(raw: &str) -> f64 {
    raw.trim().parse::<f64>().unwrap_or(0.0)
}

impl PartialEq for OperatorSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.sync_offset_ms == other.sync_offset_ms
            && (self.shairport_latency_offset_s - other.shairport_latency_offset_s).abs() < 0.0001
            && self.gain == other.gain
            && self.airplay_name == other.airplay_name
            && self.http_interface == other.http_interface
            && self.airplay_interface == other.airplay_interface
            && self.unicast_interface == other.unicast_interface
            && self.multicast_interface == other.multicast_interface
            && self.multicast_enabled == other.multicast_enabled
            && self.destinations == other.destinations
            && self.auto_join_enabled == other.auto_join_enabled
            && self.auto_join_target_id == other.auto_join_target_id
            && self.auto_join_target_name == other.auto_join_target_name
    }
}

impl OperatorSnapshot {
    pub fn from_values(v: &ConfigValues) -> Self {
        Self {
            sync_offset_ms: v.sync_offset_ms,
            shairport_latency_offset_s: v.shairport_latency_offset_s,
            gain: v.gain,
            airplay_name: v.airplay_name.trim().to_string(),
            http_interface: v.http_interface.clone(),
            airplay_interface: v.airplay_interface.clone(),
            unicast_interface: v.unicast_interface.clone(),
            multicast_interface: v.multicast_interface.clone(),
            multicast_enabled: v.multicast_enabled,
            destinations: v.destinations.clone(),
            auto_join_enabled: v.auto_join_enabled,
            auto_join_target_id: v.auto_join_target_id.trim().to_string(),
            auto_join_target_name: v.auto_join_target_name.trim().to_string(),
        }
    }

    pub fn is_dirty(&self, saved: &Self) -> bool {
        self != saved
    }

    pub fn nics_changed(&self, saved: &Self) -> bool {
        self.http_interface != saved.http_interface
            || self.airplay_interface != saved.airplay_interface
            || self.unicast_interface != saved.unicast_interface
            || self.multicast_interface != saved.multicast_interface
    }

    pub fn shairport_changed(&self, saved: &Self) -> bool {
        (self.shairport_latency_offset_s - saved.shairport_latency_offset_s).abs() > 0.0001
    }

    /// Mirrors the UI confirm: name/NIC edits need a restart warning.
    pub fn needs_restart_confirm(&self, saved: &Self, saved_has_name: bool) -> bool {
        saved_has_name
            && ((!self.airplay_name.is_empty() && self.airplay_name != saved.airplay_name)
                || self.nics_changed(saved))
    }
}

/// Overwrite inputs from the server only when forced or the form is clean.
pub fn should_overwrite_fields(force: bool, dirty: bool) -> bool {
    force || !dirty
}

pub fn action_buttons_enabled(dirty: bool) -> bool {
    dirty
}

pub fn operator_hint(
    dirty: bool,
    needs_restart: bool,
    persisted: bool,
    config_writable: bool,
    persist_error: Option<&str>,
) -> String {
    let mut bits = Vec::new();
    if dirty {
        bits.push("unsaved changes".to_string());
    }
    if needs_restart {
        bits.push("restart needed".to_string());
    }
    if !dirty && persisted {
        bits.push("saved".to_string());
    } else if !config_writable {
        bits.push("config not writable".to_string());
    }
    if let Some(err) = persist_error {
        bits.push(err.to_string());
    }
    bits.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(name: &str, dest: &[&str]) -> ConfigValues {
        ConfigValues {
            airplay_name: name.into(),
            sync_offset_ms: 1200,
            gain: AgcMode::Normal,
            shairport_latency_offset_s: -1.2,
            on_time_send_delay_ms: 1200,
            multicast_enabled: true,
            multicast_address: "239.0.0.1".into(),
            port: 11988,
            destinations: dest.iter().map(|s| (*s).to_string()).collect(),
            http_interface: String::new(),
            airplay_interface: String::new(),
            unicast_interface: String::new(),
            multicast_interface: String::new(),
            auto_join_enabled: false,
            auto_join_target_id: String::new(),
            auto_join_target_name: String::new(),
        }
    }

    fn base() -> OperatorSnapshot {
        OperatorSnapshot::from_values(&values("WLEDPlay", &["10.11.1.124"]))
    }

    #[test]
    fn operator_ui_split_delay_controls() {
        let html = operator_ui();
        assert!(html.contains("id=\"ctl-shairport\""));
        assert!(html.contains("id=\"ctl-delay\""));
        assert!(html.contains("id=\"ctl-gain\""));
        assert!(html.contains("value=\"off\""));
        assert!(html.contains("value=\"normal\" selected"));
        assert!(html.contains("value=\"vivid\""));
        assert!(html.contains("value=\"lazy\""));
        assert!(html.contains("configHydrated"));
        assert!(html.contains("fillConfig(c, first)"));
        assert!(html.contains("id=\"ctl-delay-hint\""));
        assert!(html.contains("Shairport lead (s)"));
        assert!(html.contains("Send delay (ms)"));
        assert!(html.contains("sync_offset_ms"));
        assert!(html.contains("shairport_latency_offset_s"));
        assert!(html.contains("gain"));
        assert!(html.contains("on_time_send_delay_ms"));
        assert!(html.contains("function shairportChanged"));
        assert!(!html.contains("visual_delay_ms"));
        assert!(html.contains("role=\"switch\""));
        assert!(html.contains("id=\"ctl-cancel\""));
        assert!(html.contains("id=\"ctl-autojoin\""));
        assert!(html.contains("id=\"ctl-autojoin-target\""));
        assert!(html.contains("id=\"autojoin-discovery\""));
        assert!(html.contains("document.activeElement === sel"));
        assert!(html.contains("function sortedDevices"));
        assert!(html.contains("toLocaleLowerCase"));
        assert!(html.contains("aj.discovery_error"));
        assert!(html.contains("aj.last_error"));
        assert!(html.contains("runtime.includes(aj.last_error)"));
        assert!(html.contains("phone_owned"));
        assert!(html.contains("notice"));
        assert!(html.contains("class=\"alert\"") || html.contains("\"alert\""));
        assert!(html.contains("id=\"autojoin-target-status\""));
        assert!(html.contains("id=\"autojoin-runtime\""));
        assert!(html.contains("No target"));
        assert!(html.contains("auto_join_enabled"));
        assert!(html.contains("/airplay/devices"));
        assert!(html.contains("function isDirty()"));
        assert!(!html.contains("Visualizer on"));
        assert!(html.contains("id=\"viz-source\""));
        assert!(html.contains("On LEDs (sent)"));
        assert!(html.contains("Received (early)"));
        assert!(html.contains("function sourceMeters"));
        assert!(html.contains("id=\"clock\""));
        assert!(html.contains("Now playing"));
        assert!(html.contains(">Statistics</div>"));
        assert!(html.contains(">PTP sample</div>"));
        assert!(html.contains("sample_age_ms"));
        assert!(!html.contains("Playback lead"));
        assert!(html.contains("function paintClock"));
        assert!(html.contains(">Session</div>"));
        assert!(html.contains("id=\"sess-target\""));
        assert!(html.contains("id=\"sess-flags\""));
        assert!(html.contains("id=\"sess-leader\""));
        assert!(html.contains("id=\"sess-group\""));
        assert!(html.contains("id=\"sess-encoding\""));
        assert!(html.contains("id=\"sess-client\""));
        assert!(html.contains("function paintSession"));
        assert!(html.contains("function sessionFlags"));
        let session_at = html.find(">Session</div>").unwrap();
        let control_at = html.find(">Control</div>").unwrap();
        let bands_at = html.find("id=\"bands\"").unwrap();
        assert!(bands_at < session_at && session_at < control_at);
        assert!(html.contains(">Control</div>"));
        assert!(html.contains(">Configuration</div>"));
        assert!(html.contains("Applies immediately"));
        assert!(html.contains("Takes effect after restart"));
        assert!(html.contains("popovertarget"));
        assert!(html.contains("function placeTip"));
        assert!(html.contains("ev.newState === \"open\""));
        assert!(html.contains("shownRms >= 0.95"));
        assert!(!html.contains("s.peak ?"));
        assert!(html.contains("id=\"viz-decay\""));
        assert!(html.contains("id=\"beat\""));
        assert!(html.contains("id=\"beatbar\""));
        assert!(html.contains("function fallToward"));
        assert!(html.contains("id=\"level-peak\""));
        assert!(html.contains("id=\"trackbar\""));
        assert!(html.contains("id=\"track-pos\""));
        assert!(html.contains("function formatClock"));
        assert!(html.contains("function adoptTrackProgress"));
        assert!(html.contains("function paintTrackProgress"));
        assert!(html.contains("md.position_s == null"));
        assert!(html.contains("padStart(2, \"0\")"));
        assert!(html.contains("padStart(3, \" \")"));
        assert!(html.contains("padStart(7, \" \")"));
        assert!(!html.contains("Dominant frequency"));
        assert!(html.contains(">Freq</span>"));
        assert!(html.contains(">AGC</span>"));
        assert!(html.contains("id=\"agc\""));
        assert!(html.contains("agc_on"));
        assert!(html.contains("padStart(6, \" \")"));
        let bands_at = html.find("id=\"bands\"").expect("bands");
        let level_at = html.find("id=\"levelbar\"").expect("levelbar");
        let stats_at = html.find("id=\"counters\"").expect("counters");
        assert!(stats_at < bands_at && bands_at < level_at);
    }

    #[test]
    fn dest_lines_and_delay_parse() {
        assert!(dest_lines("").is_empty());
        assert!(dest_lines("  \n  ").is_empty());
        assert_eq!(
            dest_lines(" 10.0.0.1 \n\n10.0.0.2\n"),
            ["10.0.0.1", "10.0.0.2"]
        );
        assert_eq!(parse_send_delay_ms(""), 0);
        assert_eq!(parse_send_delay_ms("1200"), 1200);
        assert_eq!(parse_send_delay_ms("-50"), 0);
        assert_eq!(parse_send_delay_ms("100.9"), 100);
        assert_eq!(parse_send_delay_ms("nope"), 0);
        assert!((parse_shairport_s("-1.2") - -1.2).abs() < 1e-9);
    }

    #[test]
    fn snapshots_dirty_and_nics() {
        let saved = base();
        let form = OperatorSnapshot {
            sync_offset_ms: 1200,
            shairport_latency_offset_s: -1.2,
            gain: AgcMode::Normal,
            airplay_name: "  WLEDPlay  ".trim().to_string(),
            http_interface: String::new(),
            airplay_interface: String::new(),
            unicast_interface: String::new(),
            multicast_interface: String::new(),
            multicast_enabled: true,
            destinations: dest_lines("10.11.1.124\n"),
            auto_join_enabled: false,
            auto_join_target_id: String::new(),
            auto_join_target_name: String::new(),
        };
        assert!(!form.is_dirty(&saved));
        assert!(!action_buttons_enabled(false));
        assert!(action_buttons_enabled(true));

        let mut dirty = form.clone();
        dirty.sync_offset_ms = 800;
        assert!(dirty.is_dirty(&saved));
        dirty.gain = AgcMode::Vivid;
        assert!(dirty.is_dirty(&saved));
        dirty.gain = AgcMode::Normal;
        dirty.sync_offset_ms = 1200;
        dirty.gain = AgcMode::Off;
        assert!(dirty.is_dirty(&saved));
        dirty.gain = AgcMode::Normal;
        assert!(!dirty.is_dirty(&saved));
        assert!(!dirty.shairport_changed(&saved));

        dirty.shairport_latency_offset_s = -2.0;
        assert!(dirty.shairport_changed(&saved));

        dirty = form.clone();
        dirty.http_interface = "en0".into();
        assert!(dirty.is_dirty(&saved));
        assert!(dirty.nics_changed(&saved));
        dirty.http_interface.clear();
        dirty.airplay_interface = "en0".into();
        assert!(dirty.nics_changed(&saved));
        dirty.airplay_interface.clear();
        dirty.unicast_interface = "en0".into();
        assert!(dirty.nics_changed(&saved));
        dirty.unicast_interface.clear();
        dirty.multicast_interface = "en0".into();
        assert!(dirty.nics_changed(&saved));
        dirty.multicast_interface.clear();
        assert!(!dirty.nics_changed(&saved));

        dirty.destinations = vec!["10.11.1.200".into()];
        assert!(dirty.is_dirty(&saved));
        assert!(!dirty.nics_changed(&saved));
        dirty.destinations = saved.destinations.clone();
        dirty.auto_join_enabled = true;
        assert!(dirty.is_dirty(&saved));
        assert!(!dirty.needs_restart_confirm(&saved, true));
        dirty.auto_join_target_id = "speaker-1".into();
        dirty.auto_join_target_name = "Kitchen".into();
        assert!(dirty.is_dirty(&saved));
    }

    #[test]
    fn restart_confirm_and_overwrite() {
        let saved = base();
        let mut form = saved.clone();
        assert!(!form.needs_restart_confirm(&saved, true));
        assert!(!form.needs_restart_confirm(&saved, false));

        form.airplay_name = "Room".into();
        assert!(form.needs_restart_confirm(&saved, true));
        assert!(!form.needs_restart_confirm(&saved, false));

        form.airplay_name.clear();
        assert!(!form.needs_restart_confirm(&saved, true));
        form.http_interface = "en0".into();
        assert!(form.needs_restart_confirm(&saved, true));

        assert!(should_overwrite_fields(true, true));
        assert!(should_overwrite_fields(true, false));
        assert!(should_overwrite_fields(false, false));
        assert!(!should_overwrite_fields(false, true));
    }

    #[test]
    fn hint_branches() {
        assert_eq!(operator_hint(false, false, false, true, None), "");
        assert_eq!(
            operator_hint(true, false, true, true, None),
            "unsaved changes"
        );
        assert_eq!(
            operator_hint(false, true, true, true, None),
            "restart needed · saved"
        );
        assert_eq!(
            operator_hint(true, true, false, false, Some("disk full")),
            "unsaved changes · restart needed · config not writable · disk full"
        );
        assert_eq!(
            operator_hint(false, false, false, false, None),
            "config not writable"
        );
        assert_eq!(operator_hint(false, false, true, false, None), "saved");
    }

    #[test]
    fn from_values_trims_name() {
        let v = values("  padded  ", &[]);
        let snap = OperatorSnapshot::from_values(&v);
        assert_eq!(snap.airplay_name, "padded");
        assert_eq!(snap.auto_join_target_id, "");
        assert!(!snap.auto_join_enabled);
        assert_eq!(snap.sync_offset_ms, 1200);
        assert!((snap.shairport_latency_offset_s - -1.2).abs() < 1e-9);
        assert!(snap.destinations.is_empty());
        assert!(snap.multicast_enabled);
    }
}
