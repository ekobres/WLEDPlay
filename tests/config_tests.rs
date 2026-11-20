use std::io::Write;
use tempfile::NamedTempFile;
use wledplay::config::Config;

#[test]
fn example_config_loads_empty_destinations_and_audio_settings() {
    let cfg: Config = serde_yaml::from_str(include_str!("../config/config-example.yaml")).unwrap();
    assert!(cfg.wled.destinations.is_empty());
    assert_eq!(cfg.audio.sync_offset_ms, 1200);
    assert!((cfg.audio.shairport_latency_offset_s - -1.2).abs() < 1e-9);
}

#[test]
fn loads_yaml_from_file_and_applies_defaults() {
    let mut file = NamedTempFile::new().unwrap();
    write!(
        file,
        r#"
airplay:
  name: "TestReceiver"
wled:
  destinations:
    - "192.0.2.10"
network:
  airplay: "eth1"
"#
    )
    .unwrap();

    let cfg = Config::load(Some(file.path()));
    assert_eq!(cfg.airplay.name, "TestReceiver");
    assert_eq!(cfg.wled.destinations, vec!["192.0.2.10"]);
    assert_eq!(cfg.network.airplay, "eth1");
    assert_eq!(cfg.web.port, 8080);
}

#[test]
fn save_and_reload_roundtrip() {
    let file = NamedTempFile::new().unwrap();
    let mut cfg = Config::default();
    cfg.airplay.name = "Roundtrip".into();
    cfg.network.unicast = "eth2".into();
    cfg.wled.multicast_enabled = false;

    cfg.save(file.path()).unwrap();
    let loaded = Config::load(Some(file.path()));
    assert_eq!(loaded.airplay.name, "Roundtrip");
    assert_eq!(loaded.network.unicast, "eth2");
    assert!(!loaded.wled.multicast_enabled);
}
