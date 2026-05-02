# Completed Task T21

## T21: Implement local config overlay

Status: completed on `2026-05-02`.

Source ADR: [ADR-0021](../decisions/0021-lokalnyy-overlay-config.md).

Implemented scope:

- Added automatic sibling `v8project.local.yaml` overlay loading for primary config files.
- Added recursive YAML map merge with scalar/list replacement before typed deserialization.
- Made missing `basePath` default to the primary config directory at the YAML boundary.
- Rejected local overlay attempts to override `source-set`, `format`, or `builder`.
- Rejected unsupported local overlay top-level keys outside `workPath`, `infobase`, `tools`,
  `tests`, and `mcp`.
- Kept precedence `project config -> local overlay -> CLI overrides`.
- Kept `v8project.local.yaml` as an overlay-only file, not a supported `--config` entrypoint.
- Synchronized configuration docs, examples, architecture invariants, `.gitignore`, and the repo
  skill reference.

Verification:

- `cargo test --locked config`
- `cargo test --locked --test cli_bootstrap`
