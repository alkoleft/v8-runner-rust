# Project Workflows

Use these flows by user intent. Do not split the workflow only because source files are Designer or EDT; many commands share the same lifecycle and differ only by `format`, `builder`, or tool availability.

For exact support rules, read `config-and-backends.md` together with this file.

## Bootstrap

Create the default config when the project has no `v8project.yaml`:

```bash
v8-runner config init
```

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

## WS-режим к session-manager

Когда рядом с проектом запущен [`v8-client-session-manager`](https://github.com/SteelMorgan/v8-client-session-manager), 1С-клиент может подключаться к нему по WebSocket вместо локального HTTP MCP-сервера (legacy `runMcp`-режим). v8-runner делает выбор автоматически.

### Транспорт и автоопределение

`tools.client_mcp.transport`:

- `auto` (по умолчанию) — короткий TCP-probe (200 ms) на хост:порт из `manager_url`. Слышим listener → WS, нет → legacy.
- `ws` — строго WS, при недоступности менеджера запуск падает с `session-manager unreachable at <url>`.
- `legacy` — старый HTTP-режим без probe.

Override через `--mcp-transport={ws|legacy|auto}`. CLI приоритет конфига.

### Что v8-runner подставляет в `/C` в WS-ветке

```text
/C"mcpMode=ws;manager_url=<URL>;client_uid=<UUID>;kind=<KIND>;corr_id=<CORR>;mcp_log_level=<LVL>;mcp_ws_timeout_ms=<MS>"
```

Источники значений:

| Ключ | По умолчанию | Override |
|------|--------------|----------|
| `manager_url` | `tools.client_mcp.manager_url` или `ws://127.0.0.1:4000/sessions` | `--manager-url <URL>` |
| `client_uid` | новый UUID v4 на каждый запуск | `--client-uid <UUID>` |
| `kind` | внутренний mapping (см. таблицу ниже) | (нет — kind не переопределяется из CLI) |
| `corr_id` | `vr-<первые 8 символов client_uid>` | `--corr-id <STR>` |
| `mcp_log_level` | `tools.client_mcp.log_level` или `info` | `--mcp-log-level={off\|error\|warn\|info\|debug\|trace}` |
| `mcp_ws_timeout_ms` | `tools.client_mcp.ws_timeout_ms` или `1000` | `--mcp-ws-timeout-ms <N>` |

### Internal `kind` mapping

| Команда v8-runner | `kind` |
|---|---|
| `launch mcp` | `v8_runner_client` |
| `launch mcp va` | `vanessa_test_client` |
| `test yaxunit ...` | `yaxunit_runner` |
| `test va ...` | `vanessa_test_client` |

`vanessa_test_client` имеет специальную семантику в менеджере: его прокси-тулы публикуются на MCP HTTP без префикса `<kind>__` (см. router в репозитории менеджера). Не подменяй `kind` вручную.

### Тестовые подкоманды (`test yaxunit`, `test va`)

Для тестовых запусков WS-фрагмент **дописывается** через `;` к существующему `/C` (`RunUnitTests=…` или Vanessa-плеер). Никаких отдельных флагов прописывать не надо — те же `--mcp-transport`/`--manager-url`/`--mcp-log-level` доступны и тут.

### JSON-output

В режиме `--json-message` ответ launch- и test-команд включает поля транспорта:

WS-ветка:
```json
{ "transport": "ws", "client_uid": "...", "kind": "...", "manager_url": "...", "corr_id": "..." }
```
Legacy-ветка:
```json
{ "transport": "legacy", "mcp_port": 9874 }
```

Внешний оркестратор (CI, AI-агент) использует `client_uid` для поиска сессии в `session_list` менеджера.

### Менеджер не запускается из v8-runner

v8-runner только подключается к запущенному менеджеру. Подъём менеджера — отдельный шаг (`cargo run --release` в репо `v8-client-session-manager`, либо systemd-юнит `systemd/v8-session-manager.service`, либо Docker-compose). Если менеджер не нужен — `--mcp-transport=legacy` форсирует старый flow.
