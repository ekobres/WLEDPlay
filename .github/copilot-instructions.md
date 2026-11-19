# Copilot instructions for WLEDPlay

## Big picture (read first)
- WLEDPlay is **not** an AirPlay implementation: Shairport Sync + NQPTP handle AirPlay 2; Rust analyzes PCM and sends WLED Audio Sync V2 UDP (`docs/ARCHITECTURE.md`, `src/app.rs`, `src/wled/`).
- Main runtime flow: AirPlay sender → Shairport stdout PCM FIFO + metadata FIFO → `src/pcm/` + `src/audio/` analyzer → scheduled send loop in `src/runtime.rs` → multicast/unicast UDP to WLED.
- HTTP operator/control plane is Axum in `src/web/mod.rs`; UI is embedded from `src/web/index.html` via `include_str!` (rebuild required for UI changes).
- Auto Join is split between Rust policy (`src/auto_join/mod.rs`) and Python sidecar transport (`src/auto_join/sidecar.rs`, `docker/autojoin_sidecar.py`).

## Non-negotiable project invariants
- AirPlay 1 stays unsupported; AirPlay 2 + NQPTP is required.
- Keep optional upstream patch toggles intact and default-off: `WLEDPLAY_SHAIRPORT_PATCH=0`, `WLEDPLAY_NQPTP_PATCH=0` (`Dockerfile`, `docker-compose.yml`).
- A paused AirPlay group stays joined (do not change Auto Join semantics).
- Operator-facing Auto Join text must not say “phone” (internal identifiers may still use phone-owned naming).

## Build, test, and validation workflow
- Preferred checks before finishing Rust changes: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (`CONTRIBUTING.md`).
- Coverage expectation for Rust edits is 100% lines/functions via `cargo llvm-cov` (regions not required).
- Fast local signal path without AirPlay: generate PCM (`scripts/gen_test_pcm.py`) and pipe to `wledplay --input stdin` as documented in `CONTRIBUTING.md`.
- Runtime debugging surfaces:
  - `GET /status` for counters, performance, timing, metadata state.
  - `GET /levels` for UI meter polling.
  - `GET /logs` / `/logs/download` for `wledplay`, `shairport`, `nqptp`, `console` logs (`src/logs.rs`).

## Codebase-specific implementation patterns
- Config authority is Rust (`src/config.rs`, `src/control.rs`); shell entrypoint only patches selected startup env-facing fields.
- Live-tunable controls (e.g., send delay, AGC, multicast, unicast destinations) go through `PATCH /config`; restart-required fields use `POST /restart`.
- Send scheduling behavior is intentional: delay queue grouping, due-time clamping, idle grace packets, backlog trimming (`src/runtime.rs` constants and queue helpers).
- Network interface pinning is first-class (`network.{http,airplay,unicast,multicast}` and `WLEDPLAY_*_INTERFACE`); preserve NIC-specific behavior in discovery/sending paths.
- Keep WLED protocol-specific work in `src/wled/` and analysis math in `src/audio/`; avoid mixing protocol transport with DSP changes.

## Integration points to verify when changing behavior
- Docker/host-network assumptions: `network_mode: host`, `SYS_NICE`, Avahi + dbus startup ordering (`docker-compose.yml`, `docker/entrypoint.sh`).
- External process contracts: Shairport stdout must remain PCM-only; metadata pipe contract must remain compatible with `src/metadata/`.
- Auto Join sidecar API contract (loopback URL, stable device IDs, join success criteria) must stay aligned with `docs/SHAIRPORT_INTEGRATION.md` and `docs/design/auto-join.md`.

## Documentation sync rules (project-specific)
- When changing behavior/config/API, update matching operator docs in the same PR: `README.md`, `config/config-example.yaml`, `docs/ARCHITECTURE.md`, `SECURITY.md`, `docs/SHAIRPORT_INTEGRATION.md`, and `CHANGELOG.md`.
- Treat `docs/design/software-requirements.md` and `docs/design/auto-join.md` as design records: if a request conflicts with them, call out the exact section and get confirmation before implementing behavior changes.

## Public documentation audience and privacy
- Write tracked user-facing docs for any WLEDPlay user or contributor, not for the current conversation or one person's setup.
- Do not include conversation-specific hardware, host roles, local paths, IPs, account names, or test procedures. Mention a platform only when it is a deliberate, generally supported deployment target.
- Do not hard-code a maintainer's GitHub username in docs, scripts, Compose defaults, or examples. Use repository-relative GitHub links, a generic `<repository-owner>` placeholder, derive the owner from the Git remote, or provide an explicit configuration override.
- Keep examples reproducible and clearly label placeholders; do not turn personal test-bed steps into project setup instructions.
