#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release/handterm"
OUT_DIR="${HANDTERM_PROFILE_OUT:-$ROOT/profile_out}"
mkdir -p "$OUT_DIR"

if [[ ! -x "$BIN" ]]; then
  echo "building release binary..."
  cargo build --release --manifest-path "$ROOT/Cargo.toml"
fi

BENCH_OUT="$OUT_DIR/bench.txt"
SIZE_OUT="$OUT_DIR/binary_size.txt"

"$BIN" bench | tee "$BENCH_OUT"
{
  ls -lh "$BIN"
  stat -c '%n %s bytes' "$BIN"
} | tee "$SIZE_OUT"

echo "wrote $BENCH_OUT and $SIZE_OUT"
