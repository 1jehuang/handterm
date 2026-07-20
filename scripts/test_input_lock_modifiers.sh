#!/usr/bin/env bash
set -euo pipefail

BIN="${BIN:-./target/debug/handterm}"
BACKEND="${BACKEND:-gpu}"

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "SKIP: lock-modifier integration requires a Linux Wayland session"
  exit 0
fi

REAL_RUNTIME="${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR must be set}"
REAL_WAYLAND="${WAYLAND_DISPLAY:?WAYLAND_DISPLAY must be set}"
TMPDIR="$(mktemp -d)"
SOCK="$TMPDIR/handterm-$BACKEND.sock"
LOG="$TMPDIR/host.log"

cleanup() {
  if [[ -n "${PID:-}" ]]; then
    kill "$PID" >/dev/null 2>&1 || true
    wait "$PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TMPDIR"
}
trap cleanup EXIT

cat >"$TMPDIR/test-shell.sh" <<'EOF'
#!/usr/bin/env bash
export PS1='> '
export PROMPT_COMMAND=
exec /usr/bin/bash --noprofile --norc -i
EOF
chmod +x "$TMPDIR/test-shell.sh"
cat >"$TMPDIR/capture_keys.py" <<'EOF'
import os
import sys
import termios
import tty

fd = sys.stdin.fileno()
old = termios.tcgetattr(fd)
try:
    tty.setraw(fd)
    sys.stdout.write("\x1b[=8u")
    sys.stdout.flush()
    for _ in range(4):
        data = os.read(fd, 4096)
        if not data:
            break
        sys.stdout.write(data.hex() + "\n")
        sys.stdout.flush()
finally:
    termios.tcsetattr(fd, termios.TCSADRAIN, old)
EOF
ln -s "$REAL_RUNTIME/$REAL_WAYLAND" "$TMPDIR/$REAL_WAYLAND"
if [[ -e "$REAL_RUNTIME/$REAL_WAYLAND.lock" ]]; then
  ln -s "$REAL_RUNTIME/$REAL_WAYLAND.lock" "$TMPDIR/$REAL_WAYLAND.lock"
fi

XDG_RUNTIME_DIR="$TMPDIR" WAYLAND_DISPLAY="$REAL_WAYLAND" SHELL="$TMPDIR/test-shell.sh" \
  "$BIN" --standalone --backend "$BACKEND" >"$LOG" 2>&1 &
PID=$!

for _ in $(seq 1 100); do
  [[ -S "$SOCK" ]] && break
  sleep 0.1
done
[[ -S "$SOCK" ]]
sleep 1

send() {
  "$BIN" --backend "$BACKEND" @ --to "$SOCK" "$1" "$2" >/dev/null
}

capture_lines() {
  local out="$1"
  "$BIN" --backend "$BACKEND" @ --to "$SOCK" get-text '{}' > "$out"
  python - "$out" <<'PY'
import json, sys
with open(sys.argv[1]) as f:
    data = json.load(f)
lines = [line for line in data["data"]["text"].splitlines() if line.strip()]
hex_lines = []
for line in lines:
    stripped = line.strip()
    if stripped and all(ch in '0123456789abcdef' for ch in stripped.lower()):
        hex_lines.append(stripped)
for line in hex_lines[-4:]:
    print(line)
PY
}

CMD_JSON="$(python -c 'import json, sys; print(json.dumps({"text": sys.argv[1]}))' "python3 $TMPDIR/capture_keys.py")"
send send-text "$CMD_JSON"
send send-key '{"key":"enter"}'
sleep 0.8

send send-key-event '{"kind":"press","key":"capslock"}'
sleep 0.15
send send-key-event '{"kind":"press","key":"x","text":"x"}'
sleep 0.15
send send-key-event '{"kind":"press","key":"numlock"}'
sleep 0.15
send send-key-event '{"kind":"press","key":"up"}'
sleep 0.3

mapfile -t lines < <(capture_lines "$TMPDIR/lock-lines.json")
printf 'lock-lines=%s\n' "${lines[*]}"

expected=(
  '1b5b35373335383b363575'
  '1b5b3132303b363575'
  '1b5b35373336303b31393375'
  '1b5b313b31393341'
)

if [[ ${#lines[@]} -ne 4 ]]; then
  echo "unexpected number of captured lock-modifier lines" >&2
  cat "$LOG" >&2
  exit 1
fi

for i in "${!expected[@]}"; do
  if [[ "${lines[$i]}" != "${expected[$i]}" ]]; then
    echo "unexpected lock-modifier line $i: got ${lines[$i]}, expected ${expected[$i]}" >&2
    cat "$LOG" >&2
    exit 1
  fi
done

echo "synthetic kitty lock-modifier input passed"
