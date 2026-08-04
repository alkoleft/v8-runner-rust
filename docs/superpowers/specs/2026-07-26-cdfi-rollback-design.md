# ConfigDumpInfo Failure Rollback Design

## Goal

Protect the tracked `ConfigDumpInfo.xml` in a Designer source set from mutations made by
`/LoadConfigFromFiles -updateConfigDumpInfo` when the encompassing build does not complete
successfully.

## Scope

This design implements GitHub issue #24 after its scope split. It covers a byte-exact
snapshot and rollback for failed, cancelled, and pre-`UpdateDBCfg` timeout paths. It does
not alter a successful platform-produced `ConfigDumpInfo.xml` and does not implement the
`preserve-unrelated` strategy; that work is issue #46.

## Design

Before the Designer load command, the build use case reads the existing
`<source_root>/ConfigDumpInfo.xml` as raw bytes and stores those bytes in a private,
run-scoped recovery artifact under the build runtime directory. The artifact is never
written into the tracked source root.

The build keeps a small recovery record: source path, snapshot path, whether the source
file existed before load, and whether recovery was attempted and completed. If the Designer
load fails, cancellation is observed at the safe point before `/UpdateDBCfg`, that safe
point's timeout expires, or `/UpdateDBCfg` fails, the use case restores the original state:
it atomically replaces the current file with the captured bytes, or removes the file when
the original state was absent. A recovery failure is attached to the primary build error;
it never turns the operation into apparent success.

On successful `UpdateDBCfg`, the snapshot is removed best-effort. Cleanup failure is
reported as a warning/degraded outcome without changing the build's successful status.

## Result Contract

`BuildResult` exposes an optional recovery summary for a Designer load that created a
snapshot. The summary contains the artifact path, the recovery action (`not_needed`,
`restored`, `removed_created_file`, or `failed`), and the count of byte-different CDFI
entries when it can be obtained without modifying XML. CLI JSON and MCP inherit the same
domain result; their human-readable output remains concise and points to the artifact on
recovery failure.

## Error Handling

The original platform/build error remains the primary error. Recovery is attempted after
the process reaches a safe outcome, never by killing Designer during the filesystem
critical section. If the snapshot cannot be created before load, the build fails before
starting Designer. If restoration fails, the error reports both the original failure and
the retained recovery artifact for manual repair.

## Testing

Tests use the existing Designer script harness to mutate `ConfigDumpInfo.xml` during load
and then simulate: load failure, cancellation/timeout before update, and update failure.
Each verifies byte-for-byte restoration of a BOM + CRLF + terminal-newline fixture. A
successful build verifies that the platform output is retained and the private snapshot is
cleaned up. A repeated failed run verifies idempotent restoration. Result serialization
tests cover the recovery summary and artifact location.

## Non-goals

- Parsing, rewriting, or reconciling XML on a successful build.
- Synthesizing UUIDs or `configVersion` values.
- Changing change-detection rules that intentionally ignore `ConfigDumpInfo.xml`.
