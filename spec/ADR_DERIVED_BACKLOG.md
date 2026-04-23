# Backlog задач по ADR

Документ сформирован 2026-04-21 по принятым ADR из `docs/decisions` с `ADR-0001` по `ADR-0020`, индексу `docs/decisions/README.md`, архитектурным инвариантам и разделу рисков arc42.

Результат ниже фиксирует не сами решения, а реализационные задачи и guardrail-задачи, которые должны попасть в активный backlog.

Приоритеты:

- `P0`: реализация сейчас расходится с принятым ADR или активный backlog не отражает реальное состояние.
- `P1`: известный migration gap или риск расхождения публичного контракта.
- `P2`: исследование, матрица поддержки или расширение покрытия тестами.
- `P3`: постоянные guardrails для будущих изменений.

## Сверка ADR с реализацией от 2026-04-22

Проверены принятые ADR `0001`-`0018`, `docs/architecture/invariants.md`, активный TODO и текущий код в `src`, `tests`, `examples`, `README.md`.

Подтвержденные gaps:

- `ADR-0012`: `SourceSetsService` уже создает два context-а для EDT (`edt-*` и `designer-*`), но `run_build_edt` принимает load-решение по EDT-анализу до export. Generated Designer context после успешного EDT export не анализируется, partial load для EDT flow не используется, а `designer-*` snapshot коммитится через EDT `StepCommit`.
- `ADR-0014`: общего command-level `execution_timeout`/deadline/cancellation context нет. `ExecutionContext` несет только EDT subprocess timeout, а MCP `server.rs` имеет отдельную bounded-модель и ранний running cancel/timeout response.
- `ADR-0018`: `docs/architecture/invariants.md` уже описывает `infobase.*` как supported contract, но `AppConfig`, `config init`, примеры и тестовые YAML продолжают использовать top-level `connection`/`credentials`; `IbcmdConnection` по-прежнему отклоняет server connection.
- `ADR-0017`: `config init` определяет source-set кандидаты частично по structure/path heuristics, а не только по marker filenames и содержимому; autodiscovery external aggregate source-set для `EXTERNAL_DATA_PROCESSORS` и `EXTERNAL_REPORTS` как часть supported config contract явно не реализован.
- `ADR-0016`: `ExecutionOutcome<T>` есть, но `ExecutionStatus::Cancelled` отсутствует, `StepResult` остается минимальным, а MCP/CLI mapping всё еще опирается на command-specific top-level поля.
- `ADR-0020`: repo-aware `convert [--source-set <name>] [--output <dir>]` уже реализован как CLI-only сценарий поверх `v8project.yaml`, а `dump format=EDT` реализован отдельным reverse-sync flow; `convert` не подменяет `dump`, `--output` является только target root с mirror layout относительно `basePath`, а public surface не принимает path-based direction/target flags.
- Guardrails частичные: есть `tests/use_case_boundaries.rs`, но он проверяет только небольшой список use-case файлов и не ловит, например, production-зависимость `src/use_cases/result.rs` от `crate::output::exit_codes`.

## Сводный список задач

### P0

1. `ADR-TASK-001`: Актуализировать активный backlog в `spec/IMPLEMENTATION_TODO.md`.

   Статус: выполнено 2026-04-21. Исторический TODO перенесен в `spec/archive/IMPLEMENTATION_TODO_2026-04-21.md`, активный `spec/IMPLEMENTATION_TODO.md` сокращен до открытых задач.

   Источники: `ADR-0002`, `ADR-0007`, `ADR-0012`, `ADR-0014`, `ADR-0016`, `docs/architecture/arc42/11-risks-and-technical-debt.md`.

   Объем: сверить старые открытые пункты EDT/MCP/runner-разделов с текущим кодом, закрытые пункты отметить `[x]`, реально открытые перенести в новый backlog по ADR, убрать двусмысленность между историческим планом и активным источником статуса.

   Готово, когда: `spec/IMPLEMENTATION_TODO.md` можно использовать как следующий рабочий backlog без чтения устаревших пунктов "Волна 2: EDT"; каждый открытый пункт имеет ссылку на ADR или task-артефакт.

2. `ADR-TASK-002`: Закрыть EDT two-state build pipeline.

   Источники: `ADR-0002`, `ADR-0012`.

   Объем: оформить EDT build как две независимые последовательные стадии. EDT stage принимает решение по `edt-<sourceSetName>`, при необходимости выполняет export и после успешного export коммитит только `edt-*` snapshot. Designer stage всегда анализирует generated Designer context `designer-<sourceSetName>` под `workPath/designer/<sourceSetName>`, независимо от результата EDT change analysis, потому что предыдущее load/apply могло быть отменено или сломано. Если Designer analysis находит изменения, выполняется Designer/IBCMD load/apply и после успеха коммитится `designer-*` snapshot; если изменений нет, Designer stage завершается skip. При ошибке на предыдущей стадии следующая стадия не запускается.

   Затронутые области: `src/use_cases/build_project.rs`, `src/change_detection/*`, unit/integration tests EDT build, `ARCHITECTURE.md`, риски arc42, публичная документация, если поведение видно пользователю.

   Готово, когда: после успешного EDT export коммитится `edt-*` snapshot; Designer analysis запускается всегда после успешной или skipped EDT stage; небольшой Designer diff ведет к partial load; `Configuration.xml`, deletion, unsafe expansion или threshold ведут к full load; отсутствие Designer diff ведет к skip; failure на EDT stage не запускает Designer stage; failure до successful load/apply не коммитит `designer-*` snapshot; tests покрывают Designer и IBCMD builders для EDT flow.

   Сверка 2026-04-22: gap подтвержден в `run_build_edt`: `analysis_by_name` строится только по `edt_contexts`, `partial_paths` в EDT execute branch игнорируется, `execute_source_set_step*` получает `commit_context=&edt_context` для load stage.

3. `ADR-TASK-003`: Ввести единую transport-neutral timeout/cancellation policy для CLI и MCP.

   Источники: `ADR-0013`, `ADR-0014`, `ADR-0016`.

   Объем: заменить MCP-only/special-case timeout model на общий execution policy contract. На первом этапе публичный contract содержит один общий `execution_timeout`; тонкие per-command overrides добавляются позже только при подтвержденной необходимости. Внутри policy также содержит cancellation token, deadline/remaining budget, interruption safety class и terminal-state semantics. Cancellation применяется на command boundary и safe points, без отдельной state machine на каждом pipeline step.

   Затронутые области: `src/use_cases/context.rs`, `src/cli/execute.rs`, `src/mcp/server.rs`, `src/platform/process.rs`, mutating use cases (`build`, `test`, `dump`, `artifacts`, `load`), CLI/MCP tests.

   Готово, когда: каждая public CLI/MCP команда получает `deadline`; queued cancellation не запускает работу; running cancellation/timeout возвращается наружу только после terminal state; critical DB/filesystem phases не hard-kill by default; deferred cancellation/shutdown во время successful critical phase возвращает `Succeeded` с warning; nested flows наследуют remaining budget.

   Сверка 2026-04-22: gap подтвержден: `ExecutionContext` содержит только `edt_timeout`, общего deadline/cancellation нет; MCP running cancellation/timeout в `McpToolServer::execute_tool` возвращает transport error до завершения worker, удерживая permit как transition mechanism.

4. `ADR-TASK-004`: Свести CLI EDT interactive execution к shared interactive режиму.

   Источники: `ADR-0007`.

   Объем: перенести shared EDT actor/manager из MCP-only boundary в `src/platform` или общий execution слой. MCP обращается к нему только через controller/DTO/presenter boundary и не содержит собственной execution-логики. Прямое создание non-shared interactive `1cedtcli` sessions в CLI use cases устраняется.

   Затронутые области: `src/use_cases/init_project.rs`, `src/use_cases/build_project.rs`, `src/use_cases/check_syntax.rs`, `src/platform/edt_session.rs`, `src/platform/edt.rs`, `src/platform/interactive.rs`, `src/mcp/edt_syntax.rs`, `src/mcp/server.rs`.

   Готово, когда: `tools.edt_cli.interactive_mode=false` использует one-shot execution; `true` маршрутизирует поддержанные EDT-сценарии через shared actor/manager; tests проверяют отсутствие третьего публичного execution path, baseline reset/probe, restart and drain semantics.

   Сверка 2026-04-22: выполнено: shared EDT actor/manager вынесен в `src/platform/edt_session.rs`, MCP использует его через thin adapter/boundary, а CLI `init`, EDT export в `build` и CLI `syntax edt` больше не создают `EdtDsl::new_interactive` в production path. Добавлены проверки на one-shot при `interactive_mode=false`, lazy CLI semantics при `auto_start=true`, reuse shared session и сохранение MCP queued/running timeout-cancel contract.

5. `ADR-TASK-011`: Согласовать `ADR-0010` с backlog и целевой CLI output policy.

   Статус: выполнено 2026-04-22.

   Источники: `ADR-0010`, `docs/architecture/invariants.md`, `ADR-TASK-007`.

   Решение: зафиксировать единый high-signal CLI output contract без audience/profile-оси; structured output выбирается булевым `--json-message`, а `--output` резервируется для user-facing output path flags; JSON contract не менять без отдельного решения.

   Готово, когда: `ADR-0010`, `docs/architecture/invariants.md`, `spec/ADR_DERIVED_BACKLOG.md`, `spec/IMPLEMENTATION_TODO.md` и CLI docs описывают одну модель output policy; tests для rendering cases привязаны к этой модели.

6. `ADR-TASK-008`: Реализовать `infobase` config contract и IBCMD server support.

   Источники: `ADR-0001`, `ADR-0003`, `ADR-0018`.

   Объем: перенести top-level `connection` и `credentials` в обязательную секцию `infobase`, добавить `infobase.dbms` для DBMS-level доступа `IBCMD`, убрать legacy aliases, обновить `config init`, docs, examples, validation и platform mapping.

   Готово, когда: старые top-level `connection`/`credentials` отклоняются; file connection для `IBCMD` использует `--db-path`; server connection для `IBCMD` использует `--dbms`, `--database-server`, `--database-name`, optional DBMS auth и отдельные `--user/--password` пользователя ИБ; Designer/Enterprise используют `infobase.connection` и `infobase.user/password`; docs и examples перешли на новый формат.

   Сверка 2026-04-22: gap подтвержден: `AppConfig` содержит top-level `connection`/`credentials`, `config init`, `README.md`, `examples/v8project.yaml`, `config::loader` fixtures и CLI tests продолжают использовать старый формат, `IbcmdConnection::from_v8_connection` возвращает `ServerConnectionNotSupported` для server connection. По критерию этого документа это уже P0, потому что код и public docs расходятся с accepted `ADR-0018` и `docs/architecture/invariants.md`.

7. `ADR-TASK-012`: Привести autodiscovery `config init` к content-based config contract.

   Источники: `ADR-0017`, `docs/architecture/invariants.md`, `docs/architecture/change-checklist.md`.

   Объем: убрать зависимость autodiscovery от имен каталогов и structure/layout heuristics. `CONFIGURATION` и `EXTENSION` должны определяться по marker filenames и содержимому marker-файлов; для `DESIGNER` и `EDT` это разные content formats. Для external types сохранить aggregate-root contract: один `source-set` на каталог внешних обработок и один `source-set` на каталог внешних отчетов. Для `DESIGNER` aggregate root определяется по однородным top-level XML descriptors, для `EDT` — по direct child projects одного external-kind, определяемого по содержимому проектных файлов. Mixed/ambiguous roots не должны autodetect-иться и остаются manual config case.

   Затронутые области: `src/use_cases/config_init.rs`, `tests/cli_config_init.rs`, unit tests autodiscovery/classification, `README.md`, `docs/CONFIGURATION.md`, при необходимости `docs/CAPABILITIES.md`.

   Готово, когда: `CONFIGURATION`/`EXTENSION` для `DESIGNER` определяются по содержимому `Configuration.xml`; `CONFIGURATION`/`EXTENSION` для `EDT` определяются по содержимому project-local markers, а не по path-name heuristics; `config init` умеет формировать `EXTERNAL_DATA_PROCESSORS` и `EXTERNAL_REPORTS` как aggregate source-set; EDT-internal markers не порождают ложные Designer candidates; mixed external roots не автогенерируются; regression coverage фиксирует nested sources, EDT-vs-Designer suppression и external aggregate detection.

   Сверка 2026-04-22: gap подтвержден: текущий `config init` по-прежнему частично опирается на structure/path heuristics для классификации source-set и не фиксирует external autodiscovery как реализованный supported contract.

### P1

7. `ADR-TASK-013`: Перевести `convert` на repo-aware contract из обновлённого `ADR-0020`.

   Источники: `ADR-0002`, `ADR-0005`, `ADR-0007`, `ADR-0011`, `ADR-0015`, `ADR-0017`, `ADR-0020`.

   Статус: выполнено 2026-04-22. Базовый этап перевёл `convert` на repo-aware public surface `convert [--source-set <name>]`; request/result contract теперь описывает scope и deterministic generated outputs, direction выводится только из `config.format`, default output без explicit `--output` публикуется под `workPath/convert/out/<sourceSetName>/<target-format>/`, а JSON validation/pre-dispatch errors сохраняют `command = "convert"`. Последующее user-facing расширение `--output` закрыто в `ADR-TASK-018`.

   Затронутые области: `src/cli/args.rs`, `src/cli/execute.rs`, `src/domain/convert.rs`, `src/use_cases/request.rs`, `src/use_cases/convert_sources.rs`, `tests/cli_convert.rs`, `README.md`, `docs/CAPABILITIES.md`, `docs/DEEP_DIVE.md`, `ARCHITECTURE.md`, arc42 summaries.

   Финальная проверка полноты субагентом вернула `APPROVED`: `convert` без аргументов обрабатывает все source-set текущего проекта, `convert --source-set <name>` обрабатывает только один source-set, direction определяется только из `config.format`, default output публикуется под `workPath/convert/out`, команда не может публиковать поверх `basePath` и исходных каталогов проекта, interactive-mode использует отдельный convert workspace, `DESIGNER -> EDT` extension использует реальное имя базового EDT-проекта из `.project`, external-only `format=DESIGNER` не требует configuration source-set, JSON validation/pre-dispatch errors сохраняют `command = "convert"`, а tests покрывают default scope, single source-set, single-extension fallback, inferred direction, output layout, busy workspace conflict, external EDT exports и safety rules. Explicit `--output` follow-up закрыт отдельным `ADR-TASK-018`.

8. `ADR-TASK-014`: Довести обратную синхронизацию `dump` до `format=EDT`.

   Источники: `ADR-0020`.

   Выполнено `2026-04-22`: `dump format=EDT` реализован как отдельный orchestration flow reverse sync из ИБ в EDT sources без подмены через `convert`: команда сначала обновляет internal Designer snapshot под `workPath/designer/<sourceSetName>`, затем импортирует его в EDT target через `1cedtcli` и публикует результат атомарной заменой каталога `source-set`. `partial`/`incremental` bootstrap-ят missing Designer snapshot, extension reverse sync выводит base project name из реального EDT configuration `.project`, а regression coverage закрывает Designer/IBCMD EDT flow и CLI path.

   Затронутые области: `src/use_cases/dump_config.rs`, domain/request model для `dump`, docs, `README.md`, `docs/CAPABILITIES.md`, `docs/DEEP_DIVE.md`, arc42 risks/decisions после фактической реализации.

   Финальная проверка полноты субагентом вернула `APPROVED`: `dump` для `format=EDT` поддерживается как отдельный сценарий, CLI/docs явно различают `dump` и `convert`, а реализация не использует `convert` как thin alias или hidden sub-step user-facing semantics.

9. `ADR-TASK-018`: Упростить публичный `convert` contract и исправить full-scope `DESIGNER -> EDT` для external source sets.

   Источники: `ADR-0020`.

   Выполнено `2026-04-23`: `convert` получил public surface `convert [--source-set <name>] [--output <dir>]`; direction по-прежнему выводится только из `config.format`, default output остается под `workPath/convert/out/<sourceSetName>/<target-format>/`, а явный `--output` трактуется только как target root и зеркалит `source-set.path` относительно `basePath`. Staged full replacement и workspace lock сохранены, публикация запрещает protected roots под `basePath`/`workPath`, пересечения со всеми source-set проекта и target-target overlap, а `DESIGNER -> EDT` импортируется во временный стабильный каталог, чтобы `.project` и base-project references не наследовали `.convert-stage-*` имя.

   Затронутые области: `src/cli/args.rs`, `src/cli/execute.rs`, `src/use_cases/request.rs`, `src/use_cases/convert_sources.rs`, `tests/cli_convert.rs`, `tests/cli_help.rs`, `README.md`, `docs/CAPABILITIES.md`, `docs/CONFIGURATION.md`, `docs/DEEP_DIVE.md`, `ARCHITECTURE.md`, arc42 summaries, `ADR-0010`, `ADR-0020`.

   Готово, когда: CLI help показывает `--output <DIR>` как target root; full-scope Designer-to-EDT конвертация external source sets публикует корректный mirror layout под explicit root; generated EDT project names стабильны; explicit output не может попасть в `basePath`, `workPath`, source-set path, filesystem root или overlapping target; docs/ADR и active backlog синхронизированы.

10. `ADR-TASK-005` / `ADR-TASK-027`: Закрыть follow-up gaps атомарной публикации.

   Источники: `ADR-0015`.

   Связь задач: `ADR-TASK-005` — исходный ADR-derived gap из `ADR-0015`;
   `ADR-TASK-027` — active delivery item из `spec/IMPLEMENTATION_TODO.md`, которым этот gap
   закрыт.

   Выполнено `2026-04-23`: full-replacement publication flow для `dump` и `artifacts`
   вынесен в `src/use_cases/staged_publication.rs`. Helper создает target-local staging
   file/dir и metadata sidecar, публикует через `run_no_process_critical_phase`, возвращает
   cleanup warning/deferred interruption, а cleanup остается явной политикой caller-а:
   `dump` чистит staging при pre-publish failure, `artifacts` сохраняет stage artifact как
   диагностический артефакт. `replace_dir_atomically` использует caller-specific backup prefix,
   external artifacts staging directory получает собственный metadata sidecar, stale cleanup
   остается metadata/identity-bound, а публичные `DumpResult`/`ArtifactsResult` контракты не
   изменены.

   Затронутые области: `src/use_cases/staged_publication.rs`,
   `src/use_cases/dump_config.rs`, `src/use_cases/artifacts.rs`,
   `src/support/fs.rs`, `docs/decisions/0015-*`, active TODO.

   Готово, когда: orphan cleanup удаляет только stale paths с matching target identity; external artifact staging directory не остается недоступным для безопасной cleanup-логики; tests покрывают stale/foreign/malformed/recent metadata.

   Проверено: helper tests покрывают file/dir prepare+publish, explicit cleanup policy,
   deferred interruption и отсутствие synthetic staging file; artifacts tests покрывают failure
   до появления staging file и interruption после появления staging file; dump/artifacts targeted
   suites проходят.

11. `ADR-TASK-006`: Довести `ExecutionOutcome<T>` и step contract до целевого состояния.

   Источники: `ADR-0016`, `ADR-0014`.

   Объем: сделать `ExecutionOutcome<T>` сериализованным source of truth для `test` и runner-like сценариев; сократить дублирование top-level `ok/message/path` там, где позволяет compatibility; добавить `ExecutionStatus::Cancelled` для фактической terminal cancellation; держать interruption diagnostics на уровне command outcome/result, а не каждого step; расширить `StepResult` или ввести `ExecutionStep` с kind/status/target/diagnostics/artifacts.

   Готово, когда: CLI JSON и MCP mapping tests проверяют outcome-driven status/errors/metrics/artifacts; новые runner-like сценарии стартуют от `ScenarioExecutionRequest` и `ExecutionOutcome<T>`; generic pipeline engine не вводится без повторяемой необходимости.

   Сверка 2026-04-22: gap подтвержден: `ExecutionStatus` не содержит `Cancelled`, `StepResult` имеет только `name/ok/duration_ms/message`, а MCP `map_test_response` собирает response из top-level полей `TestRunResult`.

12. `ADR-TASK-007`: Проработать CLI output как единый high-signal contract для человека и AI-агента.

   Источники: `ADR-0010`, `ADR-0006`.

   Объем: реализовать зафиксированную unified output policy и пересмотреть rendering rules по критериям ниже так, чтобы clean success path был кратким, а ошибки, warnings, degraded behavior, artifacts, affected target, diagnostic paths и next actionable hints были видны в обоих форматах без role-specific forks.

   Готово, когда: text output остается удобным для ручного запуска, но не шумит подробным успешным журналом без необходимости; JSON output остается стабильным structured contract для автоматизации; оба формата не скрывают ошибки, warnings, degraded behavior, artifacts и diagnostic paths; use case слой не знает presentation rules.

13. `ADR-TASK-020`: Разбить oversized orchestration use-case модули на сценарные coordinator-и и переиспользуемые policy/helper компоненты.

   Источники: `ADR-0006`, `ADR-0008`, `ADR-0016`.

   Объем: зафиксировать целевой рефакторинг для `build`, `dump`, `test` и shared EDT session path: отделить orchestration, platform dispatch, change-detection commit logic, publication/staging logic и rendering-oriented message assembly в самостоятельные stage/policy/helper компоненты.

   Готово, когда: новые backend/mode ветки добавляются через отдельные coordinator/stage/policy компоненты, а не через рост `src/use_cases/build_project.rs`, `src/use_cases/dump_config.rs`, `src/use_cases/run_tests.rs` и `src/platform/edt_session.rs`.

   Сверка 2026-04-23: review всего проекта подтвердил, что крупнейшие use-case/platform модули продолжают совмещать orchestration, platform execution, staging/publication, timeline/logging и error mapping в одном файле, что напрямую повышает стоимость сопровождения и риск регрессий.

14. `ADR-TASK-021`: Централизовать source-set/config classification и убрать дублирование XML/layout правил.

   Источники: `ADR-0017`.

   Объем: вынести общий typed classifier/parser для EDT/Designer source-set и external descriptors, который переиспользуют `config validate`, `config init`, `dump` reverse sync и external artifacts flow, включая project-name extraction и descriptor classification.

   Готово, когда: в репозитории есть один canonical implementation для project/descriptor classification и project-name extraction без копирования XML/layout логики между `config`, `use_cases` и вспомогательными flow.

   Сверка 2026-04-23: review показал дублирование XML/layout classification и project-name extraction между `src/config/validate.rs`, `src/use_cases/config_init.rs`, `src/use_cases/external_artifacts.rs` и `src/use_cases/dump_config.rs`.

15. `ADR-TASK-022`: Сузить transport adapters CLI/MCP и убрать дублирование normalization/mapping boundary.

   Источники: `ADR-0005`, `ADR-0006`, `ADR-0009`.

   Объем: свести request normalization, common validation, lock-boundary policy и failure shaping к общему adapter-neutral слою, чтобы CLI и MCP не дублировали одинаковые mapping rules, fallback response assembly и transport-neutral pre-validation.

   Готово, когда: добавление новой команды или флага не требует параллельно менять несколько почти одинаковых mapper-ов и fallback response builder-ов в `cli` и `mcp`, а use-case boundary остается transport-neutral.

   Сверка 2026-04-23: review подтвердил, что `src/cli/execute.rs`, `src/mcp/service.rs` и `src/mcp/port.rs` дублируют normalization, request mapping, failure shaping и lock-boundary orchestration поверх одних и тех же use case.

16. `ADR-TASK-023`: Перепроектировать syntax/launch request contract с boolean-heavy DTO на typed policy model.

   Источники: `ADR-0005`, `ADR-0006`.

   Объем: заменить large bool-struct-ы для syntax/launch на enum/struct policy objects, где client scopes, extension scope, extended-modules checks, modality/sync-call checks и launch target groups моделируются типами и constructors, а не набором флагов.

   Готово, когда: invalid combinations отсеиваются типами и constructors, а не ручной пост-валидацией в service layer; добавление нового режима расширяет typed policy model, а не truth table из взаимозависимых bool-полей.

   Сверка 2026-04-23: review показал, что текущие `cli`/`mcp`/`use_cases` request DTO для syntax и launch уже представляют собой large bool-struct-ы с ручной нормализацией и dependency checks.

17. `ADR-TASK-024`: Укрепить typed error contract и убрать string erasure на use-case boundary.

   Источники: `ADR-0009`, `ADR-0016`.

   Объем: эволюционировать `AppError` от `String`-категорий к typed variants/embedded source errors, сохранить различимость platform/runtime/validation subclasses для retry, diagnostics и telemetry, и отдельно разжать broad error mapping в test runner.

   Готово, когда: use case не теряют тип ошибки через `to_string()` в середине pipeline, а downstream mapping использует exhaustive match по error kind; `ProcessError`-варианты в enterprise test flow не схлопываются в один `EnterpriseSpawnFailed`.

   Сверка 2026-04-23: review подтвердил, что `AppError` остается stringly-typed, use-case слой рано erases typed source errors, а `run_tests` broad-match-ит разные `ProcessError` как один spawn failure.

18. `ADR-TASK-025`: Завершить migration на canonical `ExecutionOutcome<T>` и убрать legacy duplicated result fields.

   Статус: выполнено 2026-04-23.

   Источники: `ADR-0016`.

   Объем: для runner-like/result-heavy команд сделать `execution` единственным source of truth, а legacy top-level projections либо удалить, либо вычислять только на adapter boundary; не допускать расхождения между canonical outcome и дублирующими top-level полями.

   Готово, когда: domain result не допускает расхождения между `ok/status/diagnostics/report` и `execution`, а tests не фиксируют intentionally inconsistent state; адаптеры строят presentation-friendly projections поверх canonical outcome, а не наоборот.

   Выполнено 2026-04-23: `TestRunResult`, `ArtifactsResult` и `LoadResult` больше не хранят legacy top-level projections для статуса, diagnostics, artifacts/retained paths, parsed report, platform log и message, если эти данные уже представлены в `ExecutionOutcome<T>`; CLI/MCP строят compatibility projections на adapter boundary, а regression coverage фиксирует canonical serde shape и `load` success diagnostics message.

### P2

19. `ADR-TASK-009`: Усилить regression coverage platform locator.

   Источники: `ADR-0004`.

   Объем: проверить exact и mask selection `8.3`, `8.3.20`, `8.3.27.1789` для `1cv8`, `1cv8c`, `ibcmd`; проверить `tools.platform.path` как root/hint и стандартные корни поиска.

   Готово, когда: tests фиксируют выбор максимальной версии под маску и единый locator/facade для всех platform utilities; docs обновляются при изменении поддержанных форм version requirement.

   Выполнено 2026-04-22: в `src/platform/locator.rs` добавлена явная regression matrix для `1cv8`, `1cv8c` и `ibcmd` на exact/patch/minor requirements `8.3.27.1789`, `8.3.20`, `8.3`; отдельно зафиксированы `ibcmd` root-hint case для `tools.platform.path` и Linux default platform roots contract. В `src/platform/utilities.rs` добавлен facade-level regression test для `PlatformUtilities::from_config`, а legacy component map сохранён в `spec/archive/KEY_COMPONENTS_legacy.md`.

### P3

20. `ADR-TASK-010`: Добавить архитектурные guardrails для ADR-инвариантов.

    Источники: `ADR-0005`, `ADR-0006`, `ADR-0008`, `ADR-0009`, `ADR-0011`, `ADR-0017`.

    Объем: добавить легковесные проверки или review checklist для запрета зависимостей `src/use_cases` от CLI/MCP/output, запрета `std::process` вне `src/platform`, обязательного workspace lock на public command boundary, typed model/defaults/validation/docs для новых config fields, и явного ADR/update при изменении MCP surface.

    Готово, когда: новые public commands и config fields имеют понятный checklist; tests или статические проверки ловят самые частые нарушения; `docs/architecture/invariants.md` остается синхронизированным с ADR.

    Сверка 2026-04-22: guardrails частичные. `tests/use_case_boundaries.rs` проверяет только несколько файлов и не сканирует весь production `src/use_cases`; `src/use_cases/result.rs` импортирует `crate::output::exit_codes`, что противоречит инварианту о независимости use-case слоя от output.

## Критерии корректного CLI output

Эти критерии уточняют `ADR-TASK-007`: корректный CLI output дает минимальный достаточный сигнал для следующего действия. Structured output включается через `--json-message`, user-facing output path flags используют имя `--output`, отдельная audience/profile-ось не вводится, а JSON contract не меняется без отдельного решения.

1. Статус однозначен.

   Из вывода сразу понятно, команда завершилась успешно, failed, degraded, skipped или no-op. Статус не противоречит exit code, JSON `ok` и доменному `status`.

2. Clean success не шумит.

   Если команда прошла успешно без warnings, degraded behavior и важных artifacts, output остается коротким и не печатает подробный успешный timeline.

3. Warnings и degraded behavior нельзя скрывать.

   Любой fallback, degraded mode, cleanup warning, skipped significant step, rollback context или deferred cancellation/shutdown warning виден и в `text`, и в `json`.

4. Ошибка actionable.

   Ошибка содержит error code/class, command или stage, affected target, короткую причину, diagnostic path при наличии и следующий разумный шаг, если он не очевиден.

5. Artifacts и diagnostics указаны явно.

   Если команда создала файл, каталог, отчет, retained artifact или лог, output дает путь и kind результата. Пользователь или агент не должны искать результат вручную в `workPath`.

6. Raw stdout/stderr не является основным output.

   Сырой вывод платформы 1С сохраняется в logs. CLI показывает краткую диагностику, извлеченные structured issues и ссылку на полный лог.

7. JSON содержит machine contract, text содержит presentation.

   Машинно значимая информация есть в JSON. Text может быть удобнее для чтения, но не является единственным местом, где доступен важный факт.

8. Порядок и имена стабильны.

   JSON keys, error codes, warning codes, stage names и artifact kinds стабильны между запусками и командами.

9. Секреты не печатаются.

   Output не выводит пароли, токены, credentials и raw command line, если в ней могут быть секреты.

10. Нет противоречивого дублирования.

    Top-level `ok`, domain `status`, `execution.status` и `steps[].ok` не должны расходиться. Если одно и то же состояние можно выразить в одном canonical field, новые ad hoc поля не добавляются.

### Минимальные rendering cases

Для каждой public CLI command, где есть собственный result shape, нужны rendering tests на следующие случаи:

- clean success;
- success with artifact;
- warning/degraded success;
- validation error;
- platform/runtime failure;
- no changes или skipped path, если применимо.

## Трассировка ADR -> задачи

| ADR | Вывод после проработки | Задачи |
| --- | --- | --- |
| `ADR-0001` | Граница IBCMD остается ограниченной по сценариям, но server connection для реализованных IBCMD-сценариев закрывается через `infobase.dbms`. | `ADR-TASK-008` |
| `ADR-0002` | Целевое состояние требует двух context для EDT flow; текущему build нужна generated Designer analysis. | `ADR-TASK-001`, `ADR-TASK-002` |
| `ADR-0003` | Server infobase support для IBCMD требует явный DBMS-level config contract. | `ADR-TASK-008` |
| `ADR-0004` | Locator реализован, но ADR требует сопровождать mask behavior regression tests. | `ADR-TASK-009` |
| `ADR-0005` | MCP surface не должен меняться неявно. | `ADR-TASK-010`, `ADR-TASK-022`, `ADR-TASK-023` |
| `ADR-0006` | Use case layer остается transport-neutral. | `ADR-TASK-007`, `ADR-TASK-010`, `ADR-TASK-020`, `ADR-TASK-022`, `ADR-TASK-023` |
| `ADR-0007` | Shared EDT actor/manager вынесен в общий execution слой; direct non-shared interactive CLI path закрыт `2026-04-22`. | Закрыто `ADR-TASK-004` |
| `ADR-0008` | Граница platform DSL требует постоянных guardrails. | `ADR-TASK-010`, `ADR-TASK-020` |
| `ADR-0009` | Business/runtime failure split реализован как contract и требует защиты при добавлении новых tools. | `ADR-TASK-010`, `ADR-TASK-022`, `ADR-TASK-024` |
| `ADR-0010` | Для CLI output зафиксирована единая high-signal policy без отдельной audience-оси; naming policy `--json-message` / user-facing `--output` синхронизирована, открытой остаётся дальнейшая эволюция rendering rules. | `ADR-TASK-007` |
| `ADR-0011` | Workspace lock реализован; будущим public commands нужен lock guardrail. | `ADR-TASK-010` |
| `ADR-0012` | EDT generated Designer partial-load decision remains key gap. | `ADR-TASK-002` |
| `ADR-0013` | MCP admission/session capacity уже есть; cancellation/deadline должны идти через общую policy. | `ADR-TASK-003` |
| `ADR-0014` | Общая timeout/cancellation policy является целевой архитектурой и реализована не полностью. | `ADR-TASK-003`, `ADR-TASK-005` / `ADR-TASK-027`, `ADR-TASK-006` |
| `ADR-0015` | Atomic publication follow-ups закрыты delivery item `ADR-TASK-027`; новые full-replacement сценарии должны использовать общий staged-publication helper. | `ADR-TASK-005` / `ADR-TASK-027` |
| `ADR-0016` | `ExecutionOutcome<T>` есть частично; миграция к canonical outcome остается открытой. | `ADR-TASK-003`, `ADR-TASK-006`, `ADR-TASK-020`, `ADR-TASK-024`, `ADR-TASK-025` |
| `ADR-0017` | Config contract реализован частично; структура подключения ИБ уточнена отдельным ADR-0018, а content-based autodiscovery `config init` и external aggregate detection остаются отдельным gap. | `ADR-TASK-008`, `ADR-TASK-010`, `ADR-TASK-012`, `ADR-TASK-021` |
| `ADR-0018` | `infobase` становится единственным контрактом подключения, credentials и DBMS-level настроек. | `ADR-TASK-008` |
| `ADR-0020` | Repo-aware `convert [--source-set <name>] [--output <dir>]` и отдельный `dump format=EDT` reverse-sync flow реализованы как разные сценарии; `convert` не подменяет `dump`, а `--output` остается target-root contract без path-based direction/target flags. | `ADR-TASK-014`, `ADR-TASK-018` |

## Следующий рекомендуемый порядок

1. Следующим брать `ADR-TASK-020` как ближайший открытый архитектурный hotspot в orchestration use-case layers.
2. Затем `ADR-TASK-021` и `ADR-TASK-022`, чтобы снять основные дублирования в source-set classification и adapter boundary.
3. После этого идти в `ADR-TASK-023`, `ADR-TASK-024` и `ADR-TASK-025` как в следующий слой типобезопасности request/error/result contract.
4. Затем возвращаться к `ADR-TASK-016` и `ADR-TASK-026` как к product-contract и test-maintenance follow-up задачам.
