#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

IBCMD_CONFIG_PATH="${V8TR_IBCMD_REAL_CONFIG:-}"
BIN_PATH="${V8TR_BIN:-$ROOT_DIR/target/debug/v8-runner}"

if [[ -z "$IBCMD_CONFIG_PATH" ]]; then
    echo "SKIPPED: V8TR_IBCMD_REAL_CONFIG is not set."
    echo "Set a dedicated live config with format=DESIGNER, builder=IBCMD and a file-based infobase."
    exit 0
fi

if [[ ! -f "$IBCMD_CONFIG_PATH" ]]; then
    echo "Live IBCMD config not found: $IBCMD_CONFIG_PATH" >&2
    exit 2
fi

if ! rg -q "^builder:\s*IBCMD\s*$" "$IBCMD_CONFIG_PATH"; then
    echo "Live IBCMD config must contain 'builder: IBCMD': $IBCMD_CONFIG_PATH" >&2
    exit 2
fi

if ! rg -q "^format:\s*DESIGNER\s*$" "$IBCMD_CONFIG_PATH"; then
    echo "Live IBCMD config must contain 'format: DESIGNER': $IBCMD_CONFIG_PATH" >&2
    exit 2
fi

if ! rg -q "^connection:\s*['\"]?(File=|/F[[:space:]]+)" "$IBCMD_CONFIG_PATH"; then
    echo "Live IBCMD config must use a file-based connection ('File=...' or raw '/F ...'): $IBCMD_CONFIG_PATH" >&2
    exit 2
fi

if [[ ! -x "$BIN_PATH" ]]; then
    echo "Building v8-runner binary..." >&2
    (cd "$ROOT_DIR" && cargo build --locked --bin v8-runner >/dev/null)
fi

run_cli() {
    echo
    echo "==> $*"
    "$BIN_PATH" --config "$IBCMD_CONFIG_PATH" "$@"
}

run_cli init
run_cli build
run_cli dump --mode full
run_cli dump --mode incremental
run_cli dump --mode partial --object Catalog.Items
run_cli extensions

echo
echo "Live CLI IBCMD smoke completed successfully."
