# ConfigDumpInfo Rollback Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore tracked `ConfigDumpInfo.xml` byte-for-byte whenever a Designer build fails after the platform has begun its load.

**Architecture:** A focused recovery helper owns private snapshot creation, atomic restoration, and cleanup. `build_project` creates a guard before `/LoadConfigFromFiles`, routes every failed path through it, and exposes recovery metadata through the existing `BuildResult` contract.

**Tech Stack:** Rust, serde, tempfile, existing `support::fs` atomic file helpers, unit tests in `src/use_cases/build_project.rs` and CLI integration tests.

## Global Constraints

- Scope is GitHub #24 only; successful-build XML reconcile belongs to #46.
- Preserve original CDFI bytes, including BOM, EOL and terminal-newline state.
- Never fabricate XML, UUIDs, or platform versions.
- The primary platform error remains primary; recovery failure is attached as diagnostic context.
- TDD is mandatory: every production change begins with a failing test.
- Update `SKILL/SKILL.md` only for externally relevant command/workflow/diagnostic changes.

---

### Task 1: Define transactional CDFI recovery helper

**Files:**
- Create: `src/use_cases/build_project/cdfi_recovery.rs`
- Modify: `src/use_cases/build_project.rs`
- Test: `src/use_cases/build_project/cdfi_recovery.rs`

**Interfaces:**
- Produces `CdfiRecoveryGuard::capture(source_root: &Path, work_path: &Path) -> Result<Self, AppError>`.
- Produces `restore(&mut self) -> Result<CdfiRecoverySummary, AppError>` and `cleanup(&mut self) -> Result<(), AppError>`.
- `CdfiRecoverySummary` contains the tracked path, private snapshot path and explicit `CdfiRecoveryAction` enum.

- [ ] **Step 1: Write failing helper tests**

Add tests creating a `ConfigDumpInfo.xml` fixture containing UTF-8 BOM, CRLF and a terminal newline. Assert that capture followed by source mutation and `restore()` recreates the exact original bytes. Add a separate absent-file test: capture, create the file, restore, then assert it is absent.

- [ ] **Step 2: Run the helper tests and verify RED**

Run: `cargo test --bin v8-runner cdfi_recovery -- --format terse`

Expected: compilation/test failure because `cdfi_recovery` and its guard do not exist.

- [ ] **Step 3: Implement the smallest recovery guard**

Create a run-scoped private snapshot under `work_path`, record whether the original CDFI existed, and use the existing atomic file publication helper (or its same-directory equivalent) to restore raw bytes. Do not parse XML. Return typed recovery errors instead of panicking.

- [ ] **Step 4: Run the helper tests and verify GREEN**

Run: `cargo test --bin v8-runner cdfi_recovery -- --format terse`

Expected: all helper tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/use_cases/build_project/cdfi_recovery.rs src/use_cases/build_project.rs
git commit -m "feat(build): add CDFI recovery guard" -m "- snapshot tracked ConfigDumpInfo before Designer load\n- restore raw bytes without XML rewriting"
```

### Task 2: Route Designer build failures through recovery

**Files:**
- Modify: `src/use_cases/build_project.rs:420-597`
- Modify: `src/use_cases/build_project/coordinator.rs`
- Test: `src/use_cases/build_project.rs`

**Interfaces:**
- Consumes `CdfiRecoveryGuard` from Task 1.
- Each Designer load creates its guard immediately before `/LoadConfigFromFiles`.
- Failed load, cancellation/timeout at the pre-update safe point, and failed `/UpdateDBCfg` call `restore`; completed update calls `cleanup`.

- [ ] **Step 1: Write failing build-flow tests**

Extend the existing fake Designer script harness so a load invocation overwrites `ConfigDumpInfo.xml`. Add tests for: non-zero load exit; interruption before `UpdateDBCfg`; and non-zero update exit. Each test must assert the original fixture bytes after `run_build` returns failure. Add a successful-load test asserting the fake platform replacement remains after success.

- [ ] **Step 2: Run the focused tests and verify RED**

Run: `cargo test --bin v8-runner build_project::tests::execute_build -- --format terse`

Expected: new rollback assertions fail because build currently returns before restoring CDFI.

- [ ] **Step 3: Integrate the recovery guard**

Capture before the Designer load starts. Refactor early returns in `execute_source_set_step` through one recovery-aware failure path. Preserve existing partial-list diagnostic attachment. Restore only after the process/safe-point outcome is known; on recovery failure, attach its message to the original `AppError` without replacing it.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run: `cargo test --bin v8-runner build_project::tests::execute_build -- --format terse`

Expected: new recovery cases and existing build tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/use_cases/build_project.rs src/use_cases/build_project/coordinator.rs
git commit -m "fix(build): restore CDFI after failed Designer build" -m "- recover snapshots on load and update failure paths\n- retain platform output after successful update"
```

### Task 3: Publish recovery diagnostics and documentation

**Files:**
- Modify: `src/domain/build.rs`
- Modify: `src/cli/execute.rs`
- Modify: `tests/cli_build.rs`
- Modify: `SKILL/SKILL.md`

**Interfaces:**
- `BuildResult` gains `cdfi_recovery: Option<CdfiRecoverySummary>` with serde snake_case names.
- JSON and MCP inherit this field from `BuildResult`; human CLI output points to a retained artifact only when recovery fails.

- [ ] **Step 1: Write failing result/CLI tests**

Add a serialization assertion for recovery action, snapshot path, and failure diagnostic. Add one CLI JSON regression that simulates a failed Designer load and asserts recovery metadata is returned with the command failure payload.

- [ ] **Step 2: Run result/CLI tests and verify RED**

Run: `cargo test --test cli_build -- --format terse`

Expected: compilation or assertion failure because `BuildResult` has no recovery field.

- [ ] **Step 3: Implement typed result wiring and concise guidance**

Add an exhaustive action enum and optional summary to `BuildResult`; thread the summary through coordinator failure payloads without changing successful result semantics. Document that failed Designer build restores CDFI and reports a retained artifact only if automatic restoration failed.

- [ ] **Step 4: Run result/CLI tests and verify GREEN**

Run: `cargo test --test cli_build -- --format terse`

Expected: all CLI build tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/domain/build.rs src/cli/execute.rs tests/cli_build.rs SKILL/SKILL.md
git commit -m "feat(build): report CDFI recovery diagnostics" -m "- expose typed recovery status in build results\n- document failed-build source protection"
```

### Task 4: Verify and prepare the pull request

**Files:**
- Modify only if formatter or review identifies an issue.

- [ ] **Step 1: Run quality gates**

Run: `cargo fmt --all -- --check`, `cargo check --all-targets`, `cargo clippy --all-targets -- -D warnings`, and `git diff --check`.

- [ ] **Step 2: Run targeted and full test suites**

Run: `cargo test --test cli_build -- --format terse` and `cargo test --workspace -- --format terse`. Record any existing environment-specific failures separately from the new CDFI cases.

- [ ] **Step 3: Independent Rust and whole-branch review**

Review the branch against the design, issue #24, and the Rust best-practices checklist. Resolve every Important/Critical finding or record an explicit waiver.

- [ ] **Step 4: Create PR**

Push `fix/issue-24-cdfi-rollback`, create a PR against the intended upstream branch, reference `Closes #24`, and include exact verification evidence plus known baseline failures.
