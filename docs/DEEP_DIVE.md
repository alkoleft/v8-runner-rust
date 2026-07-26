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

- Для `format=DESIGNER` используется один логический Designer context на source-set.
- Для `format=EDT` используются два context-а:
  - EDT-source context для решения, нужен ли export;
  - generated-Designer context для решения, что именно грузить в ИБ.
- Каждый context изолирован по ИБ и source identity под `workPath/ib-state/v1`.
- Generated Designer output для EDT flow живёт под `workPath/designer/<sourceSetName>`.

Change detection выполняется on-demand во время build/export/load decision и не требует
background watcher. `build --source-set <NAME>` ограничивает анализ, export/load decision и
runtime snapshot commit только указанным source-set.
Legacy `workPath/hash-storages` намеренно не мигрируется: первое обращение к scoped state,
включая пустой source-set, является full bootstrap, а не `NoChanges`.

## Пайплайн `build`

Для `DESIGNER`:

1. Анализ изменений по выбранным `source-set`.
2. Выбор partial/full path по изменённым файлам.
3. Копирование managed source в private transaction; source `ConfigDumpInfo.xml`, symlinks и
   вложенный `workPath` исключаются, валидный private CDFI seed добавляется отдельно.
4. Загрузка через выбранный backend только из private staging.
5. Commit CDFI, baseline и source observation одной recoverable generation после успеха.

Для `EDT`:

1. Анализ выбранных EDT source-set.
2. Export затронутых EDT source-set в generated Designer representation.
3. Повторный анализ generated Designer files.
4. Load/apply generated files через `DESIGNER` или `IBCMD`.

Пайплайн намеренно не является атомарным across many `source-set`: поздний failure не откатывает
уже успешные ранние шаги.
Missing/corrupt private CDFI приводит к full bootstrap. Platform не получает живое source tree,
поэтому failed/cancelled build не создаёт и не изменяет source `ConfigDumpInfo.xml`.

## Проверка и тесты

`test` и `syntax` проектируются как часть того же локального цикла, а не как отдельная
эксплуатационная подсистема.

- `test` всегда сначала делает `build`, затем запускает YaXUnit или Vanessa Automation.
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
- Любой режим выполняется в private complete shadow; отсутствие matching baseline/private CDFI
  повышает incremental/partial request до одной full shadow operation.
- Публикация сравнивает baseline `B`, текущий source `S` и dump `D`; любой conflict или TOCTOU
  mismatch оставляет весь source-set и private generation неизменными.
- `B=absent, S=present, D=absent` является conflict, а не разрешением удалить локальный файл.
- Managed-file journal обеспечивает restart recovery. Forward recovery требует точной пары
  `(generation, UUID transaction token)`; совпавшая generation другой операции не считается успехом.
- Для `format=EDT` private platform shadow импортируется в private configured-source shadow до
  той же B/S/D publication; platform не пишет в project source или `workPath/designer/<name>`.
- Exact receipt lists — независимые audit dimensions: одинаковый target может быть одновременно
  `processed` и `skipped`, если он входил в effective platform scope, но merge сохранил/no-op
  локальный файл. Full сообщает полный managed result, incremental/partial — наблюдаемые записи
  private shadow.

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
