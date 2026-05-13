# Project Workflows

Use these flows by user intent. Do not split the workflow only because source files are Designer or EDT; many commands share the same lifecycle and differ only by `format`, `builder`, or tool availability.

For exact support rules, read `config-and-backends.md` together with this file.

## Bootstrap

Create the default config when the project has no `v8project.yaml`:

```bash
v8-runner config init
```

This creates `v8project.yaml`, a sibling empty `v8project.local.yaml` for machine-local overrides,
and a `.gitignore` entry for the local overlay when needed.

Choose a narrower init command only when the project shape is known:

```bash
v8-runner config init --connection "File=build/ib"
v8-runner config init --format edt
v8-runner config init --builder IBCMD
```

Initialize generated runtime state only when the file infobase or EDT workspace needs to be created:

```bash
v8-runner init
```

## Build

Apply Git-visible source changes to the configured runtime state:

```bash
v8-runner build
```

Use a full rebuild after branch switches, rebases, broad object moves, or suspicious incremental state:

```bash
v8-runner build --full-rebuild
```

Use dynamic update when the infobase has live HTTP services or background jobs that block the
exclusive lock required by the default static `/UpdateDBCfg`:

```bash
v8-runner build --dynamic
```

Equivalent project-wide default is `build.dynamicUpdate: true` in `v8project.yaml`. The CLI
flag overrides the config for one invocation. The platform refuses dynamic mode when the
change set requires restructuring; `v8-runner` surfaces that error verbatim and does not fall
back to a static update.

`build` is a common workflow. For EDT projects it may export EDT sources to Designer files before applying them through the configured backend. For Designer projects it applies Designer sources directly through the configured backend.

If `tools.client_mcp.extension` is configured, `build` also prepares that tool extension after the project source-set stage, including scoped `--source-set` builds. Source-backed tool extensions use their own change-detection state and are skipped when unchanged; use `build --full-rebuild` to force refresh. Do not add a tool extension as a project `source-set` or select it with `--source-set`.

## Syntax

Choose syntax checks from config capabilities, not from assumptions about the repository name.

Designer module checks:

```bash
v8-runner build
v8-runner syntax designer-modules --server --thin-client
```

Designer configuration checks:

```bash
v8-runner build
v8-runner syntax designer-config
```

EDT checks:

```bash
v8-runner build
v8-runner syntax edt
```

If a syntax command is unavailable for the current `format` or `builder`, report the config limitation instead of inventing raw platform commands.

## Dump

Use dump when the desired source of truth is the current infobase state.

Before dumping, inspect current Git changes:

```bash
git status --short
```

Incremental dump:

```bash
v8-runner dump --mode incremental
```

Partial object dump when the backend supports it:

```bash
v8-runner dump --mode partial --object <TYPE:NAME>
```

Run `git diff` after dump and report the affected files.

## Extensions

Use `extensions` when extension properties need to be synchronized without a broader recovery step.

Do not replace extension-specific synchronization with a full rebuild unless the user asks for recovery or the narrower command fails for a relevant reason.

```bash
v8-runner extensions
v8-runner extensions --name <SOURCE_SET>
```

## Launch

Prefer runner launch commands over raw `1cv8` command construction:

```bash
v8-runner launch designer
v8-runner launch thin
v8-runner launch thick
v8-runner launch ordinary
```

Launch onec-client-mcp-devkit through the supported `launch mcp` surface instead of manually assembling `/C"runMcp..."`:

```bash
v8-runner launch mcp
v8-runner launch mcp --mode thin --mcp-port <PORT>
v8-runner launch mcp --mcp-config <FILE>
```

For ordinary direct launches, typed launch flags include `--c`, `--execute`, `--use-privileged-mode`, `--output`, and repeatable `--raw-key`.

For `launch mcp`, use `--mcp-config` and `--mcp-port`; do not pass `/C` through `--c`.

`launch mcp` and `launch mcp va` do not install or update `tools.client_mcp.extension`; run `v8-runner build` first when that extension may be missing or stale.

For `launch mcp va`, read `testing.md`; it is part of the Vanessa Automation debugging and scenario-authoring workflow.
