# Active TODO For `v8-runner`

This file tracks open implementation work only.

## Current Status

- Open tasks as of `2026-05-02`: 2.

## Open Tasks

### T24: Skip unchanged source-backed tool extension preparation

Status: open.

Trigger: in `/home/alko/develop/open-source/rat`, `v8project.local.yaml` configures
`tools.client_mcp.extension.source.path` as an external EDT source directory. Each `test` command
performs the nested `build` and currently reaches `build: tool extension edt export` on every run,
even when that source directory is unchanged.

Scope:

- Apply the same on-demand change detection behavior to `tools.client_mcp.extension.source` that
  project `source-set` paths already use for EDT export decisions.
- Keep `tools.client_mcp.extension` outside project `source-set`; do not make it selectable by
  `--source-set`.
- Add a stable tool-extension change-detection context under `workPath/hash-storages` so unchanged
  source-backed tool extensions skip EDT export and load during `build` and nested `test -> build`.
- Preserve conservative behavior for `--full-rebuild`, missing/corrupt state, and recoverable scan
  errors: full export/load is allowed, but successful state must be committed only after successful
  platform steps.
- Leave artifact-backed `.cfe` handling out of this task unless implementation evidence shows the
  same change-detection helper can cover it without changing the public contract.
- Update docs and `SKILL/SKILL.md` only after implementation, so external guidance describes the
  shipped behavior rather than this planned target.

Acceptance:

- Repeated `v8-runner test ...` in a project like `rat` with unchanged
  `tools.client_mcp.extension.source.path` does not run `build: tool extension edt export`.
- Changing a file under the tool-extension source path triggers EDT export and extension load on the
  next `build` or nested `test -> build`.
- `v8-runner build --full-rebuild` bypasses the tool-extension analysis and refreshes the extension.
- Tests cover no-change skip, changed-source export, full-rebuild refresh, and failed export not
  committing the prepared tool-extension snapshot.

### T25: Generate JSON Schema field descriptions and remove config aliases

Status: open.

Trigger: generated `docs/schemas/v8project.schema.json` and
`docs/schemas/v8project.local.schema.json` are useful for YAML editing, but fields do not currently
carry `description` text for editor hover/help. The same schema generation path also carries YAML
aliases that make the public config contract broader and harder to reason about.

Scope:

- Add concise descriptions to the YAML-boundary schema structs in `src/config/schema.rs`, preferably
  through Rust doc comments consumed by `schemars`.
- Keep descriptions close to the schema model; do not parse prose from `docs/CONFIGURATION.md`.
- Cover both main config and local overlay schemas, including nested sections such as `infobase`,
  `source-set`, `tools`, `mcp`, `tests`, and `tools.client_mcp.extension`.
- Remove YAML aliases from `src/config/model.rs` and `src/config/schema.rs`; keep only canonical
  public keys.
- Remove alias documentation from `docs/CONFIGURATION.md` and update examples/tests that still use
  alias keys.
- Preserve null-reset, numeric-bound and validation semantics unrelated to aliases.
- Regenerate `docs/schemas/v8project.schema.json` and
  `docs/schemas/v8project.local.schema.json`.

Acceptance:

- Schema artifacts contain `description` entries for user-facing config fields.
- Schema artifacts no longer contain alias-only properties such as `executionTimeout`,
  `execution_timeout_ms`, `edt-cli`, `additionalLaunchKeys`, `additional_launch_keys`,
  `startup-timeout-ms`, or `command-timeout-ms`.
- Loader rejects removed alias keys with normal unknown-field/schema errors.
- `cargo test --locked generated_schema_artifacts_are_current` passes without
  `UPDATE_CONFIG_SCHEMAS=1`.
- Existing schema/loader parity tests for main config and local overlay remain green.

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
- [spec/archive/completed-tasks-t23.md](archive/completed-tasks-t23.md):
  closed YAML schema support for config editing.
