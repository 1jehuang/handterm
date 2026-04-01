#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${HANDTERM_BIN:-$ROOT/target/release/handterm}"
OUT_DIR="${HANDTERM_PROFILE_OUT:-$ROOT/profile_out}"
mkdir -p "$OUT_DIR"
STAMP="$(date +%Y%m%d-%H%M%S)"
OUT_FILE="$OUT_DIR/spawn_single_${STAMP}.txt"
BACKEND="${HANDTERM_SPAWN_BACKEND:-gpu}"
WAYLAND_DISPLAY="${WAYLAND_DISPLAY:?WAYLAND_DISPLAY must be set}"
XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR must be set}"

cargo build -q --release --workspace --manifest-path "$ROOT/Cargo.toml"

TMPDIR="$(mktemp -d /tmp/handterm-spawn-single.XXXXXX)"
ln -s "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" "$TMPDIR/$WAYLAND_DISPLAY"
HOST_LOG="$TMPDIR/host.log"

cleanup() {
  if [[ -n "${HOST_PID:-}" ]]; then
    kill "$HOST_PID" 2>/dev/null || true
    wait "$HOST_PID" 2>/dev/null || true
  fi
  rm -rf "$TMPDIR"
}
trap cleanup EXIT

env XDG_RUNTIME_DIR="$TMPDIR" WAYLAND_DISPLAY="$WAYLAND_DISPLAY" "$BIN" --backend "$BACKEND" >"$HOST_LOG" 2>&1 &
HOST_PID=$!

for _ in $(seq 1 500); do
  if env XDG_RUNTIME_DIR="$TMPDIR" WAYLAND_DISPLAY="$WAYLAND_DISPLAY" "$BIN" --backend "$BACKEND" @ list-windows >/dev/null 2>&1; then
    break
  fi
  sleep 0.02
done

START_NS=$(date +%s%N)
env XDG_RUNTIME_DIR="$TMPDIR" WAYLAND_DISPLAY="$WAYLAND_DISPLAY" "$BIN" --backend "$BACKEND" open-window >/dev/null 2>&1
for _ in $(seq 1 500); do
  COUNT_NOW=$(env XDG_RUNTIME_DIR="$TMPDIR" WAYLAND_DISPLAY="$WAYLAND_DISPLAY" "$BIN" --backend "$BACKEND" @ list-windows 2>/dev/null | python -c "import sys,json; o=json.load(sys.stdin); print((o.get('data') or {}).get('count', 0))" 2>/dev/null || echo 0)
  if [[ "$COUNT_NOW" -ge 2 ]]; then
    break
  fi
  sleep 0.01
done

for _ in $(seq 1 300); do
  if grep -q "handterm ${BACKEND} host: startup id=2" "$HOST_LOG" 2>/dev/null; then
    break
  fi
  sleep 0.01
done

END_NS=$(date +%s%N)

python - <<'PY' "$HOST_LOG" "$START_NS" "$END_NS" "$BACKEND" > "$OUT_FILE"
import sys,re
log_path,start_ns,end_ns,backend=sys.argv[1:]
wall_ms=(int(end_ns)-int(start_ns))/1_000_000
text=open(log_path, errors='replace').read()
print(f'backend={backend}')
print(f'open_window_wall_ms={wall_ms:.2f}')
patterns=[
    (r'handterm gpu host: open-window id=2\n\s+total=([0-9.]+)ms', 'open_window_internal_ms'),
    (r'handterm gpu host: first-frame id=2\n\s+open_to_first_present=([0-9.]+)ms', 'first_present_ms'),
    (r'handterm gpu host: startup id=2\n(?:.*\n)*?\s+open_to_first_visible_output=([0-9.]+)ms', 'visible_output_ms'),
    (r'handterm gpu host: open-window id=2\n(?:.*\n)*?\s+surface_total=([0-9.]+)ms', 'surface_total_ms'),
    (r'handterm gpu host: open-window id=2\n(?:.*\n)*?\s+host_cpu_user=([0-9.]+)ms host_cpu_system=([0-9.]+)ms host_cpu_total=([0-9.]+)ms', 'open_cpu'),
    (r'handterm gpu host: first-frame id=2\n(?:.*\n)*?\s+host_cpu_user=([0-9.]+)ms host_cpu_system=([0-9.]+)ms host_cpu_total=([0-9.]+)ms', 'first_present_cpu'),
    (r'handterm gpu host: startup-cpu id=2\n\s+open_to_first_visible_present_user=([0-9.]+)ms open_to_first_visible_present_system=([0-9.]+)ms open_to_first_visible_present_total=([0-9.]+)ms', 'startup_cpu'),
    (r'\s+window_create=([0-9.]+)ms ime=([0-9.]+)ms wgpu_surface=([0-9.]+)ms\n\s+default_config=([0-9.]+)ms caps=([0-9.]+)ms configure=([0-9.]+)ms', 'breakdown'),
]
for pat, label in patterns:
    m=list(re.finditer(pat, text))
    if not m:
        continue
    g=m[-1].groups()
    if label == 'breakdown':
        print(f'window_create_ms={g[0]}')
        print(f'ime_ms={g[1]}')
        print(f'wgpu_surface_ms={g[2]}')
        print(f'default_config_ms={g[3]}')
        print(f'caps_ms={g[4]}')
        print(f'configure_ms={g[5]}')
    elif label == 'open_cpu':
        print(f'open_cpu_user_ms={g[0]}')
        print(f'open_cpu_system_ms={g[1]}')
        print(f'open_cpu_total_ms={g[2]}')
    elif label == 'first_present_cpu':
        print(f'first_present_cpu_user_ms={g[0]}')
        print(f'first_present_cpu_system_ms={g[1]}')
        print(f'first_present_cpu_total_ms={g[2]}')
    elif label == 'startup_cpu':
        print(f'startup_cpu_user_ms={g[0]}')
        print(f'startup_cpu_system_ms={g[1]}')
        print(f'startup_cpu_total_ms={g[2]}')
    else:
        print(f'{label}={g[0]}')
print('--- host_log_tail ---')
for line in text.splitlines()[-40:]:
    print(line)
PY

cat "$OUT_FILE"
printf '\nSaved to %s\n' "$OUT_FILE"
