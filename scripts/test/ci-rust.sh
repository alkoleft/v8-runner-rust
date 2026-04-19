#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CI_SCOPE="${V8_RUNNER_CI_SCOPE:-contract}"

cd "$ROOT_DIR"

case "$CI_SCOPE" in
  contract|full)
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
    echo "Expected one of: contract, runtime-locks, happy-path" >&2
    exit 2
    ;;
esac
