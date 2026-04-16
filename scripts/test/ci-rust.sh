#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

cd "$ROOT_DIR"

if [[ "${V8_TEST_RUNNER_CI_SCOPE:-full}" == "runtime-locks" ]]; then
  cargo test --locked workspace_lock
  cargo test --locked advisory_lock
  cargo test --locked execute_command_reports_workspace_lock_conflict
  cargo test --locked default_port_reports_workspace_lock_conflict_before_use_case_dispatch
else
  cargo test --locked
fi
