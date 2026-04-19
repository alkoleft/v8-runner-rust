#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

CONFIG_PATH="${V8TR_REAL_CONFIG:-}"
SMOKE_MODULE="${V8TR_SMOKE_MODULE:-ЮТДымовыеТесты}"
ENABLE_LAUNCH="${V8TR_ENABLE_LAUNCH:-0}"
BIN_PATH="${V8TR_BIN:-$ROOT_DIR/target/debug/v8-runner}"

if [[ -z "$CONFIG_PATH" ]]; then
    echo "V8TR_REAL_CONFIG is not set." >&2
    echo "Example:" >&2
    echo "  export V8TR_REAL_CONFIG=/home/alko/develop/open-source/ai/mcp/onec-client-mcp-devkit/.agents/tools/onec-client-mcp-devkit.edt.yaml" >&2
    exit 2
fi

if [[ ! -f "$CONFIG_PATH" ]]; then
    echo "Live config not found: $CONFIG_PATH" >&2
    exit 2
fi

if [[ ! -x "$BIN_PATH" ]]; then
    echo "Building v8-runner binary..." >&2
    (cd "$ROOT_DIR" && cargo build --locked --bin v8-runner >/dev/null)
fi

run_cli() {
    echo
    echo "==> $*"
    "$BIN_PATH" --config "$CONFIG_PATH" "$@"
}

LAUNCH_PID=""
cleanup() {
    if [[ -n "$LAUNCH_PID" ]]; then
        kill "$LAUNCH_PID" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

run_cli build
run_cli syntax edt
run_cli test module "$SMOKE_MODULE"

if [[ "$ENABLE_LAUNCH" == "1" ]]; then
    echo
    echo "==> launch --mode thin"
    launch_json="$("$BIN_PATH" --config "$CONFIG_PATH" --output json launch --mode thin)"
    echo "$launch_json"
    LAUNCH_PID="$(
        python3 -c 'import json, sys; print(json.load(sys.stdin)["data"]["pid"])' <<<"$launch_json"
    )"
    sleep 1
    kill "$LAUNCH_PID" >/dev/null 2>&1 || true
fi

echo
echo "Live CLI smoke completed successfully."
