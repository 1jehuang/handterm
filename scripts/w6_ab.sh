#!/usr/bin/env bash
# Interleaved A/B for first-window startup: alternate base vs new binary to
# cancel out machine warm-up/thermal noise. Prints median of each metric.
set -uo pipefail
BASE="${BASE:-/tmp/handterm_base}"
NEW="${NEW:-/tmp/handterm_new}"
RUNS="${1:-12}"

run_one() {
  local bin="$1" rt="$2"
  rm -rf "$rt"; mkdir -p "$rt"
  XDG_RUNTIME_DIR="$rt" "$bin" --standalone --backend gpu --exec 'sleep 4' >"$rt/out.log" 2>&1 &
  local pid=$!
  for _ in $(seq 1 80); do grep -q 'open_to_first_present' "$rt/out.log" && break; sleep 0.05; done
  sleep 0.2
  kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
  # emit: ofp dpi window atlas
  local ofp dpiw
  ofp=$(grep -oE 'open_to_first_present=[0-9.]+' "$rt/out.log" | head -1 | cut -d= -f2)
  local dpi window
  dpi=$(grep -oE 'dpi=[0-9.]+' "$rt/out.log" | head -1 | cut -d= -f2)
  window=$(grep -oE 'window=[0-9.]+' "$rt/out.log" | head -1 | cut -d= -f2)
  echo "$ofp $dpi $window"
}

declare -a BASE_OFP NEW_OFP BASE_DW NEW_DW
for i in $(seq 1 "$RUNS"); do
  read -r o d w < <(run_one "$BASE" /tmp/ab_base)
  BASE_OFP+=("$o"); BASE_DW+=("$(echo "$d + $w" | bc)")
  read -r o d w < <(run_one "$NEW" /tmp/ab_new)
  NEW_OFP+=("$o"); NEW_DW+=("$(echo "$d + $w" | bc)")
done

median() { printf '%s\n' "$@" | sort -n | awk '{a[NR]=$1} END{ if(NR%2){print a[(NR+1)/2]} else {print (a[NR/2]+a[NR/2+1])/2} }'; }

echo "first-window open_to_first_present (ms):"
echo "  base median: $(median "${BASE_OFP[@]}")   [${BASE_OFP[*]}]"
echo "  new  median: $(median "${NEW_OFP[@]}")   [${NEW_OFP[*]}]"
echo "first-window dpi+window (ms):"
echo "  base median: $(median "${BASE_DW[@]}")   [${BASE_DW[*]}]"
echo "  new  median: $(median "${NEW_DW[@]}")   [${NEW_DW[*]}]"
