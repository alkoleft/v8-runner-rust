#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BASE_CONFIG="$ROOT_DIR/scripts/test/live-cli-designer.fixture.yaml"
OUTPUT_ROOT="${V8TR_LIVE_CLI_OUTPUT_ROOT:-$ROOT_DIR/target/manual-tests/live-cli-designer}"
WORK_CONFIG_PATH="${V8TR_DESIGNER_WORK_CONFIG:-$ROOT_DIR/target/manual-tests/live-cli-designer.generated.yaml}"

die() {
    echo "$*" >&2
    exit 2
}

materialize_designer_config() {
    python3 - "$BASE_CONFIG" "$WORK_CONFIG_PATH" "$ROOT_DIR" "$OUTPUT_ROOT" <<'PY'
import pathlib
import sys

source = pathlib.Path(sys.argv[1])
target = pathlib.Path(sys.argv[2])
root_dir = pathlib.Path(sys.argv[3])
output_root = pathlib.Path(sys.argv[4])

text = source.read_text(encoding="utf-8")
replacements = {
    "__ROOT_DIR__": root_dir.as_posix(),
    "__OUTPUT_ROOT__": output_root.as_posix(),
    "__VANESSA_EPF__": (root_dir / "tests/fixtures/vanessa-automation-single.epf").as_posix(),
    "__VANESSA_PARAMS_TEMPLATE__": (root_dir / "scripts/test/live-cli-designer.va-params.json").as_posix(),
    "__VANESSA_FEATURE_PATH__": (root_dir / "scripts/test/features/live-cli-designer").as_posix(),
}

for old, new in replacements.items():
    text = text.replace(old, new)

target.write_text(text, encoding="utf-8")
PY
}

CONFIG_PATH="${V8TR_DESIGNER_REAL_CONFIG:-}"

if [[ -z "$CONFIG_PATH" ]]; then
    [[ -f "$BASE_CONFIG" ]] || die "Base designer fixture config not found: $BASE_CONFIG"

    if ! command -v python3 >/dev/null 2>&1; then
        die "python3 is required for designer live config materialization"
    fi

    mkdir -p "$OUTPUT_ROOT" "$(dirname "$WORK_CONFIG_PATH")"
    materialize_designer_config
    CONFIG_PATH="$WORK_CONFIG_PATH"
fi

[[ -f "$CONFIG_PATH" ]] || die "Live designer config not found: $CONFIG_PATH"

export V8TR_DESIGNER_REAL_CONFIG="$CONFIG_PATH"
export V8TR_LIVE_CLI_OUTPUT_ROOT="$OUTPUT_ROOT"

bash "$ROOT_DIR/scripts/test/live-cli-fixture.sh"
