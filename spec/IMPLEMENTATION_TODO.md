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

- [ ] `ADR-TASK-020`: Split oversized orchestration use-case modules into scenario coordinators
  and reusable policy/helper components. The target refactor covers `build`, `dump`, `test`, and
  the shared EDT session path so new backend or mode branches stop accumulating inside
  `build_project.rs`, `dump_config.rs`, `run_tests.rs`, and `edt_session.rs`.
  Progress `2026-04-23`: extracted private helper submodules
  `src/use_cases/build_project/helpers.rs`, `src/use_cases/dump_config/helpers.rs`,
  `src/use_cases/run_tests/helpers.rs`, and `src/platform/edt_session/runtime.rs`; preserved
  public entrypoints, the nested `run_build_unlocked` lock contract, `artifacts` reuse of
  `run_external_dump_designer`, and shared EDT session caller contracts. Guardrails, focused
  CLI impact suites, shared EDT tests, and full `cargo test` passed.
  Remaining: finish the thin-coordinator split for `run_build_*`, `run_dump_with_context`, and
  `run_tests`, so new backend/mode branches move into dedicated coordinator/stage modules rather
  than continuing to grow the top-level orchestrators.

- [ ] `ADR-TASK-021`: Centralize source-set and config classification logic. Extract one
  canonical typed classifier/parser for EDT and Designer source sets plus external descriptors so
  `config validate`, `config init`, reverse-sync `dump`, and external artifacts reuse the same
  implementation.

- [ ] `ADR-TASK-022`: Narrow the CLI and MCP transport adapters and remove duplicated
  normalization and mapping logic. Shared request normalization, common validation, lock-boundary
  policy, and failure shaping should live in one adapter-neutral layer instead of parallel mapper
  stacks.

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
