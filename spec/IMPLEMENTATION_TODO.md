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

- [ ] `ADR-TASK-023`: Replace boolean-heavy syntax and launch DTOs with typed policy objects.
  Client scopes, extension scope, extended module checks, modality and sync-call checks, and
  launch target groups should be modeled by types and constructors rather than large bool sets.

- [ ] `ADR-TASK-024`: Strengthen the typed error contract and remove string erasure on the
  use-case boundary. `AppError` should evolve from string categories toward typed variants with
  preserved platform/runtime/validation distinctions, and test-runner process errors should stop
  collapsing into one broad spawn failure.

- [ ] `ADR-TASK-025`: Finish the migration to canonical `ExecutionOutcome<T>` and remove legacy
  duplicated result fields. For runner-like commands, `execution` should become the only domain
  source of truth, with any compatibility projections computed only at the adapter boundary.

- [ ] `ADR-TASK-018`: Simplify the public `convert` contract and fix full-scope
  `DESIGNER -> EDT` conversion for external source sets. Revisit `ADR-0020`, add an explicit
  user-facing `--output` target root, keep staged publication and overlap safety, stabilize
  generated EDT project naming, and cover the fixed external conversion flow with regression
  tests and docs updates.

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
