#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${HANDTERM_LIVE_BIN:-$ROOT/target/release/handterm-client-gpu}"
OUT_DIR="${HANDTERM_PROFILE_OUT:-$ROOT/profile_out}"
mkdir -p "$OUT_DIR"

if [[ ! -x "$BIN" ]]; then
  echo "building live-profile binary..."
  if [[ "$(basename "$BIN")" == "handterm-client-gpu" ]]; then
    cargo build --release -p handterm-client-gpu --manifest-path "$ROOT/Cargo.toml"
  elif [[ "$(basename "$BIN")" == "handterm-client-cpu" ]]; then
    cargo build --release -p handterm-client-cpu --manifest-path "$ROOT/Cargo.toml"
  else
    cargo build --release --manifest-path "$ROOT/Cargo.toml"
  fi
fi

if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
  echo "WAYLAND_DISPLAY is not set; cannot run live Wayland profiling" >&2
  exit 2
fi

if ! command -v niri >/dev/null 2>&1; then
  echo "niri is not installed; this script currently supports niri-based startup timing" >&2
  exit 2
fi

WINDOWS_BEFORE=$(niri msg windows | grep -c "Window ID" || true)
START_NS=$(date +%s%N)
"$BIN" "$@" >/tmp/handterm-live-profile.log 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true; wait $PID 2>/dev/null || true' EXIT

for _ in $(seq 1 500); do
  WINDOWS_NOW=$(niri msg windows | grep -c "Window ID" || true)
  if [[ "$WINDOWS_NOW" -gt "$WINDOWS_BEFORE" ]]; then
    break
  fi
  sleep 0.002
done
END_NS=$(date +%s%N)

STATUS_OUT="$OUT_DIR/live_window_status.txt"
{
  echo "pid=$PID"
  echo "startup_ms=$(( (END_NS - START_NS) / 1000000 ))"
  grep -E 'VmRSS|VmSize|VmData|VmSwap|Threads' "/proc/$PID/status"
} | tee "$STATUS_OUT"

if [[ -r "/proc/$PID/smaps_rollup" ]]; then
  cat "/proc/$PID/smaps_rollup" > "$OUT_DIR/live_window_smaps_rollup.txt"
fi

echo "wrote live Wayland profiling output to $OUT_DIR"
