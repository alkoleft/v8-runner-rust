#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
LIVE_SCRIPT="$ROOT_DIR/scripts/test/live-cli-ibcmd.sh"
OUTPUT_ROOT="$ROOT_DIR/target/manual-tests/live-cli-ibcmd"
GENERATED_CONFIG_PATH="$ROOT_DIR/target/manual-tests/live-cli-ibcmd.generated.yaml"
BIN_PATH="${V8TR_BIN:-$ROOT_DIR/target/debug/v8-runner}"

stage() {
    echo
    echo "==> UAT IBCMD: $1"
}

stage "build cargo binary"
(cd "$ROOT_DIR" && cargo build --locked --bin v8-runner)

stage "clean previous live IBCMD artifacts"
rm -rf "$OUTPUT_ROOT" "$GENERATED_CONFIG_PATH"

stage "run real live-cli-ibcmd scenario"
V8TR_BIN="$BIN_PATH" V8TR_LIVE_CLI_OUTPUT_ROOT="$OUTPUT_ROOT" bash "$LIVE_SCRIPT"

echo
echo "UAT CLI IBCMD live scenario completed successfully."
