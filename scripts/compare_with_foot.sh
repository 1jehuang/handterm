#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="${HANDTERM_PROFILE_OUT:-$ROOT/profile_out}"
OUT_FILE="$OUT_DIR/foot_compare.txt"
mkdir -p "$OUT_DIR"

HANDTERM_BIN="$ROOT/target/release/handterm"
FOOT_BIN="${FOOT_BIN:-$(command -v foot || true)}"
FOOTCLIENT_BIN="${FOOTCLIENT_BIN:-$(command -v footclient || true)}"

if [[ -z "$FOOT_BIN" ]] || [[ -z "$FOOTCLIENT_BIN" ]]; then
  echo "foot and footclient must be installed" >&2
  exit 2
fi

if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
  echo "WAYLAND_DISPLAY is not set" >&2
  exit 2
fi

if ! command -v niri >/dev/null 2>&1; then
  echo "niri is required for live startup comparison" >&2
  exit 2
fi

cargo build -q --release --manifest-path "$ROOT/Cargo.toml"

window_count() {
  niri msg windows | grep -c '^Window ID ' || true
}

measure_window_cmd() {
  local label="$1"
  shift

  local before after start_ns end_ns pid rss threads startup_ms
  before=$(window_count)
  start_ns=$(date +%s%N)
  "$@" >/tmp/handterm-foot-compare.log 2>&1 &
  pid=$!

  cleanup_one() {
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  }
  trap cleanup_one RETURN

  for _ in $(seq 1 1000); do
    after=$(window_count)
    if [[ "$after" -gt "$before" ]]; then
      break
    fi
    sleep 0.002
  done
  end_ns=$(date +%s%N)
  startup_ms=$(( (end_ns - start_ns) / 1000000 ))

  sleep 0.5
  rss=$(grep '^VmRSS:' /proc/$pid/status 2>/dev/null | awk '{print $2 " " $3}' || echo 'n/a')
  threads=$(grep '^Threads:' /proc/$pid/status 2>/dev/null | awk '{print $2}' || echo 'n/a')
  printf '%-24s startup_ms=%-6s rss=%-12s threads=%s\n' "$label" "$startup_ms" "$rss" "$threads" | tee -a "$OUT_FILE"

  cleanup_one
  trap - RETURN
  sleep 0.3
}

measure_footclient() {
  local label="$1"
  local server_pid=""
  "$FOOT_BIN" --server >/tmp/foot-server-compare.log 2>&1 &
  server_pid=$!
  cleanup_server() {
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  }
  trap cleanup_server RETURN

  sleep 0.3
  local server_rss server_threads
  server_rss=$(grep '^VmRSS:' /proc/$server_pid/status 2>/dev/null | awk '{print $2 " " $3}' || echo 'n/a')
  server_threads=$(grep '^Threads:' /proc/$server_pid/status 2>/dev/null | awk '{print $2}' || echo 'n/a')
  printf '%-24s rss=%-12s threads=%s\n' "$label server" "$server_rss" "$server_threads" | tee -a "$OUT_FILE"

  measure_window_cmd "$label client" "$FOOTCLIENT_BIN"
  cleanup_server
  trap - RETURN
}

: > "$OUT_FILE"
printf 'handterm vs foot comparison (%s)\n\n' "$(date --iso-8601=seconds)" | tee -a "$OUT_FILE"

measure_window_cmd "handterm gpu host" "$HANDTERM_BIN" --backend gpu
measure_window_cmd "handterm cpu host" "$HANDTERM_BIN" --backend cpu
measure_window_cmd "foot standalone" "$FOOT_BIN"
measure_footclient "foot"

printf '\nresults saved to %s\n' "$OUT_FILE" | tee -a "$OUT_FILE"
