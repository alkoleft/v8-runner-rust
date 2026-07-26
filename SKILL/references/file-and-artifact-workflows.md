# File And Artifact Workflows

Use these commands when the task is about files, artifacts, publication, or source-format conversion.

## Dump

`dump` reverse-syncs the current infobase state back to project files.

```bash
git status --short
v8-runner dump --mode incremental
git diff
```

Supported modes:

```bash
v8-runner dump --mode full
v8-runner dump --mode incremental
v8-runner dump --mode partial --object Catalog:Items
```

Useful selectors:

```bash
v8-runner dump --mode incremental --source-set <NAME>
v8-runner dump --mode incremental --extension <EXTENSION>
```

`partial` requires at least one `--object`. With `builder=IBCMD`, object-scoped partial dump degrades to incremental dump with a warning.

Use `TYPE:NAME` as the canonical partial selector form, for example `Catalog:Items`.
The dotted `TYPE.NAME` form remains compatible. The Designer list and JSON
`data.selectors[*].normalized` use `TYPE.NAME`; JSON `data.selectors[*].requested`
preserves the submitted selector. Before the platform starts, selector syntax is validated:
`TYPE` and `NAME` must be non-empty, exactly one `:` or `.` separator is required, and
control characters are rejected. With `builder=DESIGNER`, Designer validates whether the
metadata root type exists. With `builder=IBCMD`, the object list is not used because partial
degrades to incremental.

For `format=EDT`, dump uses private platform and configured-source shadows inside the scoped
`ib-state/v1` transaction. The platform never writes the project source or
`workPath/designer/<sourceSetName>` as its dump target.

Dump safety and recovery rules:

- each infobase/source-set identity owns private shadows and generation-scoped baselines under `workPath/ib-state/v1`; fingerprints are opaque and state must never be reused across infobases;
- legacy `workPath/hash-storages` is deliberately not migrated; missing scoped state means full bootstrap, never an unchanged result;
- incremental and partial dump require a valid matching baseline and private `ConfigDumpInfo.xml`; missing or corrupt state promotes that one operation to a full dump;
- receipt lists are exact but independent audit dimensions: an applied target may be both `processed` and `skipped` when platform work occurred but publication retained/no-op'd it;
- a three-way conflict publishes no project-source files or new private generation;
- `B=absent, S=present, D=absent` is a conflict; runner never deletes a local file absent from its baseline;
- full, incremental, and partial modes all use recoverable manifest publication. Forward recovery requires the exact `(generation, UUID transaction token)`; otherwise it restores the previous managed file state.

## Convert

`convert` is repo-aware file conversion between Designer and EDT source formats.

```bash
v8-runner convert
v8-runner convert --source-set <NAME>
v8-runner convert --output <DIR>
```

It is not a dump alias:

- it does not use an infobase;
- it does not use `builder`;
- direction is derived from configured `format`;
- without `--output`, results are published under `workPath/convert/out/<sourceSetName>/<designer|edt>/`;
- `--output` is a target root and mirrors `source-set.path` relative to the primary config directory.

`convert` is a CLI file workflow and does not run through an infobase.

## Load

`load` applies existing `.cf` or `.cfe` artifacts to an infobase.

```bash
v8-runner load --path <FILE>
v8-runner load --path <FILE> --mode merge --settings <FILE>
v8-runner load --path <FILE> --extension <NAME>
```

Rules:

- supported only for `format=DESIGNER`, `builder=DESIGNER`;
- `.cfe` requires `--extension`;
- `--mode merge` requires `--settings`;
- `load --mode update` is rejected by the current command contract.

## Make And Artifacts

`make` and `artifacts` are the same use case. Prefer `make` in examples unless the user uses the alias.

```bash
v8-runner make --output <TARGET>
v8-runner make --output <TARGET> --source-set <NAME>
v8-runner make --output <TARGET> --extension <NAME>
```

Behavior:

- main configuration exports to `.cf`;
- extension export uses `.cfe`;
- external data processors and reports publish `.epf` / `.erf` into the output directory;
- `builder=DESIGNER` is required.

Package and external-artifact publication uses staged backup/rollback semantics. Every dump mode uses journaled, manifest-scoped source publication and private-state recovery.
