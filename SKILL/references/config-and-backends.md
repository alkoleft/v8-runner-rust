# Config And Backends

Inspect `v8project.yaml` before diagnosing build, syntax, dump, test, and launch behavior.
If a sibling `v8project.local.yaml` exists, inspect it too because it overrides machine-local
settings before CLI overrides.

## Fields To Check First

- `workPath`: generated state, temp files, and workspace location.
- `format`: `DESIGNER` or `EDT`.
- `builder`: `DESIGNER` or `IBCMD`.
- `infobase.connection`: often `File=build/ib` for local automation.
- `infobase.unlock_code`: optional infobase locking code. Non-empty values are propagated to DESIGNER as `/UC <value>`; an empty string omits `/UC`. Required when the configuration was sealed with "Установить пароль"; masked in logs. Place this in `v8project.local.yaml` together with `infobase.password`.
- `build.dynamicUpdate`: project-wide default for `/UpdateDBCfg -Dynamic+`. Off by default. CLI `build --dynamic` overrides it for a single invocation.
- `source-set`: ordered configuration and extension sources.
- `tools.platform.path` or `tools.platform.version`: 1C platform discovery hints.
- `tools.edt_cli.path`, `version`, and `interactive-mode`: EDT CLI discovery and execution mode.
- `tests.yaxunit` and `tests.va`: test runner configuration.
- `tools.client_mcp`, `tools.va`, and `tools.enterprise`: launch and client-side MCP integration hints.
- `tools.client_mcp.extension`: optional tool extension prepared by `build`; it is not a project `source-set`.

## Format And Backend Rules

- `format=DESIGNER`, `builder=DESIGNER`: supports init, build, extensions, dump, Designer syntax checks, tests, make/load/artifact workflows if configured.
- `format=DESIGNER`, `builder=IBCMD`: supports init, build, extensions, dump with a limited backend and only file infobases.
- `format=EDT`, `builder=DESIGNER`: supports init, build through EDT export to Designer files, EDT syntax checks, extensions, and tests.
- `format=EDT`, `builder=IBCMD`: supports init and build through EDT export to Designer files followed by IBCMD import/apply; requires a file infobase.
- `extensions` supports Designer and EDT projects, but only extension `source-set` entries are actionable.
- `syntax designer-config` and `syntax designer-modules` require Designer format with Designer backend.
- `syntax edt` requires EDT format with Designer backend.
- `dump --mode partial` with IBCMD degrades to incremental dump and must be called out in user-facing summaries.
- `convert` is CLI-only, repo-aware, uses configured `source-set`, does not use `builder`, and does not require an infobase.
- `load` supports `.cf` and `.cfe` only for `format=DESIGNER`, `builder=DESIGNER`.
- `tools.client_mcp.extension.source` is prepared during `build`, skipped when unchanged, and refreshed by `build --full-rebuild`; `.artifact.path` must point to `.cfe` and currently requires `builder=DESIGNER`.
- `make` / `artifacts` require `builder=DESIGNER` and publish `.cf`, `.cfe`, `.epf`, or `.erf` depending on target/source-set.

## Source-Set Notes

`source-set.name` is the stable identity for ordering, diagnostics, runtime contexts, generated directories, and command selection.
Relative `source-set.path` values are resolved from the directory containing the primary `v8project.yaml`.

Supported `source-set.type` values:

- `CONFIGURATION`
- `EXTENSION`
- `EXTERNAL_DATA_PROCESSORS`
- `EXTERNAL_REPORTS`

Prefer `--source-set <NAME>` for narrow build, dump, convert, and artifact flows when the user's change is scoped to one configured source-set.

## Config Path

`v8project.yaml` is the default config filename. Use `--config <path>` only when the active project config is not at the default path or the user explicitly asks for that command form.

`v8project.local.yaml` is an automatic local overlay only. It may override only `workPath`,
`infobase.*`, `tools.*`, `tests.*`, and `mcp.*`; it must not define `source-set`, `format`, or
`builder`, and it must not be used as `--config`. `--workdir` wins over both config files.
`config init` creates the sibling local overlay as an empty mapping with a schema modeline and adds
`v8project.local.yaml` to `.gitignore` when needed.
