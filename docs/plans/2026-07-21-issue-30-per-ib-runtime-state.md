# Issue #30: per-IB runtime state and private shadow implementation plan

> Execution discipline: use test-driven development for every behavior change,
> independent tester/reviewer/Rust-expert passes before commits, and explicit waivers
> for findings that cannot be fixed in this PR.

**Goal:** Prevent false incremental decisions across infobases and guarantee that
build/dump never exposes platform-owned CDFI or half-applied changes in source trees.

**Architecture:** Introduce a secret-free `RuntimeStateLayout` at the domain/use-case
boundary and make change detection consume resolved storage paths. Designer build runs
against a private source copy. Dump runs against a private complete shadow and publishes
through a pure three-way merge plus an owned-file journal transaction. Receipts are
shared serializable domain values populated from hash-bearing deltas.

**Tech stack:** Rust 2021, `sha2`, `redb`, `walkdir`, existing platform fakes and CLI
integration tests.

---

## Task 1: Runtime identity and versioned layout

**Files:**

- Create: `src/domain/runtime_state.rs`
- Modify: `src/domain/mod.rs`, `src/domain/source_set.rs`
- Modify: `src/change_detection/source_sets.rs`, `src/change_detection/analyzer.rs`
- Test: module tests in the files above

1. Write failing tests for equivalent plain/raw connection forms, nonexistent and
   symlinked paths, embedded auth stripping, secret-safe `Debug`, distinct IBs,
   distinct roots/formats/backends, and the exact versioned layout.
2. Implement one fallible typed connection/path normalizer, tagged length-prefixed
   SHA-256 identity and `RuntimeStateLayout`; reject unsupported raw forms.
3. Resolve contexts with explicit state directories and remove the legacy
   `workPath/hash-storages` derivation.
4. Represent absent state as `Bootstrap`, never `NoChanges`; keep `IbBaseline` and
   `SourceObservation` as distinct types tied to one state generation.
5. Run targeted tests, format and Clippy.

## Task 2: Shared source inventory and exact hash deltas

**Files:**

- Modify: `src/change_detection/scanner.rs`, `src/change_detection/analyzer.rs`
- Create: `src/domain/sync_receipt.rs`
- Modify: `src/domain/mod.rs`, `src/domain/build.rs`, `src/domain/dump.rs`
- Test: scanner/analyzer/domain tests

1. Write failing tests for case-aware CDFI exclusion, symlink exclusion, custom nested
   `workPath` exclusion and pre/post hashes for add/modify/delete.
2. Add explicit excluded roots and one reusable source-file predicate.
3. Enrich `FileChange` and prepared state with stable relative paths and hashes.
4. Add sorted `SyncReceipt` / `SyncTarget` serialization with private fields and
   exhaustive terminal smart constructors; attach one receipt to every `BuildStep` and
   one to `DumpResult`.
5. Test add/delete null hash semantics, unchanged partial closure, failed/cancelled
   requested-only receipts and rejection of contradictory terminal states.
6. Run targeted tests and schema/snapshot checks.

## Task 3: Private Designer build transaction

**Files:**

- Create: `src/use_cases/runtime_state.rs`, `src/use_cases/source_transaction.rs`
- Modify: `src/use_cases/mod.rs`, `src/use_cases/build_project.rs`
- Modify: `src/use_cases/build_project/coordinator.rs`, helpers as required
- Test: build-project unit and CLI fake-platform tests

1. Write failing tests proving source CDFI is ignored, source remains byte-identical,
   private CDFI is seeded only when structurally valid, and missing/corrupt CDFI forces full.
2. Copy source to an owned transaction directory without following symlinks or copying
   excluded roots/files; compute partial paths from original root.
3. Point Designer load at staging and stage the produced validated CDFI after successful
   apply for the same recoverable generation commit as baseline/hash state.
4. Do not auto-retry an ambiguous platform rejection of seeded CDFI; preserve prior
   state and require explicit full rebuild. Make CDFI and source hashes visible through
   one recoverable generation; ensure all failure/cancellation paths
   leave source and prior state unchanged.
5. Apply the same runtime layout to tool-extension Designer flows.

## Task 4: Per-IB state through EDT and IBCMD

**Files:**

- Modify: `src/use_cases/build_project.rs`, coordinator/helpers
- Modify: `src/use_cases/tool_extension.rs`
- Test: use-case tests

1. Write lifecycle tests: A build/full, A repeat/skip, B build/full, A/skip, restart/skip.
2. Thread resolved per-IB contexts through Designer, IBCMD, EDT export and generated
   Designer load stages.
3. Defer EDT snapshot commit until its downstream Designer load/apply succeeds.
4. Verify old storage is untouched and never read.

## Task 5: Private dump shadow and pure three-way merge

**Files:**

- Create: `src/use_cases/shadow_merge.rs`
- Modify: `src/use_cases/dump_config.rs`, coordinator/helpers
- Modify: `src/use_cases/staged_publication.rs` if source-set transaction support is needed
- Test: merge unit tests and dump fake-platform integration tests

1. Write table-driven tests for every `B/S/D` rule, absent baseline bootstrap, delete/add,
   CDFI exclusion, TOCTOU rejection, crash rollback and preservation of symlink/nested
   workPath/ignored entries.
2. Maintain a typed complete private `IbBaseline` and execute every dump mode into a
   private shadow; upgrade first incremental/partial request to full shadow dump.
3. Build an owned-file manifest transaction with byte-exact backups and a durable journal;
   publish nothing on any conflict or revalidation mismatch, and recover unfinished
   publication before the next analysis.
4. After source publication, commit baseline/CDFI/`SourceObservation` under one recoverable
   state generation. Advance applied/converged paths; retain old state for local-only paths;
   advance nothing on conflict.
5. Populate exact requested/processed/skipped/conflicted dump receipts.

## Task 6: Public diagnostics and documentation

**Files:**

- Modify: CLI/MCP output tests or snapshots only where additive receipts surface
- Modify: `spec/decisions/README.md`, ADR-0002, ADR-0012, ADR-0015
- Modify: `spec/architecture/invariants.md`, affected arc42 views
- Modify: `docs/CONFIGURATION.md`, `docs/DEEP_DIVE.md`, `SKILL/SKILL.md`

1. Add tests for deterministic receipt JSON and sanitized diagnostics.
2. Document `ib-state/v1`, deliberate non-migration/full bootstrap and conflict recovery.
3. Update the repo-local skill with short external guidance only.

## Task 7: Verification and pull request

1. Run targeted tests after every task.
2. Run `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
   `cargo test --all-targets --all-features`, architecture guardrails and repository CI script.
3. Run independent Tester, Reviewer and Rust-expert reviews; fix or record every finding.
4. Run real disposable file-IB acceptance. If a platform installation is unavailable,
   preserve an executable acceptance script and open the PR as incomplete validation,
   without claiming that #30 is closed.
5. Commit with repository format, push `fix/per-ib-runtime-state`, open an upstream PR
   linking #30 and noting that it supersedes the narrower #24 approach. Use `Closes #30`
   only when the real Designer acceptance criterion has been recorded.
