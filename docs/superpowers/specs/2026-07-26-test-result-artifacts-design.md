# Stable Test Result Artifacts

## Goal

Make `test yaxunit` and `test va` return the same stable machine-readable
artifact contract for successful test runs, test failures, and infrastructure
failures. Native JUnit results are the source of truth for the test summary;
the native process exit code is supporting infrastructure evidence.

## Supported Native Runners

This contract targets the current releases downloaded by `v8-runner`:

- YaXUnit versions that support the `reports` array with simultaneous `jUnit`
  and `allure` outputs;
- Vanessa Automation versions that support independent JUnit and Allure
  output settings.

Older manually supplied runners that cannot produce both formats are outside
the compatibility guarantee and fail with an invalid-output infrastructure
result.

## Per-run Layout

Every invocation that reaches runner preparation allocates its existing unique directory:

```text
workPath/temp/<runner-id>/runs/<timestamp>-<pid>-<uuid>/
├── config.json | va-params.json
├── junit/
│   └── one-or-more.xml
├── allure-results/
├── runner.log
└── enterprise.out.log
```

YaXUnit writes its JUnit report into `junit/report.xml`. Vanessa may create one
or more XML files below `junit/`. Both runners write raw Allure result files
below `allure-results/`.

The run directory is retained for every terminal result after allocation. A
`--no-build` file-infobase preflight failure happens before allocation, as
required by the no-build contract, and therefore has no run directory. The internal
`run.inprogress` sentinel is removed when the in-process `RunArtifacts` guard
is dropped and is not part of the public artifact inventory.

## Artifact Contract

`ExecutionOutcome.artifacts` remains the canonical artifact container. Extend
`ArtifactKind` with exact test-result kinds:

- `junit_xml` for every discovered JUnit file;
- `allure_results` for the Allure results directory;
- `runner_log` and `platform_log` for engine and platform logs;
- `error_details` and `screenshot` for optional discovered diagnostic files.

The run directory and generated native configuration remain artifacts. Only
paths that exist at inventory time are returned. Artifact items are sorted by
kind/role/path so JSON is deterministic.

`TestEnvelopeData.retained_paths` remains a compatibility projection. It may
expose the first JUnit XML but does not replace the canonical multi-artifact
list.

The explicit summary stays in `report.summary` and
`execution.metrics` (`total`, `passed`, `failed`, `errors`, `skipped`).

## Native Configuration

YaXUnit receives:

```json
{
  "reports": [
    { "format": "jUnit", "path": "<run>/junit/report.xml" },
    { "format": "allure", "path": "<run>/allure-results" }
  ]
}
```

Vanessa receives both independent sets of keys:

```json
{
  "ДелатьОтчетВФорматеjUnit": true,
  "КаталогВыгрузкиJUnit": "<run>/junit",
  "ДелатьОтчетВФорматеАллюр": true,
  "КаталогВыгрузкиAllure": "<run>/allure-results"
}
```

The runner does not enable screenshot capture because Vanessa screenshot
capture requires environment-specific external tooling. Existing screenshots
and error-detail files produced by the native runner are inventoried when
present.

## Report Validation and Aggregation

JUnit discovery recursively collects every `.xml` file below `junit/`, sorts
paths, parses each file with the existing parser, and aggregates suites,
extracted errors, and summary counters.

JUnit is invalid infrastructure output when no XML exists, any XML is empty,
or any XML is malformed.

Allure is valid when `allure-results/` exists and contains at least one regular
file recursively. The runner treats Allure files as opaque native results; it
does not impose one Allure schema because YaXUnit and Vanessa may emit
different native result encodings.

## Terminal Classification

Classification is exhaustive and ordered:

1. cancellation or timeout preserves its terminal status;
2. missing, empty, or malformed JUnit, or missing/empty Allure, is
   `invalid_output` and an infrastructure error;
3. a valid JUnit summary with `failed > 0` or `errors > 0` is
   `test_failures`, even when the native process exit code is nonzero;
4. valid green reports plus a nonzero native exit code is
   `enterprise_exited_non_zero`;
5. valid green reports plus exit code zero is success.

A nonzero process exit remains in diagnostics when JUnit proves test
failures; it does not replace the report-authoritative classification.

## Tests

Use TDD for:

- YaXUnit and Vanessa simultaneous output configuration;
- deterministic multiple-JUnit discovery and summary aggregation;
- missing, empty, and malformed JUnit;
- missing and empty Allure directories;
- the complete report-validity × summary × process-exit classification matrix;
- existing paths only in success and failure artifact inventories;
- distinct run directories;
- CLI JSON success/failure envelopes for both runners;
- CLI/MCP semantic parity through the shared execution outcome.

Targeted Rust suites must pass. The final full-suite result is compared with
the recorded baseline of 656 passed and 48 environment-sensitive failures in
the current macOS sandbox.
