use std::env;

fn main() {
    let package_version = env::var("CARGO_PKG_VERSION").expect("Cargo package version is set");
    let metadata = env::var("WLEDPLAY_BUILD_METADATA").unwrap_or_default();
    let version = if metadata.is_empty() {
        package_version
    } else {
        assert!(
            metadata.split('.').all(|identifier| {
                !identifier.is_empty()
                    && identifier
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '-')
            }),
            "WLEDPLAY_BUILD_METADATA must be dot-separated SemVer identifiers"
        );
        format!("{package_version}+{metadata}")
    };

    println!("cargo:rustc-env=WLEDPLAY_BUILD_VERSION={version}");
    println!("cargo:rerun-if-env-changed=WLEDPLAY_BUILD_METADATA");
}
