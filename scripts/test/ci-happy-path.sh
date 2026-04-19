#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TARGET_OS_LABEL="${V8TR_CI_TARGET_OS:-$(uname -s | tr '[:upper:]' '[:lower:]')}"
BIN_PATH_UNIX="$ROOT_DIR/target/debug/v8-runner"
BIN_PATH_WINDOWS="$ROOT_DIR/target/debug/v8-runner.exe"

stage() {
    echo
    echo "============================================================"
    echo "CI HAPPY PATH [$TARGET_OS_LABEL]: $1"
    echo "============================================================"
}

detect_bin_path() {
    if [[ -x "$BIN_PATH_UNIX" ]]; then
        printf '%s\n' "$BIN_PATH_UNIX"
        return 0
    fi

    if [[ -x "$BIN_PATH_WINDOWS" ]]; then
        printf '%s\n' "$BIN_PATH_WINDOWS"
        return 0
    fi

    return 1
}

cd "$ROOT_DIR"

stage "build cargo binary"
cargo build --locked --bin v8-runner

stage "syntax/check cargo workspace"
cargo check --locked --all-targets

stage "test cargo workspace"
cargo test --locked

export V8TR_BIN="${V8TR_BIN:-$(detect_bin_path)}"
export V8TR_DESIGNER_SMOKE_PROFILE="${V8TR_DESIGNER_SMOKE_PROFILE:-mandatory}"

stage "package deploy-ready 1C artifacts"
# The deploy-ready step is still a bash helper contract; the Windows job must
# run this entrypoint from bash with python3 available in PATH.
# Mandatory smoke requires V8TR_DESIGNER_REAL_CONFIG; set
# V8TR_DESIGNER_ALLOW_MISSING_CONFIG=1 only for non-blocking soft-skip runs.
bash "$ROOT_DIR/scripts/test/live-cli-designer.sh"
