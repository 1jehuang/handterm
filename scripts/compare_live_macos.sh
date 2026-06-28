#!/usr/bin/env bash
# Live resource comparison on macOS: handterm vs Ghostty vs kitty.
#
# Measures steady-state memory footprint (physical/dirty) of an idle terminal
# window at a fixed size, and handterm's own multi-window RSS scaling.
#
# This is a LIVE harness (opens real GUI windows) and is inherently a bit noisy,
# so it samples each app a few times and reports the median. It complements the
# deterministic scripts/bench_capture.sh logic gate.
#
# Usage: scripts/compare_live_macos.sh
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HANDTERM_BIN="${HANDTERM_BIN:-$ROOT/target/release/handterm}"
OUT_DIR="${BENCH_OUT_DIR:-$ROOT/bench_out}"
mkdir -p "$OUT_DIR"
OUT="$OUT_DIR/compare_live.txt"
SETTLE="${SETTLE:-3}"

# footprint_kb PID -> physical footprint in KB (dirty + compressed + reachable),
# the number Activity Monitor shows as "Memory". Falls back to ps rss.
footprint_kb() {
  local pid="$1"
  local v
  v=$(/usr/bin/footprint "$pid" 2>/dev/null | awk -F'[:=]' '/phys_footprint/ {print $2; exit}')
  if [[ -n "${v:-}" ]]; then
    # value like " 12.3M" or "12345K"
    echo "$v" | awk '
      /[0-9]/ {
        n=$1+0
        if ($0 ~ /M/) n=n*1024
        else if ($0 ~ /G/) n=n*1024*1024
        printf "%.0f", n
      }'
    return
  fi
  ps -o rss= -p "$pid" 2>/dev/null | awk '{print $1}'
}

# Sum footprint over a set of pids
sum_footprint_kb() {
  local total=0 pid kb
  for pid in "$@"; do
    kb=$(footprint_kb "$pid")
    [[ -n "${kb:-}" ]] && total=$((total + kb))
  done
  echo "$total"
}

median() { sort -n | awk '{a[NR]=$1} END{ if(NR==0){print 0} else if(NR%2){print a[(NR+1)/2]} else {print int((a[NR/2]+a[NR/2+1])/2)} }'; }

echo "handterm live macOS comparison ($(date -u +%Y-%m-%dT%H:%M:%SZ))" | tee "$OUT"
echo "settle=${SETTLE}s, footprint=phys_footprint (KB)" | tee -a "$OUT"
echo | tee -a "$OUT"

############################################
# handterm GPU host: 1..N window RSS scaling
############################################
measure_handterm_scaling() {
  local backend="$1"
  local maxwin="${2:-4}"
  rm -rf /tmp/handterm-bench-$$; mkdir -p /tmp/handterm-bench-$$
  export XDG_RUNTIME_DIR=/tmp/handterm-bench-$$
  "$HANDTERM_BIN" --standalone --backend "$backend" >/tmp/handterm-bench-$$.log 2>&1 &
  local host_pid=$!
  # wait for socket
  local sock="/tmp/handterm-bench-$$/handterm-${backend}.sock"
  for _ in $(seq 1 300); do [[ -S "$sock" ]] && break; sleep 0.05; done
  if [[ ! -S "$sock" ]]; then
    echo "  ${backend} host: socket never appeared" | tee -a "$OUT"
    kill "$host_pid" 2>/dev/null; return 1
  fi
  sleep "$SETTLE"
  echo "handterm ${backend} host RSS scaling (phys_footprint of host process):" | tee -a "$OUT"
  local base
  base=$(footprint_kb "$host_pid")
  printf "  %-12s %8d KB\n" "1 window" "$base" | tee -a "$OUT"
  local prev=$base
  for n in $(seq 2 "$maxwin"); do
    "$HANDTERM_BIN" @ open-window '{}' --to "$sock" >/dev/null 2>&1
    sleep "$SETTLE"
    local cur delta
    cur=$(footprint_kb "$host_pid")
    delta=$((cur - prev))
    printf "  %-12s %8d KB   (+%d KB)\n" "${n} windows" "$cur" "$delta" | tee -a "$OUT"
    prev=$cur
  done
  kill "$host_pid" 2>/dev/null
  wait "$host_pid" 2>/dev/null
  rm -rf /tmp/handterm-bench-$$ /tmp/handterm-bench-$$.log
  echo | tee -a "$OUT"
}

measure_handterm_scaling gpu 4
measure_handterm_scaling cpu 4

############################################
# Single idle-window footprint: handterm vs ghostty vs kitty
############################################
echo "Single idle window footprint (one shell, settle ${SETTLE}s, median of 3):" | tee -a "$OUT"

# handterm single window
ht_samples=()
for i in 1 2 3; do
  rm -rf /tmp/handterm-one-$$; mkdir -p /tmp/handterm-one-$$
  XDG_RUNTIME_DIR=/tmp/handterm-one-$$ "$HANDTERM_BIN" --standalone --backend gpu --exec "sleep 600" >/dev/null 2>&1 &
  hp=$!
  sleep "$SETTLE"
  ht_samples+=("$(footprint_kb "$hp")")
  kill "$hp" 2>/dev/null; wait "$hp" 2>/dev/null
  rm -rf /tmp/handterm-one-$$
done
ht_med=$(printf '%s\n' "${ht_samples[@]}" | median)
printf "  %-22s %8d KB   samples: %s\n" "handterm gpu" "$ht_med" "${ht_samples[*]}" | tee -a "$OUT"

# Ghostty (open -na, then find newest ghostty pid)
measure_gui_app() {
  local label="$1"; shift
  local procmatch="$1"; shift
  local samples=()
  for i in 1 2 3; do
    # close pre-existing of this app to isolate? No: measure only the new pid set delta.
    local before after newpids
    before=$(pgrep -f "$procmatch" | sort)
    "$@" >/dev/null 2>&1
    sleep "$SETTLE"
    after=$(pgrep -f "$procmatch" | sort)
    newpids=$(comm -13 <(echo "$before") <(echo "$after"))
    if [[ -z "$newpids" ]]; then
      # app already running; sum all matching pids as fallback
      newpids=$(echo "$after")
    fi
    local kb
    kb=$(sum_footprint_kb $newpids)
    samples+=("$kb")
    # kill the new pids
    for p in $newpids; do kill "$p" 2>/dev/null; done
    sleep 1
  done
  local med
  med=$(printf '%s\n' "${samples[@]}" | median)
  printf "  %-22s %8d KB   samples: %s\n" "$label" "$med" "${samples[*]}" | tee -a "$OUT"
}

if [[ -d /Applications/Ghostty.app ]]; then
  measure_gui_app "ghostty" "Ghostty.app/Contents/MacOS" \
    open -na Ghostty.app --args -e sleep 600
fi

if [[ -d /Applications/kitty.app ]]; then
  measure_gui_app "kitty" "kitty.app/Contents/MacOS/kitty" \
    open -na kitty.app --args sleep 600
fi

echo | tee -a "$OUT"
echo "results saved to $OUT" | tee -a "$OUT"
