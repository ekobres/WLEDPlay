# Contributing

WLEDPlay is a LAN appliance: Shairport Sync and NQPTP are separate processes, and the Rust binary analyzes PCM and sends WLED UDP. Read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) before changing join policy, timing, or the optional upstream patches.

## PCM without AirPlay

A PCM file exercises the analyzer and the WLED UDP path without Shairport. The file default is 44100 Hz stereo S16LE. A live Shairport source already emits PCM in real time, so that path uses `--no-realtime`. A file still needs pacing, which is the default.

```bash
cargo build --release
python3 scripts/gen_test_pcm.py -o /tmp/test.pcm
./scripts/ensure-config.sh
cat /tmp/test.pcm | ./target/release/wledplay --config config/config.yaml --input stdin
```

Set the controller to Audio Reactive, Sync Mode Receive, port 11988. WLEDPlay stops UDP shortly after the PCM ends and exits when stdin closes.

```bash
cargo run -- --input stdin --pcm-rate 44100 --pcm-channels 2 < /tmp/test.pcm
```

## Checks

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Line and function coverage stays at 100% for Rust changes. Regions are not required. Pre-existing misses, if any, stay pre-existing. On macOS with Homebrew LLVM:

```bash
export LLVM_COV="$(brew --prefix llvm)/bin/llvm-cov"
export LLVM_PROFDATA="$(brew --prefix llvm)/bin/llvm-profdata"
cargo llvm-cov --lib --fail-under-lines 100 --fail-under-functions 100
```

GitHub Actions runs format, clippy, and `cargo test`. It does not run llvm-cov.

## Release checklist

Container image publishing is release-only and tag-driven through `.github/workflows/docker-publish.yml`.

1. Confirm CI is green on `main` (`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`).
2. Pick the next semver version and create an annotated tag in `vX.Y.Z` format (example: `v0.3.1`).
3. Push the tag to GitHub; this triggers the GHCR publish workflow.
4. Verify publish output includes `ghcr.io/<repository-owner>/wledplay` tags for `X.Y.Z`, `X.Y`, `X`, and `latest` (stable tags only).
5. Verify GHCR package visibility is **Public** for anonymous pull access.
6. Smoke-test pulling and running the image on a clean Linux host using the published stable tag.

The operator UI is [`src/web/index.html`](src/web/index.html), compiled into the binary with `include_str!`. A UI change needs a rebuild before a running player shows it.

## Do not commit

- `config/config.yaml` and `config/shairport-sync.conf` (local; the examples stay tracked)
- `config/pyatv/`
- `__pycache__/`, `*.pyc`, and `target/`
- PCM captures (`*.pcm`)

## Invariants

- AirPlay 1 stays unsupported.
- The Shairport and NQPTP patches stay optional and default off; set the corresponding build arg to `1` to opt in.
- A paused AirPlay group stays joined.
- Operator-facing Auto Join text must not say "phone".
