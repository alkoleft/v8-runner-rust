# Completed Task T26

## T26: Sync public basePath removal and master schema URLs

Status: completed on `2026-05-11`.

Implemented scope:

- `config init --output <FILE>` resolves generated `source-set[].path` relative to the selected
  primary config directory, not necessarily the current working directory.
- `basePath` was removed from the public YAML contract; the internal project base path is derived
  from the primary config directory.
- Generated JSON Schema `$id` values and `yaml-language-server` modelines point to the published
  `master` schema artifacts, not release-tag schema URLs.

Specification surfaces synchronized:

- `spec/decisions/0021-lokalnyy-overlay-config.md`
- `spec/architecture/invariants.md`
- `spec/acceptance/real-environment-validation.md`
- `spec/archive/completed-tasks-t23.md`
- `docs/CONFIGURATION.md`
- `docs/CAPABILITIES.md`
- `docs/DEEP_DIVE.md`
- `README.md`
- `SKILL/SKILL.md` and focused skill references

Verification:

- `cargo test --locked generated_schema`
- `cargo test --locked config_init`
- `cargo test --locked cli_config_init`
