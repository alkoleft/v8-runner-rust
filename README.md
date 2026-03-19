# v8-test-runner

Rust CLI for local 1C development workflows.

## Build

Current `build` support is limited to `builder=DESIGNER` and `format=DESIGNER`.

- `v8-test-runner build` runs change detection and loads only affected `source-set` entries.
- `v8-test-runner build --full-rebuild` bypasses change detection and forces full load for every Designer `source-set`.
- Execution order is always the main `CONFIGURATION` first, then extensions in config order.
- Build is intentionally non-atomic across `source-set`: if a later step fails, earlier successful steps remain applied.

Optional YAML settings:

```yaml
build:
  partialLoadThreshold: 20
```

- `partialLoadThreshold` controls when partial load falls back to full load.
- `Configuration.xml` changes and deletions always force a full load.

## Tests

Current `test` support is limited to `builder=DESIGNER` and `format=DESIGNER`.

- `v8-test-runner test all` always runs `build` first, then launches YaXUnit via `1cv8c`.
- `v8-test-runner test module <MODULE_NAME>` does the same, but writes `filter.modules = ["<MODULE_NAME>"]` into the temporary YaXUnit config.
- `v8-test-runner test --full ...` keeps passed test cases and full stack traces.
- Compact mode hides passed cases and truncates stack traces.
- If the run fails or the JUnit report cannot be parsed, sanitized retained artifacts stay under `workPath/temp/yaxunit/runs/<run-id>/`.
- YaXUnit must already be installed and callable from the target infobase.

Optional YAML settings:

```yaml
tests:
  execution_timeout_seconds: 300
```

- `execution_timeout_seconds` controls the hard timeout for the Enterprise test run.
