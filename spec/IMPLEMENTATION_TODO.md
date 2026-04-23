# Active TODO For `v8-runner`

This file is the short working source of truth for open implementation tasks.

Historical snapshots and closed plans:

- [spec/archive/IMPLEMENTATION_TODO_2026-04-21.md](archive/IMPLEMENTATION_TODO_2026-04-21.md)
- [spec/archive/IMPLEMENTATION_TODO_2026-04-23.md](archive/IMPLEMENTATION_TODO_2026-04-23.md)
- [spec/archive/MCP_IMPLEMENTATION_PLAN_2026-03-21.md](archive/MCP_IMPLEMENTATION_PLAN_2026-03-21.md)

Detailed ADR task decomposition remains in [ADR_DERIVED_BACKLOG.md](ADR_DERIVED_BACKLOG.md).

## Rules

- Keep this file short and active-only.
- Move completed task detail into `spec/archive/` after the current delivery loop ends.
- If a task changes a public or architectural contract, update the ADR and active docs layer
  before implementation.
- Take the next concrete task from top to bottom unless the user sets another priority.

## P1

- [x] `ADR-TASK-019`: Rework the public CLI help and output-path naming contract, then sync ADR
  and docs. Completed `2026-04-23`: global structured-output selection now uses
  `--json-message`, user-facing output-path flags are unified under `--output` for
  `config init`, `launch`, and `make/artifacts`, `config init` rejects global `--config` as an
  output-path shortcut with a hint, ADR/docs/arc42/live script consumers are synced, and help,
  parse, and JSON-mode regressions were added. Final completeness subagent review returned
  `APPROVED`.

- [x] `ADR-TASK-020`: Split oversized orchestration use-case modules into scenario coordinators
  and reusable policy/helper components. Completed `2026-04-23`: the previously extracted helper
  and runtime modules remain in place for `build`, `dump`, `test`, and shared EDT
  (`src/use_cases/build_project/helpers.rs`, `src/use_cases/dump_config/helpers.rs`,
  `src/use_cases/run_tests/helpers.rs`, `src/platform/edt_session/runtime.rs`), and the remaining
  thin-coordinator split is now complete via
  `src/use_cases/build_project/coordinator.rs`, `src/use_cases/dump_config/coordinator.rs`, and
  `src/use_cases/run_tests/coordinator.rs`. Top-level `run_build_*`, `run_dump_with_context`, and
  `run_tests` now stay as thin wrappers while preserving public entrypoints, the nested
  `run_build_unlocked` lock contract, dump atomic publish and interruption-safe staging behavior,
  `artifacts` reuse of `run_external_dump_designer`, test build-prerequisite and artifact-retention
  flow, and shared EDT session caller contracts. Reviewer and separate Rust expert subagent passes
  returned `no findings`; targeted `cli_build`, `cli_dump`, `cli_test`, `use_case_boundaries`,
  and `architecture_guardrails`, the tester-subagent verification matrix, and full `cargo test`
  all passed. Final completeness subagent review returned `APPROVED`.

- [x] `ADR-TASK-021`: Centralize source-set and config classification logic. Completed
  `2026-04-23`: descriptor XML classification and external logical-name parsing are now
  centralized in `src/support/source_descriptor.rs`, so `config validate`, `config init`,
  reverse-sync `dump`, and external artifacts reuse shared descriptor parsers and external root
  scanners for `Configuration`/`Extension`/`External*` markers, `MetaDataObject` wrappers, and
  `Properties/Name`, while EDT extension name reads now reuse the shared
  `edt_project::read_project_name_from_dir(...)` helper instead of local `.project` parsers.
  Local duplicate XML/layout scanners were removed from `config` and `use_cases`; targeted
  tester-subagent verification, structural grep, and full `cargo test` all passed.

- [x] `ADR-TASK-022`: Narrow the CLI and MCP transport adapters and remove duplicated
  normalization and mapping logic. Completed `2026-04-23`: adapter-shared raw normalization now
  lives in `src/support/adapter_input.rs` for string trimming, required-value checks, dump/launch
  mode parsing, syntax extension scope/defaults, dependency pre-validation, and EDT project
  normalization; final transport-neutral syntax request assembly is centralized in
  `src/use_cases/request.rs` via shared `Designer*SyntaxSelection` constructors reused by both
  CLI and MCP; shared lock/failure boundary helpers live in `src/use_cases/transport.rs`, and
  non-interactive MCP EDT syntax now routes through `McpService` instead of duplicating context
  and result mapping in `src/mcp/server.rs`. Targeted CLI/MCP mapper tests, `cargo test --locked
  --no-run`, stdio/http launch and dump transport checks, reviewer and separate Rust expert
  subagent passes, and the final completeness subagent gate all returned `APPROVED`/`no
  findings`.

- [x] `ADR-TASK-023`: Replace boolean-heavy syntax and launch DTOs with typed policy objects.
  Client scopes, extension scope, extended module checks, modality and sync-call checks, and
  launch target groups should be modeled by types and constructors rather than large bool sets.
  Completed `2026-04-23`: transport-neutral syntax requests now use typed client-scope,
  config-check, extension-scope, and extended-modules policy objects with constructor-enforced
  dependency and module-mode validation; CLI and MCP adapters build those policies while preserving
  public flag/schema contracts and rendering pre-dispatch CLI validation errors as text/JSON
  envelopes. Direct launch now uses grouped `LaunchTargetRequest`/`EnterpriseLaunchTarget` instead
  of a flat mode DTO, with existing CLI/MCP aliases preserved. Unit and integration coverage was
  added for policy constructors, syntax/launch mappers, and no-mode `designer-modules` JSON errors;
  reviewer, separate Rust expert, and final completeness subagent passes returned `no findings` /
  `APPROVED`, and full `cargo test --locked --quiet` passed.

- [x] `ADR-TASK-024`: Strengthen the typed error contract and remove string erasure on the
  use-case boundary. `AppError` should evolve from string categories toward typed variants with
  preserved platform/runtime/validation distinctions, and test-runner process errors should stop
  collapsing into one broad spawn failure.
  Completed `2026-04-23`: `AppError` now preserves typed locator, process, Designer, EDT,
  shared-session, IBCMD validation, and config sources through the use-case layer, with contextual
  variants keeping the original platform/validation/runtime class until `UseCaseError`
  normalization. Launch, init, build, dump, load, artifacts, syntax, convert, extensions, and test
  flows now propagate typed platform sources instead of stringifying locator/process/DSL errors;
  test-runner setup failures use `test_setup_failed`, and Enterprise process failures map
  exhaustively to distinct `TestErrorKind` codes for spawn, startup probe, early exit, and
  stdout/stderr log I/O. Cancellation/timeouts intentionally remain command-interruption runtime
  outcomes with `ExecutionStatus` and interruption metadata. Targeted rustfmt/tests, full
  `cargo test --locked`, reviewer, separate Rust expert, tester, and final completeness subagent
  gates passed.

- [x] `ADR-TASK-025`: Finish the migration to canonical `ExecutionOutcome<T>` and remove legacy
  duplicated result fields. Completed `2026-04-23`: `TestRunResult`, `ArtifactsResult`, and
  `LoadResult` now keep `execution` as the canonical domain source for status, diagnostics,
  structured errors, metrics, artifacts, retained paths, and typed payloads, while public CLI JSON
  compatibility fields for `test`, `artifacts`, and `load` are computed by adapter-local
  projections. MCP test mapping now reads the canonical outcome directly, and successful `load`
  compatibility messages preserve deferred diagnostics through a regression test. Targeted
  formatter/tests, tester-subagent verification, full `cargo test --locked --quiet`, reviewer, and
  separate Rust expert gates passed with `no findings`.

- [x] `ADR-TASK-018`: Simplify the public `convert` contract and fix full-scope
  `DESIGNER -> EDT` conversion for external source sets. Completed `2026-04-23`: `convert`
  now exposes `convert [--source-set <name>] [--output <dir>]`, keeps direction inference tied to
  `config.format`, preserves the default `workPath/convert/out/<sourceSetName>/<target-format>/`
  output, and treats explicit `--output` as a target root whose layout mirrors `source-set.path`
  relative to `basePath`. Staged full replacement and workspace locking are preserved; explicit
  output is rejected when it is a filesystem root, publishes under `basePath`/`workPath`, overlaps
  any source-set path, or produces overlapping targets. `DESIGNER -> EDT` staging now uses stable
  project directory names so
  generated `.project` names and extension base-project references do not inherit
  `.convert-stage-*`. Regression coverage includes full-scope external Designer-to-EDT output,
  target/source overlap, target/target overlap, single-source unselected-source overlap, and
  CLI help for the public `--output <DIR>` contract; ADR/docs/backlog surfaces are synchronized.

- [ ] `ADR-TASK-031`: Add live text progress messages for long-running CLI stages before the
  blocking work starts. Text stdout must print a stable stage name and start timestamp before each
  long operation, using the existing build-style stage vocabulary as the reference pattern, so a
  developer running commands such as `test` can see that the build prerequisite, test runner, dump,
  load, convert, syntax, init, artifacts, extensions, or launch stage has started instead of
  waiting for the final timeline only. Keep `--json-message` as the final structured envelope unless
  a separate JSON progress contract is accepted, keep raw platform stdout/stderr in logs, and add
  rendering/integration regressions proving that live text messages appear before a delayed stage
  completes and do not leak secrets.
  Open questions before implementation:
  1. Should a long stage mean every external platform/EDT/Designer/IBCMD/Enterprise process
     invocation, or only stages expected to run longer than a small threshold?
  2. Should the timestamp be a local wall-clock start time, RFC3339 with timezone, or both start
     time and final elapsed duration?
  3. Do we need periodic heartbeat lines for very long stages, or is a start line plus final elapsed
     duration enough?
  4. Should this stay text-only for now, with JSON progress left out of scope?

- [ ] `ADR-TASK-032`: Unify the CLI `--json-message` envelope and MCP `structured_content` command
  payload for AI-agent consumers. Keep MCP protocol wrapping and transport/internal errors MCP-native,
  but make successful and business-failure command payloads use the same canonical machine-readable
  envelope as CLI JSON: `ok`, `command`, `duration_ms`, `data`, `warnings`, and `steps`. Move the
  envelope contract out of `src/output` into a transport-neutral module so CLI and MCP adapters both
  project from use-case results into the same type, then retire MCP-only response shapes such as
  `success/message/build_time_ms/total_tests` or keep them only behind an explicit compatibility
  boundary during migration. Update ADR-0010 or add a follow-up ADR before implementation because
  this changes the public MCP surface, then add parity tests proving that CLI JSON and MCP structured
  content match for `build`, `test`, `dump`, `syntax`, and business failures.
  Acceptance notes:
  1. Text stdout remains a separate human-readable renderer and does not define the machine contract.
  2. MCP `CallToolResult`/`isError` semantics remain protocol-level behavior; only the command payload
     is unified.
  3. Validation/runtime/platform business failures must preserve command identity and structured error
     details in the shared envelope.
  4. The shared envelope must not depend on CLI, MCP, or presentation modules.

## P2

- [ ] `ADR-TASK-016`: Fix the CLI JSON error contract for early failures. Some validation,
  bootstrap, and pre-dispatch failures still lose command identity and return
  `command = "error"` instead of a command-specific envelope. Preserve command identity for
  `config init`, early CLI validation/preflight failures, and shared bootstrap paths, then add
  regression coverage.

- [ ] `ADR-TASK-026`: Reduce test maintenance cost with a shared test harness for shell and
  platform fixtures. Move repeated script creation, chmod, tempdir setup, cargo-bin bootstrap,
  and polling helpers into common test support so new CLI/MCP/platform regressions stop copying
  shell-stub boilerplate across many files.

- [ ] `ADR-TASK-027`: Extract a shared staged-publication mechanism for full-replacement outputs.
  `dump` and `artifacts` still duplicate the ADR-0015 flow: create target-local staging, write
  metadata, execute the platform action, check interruption before the publish safe point, publish
  through `run_no_process_critical_phase`, and merge cleanup/deferred-interruption warnings. Introduce
  a small helper for file/directory staged publication without adding a generic pipeline engine or
  changing public result contracts.

- [ ] `ADR-TASK-028`: Centralize command interruption and deferred-interruption vocabulary across
  use cases. Remaining duplicate helpers include `command_interruption_status`,
  `command_interruption_details`, `deferred_interruption_warning/details`,
  `interruption_before_safe_point`, and publication warning formatting across `build`, `dump`,
  `load`, `artifacts`, `run_tests`, `init`, `convert`, and `configure_extensions`. Keep ADR-0014
  terminal-state semantics and existing `ExecutionOutcome<T>` projections intact.

- [ ] `ADR-TASK-029`: Reduce residual build coordinator duplication after the thin-coordinator split.
  `build_project/coordinator.rs` still repeats the `analysis -> StepPlan -> execute or
  fail_with_remaining_steps` flow for Designer, IBCMD, EDT export, and generated Designer load.
  Extract narrow helpers for plan construction, remaining-step failure payloads, and
  prepared/rescan commit handling without introducing a generic pipeline engine.

- [ ] `ADR-TASK-030`: Add a read-only source-set runtime inventory/index for use-case orchestration.
  `build`, `dump`, `artifacts`, and syntax paths still rebuild `SourceSetsService` contexts,
  `contexts_by_name`, `config_by_name`, and single-configuration/extension lookup rules locally.
  Provide a shared inventory helper that preserves source-set identity, validation-boundary
  responsibilities, and the current CLI/MCP public contracts.
