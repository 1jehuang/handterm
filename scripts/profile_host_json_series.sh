#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${HANDTERM_BIN:-$ROOT/target/release/handterm}"
OUT_DIR="${HANDTERM_PROFILE_OUT:-$ROOT/profile_out}"
STAMP="$(date +%Y%m%d-%H%M%S)"
ADD_WINDOWS="${1:-${HANDTERM_PROFILE_ADD_WINDOWS:-3}}"
REPEATS="${2:-${HANDTERM_PROFILE_REPEATS:-1}}"
BACKEND="${HANDTERM_SPAWN_BACKEND:-gpu}"
SAFE_MAX_ADD_WINDOWS="${HANDTERM_PROFILE_SAFE_MAX_ADD_WINDOWS:-4}"
TMP_BASE="${HANDTERM_PROFILE_TMP_BASE:-/var/tmp}"
mkdir -p "$OUT_DIR"
JSONL_FILE="$OUT_DIR/host_json_series_${BACKEND}_${STAMP}.jsonl"
SUMMARY_FILE="$OUT_DIR/host_json_series_${BACKEND}_${STAMP}.txt"

usage() {
  cat <<EOF
Usage: $(basename "$0") [add_windows] [repeats]

Collect a small safe machine-parsed host open-window sample series using
HANDTERM_PROFILE_JSON=1.

Defaults:
  add_windows: 3
  repeats: 1

Safety:
  - defaults to a low live-window count
  - runs repeats sequentially, one host session at a time
  - refuses add_windows > ${SAFE_MAX_ADD_WINDOWS} unless HANDTERM_UNSAFE_MANY_WINDOWS=1

Environment overrides:
  HANDTERM_BIN                     binary to launch
  HANDTERM_PROFILE_OUT             output directory
  HANDTERM_PROFILE_ADD_WINDOWS     default add-window count
  HANDTERM_PROFILE_REPEATS         default repeat count
  HANDTERM_SPAWN_BACKEND           backend to sample (gpu or cpu)
  HANDTERM_PROFILE_SAFE_MAX_ADD_WINDOWS  safe cap before refusal
  HANDTERM_PROFILE_TMP_BASE        temp base dir for isolated socket/log state
  HANDTERM_SKIP_BUILD=1            skip cargo build
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ "$BACKEND" != "gpu" && "$BACKEND" != "cpu" ]]; then
  echo "error: backend must be either gpu or cpu" >&2
  exit 1
fi

if ! [[ "$ADD_WINDOWS" =~ ^[0-9]+$ ]] || (( ADD_WINDOWS < 1 )); then
  echo "error: add_windows must be a positive integer" >&2
  exit 1
fi

if ! [[ "$REPEATS" =~ ^[0-9]+$ ]] || (( REPEATS < 1 )); then
  echo "error: repeats must be a positive integer" >&2
  exit 1
fi

if (( ADD_WINDOWS > SAFE_MAX_ADD_WINDOWS )) && [[ "${HANDTERM_UNSAFE_MANY_WINDOWS:-0}" != "1" ]]; then
  echo "error: refusing add_windows=${ADD_WINDOWS}; safe cap is ${SAFE_MAX_ADD_WINDOWS}. Set HANDTERM_UNSAFE_MANY_WINDOWS=1 to override intentionally." >&2
  exit 1
fi

WAYLAND_DISPLAY="${WAYLAND_DISPLAY:?WAYLAND_DISPLAY must be set}"
XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR must be set}"

if [[ "${HANDTERM_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build -q --release --workspace --manifest-path "$ROOT/Cargo.toml"
fi

mkdir -p "$TMP_BASE"
: > "$JSONL_FILE"

if [[ "$BACKEND" == "gpu" ]]; then
  OPEN_EVENT_NAME="gpu_host_open_window"
  READY_EVENT_NAME="gpu_host_first_frame"
else
  OPEN_EVENT_NAME="cpu_host_open_window"
  READY_EVENT_NAME="cpu_host_startup"
fi

cleanup_session() {
  if [[ -n "${HOST_PID:-}" ]]; then
    kill "$HOST_PID" 2>/dev/null || true
    wait "$HOST_PID" 2>/dev/null || true
  fi
  if [[ -n "${TMPDIR:-}" ]]; then
    rm -rf "$TMPDIR"
  fi
}
trap cleanup_session EXIT

host_env() {
  env \
    XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" \
    WAYLAND_DISPLAY="$WAYLAND_DISPLAY" \
    HANDTERM_HOST_SOCKET="$SOCKET_PATH" \
    HANDTERM_PROFILE_JSON=1 \
    "$@"
}

fail_with_host_log() {
  echo "error: $1" >&2
  if [[ -f "$HOST_LOG" ]]; then
    echo "--- host log tail ---" >&2
    tail -n 80 "$HOST_LOG" >&2 || true
  fi
  exit 1
}

host_is_alive() {
  [[ -n "${HOST_PID:-}" ]] && kill -0 "$HOST_PID" 2>/dev/null
}

wait_for_window_count() {
  local expected_count="$1"
  for _ in $(seq 1 500); do
    if ! host_is_alive; then
      fail_with_host_log "${BACKEND} host exited before window count reached ${expected_count}"
    fi
    local count_now
    count_now=$(host_env "$BIN" --backend "$BACKEND" @ list-windows 2>/dev/null \
      | python -c "import sys,json; data=json.load(sys.stdin); print((data.get('data') or {}).get('count', 0))" 2>/dev/null \
      || echo 0)
    if [[ "$count_now" -ge "$expected_count" ]]; then
      return 0
    fi
    sleep 0.02
  done
  fail_with_host_log "timed out waiting for window count ${expected_count}"
}

wait_for_profile_event() {
  local event_name="$1"
  local window_id="$2"
  for _ in $(seq 1 500); do
    if ! host_is_alive; then
      fail_with_host_log "${BACKEND} host exited before ${event_name} for window ${window_id}"
    fi
    if python - <<'PY' "$HOST_LOG" "$event_name" "$window_id"
import json, sys
log_path, event_name, window_id = sys.argv[1], sys.argv[2], int(sys.argv[3])
with open(log_path, errors='replace') as fh:
    for line in fh:
        line = line.strip()
        if not line.startswith('{'):
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        if obj.get('type') != 'handterm_profile':
            continue
        if obj.get('event') != event_name:
            continue
        if (obj.get('data') or {}).get('id') == window_id:
            raise SystemExit(0)
raise SystemExit(1)
PY
    then
      return 0
    fi
    sleep 0.02
  done
  fail_with_host_log "timed out waiting for ${event_name} for window ${window_id}"
}

launch_host() {
  XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" \
  WAYLAND_DISPLAY="$WAYLAND_DISPLAY" \
  HANDTERM_HOST_SOCKET="$SOCKET_PATH" \
  HANDTERM_PROFILE_JSON=1 \
  BIN="$BIN" BACKEND="$BACKEND" HOST_LOG="$HOST_LOG" PID_FILE="$PID_FILE" python - <<'PY'
import os
import subprocess

with open(os.environ['HOST_LOG'], 'wb') as log:
    proc = subprocess.Popen(
        [os.environ['BIN'], '--backend', os.environ['BACKEND']],
        env=os.environ.copy(),
        stdout=log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
with open(os.environ['PID_FILE'], 'w') as fh:
    fh.write(str(proc.pid))
PY
  HOST_PID="$(cat "$PID_FILE")"
}

append_session_records() {
  local session_index="$1"
  python - <<'PY' "$HOST_LOG" "$JSONL_FILE" "$BACKEND" "$session_index"
import json
import sys
from pathlib import Path

log_path, jsonl_path, backend, session_index = sys.argv[1], Path(sys.argv[2]), sys.argv[3], int(sys.argv[4])
open_event_name = f'{backend}_host_open_window'
ready_event_name = 'gpu_host_first_frame' if backend == 'gpu' else 'cpu_host_startup'
records = []
with open(log_path, errors='replace') as fh:
    for line in fh:
        line = line.strip()
        if not line.startswith('{'):
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        if obj.get('type') != 'handterm_profile':
            continue
        if obj.get('event') not in {open_event_name, ready_event_name}:
            continue
        obj['session'] = session_index
        records.append(obj)
with jsonl_path.open('a') as out:
    for obj in records:
        out.write(json.dumps(obj, sort_keys=True) + '\n')
PY
}

run_session() {
  local session_index="$1"
  TMPDIR="$(mktemp -d "$TMP_BASE/handterm-host-json-series.XXXXXX")"
  SOCKET_PATH="$TMPDIR/handterm-${BACKEND}.sock"
  HOST_LOG="$TMPDIR/host.log"
  PID_FILE="$TMPDIR/host.pid"
  HOST_PID=""

  launch_host
  wait_for_window_count 1
  wait_for_profile_event "$OPEN_EVENT_NAME" 1
  wait_for_profile_event "$READY_EVENT_NAME" 1

  for window_id in $(seq 2 $((ADD_WINDOWS + 1))); do
    host_env "$BIN" --backend "$BACKEND" open-window >/dev/null 2>&1
    wait_for_window_count "$window_id"
    wait_for_profile_event "$OPEN_EVENT_NAME" "$window_id"
    wait_for_profile_event "$READY_EVENT_NAME" "$window_id"
  done

  append_session_records "$session_index"
  cleanup_session
  TMPDIR=""
  SOCKET_PATH=""
  HOST_LOG=""
  PID_FILE=""
  HOST_PID=""
}

for session_index in $(seq 1 "$REPEATS"); do
  run_session "$session_index"
done

python - <<'PY' "$JSONL_FILE" "$SUMMARY_FILE" "$ADD_WINDOWS" "$BACKEND" "$REPEATS"
import json
import statistics
import sys
from pathlib import Path

jsonl_path, summary_path, add_windows, backend, repeats = Path(sys.argv[1]), Path(sys.argv[2]), int(sys.argv[3]), sys.argv[4], int(sys.argv[5])
records = [json.loads(line) for line in jsonl_path.read_text().splitlines() if line.strip()]
open_event_name = f'{backend}_host_open_window'
ready_event_name = 'gpu_host_first_frame' if backend == 'gpu' else 'cpu_host_startup'

open_events = {}
ready_events = {}
for obj in records:
    data = obj.get('data') or {}
    session = obj.get('session')
    window_id = data.get('id')
    if not isinstance(session, int) or not isinstance(window_id, int):
        continue
    key = (session, window_id)
    if obj.get('event') == open_event_name:
        open_events[key] = data
    elif obj.get('event') == ready_event_name:
        ready_events[key] = data

rows = []
for key in sorted(open_events):
    session, window_id = key
    open_data = open_events[key]
    ready_data = ready_events.get(key, {})
    if backend == 'gpu':
        surface = open_data.get('surface') or {}
        rows.append({
            'session': session,
            'id': window_id,
            'kind': open_data.get('kind', 'unknown'),
            'total_ms': open_data.get('total_ms'),
            'host_setup_before_surface_ms': open_data.get('host_setup_before_surface_ms'),
            'compositor_facing_ms': open_data.get('compositor_facing_ms'),
            'handterm_surface_setup_ms': open_data.get('handterm_surface_setup_ms'),
            'configure_ms': surface.get('configure_ms'),
            'open_to_first_present_ms': ready_data.get('open_to_first_present_ms'),
        })
    else:
        rows.append({
            'session': session,
            'id': window_id,
            'kind': open_data.get('kind', 'unknown'),
            'total_ms': open_data.get('total_ms'),
            'dpi_ms': open_data.get('dpi_ms'),
            'bootstrap_ms': open_data.get('bootstrap_ms'),
            'window_ms': open_data.get('window_ms'),
            'atlas_ms': open_data.get('atlas_ms'),
            'pty_ms': open_data.get('pty_ms'),
            'open_to_first_present_ms': ready_data.get('open_to_first_present_ms'),
            'open_to_first_visible_output_ms': ready_data.get('open_to_first_visible_output_ms'),
            'open_to_first_visible_present_ms': ready_data.get('open_to_first_visible_present_ms'),
        })

fields = (
    [
        'total_ms',
        'host_setup_before_surface_ms',
        'compositor_facing_ms',
        'handterm_surface_setup_ms',
        'configure_ms',
        'open_to_first_present_ms',
    ]
    if backend == 'gpu'
    else [
        'total_ms',
        'dpi_ms',
        'bootstrap_ms',
        'window_ms',
        'atlas_ms',
        'pty_ms',
        'open_to_first_present_ms',
        'open_to_first_visible_output_ms',
        'open_to_first_visible_present_ms',
    ]
)

def summarize(group_name, subset):
    lines = [f'[{group_name}] count={len(subset)}']
    if not subset:
        return lines
    for field in fields:
        values = [row[field] for row in subset if isinstance(row.get(field), (int, float))]
        if not values:
            continue
        lines.append(f'  {field}: min={min(values):.2f} median={statistics.median(values):.2f} max={max(values):.2f}')
    session_ids = sorted({row['session'] for row in subset})
    lines.append('  sessions=' + ', '.join(str(session_id) for session_id in session_ids))
    return lines

first_rows = [row for row in rows if row['kind'] == 'first-window']
add_rows = [row for row in rows if row['kind'] == 'add-window']
output = []
output.append(f'backend={backend} add_windows={add_windows} repeats={repeats} total_windows={len(rows)}')
output.append(f'jsonl={jsonl_path}')
output.extend(summarize('first-window', first_rows))
output.extend(summarize('add-window', add_rows))
output.append('')
output.append('[per-window]')
for row in rows:
    if backend == 'gpu':
        output.append(
            '  session={session} id={id} kind={kind} total_ms={total_ms:.2f} compositor_facing_ms={compositor_facing_ms:.2f} '
            'configure_ms={configure_ms:.2f} open_to_first_present_ms={open_to_first_present_ms:.2f}'.format(
                session=row['session'],
                id=row['id'],
                kind=row['kind'],
                total_ms=row['total_ms'] or 0.0,
                compositor_facing_ms=row['compositor_facing_ms'] or 0.0,
                configure_ms=row['configure_ms'] or 0.0,
                open_to_first_present_ms=row['open_to_first_present_ms'] or 0.0,
            )
        )
    else:
        output.append(
            '  session={session} id={id} kind={kind} total_ms={total_ms:.2f} pty_ms={pty_ms:.2f} '
            'open_to_first_visible_present_ms={open_to_first_visible_present_ms:.2f}'.format(
                session=row['session'],
                id=row['id'],
                kind=row['kind'],
                total_ms=row['total_ms'] or 0.0,
                pty_ms=row['pty_ms'] or 0.0,
                open_to_first_visible_present_ms=row['open_to_first_visible_present_ms'] or 0.0,
            )
        )
summary_text = '\n'.join(output) + '\n'
summary_path.write_text(summary_text)
print(summary_text, end='')
PY

printf '\nSaved JSONL to %s\n' "$JSONL_FILE"
printf 'Saved summary to %s\n' "$SUMMARY_FILE"
