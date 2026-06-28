#!/usr/bin/env bash
# w6 startup latency measurement harness.
# Launches a GPU host headless, captures first-window timing, opens N added
# windows over the control socket, and prints the parsed timing lines.
#
# Usage: scripts/w6_measure.sh [RUNS] [ADDED_PER_RUN]
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${HANDTERM_BIN:-$ROOT/target/release/handterm}"
RUNS="${1:-5}"
ADDED="${2:-3}"

for r in $(seq 1 "$RUNS"); do
  RT="/tmp/ht6_run_$r"
  rm -rf "$RT"; mkdir -p "$RT"
  SOCK="$RT/handterm-gpu.sock"
  LOG="$RT/out.log"
  XDG_RUNTIME_DIR="$RT" "$BIN" --standalone --backend gpu --exec 'sleep 30' >"$LOG" 2>&1 &
  HPID=$!
  # wait for socket
  for _ in $(seq 1 100); do [[ -S "$SOCK" ]] && break; sleep 0.05; done
  # wait for first window to present
  for _ in $(seq 1 100); do grep -q 'open_to_first_present' "$LOG" && break; sleep 0.05; done
  sleep 0.3
  for a in $(seq 1 "$ADDED"); do
    XDG_RUNTIME_DIR="$RT" "$BIN" @ open-window '{}' --to "$SOCK" >/dev/null 2>&1
    sleep 0.25
  done
  sleep 0.3
  kill "$HPID" 2>/dev/null; wait "$HPID" 2>/dev/null
  echo "### run $r"
  grep -E 'open_to_first_present=' "$LOG"
  grep -E 'open-window id=' "$LOG"
  grep -E '^  total=' "$LOG"
  grep -E '^  dpi=' "$LOG"
done
