#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BASE_CONFIG="$ROOT_DIR/scripts/test/live-cli-designer.fixture.yaml"
OUTPUT_ROOT="${V8TR_LIVE_CLI_OUTPUT_ROOT:-$ROOT_DIR/target/manual-tests/live-cli-designer}"
RUNTIME_ROOT="${V8TR_CI_RUNTIME_ROOT:-$ROOT_DIR/target/manual-tests/live-cli-designer-runtime}"
CONFIG_PATH="${V8TR_DESIGNER_REAL_CONFIG:-$RUNTIME_ROOT/live-cli-designer.ci.yaml}"
INFOBASE_PATH="${V8TR_INFOBASE_PATH:-$RUNTIME_ROOT/infobase}"
PLATFORM_PATH="${V8TR_PLATFORM_PATH:-}"

die() {
    echo "$*" >&2
    exit 2
}

write_env() {
    local key="$1"
    local value="$2"
    local delimiter="__V8TR_ENV__"

    if [[ -n "${GITHUB_ENV:-}" ]]; then
        {
            printf '%s<<%s\n' "$key" "$delimiter"
            printf '%s\n' "$value"
            printf '%s\n' "$delimiter"
        } >> "$GITHUB_ENV"
    else
        printf '%s=%s\n' "$key" "$value"
    fi
}

require_command() {
    local command_name="$1"
    command -v "$command_name" >/dev/null 2>&1 || die "Required command is missing: $command_name"
}

main() {
    [[ -f "$BASE_CONFIG" ]] || die "Base designer fixture config not found: $BASE_CONFIG"
    [[ -n "$PLATFORM_PATH" ]] || die "V8TR_PLATFORM_PATH must be set before config materialization"

    require_command python3
    mkdir -p "$OUTPUT_ROOT" "$RUNTIME_ROOT" "$(dirname "$CONFIG_PATH")" "$(dirname "$INFOBASE_PATH")"

    python3 - "$BASE_CONFIG" "$CONFIG_PATH" "$ROOT_DIR" "$OUTPUT_ROOT" "$INFOBASE_PATH" "$PLATFORM_PATH" <<'PY'
import pathlib
import re
import sys

source = pathlib.Path(sys.argv[1])
target = pathlib.Path(sys.argv[2])
root_dir = pathlib.Path(sys.argv[3])
output_root = pathlib.Path(sys.argv[4])
infobase_path = pathlib.Path(sys.argv[5])
platform_path = pathlib.Path(sys.argv[6])

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

text, connection_count = re.subn(
    r'(^\s*connection:\s*).*$',
    lambda match: f'{match.group(1)}"File={infobase_path.as_posix()}"',
    text,
    count=1,
    flags=re.MULTILINE,
)
if connection_count != 1:
    raise SystemExit("failed to replace infobase.connection in CI designer config")

def inject_platform_path(match: re.Match[str]) -> str:
    body = match.group("body")
    replacement_line = f'    path: "{platform_path.as_posix()}"\n'
    if re.search(r"^    path:\s*.*$", body, re.MULTILINE):
        body = re.sub(
            r"^    path:\s*.*$",
            replacement_line.rstrip("\n"),
            body,
            count=1,
            flags=re.MULTILINE,
        )
    else:
        body = replacement_line + body
    return match.group("prefix") + body


text, platform_count = re.subn(
    r"(?m)(?P<prefix>^  platform:\n)(?P<body>(?:    .*(?:\n|\Z))+)",
    inject_platform_path,
    text,
    count=1,
)
if platform_count != 1:
    raise SystemExit("failed to inject tools.platform.path into CI designer config")

target.write_text(text, encoding="utf-8")
PY

    grep -Eq '^format:[[:space:]]*DESIGNER[[:space:]]*$' "$CONFIG_PATH" || die "Generated config must keep format: DESIGNER"
    grep -Eq '^builder:[[:space:]]*DESIGNER[[:space:]]*$' "$CONFIG_PATH" || die "Generated config must keep builder: DESIGNER"
    grep -Eq "^[[:space:]]*path:[[:space:]]*[\"']?.+[\"']?$" "$CONFIG_PATH" || die "Generated config must contain tools.platform.path"
    grep -Eq "^[[:space:]]*connection:[[:space:]]*[\"']?File=" "$CONFIG_PATH" || die "Generated config must contain file infobase.connection"

    write_env V8TR_DESIGNER_REAL_CONFIG "$CONFIG_PATH"
    write_env V8TR_LIVE_CLI_OUTPUT_ROOT "$OUTPUT_ROOT"
    write_env V8TR_CI_RUNTIME_ROOT "$RUNTIME_ROOT"
    write_env V8TR_INFOBASE_PATH "$INFOBASE_PATH"

    echo "Materialized Designer live config: $CONFIG_PATH"
    echo "Infobase path aligned for CI bootstrap: $INFOBASE_PATH"
}

main "$@"
