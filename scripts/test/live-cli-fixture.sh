#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

DESIGNER_CONFIG_PATH="${V8TR_DESIGNER_REAL_CONFIG:-}"
ALLOW_MISSING_CONFIG="${V8TR_DESIGNER_ALLOW_MISSING_CONFIG:-0}"
DESIGNER_SMOKE_PROFILE="${V8TR_DESIGNER_SMOKE_PROFILE:-mandatory}"
DESIGNER_TEST_MODE="${V8TR_DESIGNER_TEST_MODE:-none}"
DESIGNER_TEST_MODULE="${V8TR_DESIGNER_TEST_MODULE:-}"
DESIGNER_LAUNCH_SMOKE="${V8TR_DESIGNER_LAUNCH_SMOKE:-0}"
BIN_PATH="${V8TR_BIN:-$ROOT_DIR/target/debug/v8-runner}"
OUTPUT_ROOT="${V8TR_LIVE_CLI_OUTPUT_ROOT:-$ROOT_DIR/target/manual-tests/live-cli-designer}"
FIXTURE_BASE_PATH="$ROOT_DIR/tests/fixtures/designer"
VANESSA_EPF_PATH="${V8TR_VA_EPF:-$ROOT_DIR/tests/fixtures/vanessa-automation-single.epf}"
VANESSA_PARAMS_TEMPLATE_PATH="${V8TR_VA_PARAMS_TEMPLATE:-$ROOT_DIR/scripts/test/live-cli-designer.va-params.json}"
VANESSA_FEATURE_PATH="${V8TR_VA_FEATURE_PATH:-$ROOT_DIR/scripts/test/features/live-cli-designer}"
SMOKE_TITLE="LIVE CLI SMOKE"

die() {
    echo "$*" >&2
    exit 2
}

print_stage() {
    local title="$1"
    echo
    echo "============================================================"
    echo "$SMOKE_TITLE: $title"
    echo "============================================================"
}

assert_file_exists() {
    local path="$1"
    if [[ ! -f "$path" ]]; then
        die "Expected file was not produced: $path"
    fi
}

assert_file_nonempty() {
    local path="$1"
    if [[ ! -s "$path" ]]; then
        die "Expected non-empty file was not produced: $path"
    fi
}

assert_dir_exists() {
    local path="$1"
    if [[ ! -d "$path" ]]; then
        die "Expected directory was not produced: $path"
    fi
}

snapshot_dir() {
    local source_dir="$1"
    local target_dir="$2"
    rm -rf "$target_dir"
    mkdir -p "$target_dir"
    cp -R "$source_dir/." "$target_dir/"
}

trim_yaml_scalar() {
    sed -e "s/^[[:space:]]*//" -e "s/[[:space:]]*$//" -e "s/^['\"]//" -e "s/['\"]$//"
}

strip_shell_quotes() {
    local value="$1"
    value="${value#"${value%%[![:space:]]*}"}"
    value="${value%"${value##*[![:space:]]}"}"
    value="${value#\'}"
    value="${value%\'}"
    value="${value#\"}"
    value="${value%\"}"
    printf '%s\n' "$value"
}

extract_yaml_scalar() {
    local key="$1"
    awk -v key="$key" '
        $0 ~ "^[[:space:]]*" key ":[[:space:]]*" {
            sub("^[[:space:]]*" key ":[[:space:]]*", "", $0)
            print
            exit
        }
    ' "$DESIGNER_CONFIG_PATH" | trim_yaml_scalar
}

config_matches() {
    local pattern="$1"
    local path="$2"

    if command -v rg >/dev/null 2>&1; then
        rg -q "$pattern" "$path"
        return $?
    fi

    grep -Eq "$pattern" "$path"
}

extract_source_sets() {
    python3 - "$DESIGNER_CONFIG_PATH" <<'PY'
import pathlib
import re
import sys


def clean(value: str) -> str:
    return value.strip().strip("'\"")


config_path = pathlib.Path(sys.argv[1])
lines = config_path.read_text(encoding="utf-8").splitlines()
items = []
current = None
in_block = False

for line in lines:
    if not in_block:
        if re.match(r"^\s*source-set:\s*$", line):
            in_block = True
        continue

    if re.match(r"^\S", line):
        break

    name_match = re.match(r"^\s*-\s*name:\s*(.+?)\s*$", line)
    if name_match:
        if current is not None:
            items.append(current)
        current = {"name": clean(name_match.group(1))}
        continue

    field_match = re.match(r"^\s+(type|path):\s*(.+?)\s*$", line)
    if field_match and current is not None:
        current[field_match.group(1)] = clean(field_match.group(2))

if current is not None:
    items.append(current)

for item in items:
    print(
        "\t".join(
            [
                item.get("name", ""),
                item.get("type", ""),
                item.get("path", ""),
            ]
        )
    )
PY
}

extract_artifact_root_name() {
    local relative_path="$1"
    python3 - "$WORK_BASE_PATH" "$relative_path" <<'PY'
import pathlib
import sys

base_path = pathlib.Path(sys.argv[1])
relative_path = pathlib.Path(sys.argv[2])
root = base_path / relative_path

if not root.is_dir():
    raise SystemExit(f"source-set path is not a directory: {root}")

names = sorted(
    path.stem
    for path in root.glob("*.xml")
    if path.is_file() and path.name not in {"Configuration.xml", "ConfigDumpInfo.xml"}
)

if len(names) != 1:
    raise SystemExit(
        f"expected exactly one root xml artifact in {root}, found {len(names)}"
    )

print(names[0])
PY
}

assert_json_step_ok() {
    local json_path="$1"
    local source_set="$2"
    python3 - "$json_path" "$source_set" <<'PY'
import json
import sys

json_path, source_set = sys.argv[1], sys.argv[2]
with open(json_path, "r", encoding="utf-8") as fh:
    payload = json.load(fh)

steps = payload.get("data", {}).get("steps", [])
for step in steps:
    if step.get("source_set") == source_set:
        if step.get("ok") is True:
            raise SystemExit(0)
        raise SystemExit(f"build step for '{source_set}' is not successful: {step}")

raise SystemExit(f"build output does not contain step for '{source_set}'")
PY
}

assert_json_command_ok() {
    local json_path="$1"
    local expected_command="$2"

    python3 - "$json_path" "$expected_command" <<'PY'
import json
import sys

json_path, expected_command = sys.argv[1], sys.argv[2]
with open(json_path, "r", encoding="utf-8") as fh:
    payload = json.load(fh)

if payload.get("ok") is not True:
    raise SystemExit(f"{expected_command} command failed: {payload}")

if payload.get("command") != expected_command:
    raise SystemExit(
        f"unexpected command in output: {payload.get('command')}, expected {expected_command}"
    )
PY
}

extract_connection_file_path() {
    python3 - "$DESIGNER_CONFIG_PATH" <<'PY'
import pathlib
import re
import shlex
import sys

config_path = pathlib.Path(sys.argv[1])
text = config_path.read_text(encoding="utf-8")

connection = None
in_infobase = False
for line in text.splitlines():
    if re.match(r"^infobase:\s*$", line):
        in_infobase = True
        continue
    if in_infobase and re.match(r"^\S", line):
        break
    if in_infobase:
        match = re.match(r"^[ \t]+connection:\s*(.+)$", line)
        if match:
            connection = match.group(1).strip().strip("'\"")
            break

if not connection:
    raise SystemExit("infobase.connection must use File=... or raw /F ...")

if connection.startswith("/") or connection.startswith("-"):
    parts = shlex.split(connection)
    for index, part in enumerate(parts):
        if part.lower() in ("/f", "-f") and index + 1 < len(parts):
            print(pathlib.Path(parts[index + 1]).expanduser())
            raise SystemExit(0)
    raise SystemExit("infobase.connection must use File=... or raw /F ...")

for part in connection.split(";"):
    part = part.strip()
    if part.lower().startswith("file="):
        print(pathlib.Path(part[5:]).expanduser())
        raise SystemExit(0)

raise SystemExit("infobase.connection must use File=... or raw /F ...")
PY
}

assert_test_json_ok() {
    local json_path="$1"
    local expected_target="$2"
    local expected_min_total="${3:-1}"

    python3 - "$json_path" "$expected_target" "$expected_min_total" <<'PY'
import json
import sys

json_path = sys.argv[1]
expected_target = sys.argv[2]
expected_min_total = int(sys.argv[3])

with open(json_path, "r", encoding="utf-8") as fh:
    payload = json.load(fh)

if payload.get("ok") is not True:
    raise SystemExit(f"test command failed: {payload}")

if payload.get("command") != "test":
    raise SystemExit(f"unexpected command in test output: {payload.get('command')}")

data = payload.get("data", {})
if data.get("ok") is not True:
    raise SystemExit(f"test payload does not confirm success: {payload}")

report = data.get("report")
if not isinstance(report, dict):
    raise SystemExit(f"test payload does not contain report: {payload}")

summary = report.get("summary")
if not isinstance(summary, dict):
    raise SystemExit(f"test report does not contain summary: {payload}")

total = summary.get("total")
if not isinstance(total, int) or total < expected_min_total:
    raise SystemExit(
        f"test summary total must be >= {expected_min_total}, got {total}: {payload}"
    )

target = data.get("target")
if expected_target == "all":
    if target != "all":
        raise SystemExit(f"test target must be 'all', got {target}: {payload}")
elif expected_target.startswith("module:"):
    expected_name = expected_target.split(":", 1)[1]
    actual_name = None
    if isinstance(target, dict):
        module = target.get("module")
        if isinstance(module, dict):
            actual_name = module.get("name")
    if actual_name != expected_name:
        raise SystemExit(
            f"test target module must be '{expected_name}', got {actual_name}: {payload}"
        )
else:
    raise SystemExit(f"unsupported expected target contract: {expected_target}")
PY
}

materialize_live_config() {
    local source_config="$1"
    local target_config="$2"
    local output_root="$3"
    local work_base_path="$4"

    python3 - "$source_config" "$target_config" "$ROOT_DIR" "$output_root" "$work_base_path" "$VANESSA_EPF_PATH" "$VANESSA_PARAMS_TEMPLATE_PATH" "$VANESSA_FEATURE_PATH" <<'PY'
import pathlib
import re
import sys

source = pathlib.Path(sys.argv[1])
target = pathlib.Path(sys.argv[2])
root_dir = pathlib.Path(sys.argv[3])
output_root = pathlib.Path(sys.argv[4])
work_base_path = pathlib.Path(sys.argv[5])
vanessa_epf = pathlib.Path(sys.argv[6])
vanessa_params_template = pathlib.Path(sys.argv[7])
vanessa_feature_path = pathlib.Path(sys.argv[8])
text = source.read_text(encoding="utf-8")

replacements = {
    "__ROOT_DIR__": root_dir.as_posix(),
    "__OUTPUT_ROOT__": output_root.as_posix(),
    "__VANESSA_EPF__": vanessa_epf.as_posix(),
    "__VANESSA_PARAMS_TEMPLATE__": vanessa_params_template.as_posix(),
    "__VANESSA_FEATURE_PATH__": vanessa_feature_path.as_posix(),
}

for needle, replacement in replacements.items():
    text = text.replace(needle, replacement)

target.write_text(text, encoding="utf-8")
PY
}

run_cli() {
    echo
    echo "==> $*"
    "$BIN_PATH" --config "$DESIGNER_CONFIG_PATH" "$@"
}

run_cli_json_to_file() {
    local json_path="$1"
    shift
    echo
    echo "==> --json-message $*"
    "$BIN_PATH" --config "$DESIGNER_CONFIG_PATH" --json-message "$@" | tee "$json_path"
}

run_test_stage() {
    local json_path="$OUTPUT_ROOT/json/test-stage.json"

    case "$DESIGNER_TEST_MODE" in
        none)
            echo "SKIPPED: live test runner is disabled. Set V8TR_DESIGNER_TEST_MODE=va|yaxunit-all|module to run a real 1C test stage."
            ;;
        va)
            [[ -f "$VANESSA_EPF_PATH" ]] || die "Vanessa Automation EPF not found: $VANESSA_EPF_PATH"
            [[ -f "$VANESSA_PARAMS_TEMPLATE_PATH" ]] || die "Vanessa params template not found: $VANESSA_PARAMS_TEMPLATE_PATH"
            [[ -d "$VANESSA_FEATURE_PATH" ]] || die "Vanessa feature path not found: $VANESSA_FEATURE_PATH"
            run_cli_json_to_file "$json_path" test va
            assert_test_json_ok "$json_path" "all" 1
            ;;
        yaxunit-all)
            run_cli_json_to_file "$json_path" test yaxunit all
            assert_test_json_ok "$json_path" "all" 1
            ;;
        module|yaxunit-module)
            [[ -n "$DESIGNER_TEST_MODULE" ]] || die "V8TR_DESIGNER_TEST_MODULE must be set when V8TR_DESIGNER_TEST_MODE=module"
            run_cli_json_to_file "$json_path" test yaxunit module "$DESIGNER_TEST_MODULE"
            assert_test_json_ok "$json_path" "module:$DESIGNER_TEST_MODULE" 1
            ;;
        *)
            die "Unsupported V8TR_DESIGNER_TEST_MODE: $DESIGNER_TEST_MODE"
            ;;
    esac
}

run_launch_smoke() {
    if [[ "$DESIGNER_LAUNCH_SMOKE" != "1" ]]; then
        echo "SKIPPED: launch smoke is disabled. Set V8TR_DESIGNER_LAUNCH_SMOKE=1 to spawn 1C clients."
        return 0
    fi

    local launch_json="$OUTPUT_ROOT/json/launch-designer.json"
    run_cli_json_to_file "$launch_json" launch --mode designer --output "$OUTPUT_ROOT/launch/designer.log"
    assert_json_command_ok "$launch_json" "launch"
}

run_extended_steps() {
    local dump_root="$OUTPUT_ROOT/dump"

    print_stage "extended dump validation"
    rm -f \
        "$WORK_BASE_PATH/$CONFIGURATION_SOURCE_SET_PATH/Configuration.xml" \
        "$WORK_BASE_PATH/$CONFIGURATION_SOURCE_SET_PATH/ConfigDumpInfo.xml"
    run_cli dump --mode full --source-set "$CONFIGURATION_SOURCE_SET_NAME"
    assert_file_exists "$WORK_BASE_PATH/$CONFIGURATION_SOURCE_SET_PATH/Configuration.xml"
    assert_file_exists "$WORK_BASE_PATH/$CONFIGURATION_SOURCE_SET_PATH/ConfigDumpInfo.xml"
    snapshot_dir "$WORK_BASE_PATH/$CONFIGURATION_SOURCE_SET_PATH" "$dump_root/full"

    rm -f "$WORK_BASE_PATH/$CONFIGURATION_SOURCE_SET_PATH/ConfigDumpInfo.xml"
    run_cli dump --mode incremental --source-set "$CONFIGURATION_SOURCE_SET_NAME"
    assert_dir_exists "$WORK_BASE_PATH/$CONFIGURATION_SOURCE_SET_PATH"
    assert_file_exists "$WORK_BASE_PATH/$CONFIGURATION_SOURCE_SET_PATH/ConfigDumpInfo.xml"
    snapshot_dir "$WORK_BASE_PATH/$CONFIGURATION_SOURCE_SET_PATH" "$dump_root/incremental"

    rm -f "$WORK_BASE_PATH/$CONFIGURATION_SOURCE_SET_PATH/Catalogs/Справочник1.xml"
    run_cli dump --mode partial --source-set "$CONFIGURATION_SOURCE_SET_NAME" --object Catalog.Справочник1
    assert_file_exists "$WORK_BASE_PATH/$CONFIGURATION_SOURCE_SET_PATH/Catalogs/Справочник1.xml"
    snapshot_dir "$WORK_BASE_PATH/$CONFIGURATION_SOURCE_SET_PATH" "$dump_root/partial"

    rm -f "$WORK_BASE_PATH/$EXTENSION_SOURCE_SET_PATH/ConfigDumpInfo.xml"
    run_cli dump --mode incremental --source-set "$EXTENSION_SOURCE_SET_NAME" --extension "$EXTENSION_SOURCE_SET_NAME"
    assert_file_exists "$WORK_BASE_PATH/$EXTENSION_SOURCE_SET_PATH/ConfigDumpInfo.xml"
    snapshot_dir "$WORK_BASE_PATH/$EXTENSION_SOURCE_SET_PATH" "$dump_root/extension-incremental"
}

if [[ -z "$DESIGNER_CONFIG_PATH" ]]; then
    if [[ "$ALLOW_MISSING_CONFIG" == "1" ]]; then
        echo "SKIPPED: V8TR_DESIGNER_REAL_CONFIG is not set."
        echo "Set V8TR_DESIGNER_REAL_CONFIG to a dedicated format=DESIGNER,builder=DESIGNER fixture config."
        echo "Default va-path also requires tests/fixtures/vanessa-automation-single.epf, scripts/test/live-cli-designer.va-params.json, and scripts/test/features/live-cli-designer."
        exit 0
    fi
    die "V8TR_DESIGNER_REAL_CONFIG is required for mandatory designer smoke."
fi

if [[ ! -f "$DESIGNER_CONFIG_PATH" ]]; then
    die "Live Designer config not found: $DESIGNER_CONFIG_PATH"
fi

if ! command -v python3 >/dev/null 2>&1; then
    die "python3 is required for the live-cli-fixture helper contract"
fi

case "$DESIGNER_SMOKE_PROFILE" in
    mandatory|extended)
        ;;
    *)
        die "Unsupported V8TR_DESIGNER_SMOKE_PROFILE: $DESIGNER_SMOKE_PROFILE"
        ;;
esac

if ! config_matches "^format:[[:space:]]*DESIGNER[[:space:]]*$" "$DESIGNER_CONFIG_PATH"; then
    die "Live Designer config must contain 'format: DESIGNER': $DESIGNER_CONFIG_PATH"
fi

BUILDER_BACKEND="$(extract_yaml_scalar "builder")"
case "$BUILDER_BACKEND" in
    DESIGNER|IBCMD)
        ;;
    *)
        die "Live config must contain 'builder: DESIGNER' or 'builder: IBCMD': $DESIGNER_CONFIG_PATH"
        ;;
esac
SMOKE_TITLE="LIVE CLI $BUILDER_BACKEND SMOKE"

if ! extract_connection_file_path >/dev/null; then
    die "Live Designer config must use file-based infobase.connection ('File=...' or raw '/F ...'): $DESIGNER_CONFIG_PATH"
fi

declare -A SOURCE_SET_NAME_BY_TYPE=()
declare -A SOURCE_SET_PATH_BY_TYPE=()
required_types=(
    CONFIGURATION
    EXTENSION
)
if [[ "$BUILDER_BACKEND" == "DESIGNER" ]]; then
    required_types+=(
        EXTERNAL_DATA_PROCESSORS
        EXTERNAL_REPORTS
    )
fi

while IFS=$'\t' read -r source_set_name source_set_type source_set_path; do
    source_set_name="$(strip_shell_quotes "$source_set_name")"
    source_set_type="$(strip_shell_quotes "$source_set_type")"
    source_set_path="$(strip_shell_quotes "$source_set_path")"

    if [[ -z "$source_set_name" || -z "$source_set_type" || -z "$source_set_path" ]]; then
        die "Each source-set must define name, type, and path: $DESIGNER_CONFIG_PATH"
    fi

    if [[ -n "${SOURCE_SET_NAME_BY_TYPE[$source_set_type]:-}" ]]; then
        die "Live Designer config must define only one source-set with type '$source_set_type': $DESIGNER_CONFIG_PATH"
    fi

    SOURCE_SET_NAME_BY_TYPE["$source_set_type"]="$source_set_name"
    SOURCE_SET_PATH_BY_TYPE["$source_set_type"]="$source_set_path"
done < <(extract_source_sets)

for source_set_type in "${required_types[@]}"; do
    if [[ -z "${SOURCE_SET_NAME_BY_TYPE[$source_set_type]:-}" ]]; then
        die "Live Designer config must declare a source-set with type '$source_set_type': $DESIGNER_CONFIG_PATH"
    fi
done

CONFIGURATION_SOURCE_SET_NAME="${SOURCE_SET_NAME_BY_TYPE[CONFIGURATION]}"
CONFIGURATION_SOURCE_SET_PATH="${SOURCE_SET_PATH_BY_TYPE[CONFIGURATION]}"
EXTENSION_SOURCE_SET_NAME="${SOURCE_SET_NAME_BY_TYPE[EXTENSION]}"
EXTENSION_SOURCE_SET_PATH="${SOURCE_SET_PATH_BY_TYPE[EXTENSION]}"
EXTERNAL_PROCESSOR_SOURCE_SET_NAME="${SOURCE_SET_NAME_BY_TYPE[EXTERNAL_DATA_PROCESSORS]:-}"
EXTERNAL_PROCESSOR_SOURCE_SET_PATH="${SOURCE_SET_PATH_BY_TYPE[EXTERNAL_DATA_PROCESSORS]:-}"
EXTERNAL_REPORT_SOURCE_SET_NAME="${SOURCE_SET_NAME_BY_TYPE[EXTERNAL_REPORTS]:-}"
EXTERNAL_REPORT_SOURCE_SET_PATH="${SOURCE_SET_PATH_BY_TYPE[EXTERNAL_REPORTS]:-}"

WORK_BASE_PATH="$OUTPUT_ROOT/workspace/project-root"
WORK_CONFIG_PATH="$WORK_BASE_PATH/v8project.yaml"

if [[ ! -d "$FIXTURE_BASE_PATH" ]]; then
    die "Fixture source directory not found: $FIXTURE_BASE_PATH"
fi

if [[ ! -x "$BIN_PATH" ]]; then
    echo "Building v8-runner binary..." >&2
    (cd "$ROOT_DIR" && cargo build --locked --bin v8-runner >/dev/null)
fi

rm -rf "$OUTPUT_ROOT"
mkdir -p \
    "$WORK_BASE_PATH" \
    "$OUTPUT_ROOT/artifacts/external-processor" \
    "$OUTPUT_ROOT/artifacts/external-report" \
    "$OUTPUT_ROOT/launch"

cp -R "$FIXTURE_BASE_PATH/." "$WORK_BASE_PATH/"
materialize_live_config "$DESIGNER_CONFIG_PATH" "$WORK_CONFIG_PATH" "$OUTPUT_ROOT" "$WORK_BASE_PATH"

DESIGNER_CONFIG_PATH="$WORK_CONFIG_PATH"

for source_set_type in "${required_types[@]}"; do
    source_set_path="${SOURCE_SET_PATH_BY_TYPE[$source_set_type]}"
    if [[ ! -d "$WORK_BASE_PATH/$source_set_path" ]]; then
        die "Configured source-set path does not exist under fixture project root: $source_set_path"
    fi
done

if [[ "$BUILDER_BACKEND" == "DESIGNER" ]]; then
    EXTERNAL_PROCESSOR_ARTIFACT_NAME="$(extract_artifact_root_name "$EXTERNAL_PROCESSOR_SOURCE_SET_PATH")"
    EXTERNAL_REPORT_ARTIFACT_NAME="$(extract_artifact_root_name "$EXTERNAL_REPORT_SOURCE_SET_PATH")"
fi

print_stage "init and setup infobase"
run_cli init
assert_file_exists "$(extract_connection_file_path)/1Cv8.1CD"

build_json="$OUTPUT_ROOT/json/build.json"
print_stage "build full rebuild"
run_cli_json_to_file "$build_json" build --full-rebuild
assert_json_step_ok "$build_json" "$CONFIGURATION_SOURCE_SET_NAME"
assert_json_step_ok "$build_json" "$EXTENSION_SOURCE_SET_NAME"

incremental_build_json="$OUTPUT_ROOT/json/build-incremental.json"
print_stage "build incremental no-op"
run_cli_json_to_file "$incremental_build_json" build
assert_json_step_ok "$incremental_build_json" "$CONFIGURATION_SOURCE_SET_NAME"
assert_json_step_ok "$incremental_build_json" "$EXTENSION_SOURCE_SET_NAME"

extensions_json="$OUTPUT_ROOT/json/extensions.json"
print_stage "extensions properties"
run_cli_json_to_file "$extensions_json" extensions --name "$EXTENSION_SOURCE_SET_NAME"
assert_json_command_ok "$extensions_json" "extensions"

if [[ "$BUILDER_BACKEND" == "DESIGNER" ]]; then
    print_stage "syntax and checks"
    run_cli syntax designer-config --all-extensions
    run_cli syntax designer-config \
        --server \
        --extended-modules-check \
        --check-use-synchronous-calls \
        --check-use-modality \
        --extension "$EXTENSION_SOURCE_SET_NAME"
    run_cli syntax designer-modules --server --all-extensions
    run_cli syntax designer-modules --thin-client --extended-modules-check --extension "$EXTENSION_SOURCE_SET_NAME"

    print_stage "test"
    run_test_stage

    print_stage "package artifacts"
    run_cli make --output "$OUTPUT_ROOT/artifacts/configuration.cf"
    assert_file_nonempty "$OUTPUT_ROOT/artifacts/configuration.cf"

    run_cli make \
        --output "$OUTPUT_ROOT/artifacts/extension.cfe" \
        --source-set "$EXTENSION_SOURCE_SET_NAME" \
        --extension "$EXTENSION_SOURCE_SET_NAME"
    assert_file_nonempty "$OUTPUT_ROOT/artifacts/extension.cfe"

    run_cli make \
        --output "$OUTPUT_ROOT/artifacts/external-processor" \
        --source-set "$EXTERNAL_PROCESSOR_SOURCE_SET_NAME"
    assert_file_nonempty "$OUTPUT_ROOT/artifacts/external-processor/${EXTERNAL_PROCESSOR_ARTIFACT_NAME}.epf"

    run_cli make \
        --output "$OUTPUT_ROOT/artifacts/external-report" \
        --source-set "$EXTERNAL_REPORT_SOURCE_SET_NAME"
    assert_file_nonempty "$OUTPUT_ROOT/artifacts/external-report/${EXTERNAL_REPORT_ARTIFACT_NAME}.erf"

    print_stage "deploy-ready artifact validation"
    for artifact in \
        "$OUTPUT_ROOT/artifacts/configuration.cf" \
        "$OUTPUT_ROOT/artifacts/extension.cfe" \
        "$OUTPUT_ROOT/artifacts/external-processor/${EXTERNAL_PROCESSOR_ARTIFACT_NAME}.epf" \
        "$OUTPUT_ROOT/artifacts/external-report/${EXTERNAL_REPORT_ARTIFACT_NAME}.erf"; do
        assert_file_nonempty "$artifact"
        echo "READY: $artifact"
    done

    print_stage "launch smoke"
    run_launch_smoke
else
    print_stage "dump full"
    run_cli dump --mode full
    print_stage "dump incremental"
    run_cli dump --mode incremental
    print_stage "dump partial"
    run_cli dump --mode partial --object Catalog.Items
fi

if [[ "$DESIGNER_SMOKE_PROFILE" == "extended" ]]; then
    run_extended_steps
fi

echo
echo "Live CLI $BUILDER_BACKEND smoke completed successfully."
