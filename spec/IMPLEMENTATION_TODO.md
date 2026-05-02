# Active TODO For `v8-runner`

This file tracks open implementation work only.

## Current Status

- Open tasks as of `2026-05-02`:
  - `T23`: add YAML schema support for VS Code config editing.

## Open Tasks

### T23: Add YAML schemas for config editing

Status: planned

Scope:

- Add generated JSON Schema artifacts for `v8project.yaml` and `v8project.local.yaml`.
- Generate or verify schemas from the typed Rust config model during code changes, so schema drift is caught by tests or CI.
- Add `yaml-language-server` modeline to generated configs, pointing `v8project.yaml` to the main schema.
- Keep local overlay schema separate from the main schema: it must allow local-only sections and reject project identity keys forbidden by ADR-0021 (`source-set`, `format`, `builder`).
- Decide and document schema versioning; default direction: schema version equals application version.

Acceptance:

- `v8-runner config init` emits a `v8project.yaml` with a schema modeline.
- Repository contains two schema artifacts: main config and local overlay config.
- Tests fail when schema artifacts are stale relative to the Rust config model or documented schema generation path.
- `docs/CONFIGURATION.md` explains VS Code setup with `redhat.vscode-yaml`, modeline behavior, local overlay schema, and versioning policy.
- Versioning policy states how release tags, raw schema URLs and application versions map to schema versions.

## Rules

- Keep this file short and active-only.
- Move closed task detail into `spec/archive/`.
- If a task changes a public or architectural contract, update the ADR and active docs layer
  before implementation.
- Promote only immediately executable work here; keep broader ADR reconciliation in
  `ADR_DERIVED_BACKLOG.md`.

## Historical Records

- [spec/archive/IMPLEMENTATION_TODO_2026-04-30.md](archive/IMPLEMENTATION_TODO_2026-04-30.md):
  closed task ledger moved out of the active file.
- [spec/archive/MCP_IMPLEMENTATION_PLAN_2026-03-21.md](archive/MCP_IMPLEMENTATION_PLAN_2026-03-21.md):
  closed MCP rollout history.
- [spec/archive/completed-tasks-t22.md](archive/completed-tasks-t22.md):
  closed universal tool extension preparation task.
- [spec/archive/completed-tasks-t21.md](archive/completed-tasks-t21.md):
  closed local config overlay task.
