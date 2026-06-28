#!/usr/bin/env bash
# Deterministic benchmark capture for handterm on macOS.
#
# Runs the built-in `handterm bench` (pure logic: parser/grid/terminal/render/gpu-prep),
# parses the human-readable output into a stable JSON blob, and writes it to a file.
#
# This is the primary *verifiable* optimization gate for the swarm: it is fast,
# deterministic, requires no GUI/compositor, and every number maps to a hot path.
#
# Usage:
#   scripts/bench_capture.sh [LABEL] [REPEATS]
#
# Env:
#   HANDTERM_BIN   path to handterm binary (default: target/release/handterm)
#   BENCH_OUT_DIR  output dir (default: bench_out)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LABEL="${1:-baseline}"
REPEATS="${2:-5}"
HANDTERM_BIN="${HANDTERM_BIN:-$ROOT/target/release/handterm}"
OUT_DIR="${BENCH_OUT_DIR:-$ROOT/bench_out}"
mkdir -p "$OUT_DIR"

if [[ ! -x "$HANDTERM_BIN" ]]; then
  echo "handterm binary not found at $HANDTERM_BIN" >&2
  echo "build it first: cargo build --release" >&2
  exit 2
fi

# Each metric: regex over the bench text -> we take the best (max) across REPEATS,
# except latency/startup metrics where lower is better (we take the min).
# Format: key|grep_pattern|field_index|mode(max|min)
METRICS=$(cat <<'EOF'
memcpy_mb_s|memcpy (64MB)|MAX
byte_scan_mb_s|byte scan (64MB)|MAX
parser_ascii_mb_s|^  ASCII *: .*MB/s  (.*% of memcpy)|MAX_PARSER
EOF
)

# Collect raw runs.
RAW="$OUT_DIR/${LABEL}.raw.txt"
: > "$RAW"
for i in $(seq 1 "$REPEATS"); do
  echo "### run $i" >> "$RAW"
  "$HANDTERM_BIN" bench >> "$RAW" 2>&1
done

# Parse with awk into JSON. We take, per metric, the best value across runs.
# Higher-is-better for throughput; lower-is-better for *_us / *_ns / latency.
python3 - "$RAW" "$LABEL" "$OUT_DIR" <<'PYEOF'
import re, sys, json, statistics, datetime

raw_path, label, out_dir = sys.argv[1], sys.argv[2], sys.argv[3]
text = open(raw_path).read()
runs = text.split("### run")[1:]

# metric_name -> (regex with one capture group, better) where better in {"max","min"}
patterns = {
    "memcpy_mb_s":            (r"memcpy \(64MB\)\s*:\s*([\d.]+) MB/s", "max"),
    "byte_scan_mb_s":         (r"byte scan \(64MB\)\s*:\s*([\d.]+) MB/s", "max"),
    "parser_ascii_mb_s":      (r"Parser .*?\n  ASCII\s*:\s*([\d.]+) MB/s", "max"),
    "parser_sgr_mb_s":        (r"SGR color sequences\s*:\s*([\d.]+) MB/s", "max"),
    "parser_mixed_mb_s":      (r"Mixed \(SGR\+cursor\+erase\)\s*:\s*([\d.]+) MB/s", "max"),
    "grid_ascii_mb_s":        (r"Grid Write .*?\n  ASCII\s*:\s*([\d.]+) MB/s", "max"),
    "grid_utf8_mb_s":         (r"UTF-8 mixed\s*:\s*([\d.]+) MB/s", "max"),
    "grid_sgr_mb_s":          (r"SGR true-color\s*:\s*([\d.]+) MB/s", "max"),
    "terminal_ascii_mb_s":    (r"Full Terminal Pipeline .*?\n  ASCII\s*:\s*([\d.]+) MB/s", "max"),
    "terminal_sgr_mb_s":      (r"Full Terminal Pipeline .*?SGR color\s*:\s*([\d.]+) MB/s", "max"),
    "terminal_mixed_mb_s":    (r"Mixed realistic\s*:\s*([\d.]+) MB/s", "max"),
    "gpu_cell_fill_mcells_s": (r"cell info fill\s*:\s*([\d.]+) Mcells/s", "max"),
    "gpu_text_batch_mcells_s":(r"text batching\s*:\s*([\d.]+) Mcells/s", "max"),
    "gpu_prompt_fps":         (r"GPU Frame Prep.*?prompt replay\s*:\s*([\d.]+) fps", "max"),
    "gpu_tui_fps":            (r"GPU Frame Prep.*?TUI help replay\s*:\s*([\d.]+) fps", "max"),
    "cpu_full_redraw_fps":    (r"CPU Renderer.*?offscreen full redraw\s*:\s*([\d.]+) fps", "max"),
    "cpu_incremental_fps":    (r"incremental typing\s*:\s*([\d.]+) fps", "max"),
    "cpu_prompt_fps":         (r"CPU Renderer.*?prompt replay\s*:\s*([\d.]+) fps", "max"),
    "cpu_tui_fps":            (r"CPU Renderer.*?TUI help replay\s*:\s*([\d.]+) fps", "max"),
    "protocol_msg_s":         (r"message roundtrips\s*:\s*([\d.]+) msg/s", "max"),
    "cell_size_bytes":        (r"cell size\s*:\s*([\d.]+) bytes", "min"),
    "cell_write_ns":          (r"cell write latency\s*:\s*([\d.]+) ns/cell", "min"),
    "scrollback_10k_kb":      (r"10k scrollback\s*:\s*([\d.]+) KB", "min"),
}

result = {}
for name, (pat, better) in patterns.items():
    vals = []
    for run in runs:
        m = re.search(pat, run, re.S)
        if m:
            vals.append(float(m.group(1)))
    if not vals:
        result[name] = None
        continue
    chosen = max(vals) if better == "max" else min(vals)
    result[name] = {
        "value": chosen,
        "median": statistics.median(vals),
        "n": len(vals),
        "better": better,
    }

blob = {
    "label": label,
    "timestamp": datetime.datetime.utcnow().isoformat() + "Z",
    "metrics": result,
}
out = f"{out_dir}/{label}.json"
json.dump(blob, open(out, "w"), indent=2)
print(f"wrote {out}")
# Pretty summary
for k, v in result.items():
    if v is None:
        print(f"  {k:28s} MISSING")
    else:
        print(f"  {k:28s} {v['value']:>14.2f}  ({v['better']}, n={v['n']})")
PYEOF
