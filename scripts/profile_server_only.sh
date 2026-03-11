#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release/handterm-server"
SOCKET="${HANDTERM_PROFILE_SOCKET:-/tmp/handterm-profile.sock}"
OUT_DIR="${HANDTERM_PROFILE_OUT:-$ROOT/profile_out}"
mkdir -p "$OUT_DIR"

if [[ ! -x "$BIN" ]]; then
  echo "building handterm-server release binary..."
  cargo build --release -p handterm-server --manifest-path "$ROOT/Cargo.toml"
fi

cleanup() {
  if [[ -n "${SERVER_PID:-}" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -f "$SOCKET"
}
trap cleanup EXIT

rm -f "$SOCKET"
"$BIN" --socket "$SOCKET" >"$OUT_DIR/server.log" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 100); do
  [[ -S "$SOCKET" ]] && break
  sleep 0.05
done

if [[ ! -S "$SOCKET" ]]; then
  echo "server socket did not appear: $SOCKET" >&2
  exit 1
fi

STATUS="$OUT_DIR/server.status.txt"
SMAPS="$OUT_DIR/server.smaps_rollup.txt"
MAPS="$OUT_DIR/server.maps.txt"
THREADS="$OUT_DIR/server.threads.txt"

{
  echo "pid=$SERVER_PID"
  grep -E 'Name|VmRSS|VmSize|VmData|VmSwap|Threads' "/proc/$SERVER_PID/status"
} | tee "$STATUS"

if [[ -r "/proc/$SERVER_PID/smaps_rollup" ]]; then
  cat "/proc/$SERVER_PID/smaps_rollup" | tee "$SMAPS" >/dev/null
fi
if [[ -r "/proc/$SERVER_PID/maps" ]]; then
  cat "/proc/$SERVER_PID/maps" > "$MAPS"
fi
ps -T -p "$SERVER_PID" > "$THREADS"

echo "wrote profiling output to $OUT_DIR"
