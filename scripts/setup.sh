#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

usage() {
  cat <<'EOF'
Usage: scripts/setup.sh [--check | --install | --install-rustup] [--coverage]

  --check           Check prerequisites without installing anything (default).
  --install         Install the pinned Rust toolchain and its required components.
  --install-rustup  Interactively install rustup first, then perform --install.
  --coverage        Also check or install LLVM tools and cargo-llvm-cov.
  --help            Show this help.

The rustup installer may offer to modify shell startup files. No system package
manager or sudo is run by this script.
EOF
}

mode=check
mode_selected=0
coverage=0
for argument in "$@"; do
  case "$argument" in
    --check|--install|--install-rustup)
      if [[ "$mode_selected" == 1 ]]; then
        echo "Choose only one of --check, --install, and --install-rustup." >&2
        exit 2
      fi
      mode="${argument#--}"
      mode_selected=1
      ;;
    --coverage) coverage=1 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $argument" >&2; usage >&2; exit 2 ;;
  esac
done

missing=0
for tool in git python3 cc; do
  if command -v "$tool" >/dev/null 2>&1; then
    echo "Found $tool: $(command -v "$tool")"
  else
    echo "Missing $tool; install it using your system's usual package manager." >&2
    missing=1
  fi
done

cargo_home="${CARGO_HOME:-$HOME/.cargo}"
cargo_bin="$cargo_home/bin"
original_cargo="$(command -v cargo || true)"
if [[ -x "$cargo_bin/rustup" ]]; then
  rustup_cmd="$cargo_bin/rustup"
elif command -v rustup >/dev/null 2>&1; then
  rustup_cmd="$(command -v rustup)"
else
  rustup_cmd=""
fi

if [[ -z "$rustup_cmd" && "$mode" == install-rustup ]]; then
  if [[ ! -t 0 || ! -t 1 ]]; then
    echo "The rustup installer is interactive; run this command in a terminal." >&2
    exit 1
  fi
  if ! command -v curl >/dev/null 2>&1; then
    echo "Missing curl; install it before running the rustup installer." >&2
    exit 1
  fi
  echo "Running the official interactive installer from https://sh.rustup.rs."
  echo "Review its prompts: it may modify shell startup files and install tools in your home directory."
  installer="$(mktemp "${TMPDIR:-/tmp}/wledplay-rustup.XXXXXX")"
  trap 'rm -f "$installer"' EXIT
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o "$installer"
  sh "$installer"
  if [[ -x "$cargo_bin/rustup" ]]; then
    rustup_cmd="$cargo_bin/rustup"
  elif command -v rustup >/dev/null 2>&1; then
    rustup_cmd="$(command -v rustup)"
  fi
fi

if [[ -z "$rustup_cmd" ]]; then
  echo "rustup not found. Install it from https://rustup.rs/ or run scripts/setup.sh --install-rustup." >&2
  exit 1
fi

channel="$(sed -n 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$ROOT/rust-toolchain.toml")"
if [[ -z "$channel" ]]; then
  echo "Could not read the pinned Rust channel from rust-toolchain.toml." >&2
  exit 1
fi

export PATH="$cargo_bin:$PATH"
if [[ -n "$original_cargo" && "$original_cargo" != "$cargo_bin/cargo" ]]; then
  echo "Note: your current PATH selects $original_cargo instead of $cargo_bin/cargo." >&2
  echo "Ensure $cargo_bin is first on PATH in your terminal and editor; restart the editor if needed." >&2
  missing=1
elif [[ -z "$original_cargo" ]]; then
  echo "Ensure $cargo_bin is on PATH in your terminal and editor; restart the editor if needed." >&2
  missing=1
fi
if [[ -n "${RUSTUP_TOOLCHAIN:-}" ]]; then
  case "$RUSTUP_TOOLCHAIN" in
    "$channel"|"$channel".*|"$channel"-*) ;;
    *)
      echo "RUSTUP_TOOLCHAIN=$RUSTUP_TOOLCHAIN overrides the project toolchain $channel; unset it for this project." >&2
      missing=1
      ;;
  esac
fi

component_installed() {
  "$rustup_cmd" component list --toolchain "$channel" --installed | grep -E "^${1}(-preview)?-" >/dev/null
}

if [[ "$mode" != check ]]; then
  if "$rustup_cmd" run "$channel" rustc --version >/dev/null 2>&1 \
    && "$rustup_cmd" run "$channel" cargo --version >/dev/null 2>&1 \
    && component_installed rustfmt && component_installed clippy \
    && component_installed rust-analyzer; then
    echo "Rust $channel with rustfmt, clippy, and rust-analyzer is already installed."
  else
    echo "Installing the Rust $channel toolchain with rustfmt, clippy, and rust-analyzer."
    "$rustup_cmd" toolchain install "$channel" --profile minimal --component rustfmt --component clippy --component rust-analyzer --no-self-update
  fi
fi

if "$rustup_cmd" run "$channel" rustc --version; then
  "$rustup_cmd" run "$channel" cargo --version
  for component in rustfmt clippy rust-analyzer; do
    if ! component_installed "$component"; then
      echo "Missing Rust component $component for $channel; run scripts/setup.sh --install." >&2
      missing=1
    fi
  done
else
  echo "Rust toolchain $channel is not installed; run scripts/setup.sh --install." >&2
  missing=1
fi

if [[ "$coverage" == 1 ]]; then
  if [[ "$mode" != check ]]; then
    if component_installed llvm-tools; then
      echo "LLVM coverage tools for Rust $channel are already installed."
    else
      echo "Installing LLVM coverage tools for Rust $channel."
      "$rustup_cmd" component add --toolchain "$channel" llvm-tools-preview
    fi
    if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
      echo "Installing cargo-llvm-cov with the stable Rust toolchain."
      "$rustup_cmd" toolchain install stable --profile minimal --no-self-update
      "$rustup_cmd" run stable cargo install cargo-llvm-cov --locked
    fi
  fi
  if ! component_installed llvm-tools; then
    echo "Missing LLVM coverage tools for $channel; run scripts/setup.sh --install --coverage." >&2
    missing=1
  fi
  if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
    echo "Missing cargo-llvm-cov; run scripts/setup.sh --install --coverage." >&2
    missing=1
  fi
fi

if [[ "$missing" != 0 ]]; then
  exit 1
fi
echo "Development prerequisites are available."