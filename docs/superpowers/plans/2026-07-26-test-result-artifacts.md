# Stable Test Result Artifacts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Return retained, typed JUnit, Allure, and log artifacts with a report-authoritative summary for both YaXUnit and Vanessa Automation.

**Architecture:** Extend the existing `ExecutionOutcome< TestReport >` and `ArtifactSet` contracts instead of adding a second result envelope. Both native runners write into one unique per-run layout; a shared collector validates and inventories their outputs, aggregates all JUnit XML files, and applies one exhaustive terminal-classification policy.

**Tech Stack:** Rust, serde, quick-xml through the existing JUnit parser, clap CLI integration tests, MCP service unit tests.

## Global Constraints

- Base commit is canonical `alkoleft/master` at `d612e2d07e0702c6c829f3981d083022611f6373`.
- Both JUnit and Allure are mandatory for supported current/latest native runners.
- Allure validity means the results directory contains at least one regular file recursively; native files remain opaque.
- Native JUnit summary overrides a nonzero process exit when the report contains failures or errors.
- Successful and failed terminal runs retain their unique run directory.
- Public artifact inventory contains only paths that exist when the outcome is built.
- CLI and MCP expose the same canonical `ExecutionOutcome` artifact and metrics semantics.
- Follow TDD: every production behavior starts with a test that fails for the intended missing behavior.
- Apply `rust-expert-best-practices-code-review`; use typed enums, exhaustive matches, borrowed `&Path`, and `Result` propagation without new `unwrap`/`expect` in production paths.
- Update `SKILL/SKILL.md` or `SKILL/references/testing.md` for the changed external test workflow.
- Do not fix unrelated baseline failures; compare the final full-suite failure set with the recorded 656-pass/48-fail sandbox baseline.

---

### Task 1: Typed test artifact and error vocabulary

**Files:**
- Modify: `src/domain/artifact.rs`
- Modify: `src/domain/test.rs`
- Modify: `src/domain/runner.rs`
- Modify: `src/use_cases/request.rs`
- Modify: `src/cli/execute.rs`
- Modify: `src/mcp/service.rs`

**Interfaces:**
- Produces `ArtifactKind::{JunitXml, AllureResults, ErrorDetails, Screenshot}`.
- Produces roles `junit_xml`, `allure_results`, `error_details`, and `screenshot`.
- Produces `RunnerOutputFormat::AllureResults`.
- Produces `TestErrorKind::{AllureNotProduced, AllureEmpty}` mapped to `ExecutionStatus::InvalidOutput`.
- Keeps `RetainedPaths.junit_xml` as the first-report compatibility projection and adds `allure_results`.

- [ ] **Step 1: Write failing domain serialization and status tests**

Add literal assertions that:

```rust
assert_eq!(
    serde_json::to_value(ArtifactKind::JunitXml).unwrap(),
    serde_json::json!("junit_xml")
);
assert_eq!(
    serde_json::to_value(ArtifactKind::AllureResults).unwrap(),
    serde_json::json!("allure_results")
);
assert_eq!(
    test_execution_status(Some(TestErrorKind::AllureNotProduced), false),
    ExecutionStatus::InvalidOutput
);
```

Extend the retained-path roundtrip test with an Allure directory and verify
that repeated JUnit artifacts keep the first sorted report as the compatibility
`junit_xml`.

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```bash
cargo test domain::artifact domain::test use_cases::request cli::execute::tests::maps_vanessa_request_from_configured_profile mcp::service::tests::run_all_tests_maps_vanessa_request_with_profile_overrides -- --nocapture
```

Expected: compilation/test failures because the new variants and fields do not
exist.

- [ ] **Step 3: Implement the minimal typed vocabulary**

Add the enum variants and constants. Add:

```rust
pub fn get_all_by_role<'a>(&'a self, role: &'a str) -> impl Iterator<Item = &'a Path> + 'a
```

Use it to select a deterministic first JUnit report in `RetainedPaths`.
Add Allure to YaXUnit and Vanessa `RunnerProfile.output_formats`, and set
`retain_artifacts_on_success: true` for both CLI and MCP request builders.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run the Step 2 command and require zero failures.

- [ ] **Step 5: Self-review and commit**

Check exhaustive `TestErrorKind` matches and JSON names, then commit:

```text
feat(test): add typed result artifact vocabulary

- distinguish JUnit and Allure outputs
- retain supported test artifacts on success
```

---

### Task 2: Generate simultaneous native reports and inventory existing outputs

**Files:**
- Modify: `src/use_cases/run_tests.rs`
- Modify: `src/use_cases/vanessa.rs`

**Interfaces:**
- `RunArtifacts` owns `junit_dir`, primary YaXUnit `junit_xml`, and `allure_results_dir`.
- `build_yaxunit_config` serializes `reports: Vec<YaXUnitReportConfig>`.
- `VanessaTestArtifacts` carries the Allure directory.
- `collect_run_artifacts(&RunArtifacts) -> ArtifactSet` returns only existing paths.
- A `Drop` implementation removes `run.inprogress` without deleting the run directory.

- [ ] **Step 1: Write failing YaXUnit and Vanessa configuration tests**

Replace the legacy single-format assertion with literal JSON behavior:

```rust
assert_eq!(json["reports"][0]["format"], "jUnit");
assert_eq!(json["reports"][0]["path"], artifacts.junit_xml.display().to_string());
assert_eq!(json["reports"][1]["format"], "allure");
assert_eq!(
    json["reports"][1]["path"],
    artifacts.allure_results_dir.display().to_string()
);
assert!(json.get("reportFormat").is_none());
```

Add a Vanessa overlay test asserting:

```rust
assert_eq!(payload["ДелатьОтчетВФорматеjUnit"], true);
assert_eq!(payload["ДелатьОтчетВФорматеАллюр"], true);
assert_eq!(
    payload["КаталогВыгрузкиAllure"],
    artifacts.allure_results_dir.display().to_string()
);
```

Add inventory tests proving a missing expected JUnit path is omitted while
existing config/log/Allure paths are present, and proving the sentinel is
removed when `RunArtifacts` is dropped.

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```bash
cargo test use_cases::run_tests use_cases::vanessa -- --nocapture
```

Expected: failures because simultaneous configuration, Allure layout, filtered
inventory, and sentinel cleanup do not exist.

- [ ] **Step 3: Implement simultaneous native configuration**

Use:

```rust
#[derive(Debug, Serialize)]
struct YaXUnitReportConfig {
    format: &'static str,
    path: String,
}
```

Create both output directories before launch. Add Vanessa keys
`ДелатьОтчетВФорматеАллюр` and `КаталогВыгрузкиAllure`. Do not enable
environment-dependent screenshot capture.

- [ ] **Step 4: Implement existing-path inventory**

Build `ArtifactSet` from the run directory, generated config, discovered JUnit
files, Allure directory, runner log, platform log, and optional diagnostics.
Call `metadata`/`is_file`/`is_dir` before inserting each item. Sort repeated
items by path. Remove the sentinel in `Drop for RunArtifacts`.

- [ ] **Step 5: Run focused tests and verify GREEN**

Run the Step 2 command and require zero failures.

- [ ] **Step 6: Self-review and commit**

Verify borrowed path arguments, safe recursive traversal, and no deletion of
the run directory, then commit:

```text
feat(test): generate JUnit and Allure artifacts

- configure simultaneous native reports
- inventory only materialized run outputs
```

---

### Task 3: Aggregate native reports and classify terminal results

**Files:**
- Modify: `src/use_cases/run_tests.rs`
- Modify: `src/use_cases/run_tests/coordinator.rs`
- Modify: `src/use_cases/run_tests/helpers.rs`
- Modify: `src/parsers/junit.rs` only if a parser-level aggregation helper is cleaner

**Interfaces:**
- `discover_junit_reports(&Path) -> std::io::Result<Vec<PathBuf>>` is recursive and sorted.
- `parse_junit_reports(&[PathBuf]) -> NormalizedParse<TestReport>` aggregates every report.
- `validate_allure_results(&Path) -> Result<(), TestErrorKind>` checks recursive non-emptiness.
- `classify_test_completion(&TestSummary, i32) -> TestErrorKind/ExecutionStatus decision` encodes report-first precedence.

- [ ] **Step 1: Write failing discovery and aggregation tests**

Create two nested JUnit fixtures in reverse filesystem order. Assert the
returned paths are sorted and the aggregate literal summary is:

```rust
TestSummary {
    total: 3,
    passed: 1,
    failed: 1,
    skipped: 0,
    errors: 1,
}
```

Add a malformed second report and assert the aggregate is invalid output rather
than silently using the first report.

- [ ] **Step 2: Write failing Allure validation and classification table tests**

Use table rows:

```rust
// summary failed, exit 1 => TestFailures
// summary errors, exit 2 => TestFailures
// green summary, exit 1 => EnterpriseExitedNonZero
// green summary, exit 0 => success/no error
```

Add empty/missing Allure cases mapping to `AllureEmpty` and
`AllureNotProduced`.

- [ ] **Step 3: Run focused tests and verify RED**

Run:

```bash
cargo test use_cases::run_tests parsers::junit -- --nocapture
```

Expected: failures for missing collection, aggregation, validation, and
classification behavior.

- [ ] **Step 4: Implement deterministic aggregation and validation**

Traverse directories without following symlinks. Sort paths before parsing.
Sum counters with `saturating_add`, append suites and extracted errors, and
return all parse errors with the offending path in details. Treat any invalid
JUnit file as invalid native output.

- [ ] **Step 5: Implement report-authoritative coordinator flow**

After native process completion:

1. materialize the Vanessa runner log;
2. collect and parse every JUnit report;
3. validate Allure;
4. parse runner logs;
5. classify from summary before considering nonzero exit;
6. attach the existing-path artifact inventory to every terminal outcome,
   including success.

Replace `expect("junit parse error")` with explicit `Result`/fallback handling.
Do not call `cleanup_run_dir`.

- [ ] **Step 6: Run focused tests and verify GREEN**

Run the Step 3 command and require zero failures.

- [ ] **Step 7: Self-review and commit**

Check the classification matrix, overflow behavior, path diagnostics, and
successful retention, then commit:

```text
feat(test): classify results from native reports

- aggregate deterministic JUnit summaries
- preserve test failures across nonzero exits
```

---

### Task 4: Public contract integration, documentation, and regression coverage

**Files:**
- Modify: `tests/cli_test.rs`
- Modify: `tests/snapshots/cli_test__test_module_compact_json.snap`
- Modify: `tests/snapshots/cli_test__test_module_full_json.snap`
- Modify: `src/command_envelope.rs` if compatibility projection needs adjustment
- Modify: `docs/CAPABILITIES.md`
- Modify: `SKILL/references/testing.md`
- Modify: `SKILL/SKILL.md` only if its concise top-level guidance must change

**Interfaces:**
- Fake YaXUnit and Vanessa executables read the new native configuration and materialize both report types.
- CLI JSON returns `execution.artifacts.items[]` with exact kind/path and `execution.metrics`.
- MCP continues to serialize the same `ExecutionOutcome`.

- [ ] **Step 1: Update fake native runners and write failing CLI contract tests**

Make each fake runner:

- read JUnit and Allure destinations from its generated native config;
- create a JUnit XML and at least one Allure result file;
- create engine and platform logs.

Add assertions for both YaXUnit and Vanessa success:

```rust
assert_eq!(payload["data"]["execution"]["metrics"]["total"], 1);
assert!(artifact_items.iter().any(|item| item["kind"] == "junit_xml"));
assert!(artifact_items.iter().any(|item| item["kind"] == "allure_results"));
for item in artifact_items {
    assert!(Path::new(item["path"].as_str().unwrap()).exists());
}
```

Add failures for:

- nonzero exit plus failing JUnit returns `test_failures` with summary;
- missing JUnit returns `invalid_output`;
- missing/empty Allure returns `invalid_output`;
- two runs return distinct retained roots.

- [ ] **Step 2: Run CLI tests and verify RED**

Run:

```bash
cargo test --test cli_test -- --nocapture
```

Expected: contract failures until the fake runners and public projection match
the new behavior.

- [ ] **Step 3: Complete CLI/MCP projection and snapshots**

Keep `execution.artifacts` canonical. Preserve the legacy compatibility
projection where its required primary paths exist. Update snapshots with
stable scrubbed run paths and exact artifact kinds.

Add or update MCP service tests to assert `RunnerOutputFormat::AllureResults`
and success retention policy for both runner kinds.

- [ ] **Step 4: Update external documentation and repo-local skill**

Document:

- retained location `workPath/temp/<runner-id>/runs/<run-id>/`;
- simultaneous JUnit and Allure;
- summary and artifact locations in `--json-message`;
- invalid native reports as infrastructure failure;
- current/latest native runner compatibility;
- storage lifecycle: successful results remain until explicitly removed.

Keep `SKILL/references/testing.md` concise and actionable.

- [ ] **Step 5: Run integration and documentation checks**

Run:

```bash
cargo test --test cli_test -- --nocapture
cargo test mcp::service::tests -- --nocapture
cargo test generated_schema_artifacts_are_current -- --nocapture
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
git diff --check
```

Require zero issue-related failures. Pre-existing warnings must be reported and
must not be silently waived.

- [ ] **Step 6: Self-review and commit**

Check every issue acceptance criterion against a named test, then commit:

```text
feat(test): expose stable native result artifacts

- cover CLI and MCP artifact contracts
- document retained test diagnostics
```

---

### Task 5: Independent final verification and review

**Files:**
- Review all changes since `d612e2d07e0702c6c829f3981d083022611f6373`.

**Interfaces:**
- Produces independent tester, general reviewer, and Rust-expert reports.
- Produces final verification evidence without modifying unrelated code.

- [ ] **Step 1: Run the focused verification matrix**

Run:

```bash
cargo test parsers::junit -- --nocapture
cargo test use_cases::run_tests -- --nocapture
cargo test --test cli_test -- --nocapture
cargo test mcp::service::tests -- --nocapture
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

- [ ] **Step 2: Run the complete suite and compare baseline**

Run:

```bash
cargo test --all-targets
```

Any new failing test relative to the recorded 48-failure baseline is blocking.
Existing sandbox-only failures are reported with exact counts.

- [ ] **Step 3: Dispatch independent reviews**

The general reviewer checks issue criteria, architecture, CLI/MCP compatibility,
and test completeness. A separate Rust expert explicitly applies
`rust-expert-best-practices-code-review` to type safety, error handling, API
design, filesystem traversal, and performance. Every finding is fixed or
recorded as an accepted waiver with a concise technical reason.

- [ ] **Step 4: Verify repository state and prepare final commit if needed**

Run:

```bash
git status --short
git diff --check
git log --oneline d612e2d07e0702c6c829f3981d083022611f6373..HEAD
```

Ensure no generated scratch files or unrelated changes remain.
