use std::io::Write;
use std::process::{Command, Stdio};
use tempfile::NamedTempFile;

#[test]
fn binary_exits_after_finite_pcm_file() {
    let mut pcm = NamedTempFile::new().unwrap();
    pcm.write_all(&[0u8; 256]).unwrap();
    pcm.flush().unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_wledplay"))
        .args([
            "--no-web",
            "--no-realtime",
            "--input",
            pcm.path().to_str().unwrap(),
            "--destination",
            "127.0.0.1",
            "--pcm-rate",
            "22050",
            "--pcm-channels",
            "1",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .expect("run wledplay");

    assert!(
        out.status.success(),
        "wledplay failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
