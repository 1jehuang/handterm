#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${HANDTERM_BIN:-$ROOT/target/debug/handterm}"
OUT_DIR="${HANDTERM_PROFILE_OUT:-$ROOT/profile_out}"
STAMP="$(date +%Y%m%d-%H%M%S)"
TMP_BASE="${HANDTERM_PROFILE_TMP_BASE:-/var/tmp}"
TOKEN="HANDTERM_GPU_LIVE_OK_${STAMP}"
mkdir -p "$OUT_DIR" "$TMP_BASE"
OUT_FILE="$OUT_DIR/gpu_live_validation_${STAMP}.txt"

WAYLAND_DISPLAY="${WAYLAND_DISPLAY:?WAYLAND_DISPLAY must be set}"
XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR must be set}"

TMPDIR="$(mktemp -d "$TMP_BASE/handterm-gpu-live.XXXXXX")"
SOCKET_PATH="$TMPDIR/handterm-gpu.sock"
HOST_LOG="$TMPDIR/host.log"
PID_FILE="$TMPDIR/host.pid"
HOST_PID=""

cleanup() {
  if [[ -n "$HOST_PID" ]]; then
    kill "$HOST_PID" 2>/dev/null || true
    wait "$HOST_PID" 2>/dev/null || true
  fi
  rm -rf "$TMPDIR"
}
trap cleanup EXIT

host_env() {
  env \
    XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" \
    WAYLAND_DISPLAY="$WAYLAND_DISPLAY" \
    HANDTERM_HOST_SOCKET="$SOCKET_PATH" \
    "$@"
}

fail_with_log() {
  echo "error: $1" >&2
  if [[ -f "$HOST_LOG" ]]; then
    echo "--- host log tail ---" >&2
    tail -n 80 "$HOST_LOG" >&2 || true
  fi
  exit 1
}

json_extract() {
  local expr="$1"
  python -c '
import json, sys
expr = sys.argv[1]
obj = json.load(sys.stdin)
parts = expr.split(".") if expr else []
cur = obj
for part in parts:
    if isinstance(cur, dict):
        cur = cur.get(part)
    else:
        cur = None
        break
print(cur if cur is not None else "")
' "$expr"
}

wait_for_count() {
  local expected="$1"
  for _ in $(seq 1 500); do
    if [[ -n "$HOST_PID" ]] && ! kill -0 "$HOST_PID" 2>/dev/null; then
      fail_with_log "gpu host exited before reaching window count ${expected}"
    fi
    local count_now
    count_now=$(host_env "$BIN" --backend gpu @ list-windows 2>/dev/null | json_extract 'data.count' 2>/dev/null || echo '')
    if [[ "$count_now" == "$expected" ]]; then
      return 0
    fi
    sleep 0.02
  done
  fail_with_log "timed out waiting for window count ${expected}"
}

wait_for_text() {
  local token="$1"
  for _ in $(seq 1 500); do
    local text
    text=$(host_env "$BIN" --backend gpu @ get-text '{"window_id":1}' 2>/dev/null | json_extract 'data.text' 2>/dev/null || echo '')
    if [[ "$text" == *"$token"* ]]; then
      return 0
    fi
    sleep 0.02
  done
  fail_with_log "timed out waiting for startup token text"
}

host_env BIN="$BIN" HOST_LOG="$HOST_LOG" PID_FILE="$PID_FILE" TOKEN="$TOKEN" python - <<'PY'
import os, subprocess
with open(os.environ['HOST_LOG'], 'wb') as log:
    proc = subprocess.Popen(
        [os.environ['BIN'], '--backend', 'gpu'],
        env=os.environ.copy(),
        stdout=log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
with open(os.environ['PID_FILE'], 'w') as fh:
    fh.write(str(proc.pid))
PY
HOST_PID="$(cat "$PID_FILE")"

wait_for_count 1
host_env "$BIN" --backend gpu @ send-text "{\"window_id\":1,\"text\":\"printf '$TOKEN\\r\\n'\\r\"}" >/dev/null
wait_for_text "$TOKEN"

SIZE_JSON=$(host_env "$BIN" --backend gpu @ get-size '{"window_id":1}')
SCROLL_JSON=$(host_env "$BIN" --backend gpu @ get-scroll-state '{"window_id":1}')
SET_TITLE_JSON=$(host_env "$BIN" --backend gpu @ set-title '{"window_id":1,"title":"gpu-live-validation"}')
OPEN_JSON=$(host_env "$BIN" --backend gpu open-window 2>/dev/null || true)
wait_for_count 2
FOCUS_JSON=$(host_env "$BIN" --backend gpu @ focus-window '{"window_id":2}')
CLOSE_JSON=$(host_env "$BIN" --backend gpu @ close '{"window_id":2}')
wait_for_count 1

python - <<'PY' "$OUT_FILE" "$TOKEN" "$SIZE_JSON" "$SCROLL_JSON" "$SET_TITLE_JSON" "$FOCUS_JSON" "$CLOSE_JSON" "$HOST_LOG"
import json, pathlib, sys
out_path = pathlib.Path(sys.argv[1])
token = sys.argv[2]
size = json.loads(sys.argv[3])
scroll = json.loads(sys.argv[4])
set_title = json.loads(sys.argv[5])
focus = json.loads(sys.argv[6])
close = json.loads(sys.argv[7])
host_log = pathlib.Path(sys.argv[8])
text = []
text.append(f'token={token}')
text.append(f'size_ok={size.get("ok")} cols={(size.get("data") or {}).get("cols")} rows={(size.get("data") or {}).get("rows")}')
scroll_data = scroll.get('data') or {}
text.append(f'scroll_ok={scroll.get("ok")} backend={scroll_data.get("backend")} smooth_supported={scroll_data.get("smooth_supported")}')
text.append(f'set_title_ok={set_title.get("ok")}')
text.append(f'focus_ok={focus.get("ok")}')
text.append(f'close_ok={close.get("ok")}')
text.append('--- host_log_tail ---')
if host_log.exists():
    lines = host_log.read_text(errors='replace').splitlines()[-40:]
    text.extend(lines)
out_path.write_text('\n'.join(text) + '\n')
print('\n'.join(text))
PY

echo
echo "Saved to $OUT_FILE"
