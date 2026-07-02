#!/usr/bin/env bash
# Feature-matrix hygiene gate for the handterm workspace.
#
# Checks the handterm lib across the meaningful feature combinations and
# fails on any rustc warning (RUSTFLAGS="-D warnings"). The bin target is
# feature-gated on `cli` in Cargo.toml, so lib-only checks cover every combo.
#
# Usage: scripts/check-features.sh
set -euo pipefail

cd "$(dirname "$0")/.."

combos=(
    ""                                                # default features
    "--no-default-features"                           # bare lib
    "--no-default-features --features cli"            # cli (implies standalone)
    "--no-default-features --features cpu,standalone"
    "--no-default-features --features gpu,standalone"
    "--no-default-features --features cpu,standalone,cli"
    "--no-default-features --features gpu,standalone,cli"
)

failed=0
for combo in "${combos[@]}"; do
    label="${combo:-default}"
    echo "==> cargo check --lib ${label}"
    # shellcheck disable=SC2086
    if ! RUSTFLAGS="-D warnings" cargo check --quiet --lib -p handterm ${combo}; then
        echo "FAILED: ${label}" >&2
        failed=1
    fi
done

if [[ "${failed}" -ne 0 ]]; then
    echo "feature matrix check failed" >&2
    exit 1
fi
echo "feature matrix check passed (${#combos[@]} combos)"
