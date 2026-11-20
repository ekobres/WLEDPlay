#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
fixture="$(mktemp -d "${TMPDIR:-/tmp}/wledplay-setup-test.XXXXXX")"
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/bin" "$fixture/cargo/bin" "$fixture/state"

for tool in bash dirname git python3 cc sed grep; do
  ln -s "$(command -v "$tool")" "$fixture/bin/$tool"
done

run_setup() {
  env HOME="$fixture/home" CARGO_HOME="$fixture/cargo" PATH="$fixture/cargo/bin:$fixture/bin" \
    MOCK_STATE="$fixture/state" bash "$ROOT/scripts/setup.sh" "$@"
}

if run_setup --check >"$fixture/check.log" 2>&1; then
  echo "Expected a missing-rustup check to fail." >&2
  exit 1
fi
grep -q 'rustup not found' "$fixture/check.log"

if run_setup --install-rustup </dev/null >"$fixture/interactive.log" 2>&1; then
  echo "Expected non-interactive rustup installation to fail." >&2
  exit 1
fi
grep -q 'installer is interactive' "$fixture/interactive.log"

cat >"$fixture/cargo/bin/rustup" <<'EOF'
#!/bin/bash
set -eu
case "$1" in
  run)
    if [[ "$2" == stable && "$3" == cargo && "$4" == install ]]; then
      echo 'install coverage cargo tool' >> "$MOCK_STATE/actions"
      printf '#!/bin/sh\nexit 0\n' > "$CARGO_HOME/bin/cargo-llvm-cov"
      /bin/chmod +x "$CARGO_HOME/bin/cargo-llvm-cov"
    elif [[ -f "$MOCK_STATE/toolchain" ]]; then
      echo "$3 mock version"
    else
      exit 1
    fi
    ;;
  toolchain)
    echo "install toolchain $3 $*" >> "$MOCK_STATE/actions"
    if [[ "$3" != stable ]]; then
      : > "$MOCK_STATE/toolchain"
      case " $* " in
        *" --component rust-analyzer "*) : > "$MOCK_STATE/analyzer" ;;
      esac
    fi
    ;;
  component)
    case "$2" in
      list)
        if [[ -f "$MOCK_STATE/toolchain" ]]; then
          echo 'rustfmt-test-target'
          echo 'clippy-test-target'
        fi
        if [[ -f "$MOCK_STATE/analyzer" ]]; then
          echo 'rust-analyzer-test-target'
        fi
        if [[ -f "$MOCK_STATE/coverage" ]]; then
          echo 'llvm-tools-test-target'
        fi
        ;;
      add)
        echo 'install llvm component' >> "$MOCK_STATE/actions"
        : > "$MOCK_STATE/coverage"
        ;;
      *) exit 1 ;;
    esac
    ;;
  *) exit 1 ;;
esac
EOF
chmod +x "$fixture/cargo/bin/rustup"
printf '#!/bin/sh\nexit 0\n' > "$fixture/cargo/bin/cargo"
chmod +x "$fixture/cargo/bin/cargo"

run_setup --install >"$fixture/install.log" 2>&1
channel="$(sed -n 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$ROOT/rust-toolchain.toml")"
grep -Fq "install toolchain $channel" "$fixture/state/actions"
grep -Fq -- '--component rust-analyzer' "$fixture/state/actions"
[[ ! -e "$fixture/state/coverage" ]]
[[ ! -e "$fixture/cargo/bin/cargo-llvm-cov" ]]
run_setup --install --coverage >"$fixture/coverage.log" 2>&1
[[ "$(grep -Fc "install toolchain $channel" "$fixture/state/actions")" == 1 ]]
grep -q 'install llvm component' "$fixture/state/actions"
grep -q 'install coverage cargo tool' "$fixture/state/actions"
actions_before="$(wc -l < "$fixture/state/actions")"
if ! run_setup --install --coverage >"$fixture/repeat.log" 2>&1; then
  cat "$fixture/repeat.log" >&2
  exit 1
fi
actions_after="$(wc -l < "$fixture/state/actions")"
[[ "$actions_before" == "$actions_after" ]]
run_setup --check --coverage >"$fixture/ready.log" 2>&1
grep -q 'Development prerequisites are available' "$fixture/ready.log"

rm "$fixture/state/analyzer"
if run_setup --check >"$fixture/analyzer-missing.log" 2>&1; then
  echo "Expected a missing rust-analyzer component to fail the check." >&2
  exit 1
fi
grep -q 'Missing Rust component rust-analyzer' "$fixture/analyzer-missing.log"
run_setup --install >"$fixture/analyzer-install.log" 2>&1
[[ -f "$fixture/state/analyzer" ]]
actions_after="$(wc -l < "$fixture/state/actions")"

if env HOME="$fixture/home" CARGO_HOME="$fixture/cargo" PATH="$fixture/bin" \
  MOCK_STATE="$fixture/state" bash "$ROOT/scripts/setup.sh" --check >"$fixture/path.log" 2>&1; then
  echo "Expected a missing Cargo proxy on PATH to fail the check." >&2
  exit 1
fi
grep -q 'is on PATH' "$fixture/path.log"

if RUSTUP_TOOLCHAIN=nightly-wledplay-test run_setup --check >"$fixture/override.log" 2>&1; then
  echo "Expected a conflicting toolchain override to fail the check." >&2
  exit 1
fi
grep -q 'overrides the project toolchain' "$fixture/override.log"

if run_setup --check --install >"$fixture/conflict.log" 2>&1; then
  echo "Expected conflicting modes to fail." >&2
  exit 1
fi
[[ "$actions_after" == "$(wc -l < "$fixture/state/actions")" ]]

echo "Setup checks passed."