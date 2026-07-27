# Artifact Review Follow-ups Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve the still-valid PR #52 artifact review findings while keeping optional diagnostics bounded and test fixtures resistant to silent drift.

**Architecture:** Artifact collection will share one remaining-file budget across error details and screenshots and will add a category root entry when files are omitted. Embedded YaXUnit scripts will resolve reports by `format`; Vanessa fixture rewrites will use a checked replacement helper. Small duplicated test helpers will be collapsed without changing observable behavior.

**Tech Stack:** Rust 2021, serde JSON fixtures, shell/Python embedded test scripts, Cargo test/Clippy.

## Global Constraints

- Keep at most 100 regular diagnostic files across `error-details/` and `screenshots/`.
- A truncated category adds its root directory as an artifact, so optional diagnostic inventory is bounded to 102 entries.
- Preserve deterministic category and path ordering.
- Do not change `ArtifactSet::get_all_by_role`; document the rejected lifetime finding in the PR response.
- Update external-facing documentation for the bounded inventory contract.

---

### Task 1: Bound Optional Diagnostic Inventory

**Files:**
- Modify: `src/use_cases/run_tests.rs:780-870`
- Test: `src/use_cases/run_tests.rs:1190-1285`
- Modify: `docs/CAPABILITIES.md:220-232`
- Modify: `SKILL/SKILL.md`

**Interfaces:**
- Consumes: `collect_regular_files(root: &Path) -> std::io::Result<Vec<PathBuf>>`
- Produces: `const OPTIONAL_DIAGNOSTIC_FILE_LIMIT: usize = 100`
- Produces: `push_optional_diagnostics(set, kind, role, root, remaining: &mut usize)`

- [ ] **Step 1: Write a failing shared-limit test**

Add a unit test that creates 101 sorted error-detail files and one screenshot,
calls `collect_run_artifacts`, and asserts:

```rust
assert_eq!(diagnostic_files.len(), OPTIONAL_DIAGNOSTIC_FILE_LIMIT);
assert!(collected.items.iter().any(|artifact| {
    artifact.kind == ArtifactKind::ErrorDetails
        && artifact.path == artifacts.error_details_dir
}));
assert!(collected.items.iter().any(|artifact| {
    artifact.kind == ArtifactKind::Screenshot
        && artifact.path == artifacts.screenshots_dir
}));
assert!(!collected.items.iter().any(|artifact| artifact.path == screenshot));
```

The production mutation caught is independently budgeting each category or
serializing every discovered file.

- [ ] **Step 2: Run the new test and verify RED**

Run:

```bash
cargo test --locked collect_run_artifacts_bounds_optional_diagnostics_across_categories -- --nocapture
```

Expected: FAIL because all 102 files are currently serialized and no fallback
directory entry exists.

- [ ] **Step 3: Implement the shared budget**

Define:

```rust
const OPTIONAL_DIAGNOSTIC_FILE_LIMIT: usize = 100;
```

In `collect_run_artifacts`, create one mutable remaining counter and pass it to
both category calls in existing error-details-then-screenshots order. Change
`push_optional_diagnostics` to take the first `remaining` sorted paths, reduce
the counter by the number added, and add the category root with the same kind
and role when `paths.len() > added`.

- [ ] **Step 4: Verify GREEN and existing ordering**

Run:

```bash
cargo test --locked collect_run_artifacts_ -- --nocapture
```

Expected: the new limit test and existing ordering/symlink tests PASS.

- [ ] **Step 5: Document the contract**

Update `docs/CAPABILITIES.md` and the test-artifact guidance in `SKILL/SKILL.md`
to state the exact 100-file shared cap and category-directory fallback.

- [ ] **Step 6: Commit**

```bash
git add src/use_cases/run_tests.rs docs/CAPABILITIES.md SKILL/SKILL.md
git commit -m "fix(test): bound optional artifact inventory" \
  -m "- cap diagnostic files across error details and screenshots
- retain truncated category directories and document the contract"
```

### Task 2: Select YaXUnit Reports by Format

**Files:**
- Modify: `tests/cli_test.rs:80-125`
- Modify: `tests/cli_test.rs:840-885`
- Modify: `tests/cli_test.rs:1015-1055`
- Modify: `tests/mcp_stdio.rs:340-365`

**Interfaces:**
- Consumes: YaXUnit JSON `reports` entries with `format` and `path`
- Produces: embedded Python selection using `next(report["path"] for report in reports if report["format"] == "...")`

- [ ] **Step 1: Replace positional selection in every fixture**

For each embedded Python block, replace `reports[0]`/`reports[1]` with:

```python
reports = json.load(fh)['reports']
print(next(report['path'] for report in reports if report['format'] == 'jUnit'))
```

and:

```python
reports = json.load(fh)['reports']
print(next(report['path'] for report in reports if report['format'] == 'allure'))
```

Use the exact format strings produced by `src/use_cases/run_tests.rs`.

- [ ] **Step 2: Verify fixture behavior**

Run:

```bash
cargo test --locked --test cli_test test_yaxunit
cargo test --locked --test mcp_stdio mcp_stdio_test
```

Expected: all matching CLI and MCP tests PASS with unchanged outputs.

- [ ] **Step 3: Commit**

```bash
git add tests/cli_test.rs tests/mcp_stdio.rs
git commit -m "test(yaxunit): select reports by format" \
  -m "- remove positional assumptions from CLI and MCP fixtures"
```

### Task 3: Harden and Simplify CLI Test Helpers

**Files:**
- Modify: `tests/cli_test.rs:175-225`
- Modify: `tests/cli_test.rs:255-325`
- Modify: `tests/cli_test.rs:460-490`

**Interfaces:**
- Produces: `replace_once(body: &str, from: &str, to: &str) -> String`
- Removes: `setup_project_with_additional_launch_keys`

- [ ] **Step 1: Add a checked replacement helper**

Add:

```rust
fn replace_once(body: &str, from: &str, to: &str) -> String {
    let replaced = body.replacen(from, to, 1);
    assert_ne!(replaced, body, "VA script fixture pattern not found: {from}");
    replaced
}
```

Use it for both preparatory rewrites and every `NativeReportFixture` rewrite in
`write_va_test_script`. For `MissingAllure`, apply the two checked replacements
sequentially.

- [ ] **Step 2: Run Vanessa fixture tests**

Run:

```bash
cargo test --locked --test cli_test test_va_
```

Expected: complete, missing, and empty native-report fixtures PASS.

- [ ] **Step 3: Remove the empty setup delegate**

Change `setup_project` and the explicit additional-launch-keys caller to invoke
`setup_project_with_native_reports` directly, then delete
`setup_project_with_additional_launch_keys`.

- [ ] **Step 4: Collapse retained-path normalization**

Replace the five repeated `is_string` blocks with:

```rust
for key in [
    "config_json",
    "junit_xml",
    "allure_results",
    "yaxunit_log",
    "platform_log",
] {
    if value["data"]["retained_paths"][key].is_string() {
        value["data"]["retained_paths"][key] = Value::String(format!("<{key}>"));
    }
}
```

- [ ] **Step 5: Verify the CLI integration suite**

Run:

```bash
cargo test --locked --test cli_test
```

Expected: PASS with unchanged snapshots.

- [ ] **Step 6: Commit**

```bash
git add tests/cli_test.rs
git commit -m "test(cli): harden artifact fixtures" \
  -m "- fail fast when Vanessa fixture templates drift
- remove duplicated setup and snapshot normalization"
```

### Task 4: Independent Review and Full Verification

**Files:**
- Review: all files changed since `b5e5dff`

**Interfaces:**
- Consumes: Tasks 1-3 commits
- Produces: review findings resolved or explicitly waived

- [ ] **Step 1: Format and inspect the diff**

Run:

```bash
cargo fmt --all -- --check
git diff --check b5e5dff..HEAD
```

Expected: both commands exit successfully.

- [ ] **Step 2: Run static and full test verification**

Run:

```bash
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Expected: both commands exit successfully with no warnings or failures.

- [ ] **Step 3: Run independent reviewer and Rust-expert checks**

The reviewer checks architecture, public contract, test fidelity, and scope.
The Rust expert independently applies the complete
`rust-expert-best-practices-code-review` checklist. Resolve every finding or
record an accepted waiver with a reason.

- [ ] **Step 4: Push and report in the PR**

Push `feat/issue-26-test-artifacts`, report the verified fixes, and state that
the lifetime nitpick was skipped because its claimed temporary-string failure
does not reproduce and the iterator necessarily captures the role reference.
