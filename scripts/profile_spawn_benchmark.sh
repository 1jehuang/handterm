#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release/handterm"
OUT_DIR="${HANDTERM_PROFILE_OUT:-$ROOT/profile_out}"
mkdir -p "$OUT_DIR"
OUT_FILE="$OUT_DIR/spawn_benchmark.txt"
COUNT="${1:-100}"
BACKEND="${HANDTERM_SPAWN_BACKEND:-gpu}"

cargo build -q --release --workspace --manifest-path "$ROOT/Cargo.toml"

HOST_LOG=/tmp/handterm-host-bench.log
"$BIN" --standalone --backend "$BACKEND" >"$HOST_LOG" 2>&1 &
HOST_PID=$!
cleanup(){
  kill "$HOST_PID" 2>/dev/null || true
  wait "$HOST_PID" 2>/dev/null || true
}
trap cleanup EXIT

for _ in $(seq 1 400); do
  if "$BIN" --backend "$BACKEND" @ list-windows >/dev/null 2>&1; then
    break
  fi
  sleep 0.01
done

read_cpu_ticks() {
  awk '{print $14 + $15}' "/proc/$HOST_PID/stat"
}
CLK_TCK=$(getconf CLK_TCK)
START_TICKS=$(read_cpu_ticks)
START_RSS=$(grep '^VmRSS:' /proc/$HOST_PID/status | awk '{print $2}')

: > "$OUT_FILE"
printf 'backend=%s count=%s\n' "$BACKEND" "$COUNT" | tee -a "$OUT_FILE"

for i in $(seq 2 "$COUNT"); do
  START_NS=$(date +%s%N)
  "$BIN" --backend "$BACKEND" open-window >/dev/null 2>&1
  for _ in $(seq 1 400); do
    COUNT_NOW=$("$BIN" --backend "$BACKEND" @ list-windows 2>/dev/null | python -c "import sys,json; \
data=json.load(sys.stdin); print((data.get('data') or {}).get('count', 0))" 2>/dev/null || echo 0)
    if [[ "$COUNT_NOW" -ge "$i" ]]; then
      break
    fi
    sleep 0.005
  done
  END_NS=$(date +%s%N)
  if (( i == 2 )) || (( i % 10 == 0 )) || (( i == COUNT )); then
    RSS=$(grep '^VmRSS:' /proc/$HOST_PID/status | awk '{print $2}')
    printf 'window=%s startup_ms=%s rss_kb=%s\n' "$i" "$(( (END_NS-START_NS)/1000000 ))" "$RSS" | tee -a "$OUT_FILE"
  fi
done

END_TICKS=$(read_cpu_ticks)
END_RSS=$(grep '^VmRSS:' /proc/$HOST_PID/status | awk '{print $2}')
CPU_MS=$(( (END_TICKS - START_TICKS) * 1000 / CLK_TCK ))
RSS_DELTA=$(( END_RSS - START_RSS ))
printf 'cpu_time_ms=%s rss_delta_kb=%s\n' "$CPU_MS" "$RSS_DELTA" | tee -a "$OUT_FILE"
