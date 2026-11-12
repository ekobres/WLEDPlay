//! Idle-only AirPlay group join. Policy lives here; MRP lives in the sidecar.

mod sidecar;

pub use sidecar::{from_env, AutoJoinHub};

use crate::web::{AirPlayDeviceInfo, AutoJoinStatus};
use serde::Deserialize;

pub const IDLE_DEBOUNCE_MS: u64 = 3_000;
pub const JOIN_TIMEOUT_MS: u64 = 45_000;
pub const REQUEST_TIMEOUT_MS: u64 = 20_000;
const RETRY_DELAYS_MS: [u64; 5] = [0, 500, 1_000, 2_000, 4_000];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AirPlayDevice {
    pub id: String,
    pub name: String,
    pub model: String,
    pub address: String,
    pub available: bool,
    pub playing: bool,
    pub airplay2: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinRequest {
    pub target_id: String,
    pub receiver_id: String,
    pub revision: u64,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinResult {
    pub revision: u64,
    pub generation: u64,
    pub ok: bool,
    pub error: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Disabled,
    NoTarget,
    IdleWaiting,
    TargetUnavailable,
    TargetActive,
    Joining,
    Joined,
    Busy,
    Error,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::NoTarget => "no_target",
            Self::IdleWaiting => "idle_waiting",
            Self::TargetUnavailable => "target_unavailable",
            Self::TargetActive => "target_active",
            Self::Joining => "joining",
            Self::Joined => "joined",
            Self::Busy => "busy",
            Self::Error => "error",
        }
    }
}

pub struct TickInput<'a> {
    pub now_ms: u64,
    pub raw_busy: bool,
    pub enabled: bool,
    pub target_id: &'a str,
    pub target_name: &'a str,
    pub self_id: &'a str,
    pub devices: &'a [AirPlayDevice],
    pub discovery_active: bool,
    pub discovery_error: &'a str,
    pub can_join: bool,
    pub join_results: &'a [JoinResult],
}

pub struct TickOutput {
    pub view: AutoJoinStatus,
    pub join: Option<JoinRequest>,
    pub logs: Vec<String>,
}

#[derive(Debug)]
pub struct AutoJoinService {
    phase: Phase,
    revision: u64,
    generation: u64,
    have_seen: bool,
    seen_enabled: bool,
    seen_target: String,
    seen_name: String,
    pending: bool,
    was_in_session: bool,
    idle_since: Option<u64>,
    attempt: usize,
    next_try_ms: u64,
    outstanding: bool,
    outstanding_deadline: u64,
    awaiting_pcm: bool,
    pcm_deadline: u64,
    last_error: String,
    last_log: String,
    target_was_available: bool,
    announced_missing: bool,
    was_playing: bool,
    hold: bool,
}

impl Default for AutoJoinService {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoJoinService {
    pub fn new() -> Self {
        Self {
            phase: Phase::Disabled,
            revision: 0,
            generation: 0,
            have_seen: false,
            seen_enabled: false,
            seen_target: String::new(),
            seen_name: String::new(),
            pending: false,
            was_in_session: false,
            idle_since: None,
            attempt: 0,
            next_try_ms: 0,
            outstanding: false,
            outstanding_deadline: 0,
            awaiting_pcm: false,
            pcm_deadline: 0,
            last_error: String::new(),
            last_log: String::new(),
            target_was_available: false,
            announced_missing: false,
            was_playing: false,
            hold: false,
        }
    }

    pub fn abandon_join(&mut self, now_ms: u64) {
        if !self.outstanding {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        self.outstanding = false;
        let mut logs = Vec::new();
        self.fail(now_ms, "sidecar command not sent", &mut logs);
    }

    pub fn tick(&mut self, input: TickInput<'_>) -> TickOutput {
        let mut logs = Vec::new();
        self.note_config(&input, &mut logs);
        self.note_presence(&input, &mut logs);
        self.update_idle(input.now_ms, input.raw_busy);
        if !input.raw_busy {
            self.apply_results(&input, &mut logs);
            self.arm_timeouts(input.now_ms, &mut logs);
        } else {
            self.clear_finished_requests(&input);
        }

        if input.raw_busy {
            if self.phase == Phase::Joining || self.awaiting_pcm || self.phase == Phase::Error {
                self.succeed(&mut logs);
            } else if self.phase != Phase::Joined {
                self.phase = Phase::Busy;
            }
            self.hold = false;
            return self.finish(&input, None, logs);
        }

        if self.hold {
            return self.finish(&input, None, logs);
        }

        if !input.enabled {
            self.cancel_join();
            self.phase = Phase::Disabled;
            self.attempt = 0;
            return self.finish(&input, None, logs);
        }
        if input.target_id.is_empty() {
            self.cancel_join();
            self.phase = Phase::NoTarget;
            self.attempt = 0;
            self.last_error.clear();
            return self.finish(&input, None, logs);
        }

        let device = find_target(input.devices, input.target_id);
        let available = device.is_some_and(|d| d.available);
        let playing = device.is_some_and(|d| d.playing && d.available);
        if !available {
            self.cancel_join();
            self.phase = Phase::TargetUnavailable;
            self.attempt = 0;
            return self.finish(&input, None, logs);
        }
        if !playing {
            self.cancel_join();
            self.phase = Phase::IdleWaiting;
            self.attempt = 0;
            self.last_error.clear();
            self.was_playing = false;
            return self.finish(&input, None, logs);
        }
        if !self.was_playing {
            self.log(
                &mut logs,
                format!(
                    "Auto Join target started playback: {}",
                    display_target(&self.seen_name, input.target_id)
                ),
            );
            self.was_playing = true;
        }
        if self.phase == Phase::Error {
            return self.finish(&input, None, logs);
        }
        if self.outstanding {
            self.phase = Phase::Joining;
            return self.finish(&input, None, logs);
        }
        if self.awaiting_pcm {
            self.phase = Phase::Joining;
            return self.finish(&input, None, logs);
        }
        if !input.can_join {
            self.phase = Phase::TargetActive;
            self.last_error = "discovery inactive".into();
            self.log(&mut logs, "Auto Join failed: discovery inactive".into());
            return self.finish(&input, None, logs);
        }
        if input.now_ms < self.next_try_ms {
            self.phase = Phase::TargetActive;
            return self.finish(&input, None, logs);
        }
        if input.self_id.is_empty() {
            self.phase = Phase::TargetActive;
            self.last_error = "unable to identify WLEDPlay receiver".into();
            self.log(
                &mut logs,
                "Auto Join failed: unable to identify WLEDPlay receiver".into(),
            );
            return self.finish(&input, None, logs);
        }

        self.generation = self.generation.wrapping_add(1);
        self.outstanding = true;
        self.outstanding_deadline = input.now_ms.saturating_add(REQUEST_TIMEOUT_MS);
        self.phase = Phase::Joining;
        let name = display_target(&self.seen_name, input.target_id);
        self.log(
            &mut logs,
            format!("Auto Join group leader identified: {name}"),
        );
        self.log(
            &mut logs,
            format!(
                "Auto Join attempting to add WLEDPlay to AirPlay group (attempt {})",
                self.attempt + 1
            ),
        );
        let join = JoinRequest {
            target_id: input.target_id.to_string(),
            receiver_id: input.self_id.to_string(),
            revision: self.revision,
            generation: self.generation,
        };
        self.finish(&input, Some(join), logs)
    }

    fn note_config(&mut self, input: &TickInput<'_>, logs: &mut Vec<String>) {
        let id = input.target_id;
        let name = if input.target_name.is_empty() {
            id.to_string()
        } else {
            input.target_name.to_string()
        };
        if !self.have_seen {
            self.have_seen = true;
            self.seen_enabled = input.enabled;
            self.seen_target = id.to_string();
            self.seen_name = name;
            if input.enabled {
                self.log(logs, "Auto Join enabled".into());
            }
            return;
        }
        if id == self.seen_target && name != self.seen_name {
            self.seen_name = name.clone();
        }
        let changed = input.enabled != self.seen_enabled || id != self.seen_target;
        if !changed {
            return;
        }
        self.revision = self.revision.wrapping_add(1);
        if input.enabled != self.seen_enabled {
            self.log(
                logs,
                if input.enabled {
                    "Auto Join enabled".into()
                } else {
                    "Auto Join disabled".into()
                },
            );
        }
        if id != self.seen_target {
            let old = display_target(&self.seen_name, &self.seen_target);
            self.log(logs, format!("Auto Join target changed: {old} -> {name}"));
            self.seen_target = id.to_string();
            self.seen_name = name;
            self.target_was_available = false;
            self.announced_missing = false;
            self.was_playing = false;
            self.attempt = 0;
            self.next_try_ms = 0;
        }
        self.seen_enabled = input.enabled;
        if input.raw_busy {
            self.pending = true;
            self.log(
                logs,
                "Auto Join configuration changed while active; change deferred until idle".into(),
            );
            self.log(logs, "Auto Join deferred: WLEDPlay currently busy".into());
        } else {
            self.cancel_join();
            self.pending = false;
        }
    }

    fn note_presence(&mut self, input: &TickInput<'_>, logs: &mut Vec<String>) {
        if self.seen_target.is_empty() {
            return;
        }
        let present = find_target(input.devices, &self.seen_target).is_some_and(|d| d.available);
        let label = display_target(&self.seen_name, &self.seen_target);
        if present && !self.target_was_available {
            self.log(logs, format!("Auto Join target discovered: {label}"));
            self.announced_missing = false;
        } else if !present && self.target_was_available {
            self.log(logs, format!("Auto Join target unavailable: {label}"));
        } else if !present && !self.announced_missing {
            self.log(logs, format!("Auto Join target unavailable: {label}"));
            self.announced_missing = true;
        }
        self.target_was_available = present;
    }

    fn update_idle(&mut self, now: u64, raw_busy: bool) {
        if raw_busy {
            self.was_in_session = true;
            self.idle_since = None;
            self.hold = false;
            return;
        }
        if self.idle_since.is_none() {
            self.idle_since = Some(if self.was_in_session {
                now
            } else {
                now.saturating_sub(IDLE_DEBOUNCE_MS)
            });
        }
        let since = self.idle_since.unwrap_or(now);
        let debounced = now.saturating_sub(since) >= IDLE_DEBOUNCE_MS;
        self.hold = self.was_in_session && !debounced;
        if debounced {
            self.was_in_session = false;
            self.pending = false;
        }
    }

    fn clear_finished_requests(&mut self, input: &TickInput<'_>) {
        for result in input.join_results {
            if result.generation == self.generation {
                self.outstanding = false;
            }
        }
    }

    fn apply_results(&mut self, input: &TickInput<'_>, logs: &mut Vec<String>) {
        for result in input.join_results {
            if result.generation != self.generation {
                continue;
            }
            self.outstanding = false;
            if result.revision != self.revision {
                self.log(logs, "Auto Join aborted: configuration changed".into());
                continue;
            }
            if result.ok {
                self.awaiting_pcm = true;
                self.pcm_deadline = input.now_ms.saturating_add(JOIN_TIMEOUT_MS);
                if self.phase != Phase::Joined {
                    self.phase = Phase::Joining;
                }
                self.log(
                    logs,
                    "Auto Join join request succeeded; waiting for inbound AirPlay session".into(),
                );
            } else if self.phase != Phase::Joined && self.phase != Phase::Busy {
                self.fail(input.now_ms, &result.error, logs);
            }
        }
    }

    fn arm_timeouts(&mut self, now: u64, logs: &mut Vec<String>) {
        if self.outstanding && now >= self.outstanding_deadline {
            self.generation = self.generation.wrapping_add(1);
            self.outstanding = false;
            self.fail(now, "group request timed out", logs);
        }
        if self.awaiting_pcm && now >= self.pcm_deadline {
            self.awaiting_pcm = false;
            self.fail(now, "timed out waiting for inbound AirPlay session", logs);
        }
    }

    fn fail(&mut self, now: u64, err: &str, logs: &mut Vec<String>) {
        self.awaiting_pcm = false;
        self.outstanding = false;
        self.last_error = err.to_string();
        self.attempt = self.attempt.saturating_add(1);
        // Another add uses session interruption and drops the RTSP session that
        // already delivered metadata. Leave that session up and wait for PCM.
        if err == "timed out waiting for inbound AirPlay session" {
            self.attempt = RETRY_DELAYS_MS.len();
        }
        if self.attempt >= RETRY_DELAYS_MS.len() {
            self.phase = Phase::Error;
        } else {
            self.next_try_ms = now.saturating_add(RETRY_DELAYS_MS[self.attempt]);
            self.phase = Phase::TargetActive;
        }
        self.log(
            logs,
            format!("Auto Join failed: {err} (attempt {})", self.attempt),
        );
    }

    fn succeed(&mut self, logs: &mut Vec<String>) {
        self.phase = Phase::Joined;
        self.attempt = 0;
        self.awaiting_pcm = false;
        self.outstanding = false;
        self.last_error.clear();
        self.log(logs, "Auto Join successful".into());
    }

    fn cancel_join(&mut self) {
        self.outstanding = false;
        self.awaiting_pcm = false;
    }

    fn log(&mut self, logs: &mut Vec<String>, msg: String) {
        if self.last_log == msg {
            return;
        }
        self.last_log = msg.clone();
        tracing::info!("{msg}");
        logs.push(msg);
    }

    fn finish(
        &self,
        input: &TickInput<'_>,
        join: Option<JoinRequest>,
        logs: Vec<String>,
    ) -> TickOutput {
        let device = find_target(input.devices, input.target_id);
        let target_available = device.is_some_and(|d| d.available);
        let target_playing = device.is_some_and(|d| d.playing && d.available);
        let target_status = if input.target_id.is_empty() {
            String::new()
        } else if !target_available {
            "Offline".into()
        } else if target_playing {
            "Playing".into()
        } else {
            "Idle".into()
        };
        let name = display_target(&self.seen_name, input.target_id);
        let mut label = match self.phase {
            Phase::Disabled => "Disabled".to_string(),
            Phase::NoTarget => "Enabled — no target selected".to_string(),
            Phase::IdleWaiting => format!("Waiting for {name}"),
            Phase::TargetUnavailable => "Target unavailable".to_string(),
            Phase::TargetActive => "Target playing".to_string(),
            Phase::Joining => "Joining group".to_string(),
            Phase::Joined => "Joined".to_string(),
            Phase::Busy => "WLEDPlay busy".to_string(),
            Phase::Error => format!("Join failed · {}", self.last_error),
        };
        if self.pending || self.hold {
            label = "Waiting until idle".into();
        }
        let device_count = detected_count(input.devices, input.self_id, name_for_self(input));
        TickOutput {
            view: AutoJoinStatus {
                enabled: input.enabled,
                target_id: input.target_id.to_string(),
                target_name: input.target_name.to_string(),
                state: self.phase.as_str().into(),
                label,
                target_available,
                target_playing,
                target_status,
                wledplay_busy: input.raw_busy,
                pending_config_change: self.pending,
                discovery_active: input.discovery_active,
                device_count,
                last_error: self.last_error.clone(),
                self_id: input.self_id.to_string(),
                discovery_error: input.discovery_error.to_string(),
            },
            join,
            logs,
        }
    }
}

fn name_for_self<'a>(input: &TickInput<'a>) -> &'a str {
    input
        .devices
        .iter()
        .find(|d| !input.self_id.is_empty() && d.id.eq_ignore_ascii_case(input.self_id))
        .map(|d| d.name.as_str())
        .unwrap_or("")
}

fn display_target(name: &str, id: &str) -> String {
    if !name.is_empty() {
        name.to_string()
    } else if !id.is_empty() {
        id.to_string()
    } else {
        "(none)".into()
    }
}

fn find_target<'a>(devices: &'a [AirPlayDevice], id: &str) -> Option<&'a AirPlayDevice> {
    if id.is_empty() {
        return None;
    }
    devices.iter().find(|d| d.id.eq_ignore_ascii_case(id))
}

pub fn detected_count(devices: &[AirPlayDevice], self_id: &str, self_name: &str) -> u32 {
    devices
        .iter()
        .filter(|d| selectable(d, self_id, self_name))
        .count() as u32
}

fn selectable(device: &AirPlayDevice, self_id: &str, self_name: &str) -> bool {
    if !device.airplay2 {
        return false;
    }
    if !self_id.is_empty() && device.id.eq_ignore_ascii_case(self_id) {
        return false;
    }
    if !self_name.is_empty() && device.name.eq_ignore_ascii_case(self_name) {
        return false;
    }
    true
}

pub fn device_choices(
    devices: &[AirPlayDevice],
    self_id: &str,
    self_name: &str,
    target_id: &str,
    target_name: &str,
) -> Vec<AirPlayDeviceInfo> {
    let shown: Vec<&AirPlayDevice> = devices
        .iter()
        .filter(|d| selectable(d, self_id, self_name))
        .collect();
    let mut choices: Vec<AirPlayDeviceInfo> = shown
        .iter()
        .map(|d| {
            let dup = shown.iter().filter(|o| o.name == d.name).count() > 1;
            AirPlayDeviceInfo {
                id: d.id.clone(),
                name: d.name.clone(),
                label: choice_label(d, dup),
                model: d.model.clone(),
                address: d.address.clone(),
                available: d.available,
                playing: d.playing,
            }
        })
        .collect();
    if !target_id.is_empty() && !choices.iter().any(|c| c.id.eq_ignore_ascii_case(target_id)) {
        let name = if target_name.is_empty() {
            target_id.to_string()
        } else {
            target_name.to_string()
        };
        choices.push(AirPlayDeviceInfo {
            id: target_id.to_string(),
            name: name.clone(),
            label: format!("{name} — currently unavailable"),
            model: String::new(),
            address: String::new(),
            available: false,
            playing: false,
        });
    }
    choices.sort_by(|a, b| {
        a.label
            .to_ascii_lowercase()
            .cmp(&b.label.to_ascii_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    choices
}

fn choice_label(device: &AirPlayDevice, duplicate: bool) -> String {
    let name = if device.name.is_empty() {
        device.id.as_str()
    } else {
        device.name.as_str()
    };
    if !duplicate {
        return name.to_string();
    }
    if !device.model.is_empty() {
        return format!("{name} — {}", device.model);
    }
    let suffix = if device.id.len() > 8 {
        &device.id[device.id.len() - 8..]
    } else {
        device.id.as_str()
    };
    format!("{name} — {suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, name: &str, playing: bool) -> AirPlayDevice {
        AirPlayDevice {
            id: id.into(),
            name: name.into(),
            model: String::new(),
            address: "10.0.0.8".into(),
            available: true,
            playing,
            airplay2: true,
        }
    }

    fn tick(
        svc: &mut AutoJoinService,
        now: u64,
        busy: bool,
        enabled: bool,
        target: &str,
        name: &str,
        self_id: &str,
        devices: &[AirPlayDevice],
        can_join: bool,
        results: &[JoinResult],
    ) -> TickOutput {
        svc.tick(TickInput {
            now_ms: now,
            raw_busy: busy,
            enabled,
            target_id: target,
            target_name: name,
            self_id,
            devices,
            discovery_active: true,
            discovery_error: "",
            can_join,
            join_results: results,
        })
    }

    #[test]
    fn stays_disabled_until_enabled() {
        let mut svc = AutoJoinService::new();
        let out = tick(&mut svc, 0, false, false, "", "", "", &[], true, &[]);
        assert_eq!(out.view.state, "disabled");
        assert!(out.join.is_none());
        assert!(out.logs.is_empty());
        svc.abandon_join(0);
    }

    #[test]
    fn enable_without_target_does_not_join() {
        let mut svc = AutoJoinService::new();
        let out = tick(&mut svc, 0, false, true, "", "", "self", &[], true, &[]);
        assert_eq!(out.view.label, "Enabled — no target selected");
        assert!(out.logs.iter().any(|l| l == "Auto Join enabled"));
        assert!(out.join.is_none());
    }

    #[test]
    fn missing_target_is_retained_as_offline() {
        let mut svc = AutoJoinService::new();
        let out = tick(
            &mut svc,
            0,
            false,
            true,
            "aa",
            "Living Room",
            "self",
            &[],
            true,
            &[],
        );
        assert_eq!(out.view.state, "target_unavailable");
        assert_eq!(out.view.target_status, "Offline");
        assert!(out.logs.iter().any(|l| l.contains("unavailable")));
        let again = tick(
            &mut svc,
            10,
            false,
            true,
            "aa",
            "Living Room",
            "self",
            &[],
            true,
            &[],
        );
        assert!(again.logs.is_empty());
    }

    #[test]
    fn idle_target_then_playback_issues_one_join() {
        let mut svc = AutoJoinService::new();
        let devices = [dev("aa", "Kitchen", false), dev("self", "WLEDPlay", false)];
        let waiting = tick(
            &mut svc,
            0,
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &devices,
            true,
            &[],
        );
        assert_eq!(waiting.view.label, "Waiting for Kitchen");
        assert!(waiting.logs.iter().any(|l| l.contains("discovered")));
        let join = tick(
            &mut svc,
            20,
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &[dev("aa", "Kitchen", true), dev("self", "WLEDPlay", false)],
            true,
            &[],
        );
        let req = join.join.expect("join");
        assert_eq!(req.target_id, "aa");
        assert_eq!(req.receiver_id, "self");
        assert_eq!(join.view.label, "Joining group");
        assert!(join.logs.iter().any(|l| l.contains("group leader")));
        assert!(join.logs.iter().any(|l| l.contains("started playback")));
        let held = tick(
            &mut svc,
            30,
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &[dev("aa", "Kitchen", true)],
            true,
            &[],
        );
        assert!(held.join.is_none());
        assert_eq!(held.view.state, "joining");
    }

    #[test]
    fn pcm_not_mrp_marks_success_and_failure_retries() {
        let mut svc = AutoJoinService::new();
        let playing = [dev("aa", "Kitchen", true)];
        let first = tick(
            &mut svc,
            0,
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            true,
            &[],
        );
        let req = first.join.unwrap();
        let accepted = JoinResult {
            revision: req.revision,
            generation: req.generation,
            ok: true,
            error: String::new(),
        };
        let wait = tick(
            &mut svc,
            10,
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            true,
            &[accepted],
        );
        assert!(wait.logs.iter().any(|l| l.contains("waiting for inbound")));
        assert_eq!(wait.view.state, "joining");
        assert!(wait.join.is_none());
        let joined = tick(
            &mut svc,
            20,
            true,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            true,
            &[],
        );
        assert_eq!(joined.view.state, "joined");
        assert_eq!(joined.view.label, "Joined");
        assert!(joined.logs.iter().any(|l| l.contains("successful")));

        let mut svc = AutoJoinService::new();
        let req = tick(
            &mut svc,
            0,
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            true,
            &[],
        )
        .join
        .unwrap();
        let failed = JoinResult {
            revision: req.revision,
            generation: req.generation,
            ok: false,
            error: "group modification not supported".into(),
        };
        let retry = tick(
            &mut svc,
            10,
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            true,
            &[failed],
        );
        assert!(retry.logs.iter().any(|l| l.contains("not supported")));
        assert!(retry.join.is_none());
        assert_eq!(retry.view.state, "target_active");
        let too_soon = tick(
            &mut svc,
            100,
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            true,
            &[],
        );
        assert!(too_soon.join.is_none());
        let again = tick(
            &mut svc,
            10 + 500,
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            true,
            &[],
        );
        assert!(again.join.is_some());
    }

    #[test]
    fn retries_exhaust_then_new_playback_starts_over() {
        let mut svc = AutoJoinService::new();
        let playing = [dev("aa", "Kitchen", true)];
        let mut now = 0u64;
        let mut joins = 0;
        for _ in 0..12 {
            let out = tick(
                &mut svc,
                now,
                false,
                true,
                "aa",
                "Kitchen",
                "self",
                &playing,
                true,
                &[],
            );
            if let Some(req) = out.join {
                joins += 1;
                let failed = JoinResult {
                    revision: req.revision,
                    generation: req.generation,
                    ok: false,
                    error: "nope".into(),
                };
                let _ = tick(
                    &mut svc,
                    now,
                    false,
                    true,
                    "aa",
                    "Kitchen",
                    "self",
                    &playing,
                    true,
                    &[failed],
                );
            }
            if svc.phase == Phase::Error {
                break;
            }
            now = svc.next_try_ms.max(now.saturating_add(1));
        }
        assert_eq!(joins, 5);
        assert_eq!(svc.phase, Phase::Error);
        let idle = tick(
            &mut svc,
            now,
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &[dev("aa", "Kitchen", false)],
            true,
            &[],
        );
        assert_eq!(idle.view.state, "idle_waiting");
        let again = tick(
            &mut svc,
            now.saturating_add(1),
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            true,
            &[],
        );
        assert!(again.join.is_some());
    }

    #[test]
    fn busy_session_defers_config_and_direct_play_wins() {
        let mut svc = AutoJoinService::new();
        let kitchen = [dev("kit", "Kitchen", true)];
        let _ = tick(
            &mut svc,
            0,
            false,
            true,
            "liv",
            "Living Room",
            "self",
            &kitchen,
            true,
            &[],
        );
        let busy = tick(
            &mut svc,
            5,
            true,
            true,
            "liv",
            "Living Room",
            "self",
            &kitchen,
            true,
            &[],
        );
        assert_eq!(busy.view.state, "busy");
        assert!(busy.join.is_none());
        let changed = tick(
            &mut svc,
            6,
            true,
            true,
            "kit",
            "Kitchen",
            "self",
            &kitchen,
            true,
            &[],
        );
        assert!(changed.view.pending_config_change);
        assert_eq!(changed.view.label, "Waiting until idle");
        assert!(changed
            .logs
            .iter()
            .any(|l| l.contains("deferred until idle")));
        let disabled = tick(
            &mut svc,
            7,
            true,
            false,
            "kit",
            "Kitchen",
            "self",
            &kitchen,
            true,
            &[],
        );
        assert!(disabled.view.pending_config_change);
        assert_eq!(disabled.view.state, "busy");
        let hold = tick(
            &mut svc,
            7,
            false,
            false,
            "kit",
            "Kitchen",
            "self",
            &kitchen,
            true,
            &[],
        );
        assert_eq!(hold.view.label, "Waiting until idle");
        assert!(hold.join.is_none());
        let idle = tick(
            &mut svc,
            7 + IDLE_DEBOUNCE_MS,
            false,
            false,
            "kit",
            "Kitchen",
            "self",
            &kitchen,
            true,
            &[],
        );
        assert_eq!(idle.view.state, "disabled");
        assert!(!idle.view.pending_config_change);
    }

    #[test]
    fn already_playing_when_idle_joins_and_revision_ignores_stale_result() {
        let mut svc = AutoJoinService::new();
        let _ = tick(
            &mut svc,
            0,
            true,
            true,
            "liv",
            "Living Room",
            "self",
            &[],
            true,
            &[],
        );
        let kitchen = [dev("kit", "Kitchen", true)];
        let hold = tick(
            &mut svc,
            10,
            false,
            true,
            "kit",
            "Kitchen",
            "self",
            &kitchen,
            true,
            &[],
        );
        assert!(hold.join.is_none());
        let join = tick(
            &mut svc,
            10 + IDLE_DEBOUNCE_MS,
            false,
            true,
            "kit",
            "Kitchen",
            "self",
            &kitchen,
            true,
            &[],
        );
        let req = join.join.expect("join kitchen");
        let stale = JoinResult {
            revision: req.revision,
            generation: req.generation,
            ok: true,
            error: String::new(),
        };
        let switched = tick(
            &mut svc,
            4_000,
            false,
            true,
            "other",
            "Office",
            "self",
            &kitchen,
            true,
            &[],
        );
        assert!(switched.logs.iter().any(|l| l.contains("target changed")));
        let ignored = tick(
            &mut svc,
            4_010,
            false,
            true,
            "other",
            "Office",
            "self",
            &[dev("other", "Office", false)],
            true,
            &[stale],
        );
        assert!(ignored.logs.iter().any(|l| l.contains("aborted")));
        assert_ne!(ignored.view.state, "joining");
    }

    #[test]
    fn timeouts_name_only_change_and_inactive_discovery() {
        let mut svc = AutoJoinService::new();
        let playing = [dev("aa", "Kitchen", true)];
        let req = tick(
            &mut svc,
            0,
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            true,
            &[],
        )
        .join
        .unwrap();
        let timed = tick(
            &mut svc,
            REQUEST_TIMEOUT_MS,
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            true,
            &[],
        );
        assert!(timed
            .logs
            .iter()
            .any(|l| l.contains("group request timed out")));
        let _ = req;
        let renamed = tick(
            &mut svc,
            REQUEST_TIMEOUT_MS + 10,
            false,
            true,
            "aa",
            "Cook Room",
            "self",
            &playing,
            true,
            &[],
        );
        assert_eq!(renamed.view.target_name, "Cook Room");
        let mut svc = AutoJoinService::new();
        let inactive = tick(
            &mut svc,
            0,
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            false,
            &[],
        );
        assert!(inactive.join.is_none());
        assert!(inactive.view.last_error.contains("discovery inactive"));
        let no_self = tick(
            &mut svc,
            1,
            false,
            true,
            "aa",
            "Kitchen",
            "",
            &playing,
            true,
            &[],
        );
        assert!(no_self.view.last_error.contains("unable to identify"));
        assert!(no_self.join.is_none());
    }

    #[test]
    fn pcm_timeout_and_choices_hide_self_and_keep_offline_target() {
        let mut svc = AutoJoinService::new();
        let playing = [dev("aa", "Kitchen", true)];
        let req = tick(
            &mut svc,
            0,
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            true,
            &[],
        )
        .join
        .unwrap();
        let ok = JoinResult {
            revision: req.revision,
            generation: req.generation,
            ok: true,
            error: String::new(),
        };
        let _ = tick(
            &mut svc,
            5,
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            true,
            &[ok],
        );
        let expired = tick(
            &mut svc,
            5 + JOIN_TIMEOUT_MS,
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            true,
            &[],
        );
        assert!(expired.logs.iter().any(|l| l.contains("timed out waiting")));
        assert_eq!(expired.view.state, "error");
        assert!(expired.view.label.contains("timed out waiting for inbound AirPlay session"));
        assert!(expired.join.is_none());
        let still = tick(
            &mut svc,
            6 + JOIN_TIMEOUT_MS,
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            true,
            &[],
        );
        assert!(still.join.is_none());
        assert_eq!(still.view.state, "error");
        let late = tick(
            &mut svc,
            7 + JOIN_TIMEOUT_MS,
            true,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            true,
            &[],
        );
        assert_eq!(late.view.state, "joined");
        assert!(late.view.last_error.is_empty());

        let mut homepod = dev("h1", "Kitchen", false);
        homepod.model = "HomePod".into();
        let mut tv = dev("t1", "Kitchen", false);
        tv.model = "Apple TV".into();
        let mut bare = dev("b1", "Office", false);
        bare.name.clear();
        let mut twin = dev("0123456789abcdef", "Office", false);
        twin.name = "Den".into();
        let twin2 = dev("ffffeeeeddddcccc", "Den", false);
        let classic = AirPlayDevice {
            airplay2: false,
            ..dev("old", "Old", false)
        };
        let self_dev = dev("self", "WLEDPlay", false);
        let list = [homepod, tv, bare, twin, twin2, classic, self_dev];
        let choices = device_choices(&list, "self", "WLEDPlay", "gone", "Basement");
        assert!(choices.iter().any(|c| c.label == "Kitchen — HomePod"));
        assert!(choices.iter().any(|c| c.label == "Kitchen — Apple TV"));
        assert!(choices.iter().any(|c| c.id == "b1"));
        assert!(choices.iter().any(|c| c.label.contains("Den —")));
        assert!(choices
            .iter()
            .any(|c| c.label.contains("currently unavailable")));
        assert!(!choices.iter().any(|c| c.id == "self" || c.id == "old"));
        let labels: Vec<_> = choices.iter().map(|c| c.label.to_ascii_lowercase()).collect();
        let mut ordered = labels.clone();
        ordered.sort();
        assert_eq!(labels, ordered);
        assert_eq!(detected_count(&list, "self", "WLEDPlay"), 5);
        assert!(find_target(&[], "").is_none());
        assert_eq!(display_target("", ""), "(none)");
    }

    #[test]
    fn direct_playback_while_joining_counts_as_success() {
        let mut svc = AutoJoinService::new();
        let playing = [dev("aa", "Kitchen", true)];
        let _ = tick(
            &mut svc,
            0,
            false,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            true,
            &[],
        );
        let joined = tick(
            &mut svc,
            15,
            true,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            true,
            &[],
        );
        assert_eq!(joined.view.state, "joined");
        let still = tick(
            &mut svc,
            16,
            true,
            true,
            "aa",
            "Kitchen",
            "self",
            &playing,
            true,
            &[],
        );
        assert_eq!(still.view.state, "joined");
        assert!(still.logs.is_empty());
    }

    #[test]
    fn remaining_branches_and_derived_impls() {
        let _ = AutoJoinService::default();
        let device = dev("ab", "Den", false);
        let _ = format!("{:?}", device.clone());
        let req = JoinRequest {
            target_id: "ab".into(),
            receiver_id: "self".into(),
            revision: 1,
            generation: 2,
        };
        let _ = format!("{:?}", req.clone());
        let result = JoinResult {
            revision: 1,
            generation: 2,
            ok: false,
            error: "nope".into(),
        };
        let _ = format!("{:?}", result.clone());
        assert_eq!(display_target("", "ab"), "ab");
        assert_eq!(display_target("", ""), "(none)");

        let mut svc = AutoJoinService::new();
        tick(&mut svc, 0, false, false, "", "", "", &[], false, &[]);
        let enabled = tick(&mut svc, 1, false, true, "ab", "", "", &[], false, &[]);
        assert!(enabled.logs.iter().any(|l| l == "Auto Join enabled"));
        let again = tick(&mut svc, 2, false, true, "ab", "", "", &[], false, &[]);
        assert!(again.logs.is_empty());

        let up = dev("ab", "Den", false);
        tick(
            &mut svc,
            3,
            false,
            true,
            "ab",
            "Den",
            "",
            &[up.clone()],
            true,
            &[],
        );
        let gone = tick(&mut svc, 4, false, true, "ab", "Den", "", &[], true, &[]);
        assert!(gone.logs.iter().any(|l| l.contains("unavailable")));

        let mut svc = AutoJoinService::new();
        let playing = dev("ab", "Den", true);
        let issued = tick(
            &mut svc,
            0,
            false,
            true,
            "ab",
            "Den",
            "self",
            &[playing.clone()],
            true,
            &[],
        );
        let join = issued.join.expect("join");
        let busy = tick(
            &mut svc,
            1,
            true,
            true,
            "ab",
            "Den",
            "self",
            &[playing.clone()],
            true,
            &[JoinResult {
                revision: join.revision,
                generation: join.generation,
                ok: false,
                error: "late".into(),
            }],
        );
        assert_eq!(busy.view.state, "joined");

        let mut svc = AutoJoinService::new();
        let stale = tick(
            &mut svc,
            0,
            false,
            true,
            "ab",
            "Den",
            "self",
            &[playing.clone()],
            true,
            &[JoinResult {
                revision: 0,
                generation: 99,
                ok: false,
                error: "stale".into(),
            }],
        );
        assert!(stale.join.is_some());

        let hidden = device_choices(&[dev("other", "WLEDPlay", false)], "", "WLEDPlay", "", "");
        assert!(hidden.is_empty());
        let offline = device_choices(&[], "", "", "missing", "");
        assert!(offline[0].label.contains("missing"));
        let short = device_choices(
            &[dev("aa", "Den", false), dev("bb", "Den", false)],
            "",
            "",
            "",
            "",
        );
        assert!(short.iter().any(|d| d.label.contains("aa")));
        assert!(short.iter().any(|d| d.label.contains("bb")));

        let mut svc = AutoJoinService::new();
        let playing = dev("ab", "Den", true);
        tick(
            &mut svc,
            0,
            false,
            true,
            "ab",
            "Den",
            "self",
            &[playing.clone()],
            false,
            &[],
        );
        let repeated = tick(
            &mut svc,
            1,
            false,
            true,
            "ab",
            "Den",
            "self",
            &[playing],
            false,
            &[],
        );
        assert!(repeated.logs.is_empty());
    }
}
