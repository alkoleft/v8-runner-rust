# Artifact Review Follow-ups Design

## Scope

Address the still-valid CodeRabbit nitpicks on PR #52 without changing the
stable test-artifact contract beyond bounding optional diagnostic inventory.
The already-addressed Windows coverage, retained inventory assertion, and
Allure text output remain unchanged.

The `ArtifactSet::get_all_by_role` lifetime suggestion is not implemented.
The iterator captures `role`, so the role must live for the iterator's use;
the current signature already accepts an inline temporary when the iterator is
consumed in the same statement. The suggested lifetime bound does not enable
the claimed temporary-string use case.

## Changes

### Bound optional diagnostics

Keep at most 100 regular diagnostic files across `error-details/` and
`screenshots/`, preserving the existing deterministic category and path order.
If a category is truncated, add one artifact for that category's root
directory. The directory entry tells consumers where the omitted files remain
available while keeping the serialized `ArtifactSet` bounded to at most 102
optional diagnostic entries.

Only optional diagnostics are limited. JUnit, Allure, logs, configuration, and
the run-directory entries retain their current behavior.

### Harden test fixtures

- Select YaXUnit JUnit and Allure report paths by their `format` field instead
  of array position in CLI and MCP embedded scripts.
- Make every Vanessa fixture-specific textual replacement assert that its
  expected source fragment exists before replacement.
- Remove the redundant `setup_project_with_additional_launch_keys` delegate.
- Collapse repeated retained-path snapshot normalization into a deterministic
  key loop.

These changes affect test infrastructure only and preserve fixture output.

## Testing

Use test-first coverage for the diagnostic bound:

- more than 100 files never produce more than 100 file entries;
- truncation adds the appropriate category directory entry;
- the limit is shared across both diagnostic categories;
- existing small inventories keep their exact stable ordering.

Run targeted unit and integration tests for artifact collection, CLI fixtures,
and MCP stdio fixtures, followed by formatting, Clippy, and the full repository
test suite. Windows-specific behavior remains covered by the existing PR CI.

## Documentation

Update `docs/CAPABILITIES.md` and `SKILL/SKILL.md` to describe the bounded
optional diagnostic inventory and directory fallback because it is visible to
external CLI/MCP consumers.
