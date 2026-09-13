use std::io::Write;
use tempfile::NamedTempFile;
use wledplay::config::Config;

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
    - "10.11.1.124"
network:
  airplay: "eth1"
"#
    )
    .unwrap();

    let cfg = Config::load(Some(file.path()));
    assert_eq!(cfg.airplay.name, "TestReceiver");
    assert_eq!(cfg.wled.destinations, vec!["10.11.1.124"]);
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
