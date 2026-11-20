# Contributing

WLEDPlay is a LAN appliance: Shairport Sync and NQPTP are separate processes, and the Rust binary analyzes PCM and sends WLED UDP. Read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) before changing join policy, timing, or the optional upstream patches.

## Development setup

Install Git, Python 3, a C compiler/linker, and the Rust Analyzer extension for VS Code or Cursor. Run the setup script from the checkout:

```bash
./scripts/setup.sh --check
```

The check is read-only and reports missing prerequisites. If rustup is already installed, install the pinned toolchain and its formatting and lint components explicitly:

```bash
./scripts/setup.sh --install
```

If rustup is not installed, either install it from [rustup.rs](https://rustup.rs/) or opt into the official interactive installer:

```bash
./scripts/setup.sh --install-rustup
```

The installer may modify shell startup files after prompting. The script does not use `sudo` or install system packages. If it reports that another Cargo takes precedence, put `${CARGO_HOME:-$HOME/.cargo}/bin` first on `PATH` in the terminal and editor; restart the editor if needed. Verify the selected toolchain with `rustup show active-toolchain` and `cargo --version`.

Coverage tooling is optional. To install the pinned toolchain's LLVM tools and `cargo-llvm-cov` when missing, run:

```bash
./scripts/setup.sh --install --coverage
```

`cargo-llvm-cov` is installed using the stable Rust toolchain if it is not already available. All installation commands are explicit, and the script can be rerun safely. Docker Buildx and registry credentials are only required for publishing container images.

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
./scripts/check.sh
```

This runs the Rust format, clippy, and test checks, the Python sidecar tests, and the setup-script regression tests. Individual Cargo commands remain available for focused work.

Line and function coverage stays at 100% for Rust changes. Regions are not required. Pre-existing misses, if any, stay pre-existing. After the optional coverage setup above, run:

```bash
cargo llvm-cov --lib --fail-under-lines 100 --fail-under-functions 100
```

GitHub Actions runs the same `scripts/check.sh` sequence. It does not run llvm-cov.

## Editor tasks

VS Code and Cursor can run the shared tasks from **Terminal > Run Task**. The **Setup** tasks are explicit and never run automatically; the rustup installer task is interactive. The default build task runs `cargo build`, and the default test task runs `cargo test`. **Check: format, clippy, and tests** runs `scripts/check.sh`, just as CI does. The **Native** tasks call `scripts/build-wledplay.sh` and `scripts/run-wledplay.sh` on supported hosts; they are separate from the Cargo-only build. Task output appears in the terminal.

**Cargo: coverage** requires `cargo-llvm-cov` and LLVM coverage tools; use the optional setup command above. **Cargo: package** creates and verifies a Rust crate archive; it does not build a container and Cargo may reject uncommitted package files. **Container: build and push dev image** runs `scripts/build-dev.sh` and publishes to GHCR, so it requires Docker Buildx, registry login, and a clean, pushed `dev` branch. It is never run automatically.

## Release checklist

Container image publishing is release-only and tag-driven through `.github/workflows/docker-publish.yml`.

1. Confirm CI is green on `main` (`./scripts/check.sh`).
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
