#!/usr/bin/env bash
set -euo pipefail

BIN="${BIN:-./target/debug/handterm}"
BACKEND="${BACKEND:-gpu}"
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
ln -s "$REAL_RUNTIME/$REAL_WAYLAND" "$TMPDIR/$REAL_WAYLAND"
if [[ -e "$REAL_RUNTIME/$REAL_WAYLAND.lock" ]]; then
  ln -s "$REAL_RUNTIME/$REAL_WAYLAND.lock" "$TMPDIR/$REAL_WAYLAND.lock"
fi

XDG_RUNTIME_DIR="$TMPDIR" WAYLAND_DISPLAY="$REAL_WAYLAND" SHELL="$TMPDIR/test-shell.sh" HANDTERM_TRACE_INPUT=1 \
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

capture_line() {
  local out="$1"
  "$BIN" --backend "$BACKEND" @ --to "$SOCK" get-text '{}' > "$out"
  python - "$out" <<'PY'
import json,sys
with open(sys.argv[1]) as f:
    data=json.load(f)
lines = [line for line in data["data"]["text"].splitlines() if line.strip()]
print(lines[-1] if lines else "")
PY
}

send send-key '{"key":"ctrl+u"}'
sleep 0.1

send send-key-event '{"kind":"press","key":"a","text":"a"}'
send send-key-event '{"kind":"press","key":"space","text":" "}'
send send-ime-commit '{"text":" "}'
send send-key-event '{"kind":"press","key":"b","text":"b"}'
sleep 0.2
line1="$(capture_line "$TMPDIR/line1.json")"

send send-key-event '{"kind":"press","key":"backspace"}'
send send-ime-commit '{"text":"\u007f"}'
sleep 0.2
line2="$(capture_line "$TMPDIR/line2.json")"

printf 'line1=%q\n' "$line1"
printf 'line2=%q\n' "$line2"

if [[ "$line1" != '> a b' ]]; then
  echo "unexpected line after synthetic space/IME test" >&2
  cat "$LOG" >&2
  exit 1
fi

if [[ "$line2" != '> a' ]]; then
  echo "unexpected line after synthetic backspace/IME test" >&2
  cat "$LOG" >&2
  exit 1
fi

echo "synthetic frontend input dedupe passed"
