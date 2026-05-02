# Active TODO For `v8-runner`

This file tracks open implementation work only.

## Current Status

- Open tasks as of `2026-05-02`:
  - `T21`: implement local config overlay from [ADR-0021](decisions/0021-lokalnyy-overlay-config.md).
  - `T22`: implement universal extension preparation and `tools.client_mcp.extension` from [ADR-0022](decisions/0022-universalnyy-mehanizm-podgotovki-rasshireniy-i-client-mcp-extension.md).

## Open Tasks

### T21: Implement local config overlay

Status: planned

Scope:

- Add automatic `v8project.local.yaml` overlay loading next to the primary `v8project.yaml`.
- Make `basePath` optional at YAML boundary with default equal to the primary config directory.
- Forbid local overlay from changing `source-set`, `format`, or `builder`.
- Keep precedence `project config -> local overlay -> CLI overrides`.

Acceptance:

- `cargo test --locked config` covers overlay merge, forbidden local keys and `basePath` default.
- `docs/CONFIGURATION.md`, examples and architecture invariants are synchronized with ADR-0021.

### T22: Implement universal extension preparation and client MCP tool extension

Status: planned

Scope:

- Add `tools.client_mcp.extension` with mutually exclusive `source` and `.cfe` `artifact` inputs.
- Keep `client_mcp` extension out of project `source-set`.
- Introduce a shared internal extension preparation mechanism instead of a client-MCP-only loader.
- Make `init` add EDT source tool extension projects to the EDT workspace.
- Make `build` prepare/load configured tool extensions after the project source-set build.
- Keep `launch mcp` / `launch mcp va` from installing or updating the extension; emit an actionable `build` hint when needed.

Acceptance:

- Targeted tests cover source/artifact validation, `.cfe` loading on `build`, EDT workspace `init` behavior and no-install launch behavior.
- Existing project extension source-set behavior stays covered and does not regress.
- `docs/CONFIGURATION.md`, `docs/CAPABILITIES.md`, examples and architecture invariants are synchronized with ADR-0022.

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
