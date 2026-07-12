#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${HANDTERM_BIN:-$ROOT/target/release/handterm}"
OUT_DIR="${HANDTERM_PROFILE_OUT:-$ROOT/profile_out}"
mkdir -p "$OUT_DIR"
OUT_FILE="$OUT_DIR/spawn_benchmark.txt"
COUNT="${1:-100}"
BACKEND="${HANDTERM_SPAWN_BACKEND:-gpu}"
WAYLAND_DISPLAY="${WAYLAND_DISPLAY:?WAYLAND_DISPLAY must be set}"
XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR must be set}"

cargo build -q --release --workspace --manifest-path "$ROOT/Cargo.toml"

TMPDIR="$(mktemp -d /tmp/handterm-spawn-benchmark.XXXXXX)"
ln -s "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" "$TMPDIR/$WAYLAND_DISPLAY"
HOST_LOG="$TMPDIR/host.log"

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

before_windows=$(mktemp)
after_windows=$(mktemp)
capture_windows "$before_windows"
env_host() {
  env XDG_RUNTIME_DIR="$TMPDIR" WAYLAND_DISPLAY="$WAYLAND_DISPLAY" "$@"
}

env_host "$BIN" --backend "$BACKEND" >"$HOST_LOG" 2>&1 &
LAUNCH_PID=$!
HOST_PID=""
cleanup(){
  kill "$LAUNCH_PID" 2>/dev/null || true
  wait "$LAUNCH_PID" 2>/dev/null || true
  rm -f "$before_windows" "$after_windows"
  rm -rf "$TMPDIR"
}
trap cleanup EXIT

for _ in $(seq 1 400); do
  if env_host "$BIN" --backend "$BACKEND" @ list-windows >/dev/null 2>&1; then
    HOST_PID=$(sed -n 's/.* pid=\([0-9][0-9]*\) .*/\1/p' "$HOST_LOG" | head -n1)
    if [[ -n "$HOST_PID" ]] && [[ -r "/proc/$HOST_PID/status" ]]; then
      break
    fi
  fi
  sleep 0.01
done

if [[ -z "$HOST_PID" ]]; then
  HOST_PID="$LAUNCH_PID"
fi

read_cpu_ticks() {
  awk '{print $14 + $15}' "/proc/$HOST_PID/stat"
}
CLK_TCK=$(getconf CLK_TCK)
START_TICKS=$(read_cpu_ticks)
START_RSS=$(grep '^VmRSS:' "/proc/$HOST_PID/status" | awk '{print $2}')

: > "$OUT_FILE"
printf 'backend=%s count=%s\n' "$BACKEND" "$COUNT" | tee -a "$OUT_FILE"

for i in $(seq 2 "$COUNT"); do
  START_NS=$(date +%s%N)
  env_host "$BIN" --backend "$BACKEND" open-window >/dev/null 2>&1
  for _ in $(seq 1 400); do
    COUNT_NOW=$(env_host "$BIN" --backend "$BACKEND" @ list-windows 2>/dev/null | python -c "import sys,json; data=json.load(sys.stdin); print((data.get('data') or {}).get('count', 0))" 2>/dev/null || echo 0)
    if [[ "$COUNT_NOW" -ge "$i" ]]; then
      break
    fi
    sleep 0.005
  done
  END_NS=$(date +%s%N)
  if (( i == 2 )) || (( i % 10 == 0 )) || (( i == COUNT )); then
    RSS=$(grep '^VmRSS:' "/proc/$HOST_PID/status" | awk '{print $2}')
    printf 'window=%s startup_ms=%s rss_kb=%s\n' "$i" "$(( (END_NS-START_NS)/1000000 ))" "$RSS" | tee -a "$OUT_FILE"
  fi
done

END_TICKS=$(read_cpu_ticks)
END_RSS=$(grep '^VmRSS:' "/proc/$HOST_PID/status" | awk '{print $2}')
CPU_MS=$(( (END_TICKS - START_TICKS) * 1000 / CLK_TCK ))
RSS_DELTA=$(( END_RSS - START_RSS ))
printf 'cpu_time_ms=%s rss_delta_kb=%s\n' "$CPU_MS" "$RSS_DELTA" | tee -a "$OUT_FILE"
