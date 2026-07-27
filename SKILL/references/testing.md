# Testing

Use tests when behavior matters. Test commands build first, so do not run a separate `build` unless the user specifically asked for a build-only diagnosis. For an immutable prepared infobase clone, use `test --no-build`; never infer this mode merely because a previous build appears successful.

## YaXUnit

All tests:

```bash
v8-runner test yaxunit all
v8-runner test yaxunit --full all
v8-runner test --no-build yaxunit all
```

Target one module:

```bash
v8-runner test yaxunit module <MODULE_NAME>
v8-runner test yaxunit --full module <MODULE_NAME>
```

Use module-level runs for narrow code changes. Use all tests for pre-push confidence or broad changes.

## Vanessa Automation

Run the configured Vanessa Automation profile:

```bash
v8-runner test va
v8-runner test --no-build va
```

If the user points to a specific feature or profile, inspect `tests.va` in `v8project.yaml` before changing the command.

`test va` uses the configured `tests.va.profile`; do not invent ad hoc feature paths without updating config or using the repo's established wrapper.

`--no-build` requires an existing `1Cv8.1CD` for file infobases. Server infobases are validated by the test-engine connection because a local filesystem preflight is not possible.
This mode does not require project source-set directories or build tooling to be present; runner and platform inputs are still validated.

When driving tests through the MCP `run_all_tests` tool, pass `runner: "vanessa"` plus optional `profile`, `feature`, `filterTag`, `ignoreTag`, or `scenarioFilter`; do not use the default YaXUnit runner for functional `.feature` acceptance scenarios.

`tests.va.fail_fast` defaults to `false`.

When setting `tests.va.profiles.<name>.filter_tags` or `ignore_tags`, or passing `--filter-tag` / `--ignore-tag`, a leading `@` is accepted for user convenience but the generated `СписокТеговОтбор` and `СписокТеговИсключение` in runtime `VAParams` must be written without that leading `@`.

## VA Debugging And Scenario Authoring

Use `launch mcp va` when the goal is interactive Vanessa Automation debugging, scenario writing, or driving the VA feature player through onec-client-mcp-devkit:

```bash
v8-runner launch mcp va
v8-runner launch mcp va --mode thin
v8-runner launch mcp va --mcp-port <PORT>
v8-runner launch mcp va --mcp-config <FILE>
v8-runner launch mcp va --mcp-port <PORT> --wait-ready
```

This starts the client-side MCP server in 1C and loads Vanessa Automation from `tools.va`. Prefer `--wait-ready` before an agent connects: it probes `/mcp`, runs MCP initialization, lists tools, and confirms VA tools such as `load_features`, `run_scenario`, and `get_test_results` are registered. Tune that readiness wait with `tools.client_mcp.wait_ready_timeout_ms`; it falls back to `execution_timeout` and remains capped by the command deadline.

For functional `.feature` acceptance work, use Vanessa Automation (`test va` or `launch mcp va --wait-ready`), not bare `launch mcp`.

## Launch Options During Tests

Test commands accept launch-related options such as `--client-mode`, `--c`, `--execute`, `--use-privileged-mode`, and repeatable `--raw-key`.

Use these only when the user needs a specific 1C launch context; otherwise prefer the configured defaults.

## Syntax As Validation

Designer module syntax:

```bash
v8-runner syntax designer-modules --server --thin-client
```

Designer configuration syntax:

```bash
v8-runner syntax designer-config
```

EDT syntax:

```bash
v8-runner syntax edt
```

## Artifacts

Each YaXUnit or Vanessa Automation test run retains its diagnostics under:

```text
workPath/temp/<runner-id>/runs/<run-id>/
```

Supported current/latest native runners produce JUnit XML and Allure results together. In
`--json-message`, read the summary from `data.execution.metrics` and every existing typed path
from `data.execution.artifacts.items`; `data.retained_paths` is only a compatibility projection.
Missing, empty, or malformed JUnit and missing or empty Allure results are `invalid_output`
infrastructure failures. Successful and failed run directories remain until explicitly removed;
internal cleanup markers are never public artifacts. A missing file-infobase rejected by
`--no-build` preflight creates no run directory because validation precedes artifact preparation.
Optional runner diagnostics may appear under `error-details/` and `screenshots/` in the run
directory. These directories are not pre-created; existing regular files are inventoried as
`error_details` and `screenshot`, respectively. Their inventory has one shared cap of 100 regular
files, scanning `error-details/` before `screenshots/`; when a category is truncated, its directory
path is retained as the fallback artifact with that category's kind and role.

In final answers, include the command, pass/fail result, and artifact path when present.
