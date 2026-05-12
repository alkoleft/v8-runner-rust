# Completed Task T23

## T23: Add YAML schemas for config editing

Status: completed on `2026-05-02`.

Implemented scope:

- Added generated JSON Schema artifacts for `v8project.yaml` and `v8project.local.yaml`.
- Kept the main config schema aligned with the YAML boundary where `basePath` is optional.
- Added a recursively partial local overlay schema that rejects unsupported and project identity
  keys before merge.
- Made `config init` emit a `yaml-language-server` modeline that points to the versioned raw
  schema URL for the current application version.
- Documented VS Code setup, local overlay schema usage and schema versioning policy.

Current status after `2026-05-11` follow-up:

- `basePath` is no longer a public YAML key; the project base path is derived from the primary
  config directory.
- Generated schema `$id` values and `yaml-language-server` modelines now point to the published
  `master` schema artifacts.

Verification:

- `cargo test --locked config::schema::tests`
- `cargo test --locked config_init_creates_yaml_with_detected_designer_sources`
