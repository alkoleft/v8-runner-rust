# Deep Dive

Этот документ описывает execution semantics и operational nuances `v8-runner` без дублирования
полного каталога команд. За точным пользовательским surface обращайтесь к
[CAPABILITIES.md](CAPABILITIES.md), за YAML-контрактом к [CONFIGURATION.md](CONFIGURATION.md).

## Навигация

- [Модель выполнения](#модель-выполнения)
- [source-set и change detection](#source-set-и-change-detection)
- [Пайплайн build](#пайплайн-build)
- [Проверка и тесты](#проверка-и-тесты)
- [Файловые сценарии и публикация](#файловые-сценарии-и-публикация)
- [Shared EDT](#shared-edt)
- [workPath, lock и interruption policy](#workpath-lock-и-interruption-policy)
- [MCP runtime semantics](#mcp-runtime-semantics)

## Модель выполнения

`v8-runner` разделяет public surface и execution model:

- CLI и MCP являются разными публичными поверхностями.
- Use case слой остаётся transport-neutral orchestration boundary.
- Platform DSL и process execution остаются ниже use case слоя.
- Text output и machine-readable envelope проектируются отдельно от доменного результата.

Это позволяет держать один orchestration model для CLI и MCP, не смешивая `clap`, `Presenter` и
MCP DTO в одном слое.

## `source-set` и change detection

`source-set` — минимальная единица оркестрации.

- Для `format=DESIGNER` используется один runtime context `designer-<sourceSetName>`.
- Для `format=EDT` используются два context-а:
  - `edt-<sourceSetName>` для решения, нужен ли export;
  - `designer-<sourceSetName>` для решения, что именно грузить в ИБ.
- Persisted state живёт в `workPath/hash-storages/`.
- Generated Designer output для EDT flow живёт под `workPath/designer/<sourceSetName>`.

Change detection выполняется on-demand во время build/export/load decision и не требует
background watcher. `build --source-set <NAME>` ограничивает анализ, export/load decision и
runtime snapshot commit только указанным source-set.

## Пайплайн `build`

Для `DESIGNER`:

1. Анализ изменений по выбранным `source-set`.
2. Выбор partial/full path по изменённым файлам.
3. Загрузка через выбранный backend.
4. Commit runtime snapshot только после успешного шага.

Для `EDT`:

1. Анализ выбранных EDT source-set.
2. Export затронутых EDT source-set в generated Designer representation.
3. Повторный анализ generated Designer files.
4. Load/apply generated files через `DESIGNER` или `IBCMD`.

Пайплайн намеренно не является атомарным across many `source-set`: поздний failure не откатывает
уже успешные ранние шаги.

## Проверка и тесты

`test` и `syntax` проектируются как часть того же локального цикла, а не как отдельная
эксплуатационная подсистема.

- `test` всегда сначала делает `build` со статическим `/UpdateDBCfg`, затем запускает YaXUnit или
  Vanessa Automation. Динамическая подготовка перед тестами выполняется отдельным
  `build --dynamic`.
- `syntax designer-*` работает только для `DESIGNER` source format.
- `syntax edt` использует EDT `validate` и привязан к `format=EDT`.
- Таймауты и interruption metadata должны проходить через общий command-level contract, а не
  жить как ad hoc special case конкретной команды.

## Файловые сценарии и публикация

Важно различать три разных класса файловых операций:

### `dump`

Это reverse sync из ИБ обратно в файловые исходники.

- Для `DESIGNER` может быть full, incremental или partial.
- Для `IBCMD` object-scoped partial деградирует в incremental.
- Для `format=EDT` использует internal Designer snapshot, затем EDT import.

### `convert`

Это repo-aware файловая конвертация текущих project files между `DESIGNER` и `EDT`.

- Не использует ИБ.
- Не является alias для `dump`.
- Работает только в модели `v8project.yaml` + `source-set`.

### `load`, `make`, `artifacts`

Это materialization сценарии поверх готовых артефактов или publish targets.

- `load` работает с готовыми `.cf` / `.cfe`.
- `make` / `artifacts` публикуют final `.cf`, `.cfe`, `.epf`, `.erf`.
- Full replacement target publication идёт через staged publication model.

## Shared EDT

`tools.edt_cli.interactive_mode` включает shared interactive EDT execution model.

- `false` означает one-shot `1cedtcli`.
- `true` означает shared actor/manager и одну interactive session для поддержанных EDT-сценариев.
- Для CLI shared EDT стартует лениво при первом EDT-вызове.
- `tools.edt_cli.auto-start` относится только к long-lived host process, сейчас это MCP server.

Shared EDT нужен не ради отдельного public режима, а ради повторного использования одного
execution model для CLI и MCP.

## `workPath`, lock и interruption policy

`workPath` является корнем runtime state.

- Логи, temp files, generated outputs и persisted snapshots не должны расползаться по каталогу primary config.
- Public CLI/MCP команды, работающие с runtime state под `workPath`, должны брать workspace lock.
- Workspace lock сериализует доступ к конкретному runtime root, но не заменяет admission limits и
  не делает multi-step orchestration fully atomic.

Interruption policy:

- timeout/cancellation являются общим CLI/MCP contract;
- terminal cancellation и deferred interruption должны различаться;
- critical publish/apply phases не hard-kill by default.

## MCP runtime semantics

MCP deliberately narrower than CLI.

- Опубликованы только 8 tool-операций.
- `CallToolResult` / `isError` остаются MCP-native protocol behavior.
- Business failure payload uses the shared command envelope.
- HTTP session capacity и execution admission являются разными guardrails.
- Shared EDT under MCP reuses the same execution model instead of inventing a separate MCP-only
  runtime path.
