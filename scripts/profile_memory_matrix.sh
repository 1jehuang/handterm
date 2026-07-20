#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="${HANDTERM_PROFILE_OUT:-$ROOT/profile_out}"
mkdir -p "$OUT_DIR"
OUT_FILE="$OUT_DIR/memory_matrix.txt"
HANDTERM_BIN="$ROOT/target/release/handterm"

cargo build -q --release --workspace --manifest-path "$ROOT/Cargo.toml"

if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
  echo "WAYLAND_DISPLAY is not set" >&2
  exit 2
fi

if ! command -v niri >/dev/null 2>&1; then
  echo "niri is required for live memory matrix profiling" >&2
  exit 2
fi

window_count() {
  niri msg windows | grep -c '^Window ID ' || true
}

window_pid_for_process() {
  local process_pid="$1"
  niri msg windows | awk -v target="$process_pid" '
    /^  PID: / && $2 == target { print $2; exit }
  '
}

measure_pid() {
  local label="$1"
  local pid="$2"
  sleep 1
  local status="/proc/$pid/status"
  local rss vsz threads
  rss=$(grep '^VmRSS:' "$status" | awk '{print $2 " " $3}')
  vsz=$(grep '^VmSize:' "$status" | awk '{print $2 " " $3}')
  threads=$(grep '^Threads:' "$status" | awk '{print $2}')
  printf '%-22s pid=%-8s rss=%-12s vsz=%-12s threads=%s\n' "$label" "$pid" "$rss" "$vsz" "$threads" | tee -a "$OUT_FILE"
}

measure_window_cmd() {
  local label="$1"
  shift
  local pid="" cmd_pid
  "$@" >/tmp/handterm-memory-matrix.log 2>&1 &
  cmd_pid=$!
  trap 'kill $cmd_pid 2>/dev/null || true; wait $cmd_pid 2>/dev/null || true' RETURN
  for _ in $(seq 1 500); do
    pid=$(window_pid_for_process "$cmd_pid")
    if [[ -n "${pid:-}" ]] && [[ -r "/proc/$pid/status" ]]; then
      measure_pid "$label" "$pid"
      break
    fi
    sleep 0.01
  done
  if [[ -z "${pid:-}" ]]; then
    echo "failed to detect window PID for $label" | tee -a "$OUT_FILE"
    return 1
  fi
  kill "$cmd_pid" 2>/dev/null || true
  wait "$cmd_pid" 2>/dev/null || true
  trap - RETURN
  sleep 0.3
}

: > "$OUT_FILE"
printf 'handterm memory matrix (%s)\n\n' "$(date --iso-8601=seconds)" | tee -a "$OUT_FILE"

measure_window_cmd "standalone-gpu" "$HANDTERM_BIN" --standalone --backend gpu
measure_window_cmd "standalone-cpu" "$HANDTERM_BIN" --standalone --backend cpu

printf '\nresults saved to %s\n' "$OUT_FILE" | tee -a "$OUT_FILE"
