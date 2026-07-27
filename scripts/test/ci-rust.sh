#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CI_SCOPE="${V8_RUNNER_CI_SCOPE:-contract}"
TARGET_OS_LABEL="${V8TR_CI_TARGET_OS:-$(uname -s)}"

cd "$ROOT_DIR"

case "$CI_SCOPE" in
  contract)
    case "$TARGET_OS_LABEL" in
      Windows|MINGW*|MSYS*|CYGWIN*)
        echo "Windows contract scope runs compile/check smoke plus targeted platform-specific tests; full cargo test remains Linux-owned until the Windows test suite is hardened."
        cargo check --locked --all-targets
        cargo test --locked --bin v8-runner materialize_vanessa_runner_log
        cargo test --locked --bin v8-runner windows_atomic_replace_supports_extended_length_paths
        ;;
      *)
        cargo test --locked
        ;;
    esac
    ;;
  full)
    cargo test --locked
    ;;
  runtime-locks)
    cargo test --locked workspace_lock
    cargo test --locked advisory_lock
    cargo test --locked execute_command_reports_workspace_lock_conflict
    cargo test --locked default_port_reports_workspace_lock_conflict_before_use_case_dispatch
    ;;
  happy-path)
    bash "$ROOT_DIR/scripts/test/ci-happy-path.sh"
    ;;
  *)
    echo "Unsupported V8_RUNNER_CI_SCOPE: $CI_SCOPE" >&2
    echo "Expected one of: contract, full, runtime-locks, happy-path" >&2
    exit 2
    ;;
esac
