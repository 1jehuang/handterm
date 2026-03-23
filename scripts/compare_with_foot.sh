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

if [[ -z "${WAYLAND_DISPLAY:-}" ]] || [[ -z "${XDG_RUNTIME_DIR:-}" ]]; then
  echo "WAYLAND_DISPLAY and XDG_RUNTIME_DIR must be set" >&2
  exit 2
fi

if ! command -v niri >/dev/null 2>&1; then
  echo "niri is required for live startup comparison" >&2
  exit 2
fi

cargo build -q --release --manifest-path "$ROOT/Cargo.toml"

capture_windows() {
  local out_file="$1"
  niri msg windows | awk '
    /^Window ID / {
      id=$3
      sub(/:/, "", id)
      pid=""
      app=""
      title=""
      next
    }
    /^  Title: / {
      title=$0
      sub(/^  Title: "/, "", title)
      sub(/"$/, "", title)
      next
    }
    /^  App ID: / {
      app=$0
      sub(/^  App ID: "/, "", app)
      sub(/"$/, "", app)
      next
    }
    /^  PID: / {
      pid=$2
      if (id != "" && pid != "") {
        printf "%s|%s|%s|%s\n", id, pid, app, title
      }
    }
  ' > "$out_file"
}

find_new_window_pid() {
  local before_file="$1"
  local after_file="$2"
  local expected_app_id="$3"
  awk -F'|' -v app="$expected_app_id" '
    NR==FNR { seen[$1]=1; next }
    !seen[$1] && (app == "" || $3 == app) {
      print $2
      exit
    }
  ' "$before_file" "$after_file"
}

measure_pid() {
  local label="$1"
  local pid="$2"
  local startup_ms="$3"
  local status="/proc/$pid/status"
  local rss threads
  rss=$(grep '^VmRSS:' "$status" 2>/dev/null | awk '{print $2 " " $3}' || echo 'n/a')
  threads=$(grep '^Threads:' "$status" 2>/dev/null | awk '{print $2}' || echo 'n/a')
  printf '%-24s startup_ms=%-6s rss=%-12s threads=%s pid=%s\n' "$label" "$startup_ms" "$rss" "$threads" "$pid" | tee -a "$OUT_FILE"
}

kill_if_alive() {
  local pid="$1"
  if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
}

measure_window_cmd() {
  local label="$1"
  local expected_app_id="$2"
  shift 2

  local before_file after_file launcher_pid target_pid startup_ms
  before_file=$(mktemp)
  after_file=$(mktemp)
  capture_windows "$before_file"

  local start_ns end_ns
  start_ns=$(date +%s%N)
  "$@" >/tmp/handterm-foot-compare.log 2>&1 &
  launcher_pid=$!
  target_pid=""

  for _ in $(seq 1 1000); do
    capture_windows "$after_file"
    target_pid=$(find_new_window_pid "$before_file" "$after_file" "$expected_app_id" || true)
    if [[ -n "$target_pid" ]]; then
      break
    fi
    sleep 0.002
  done
  end_ns=$(date +%s%N)
  startup_ms=$(( (end_ns - start_ns) / 1000000 ))

  if [[ -z "$target_pid" ]]; then
    target_pid="$launcher_pid"
  fi

  sleep 0.5
  measure_pid "$label" "$target_pid" "$startup_ms"

  kill_if_alive "$launcher_pid"
  if [[ "$target_pid" != "$launcher_pid" ]]; then
    kill_if_alive "$target_pid"
  fi
  rm -f "$before_file" "$after_file"
  sleep 0.3
}

measure_handterm_host() {
  local label="$1"
  local backend="$2"
  local tmpdir
  tmpdir=$(mktemp -d /tmp/handterm-foot-compare.XXXXXX)
  ln -s "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" "$tmpdir/$WAYLAND_DISPLAY"
  measure_window_cmd "$label" "handterm" env XDG_RUNTIME_DIR="$tmpdir" WAYLAND_DISPLAY="$WAYLAND_DISPLAY" "$HANDTERM_BIN" --backend "$backend"
  rm -rf "$tmpdir"
}

measure_foot_standalone() {
  local app_id="foot-compare-standalone"
  measure_window_cmd "foot standalone" "$app_id" "$FOOT_BIN" --app-id="$app_id"
}

measure_footclient() {
  local socket_path pid_file server_pid app_id
  socket_path=$(mktemp -u "$XDG_RUNTIME_DIR/foot-compare-$WAYLAND_DISPLAY.XXXX.sock")
  pid_file="$XDG_RUNTIME_DIR/foot-compare-server.pid"
  rm -f "$pid_file"
  app_id="foot-compare-client"

  "$FOOT_BIN" --server="$socket_path" --print-pid="$pid_file" >/tmp/foot-server-compare.log 2>&1 &
  local launcher_pid=$!

  for _ in $(seq 1 200); do
    if [[ -s "$pid_file" ]] && [[ -S "$socket_path" ]]; then
      break
    fi
    sleep 0.01
  done

  server_pid=$(tr -d '\n' < "$pid_file" 2>/dev/null || true)
  if [[ -z "$server_pid" ]]; then
    server_pid="$launcher_pid"
  fi

  sleep 0.2
  measure_pid "foot server" "$server_pid" "n/a"
  measure_window_cmd "foot client" "$app_id" "$FOOTCLIENT_BIN" --server-socket="$socket_path" --app-id="$app_id"

  kill_if_alive "$launcher_pid"
  if [[ "$server_pid" != "$launcher_pid" ]]; then
    kill_if_alive "$server_pid"
  fi
  rm -f "$pid_file" "$socket_path"
}

: > "$OUT_FILE"
printf 'handterm vs foot comparison (%s)\n\n' "$(date --iso-8601=seconds)" | tee -a "$OUT_FILE"

measure_handterm_host "handterm gpu host" gpu
measure_handterm_host "handterm cpu host" cpu
measure_foot_standalone
measure_footclient

printf '\nresults saved to %s\n' "$OUT_FILE" | tee -a "$OUT_FILE"
