#!/bin/sh

set -eu

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
BASE_CONFIG="$ROOT_DIR/scripts/test/live-cli-designer.fixture.yaml"
FIXTURE_BASE="$ROOT_DIR/tests/fixtures/designer"
LIVE_SCRIPT="$ROOT_DIR/scripts/test/live-cli-ibcmd.sh"
OUTPUT_ROOT="$ROOT_DIR/target/manual-tests/live-cli-ibcmd"
WORK_CONFIG_PATH="$ROOT_DIR/target/manual-tests/live-cli-ibcmd.yaml"
BIN_PATH="${V8TR_BIN:-$ROOT_DIR/target/debug/v8-runner}"

die() {
    echo "$*" >&2
    exit 2
}

stage() {
    echo
    echo "==> UAT IBCMD: $1"
}

materialize_ibcmd_config() {
    python3 - "$BASE_CONFIG" "$WORK_CONFIG_PATH" "$ROOT_DIR" "$OUTPUT_ROOT" <<'PY'
import pathlib
import re
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

text = re.sub(
    r"^builder:\s*DESIGNER\s*$",
    "builder: IBCMD",
    text,
    count=1,
    flags=re.MULTILINE,
)
text = re.sub(
    r"\n  - name: external-processor\n    type: EXTERNAL_DATA_PROCESSORS\n    path: external/processor",
    "",
    text,
)
text = re.sub(
    r"\n  - name: external-report\n    type: EXTERNAL_REPORTS\n    path: external/report",
    "",
    text,
)
target.write_text(text, encoding="utf-8")
PY
}

[ -f "$BASE_CONFIG" ] || die "Base fixture config not found: $BASE_CONFIG"
[ -d "$FIXTURE_BASE" ] || die "Fixture source directory not found: $FIXTURE_BASE"

if ! command -v python3 >/dev/null 2>&1; then
    die "python3 is required for fixture config materialization"
fi

stage "prepare IBCMD fixture config"
rm -rf "$OUTPUT_ROOT"
mkdir -p "$OUTPUT_ROOT"
materialize_ibcmd_config

stage "build cargo binary"
(cd "$ROOT_DIR" && cargo build --locked --bin v8-runner)

stage "run real live-cli-ibcmd scenario"
V8TR_BIN="$BIN_PATH" V8TR_LIVE_CLI_OUTPUT_ROOT="$OUTPUT_ROOT" V8TR_IBCMD_REAL_CONFIG="$WORK_CONFIG_PATH" bash "$LIVE_SCRIPT"

echo
echo "UAT CLI IBCMD live scenario completed successfully."
