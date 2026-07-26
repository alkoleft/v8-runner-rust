# Troubleshooting

Separate project source failures from local environment or runner setup failures.

## Initial Checks

```bash
git status --short
test -f v8project.yaml
```

Inspect `v8project.yaml` fields that affect the failing command:

- `format`
- `builder`
- `connection`
- primary config directory
- `workPath`
- `source-set`
- `tools.platform`
- `tools.edt_cli`
- `tests`

## Common Situations

Missing 1C platform, EDT CLI, IBCMD, or test runner utilities are environment/setup issues. Report the missing utility and the config fields used for discovery.

`tools download` errors that mention the maximum download size mean the selected GitHub release asset or source archive exceeded the 512 MiB response-body limit. Do not retry with `--force`; pick a smaller artifact or install the tool manually and point `v8project.local.yaml` to that local path.

Stale incremental state after branch switches, rebases, or large source moves usually calls for:

```bash
v8-runner build --full-rebuild
```

Partial dump with IBCMD degrades to incremental dump. Mention this in the summary and check the resulting Git diff.

Failed Designer partial-load builds report `partial load list path: ...`. Inspect that file together with the adjacent platform log to see the exact relative file paths passed to `-listFile`; directory entries in that list are a runner bug.

Do not clean failed run directories until diagnostics are complete. Failed artifacts should remain under:

```text
workPath/temp/<runner-id>/runs/<run-id>/
```

## Runtime Directories

Useful `workPath` locations:

- `workPath/ib-state/v1/`: opaque per-infobase/per-source CDFI, baselines, observations, and recovery journals. Do not edit, copy between infobases, or delete it as a routine fix; legacy `hash-storages` is not migrated.
- `workPath/edt-workspace/`: shared EDT workspace for `init`.
- `workPath/convert/edt-workspace/`: separate EDT workspace for `convert`.
- `workPath/designer/<sourceSetName>/`: generated Designer representation, especially for EDT flows.
- `workPath/ibcmd-data/`: project-local standalone-server data for IBCMD dump; safe to remove only when no project command is running.
- `workPath/logs/platform/`: platform logs.
- `workPath/temp/partial-lists/`: Designer partial load/dump list files; failed partial-load builds preserve the relevant list file for diagnostics.
- `workPath/temp/`: temporary run artifacts and diagnostics.

If dump reports a three-way conflict, inspect `git diff` and resolve the local/source intent before
retrying. Conflict publishes no source files and advances no private generation. For a deliberate
build recovery use `v8-runner build --full-rebuild`; do not repair opaque state files manually.
