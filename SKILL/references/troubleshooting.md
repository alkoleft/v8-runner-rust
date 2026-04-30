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
- `basePath`
- `workPath`
- `source-set`
- `tools.platform`
- `tools.edt_cli`
- `tests`

## Common Situations

Missing 1C platform, EDT CLI, IBCMD, or test runner utilities are environment/setup issues. Report the missing utility and the config fields used for discovery.

Stale incremental state after branch switches, rebases, or large source moves usually calls for:

```bash
v8-runner build --full-rebuild
```

Partial dump with IBCMD degrades to incremental dump. Mention this in the summary and check the resulting Git diff.

Do not clean failed run directories until diagnostics are complete. Failed artifacts should remain under:

```text
workPath/temp/<runner-id>/runs/<run-id>/
```

## Runtime Directories

Useful `workPath` locations:

- `workPath/hash-storages/`: persisted change-detection state.
- `workPath/edt-workspace/`: shared EDT workspace for `init`.
- `workPath/convert/edt-workspace/`: separate EDT workspace for `convert`.
- `workPath/designer/<sourceSetName>/`: generated Designer representation, especially for EDT flows.
- `workPath/logs/platform/`: platform logs.
- `workPath/temp/`: temporary run artifacts and diagnostics.
