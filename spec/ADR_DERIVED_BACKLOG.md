# Backlog задач по ADR

Документ сформирован 2026-04-21 по принятым ADR из `docs/decisions` с `ADR-0001` по `ADR-0018`, индексу `docs/decisions/README.md`, архитектурным инвариантам и разделу рисков arc42.

Результат ниже фиксирует не сами решения, а реализационные задачи и guardrail-задачи, которые должны попасть в активный backlog.

Приоритеты:

- `P0`: реализация сейчас расходится с принятым ADR или активный backlog не отражает реальное состояние.
- `P1`: известный migration gap или риск расхождения публичного контракта.
- `P2`: исследование, матрица поддержки или расширение покрытия тестами.
- `P3`: постоянные guardrails для будущих изменений.

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

3. `ADR-TASK-003`: Ввести единую transport-neutral timeout/cancellation policy для CLI и MCP.

   Источники: `ADR-0013`, `ADR-0014`, `ADR-0016`.

   Объем: заменить MCP-only/special-case timeout model на общий execution policy contract. На первом этапе публичный contract содержит один общий `execution_timeout`; тонкие per-command overrides добавляются позже только при подтвержденной необходимости. Внутри policy также содержит cancellation token, deadline/remaining budget, interruption safety class и terminal-state semantics. Cancellation применяется на command boundary и safe points, без отдельной state machine на каждом pipeline step.

   Затронутые области: `src/use_cases/context.rs`, `src/cli/execute.rs`, `src/mcp/server.rs`, `src/platform/process.rs`, mutating use cases (`build`, `test`, `dump`, `artifacts`, `load`), CLI/MCP tests.

   Готово, когда: каждая public CLI/MCP команда получает `deadline`; queued cancellation не запускает работу; running cancellation/timeout возвращается наружу только после terminal state; critical DB/filesystem phases не hard-kill by default; deferred cancellation/shutdown во время successful critical phase возвращает `Succeeded` с warning; nested flows наследуют remaining budget.

4. `ADR-TASK-004`: Свести CLI EDT interactive execution к shared interactive режиму.

   Источники: `ADR-0007`.

   Объем: перенести shared EDT actor/manager из MCP-only boundary в `src/platform` или общий execution слой. MCP обращается к нему только через controller/DTO/presenter boundary и не содержит собственной execution-логики. Прямое создание non-shared interactive `1cedtcli` sessions в CLI use cases устраняется.

   Затронутые области: `src/use_cases/init_project.rs`, `src/use_cases/build_project.rs`, `src/use_cases/check_syntax.rs`, `src/mcp/edt_session.rs`, `src/platform/edt.rs`, `src/platform/interactive.rs`.

   Готово, когда: `tools.edt_cli.interactive_mode=false` использует one-shot execution; `true` маршрутизирует поддержанные EDT-сценарии через shared actor/manager; tests проверяют отсутствие третьего публичного execution path, baseline reset/probe, restart and drain semantics.

### P1

5. `ADR-TASK-005`: Закрыть follow-up gaps атомарной публикации.

   Источники: `ADR-0015`.

   Объем: сделать backup prefix в `replace_dir_atomically` neutral/caller-specific или явно internal; ставить metadata sidecar на cleanup unit для external artifacts staging directory; после `ADR-TASK-003` пометить publication phase как `CriticalNonAbortable`; сохранить cleanup warning/degraded message в agent-oriented output.

   Готово, когда: orphan cleanup удаляет только stale paths с matching target identity; external artifact staging directory не остается недоступным для безопасной cleanup-логики; tests покрывают stale/foreign/malformed/recent metadata.

6. `ADR-TASK-006`: Довести `ExecutionOutcome<T>` и step contract до целевого состояния.

   Источники: `ADR-0016`, `ADR-0014`.

   Объем: сделать `ExecutionOutcome<T>` сериализованным source of truth для `test` и runner-like сценариев; сократить дублирование top-level `ok/message/path` там, где позволяет compatibility; добавить `ExecutionStatus::Cancelled` для фактической terminal cancellation; держать interruption diagnostics на уровне command outcome/result, а не каждого step; расширить `StepResult` или ввести `ExecutionStep` с kind/status/target/diagnostics/artifacts.

   Готово, когда: CLI JSON и MCP mapping tests проверяют outcome-driven status/errors/metrics/artifacts; новые runner-like сценарии стартуют от `ScenarioExecutionRequest` и `ExecutionOutcome<T>`; generic pipeline engine не вводится без повторяемой необходимости.

7. `ADR-TASK-007`: Проработать CLI output как единый high-signal contract для человека и AI-агента.

   Источники: `ADR-0010`, `ADR-0006`.

   Объем: не добавлять отдельный `--audience`; оставить публичной осью только `--output text|json`, но пересмотреть rendering rules по критериям ниже так, чтобы clean success path был кратким, а ошибки, warnings, degraded behavior, artifacts, affected target, diagnostic paths и next actionable hints были видны и человеку, и агенту.

   Готово, когда: text output остается удобным для ручного запуска, но не шумит подробным успешным журналом без необходимости; JSON output остается стабильным structured contract для автоматизации; оба формата не скрывают ошибки, warnings, degraded behavior, artifacts и diagnostic paths; use case слой не знает presentation rules.

### P2

8. `ADR-TASK-008`: Реализовать `infobase` config contract и IBCMD server support.

   Источники: `ADR-0001`, `ADR-0003`, `ADR-0018`.

   Объем: перенести top-level `connection` и `credentials` в обязательную секцию `infobase`, добавить `infobase.dbms` для DBMS-level доступа `IBCMD`, убрать legacy aliases, обновить `config init`, docs, examples, validation и platform mapping.

   Готово, когда: старые top-level `connection`/`credentials` отклоняются; file connection для `IBCMD` использует `--db-path`; server connection для `IBCMD` использует `--dbms`, `--database-server`, `--database-name`, optional DBMS auth и отдельные `--user/--password` пользователя ИБ; Designer/Enterprise используют `infobase.connection` и `infobase.user/password`; docs и examples перешли на новый формат.

9. `ADR-TASK-009`: Усилить regression coverage platform locator.

   Источники: `ADR-0004`.

   Объем: проверить exact и mask selection `8.3`, `8.3.20`, `8.3.27.1789` для `1cv8`, `1cv8c`, `ibcmd`; проверить `tools.platform.path` как root/hint и стандартные корни поиска.

   Готово, когда: tests фиксируют выбор максимальной версии под маску и единый locator/facade для всех platform utilities; docs обновляются при изменении поддержанных форм version requirement.

### P3

10. `ADR-TASK-010`: Добавить архитектурные guardrails для ADR-инвариантов.

    Источники: `ADR-0005`, `ADR-0006`, `ADR-0008`, `ADR-0009`, `ADR-0011`, `ADR-0017`.

    Объем: добавить легковесные проверки или review checklist для запрета зависимостей `src/use_cases` от CLI/MCP/output, запрета `std::process` вне `src/platform`, обязательного workspace lock на public command boundary, typed model/defaults/validation/docs для новых config fields, и явного ADR/update при изменении MCP surface.

    Готово, когда: новые public commands и config fields имеют понятный checklist; tests или статические проверки ловят самые частые нарушения; `docs/architecture/invariants.md` остается синхронизированным с ADR.

## Критерии корректного CLI output

Эти критерии уточняют `ADR-TASK-007`: корректный CLI output дает минимальный достаточный сигнал для следующего действия и не требует отдельного `--audience`.

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
| `ADR-0005` | MCP surface не должен меняться неявно. | `ADR-TASK-010` |
| `ADR-0006` | Use case layer остается transport-neutral. | `ADR-TASK-007`, `ADR-TASK-010` |
| `ADR-0007` | Non-shared interactive EDT in CLI is documented implementation gap. | `ADR-TASK-004` |
| `ADR-0008` | Граница platform DSL требует постоянных guardrails. | `ADR-TASK-010` |
| `ADR-0009` | Business/runtime failure split реализован как contract и требует защиты при добавлении новых tools. | `ADR-TASK-010` |
| `ADR-0010` | CLI output должен одновременно быть удобным человеку и достаточно кратким/предсказуемым для AI-агента без добавления отдельного audience-параметра. | `ADR-TASK-007` |
| `ADR-0011` | Workspace lock реализован; будущим public commands нужен lock guardrail. | `ADR-TASK-010` |
| `ADR-0012` | EDT generated Designer partial-load decision remains key gap. | `ADR-TASK-002` |
| `ADR-0013` | MCP admission/session capacity уже есть; cancellation/deadline должны идти через общую policy. | `ADR-TASK-003` |
| `ADR-0014` | Общая timeout/cancellation policy является целевой архитектурой и реализована не полностью. | `ADR-TASK-003`, `ADR-TASK-005`, `ADR-TASK-006` |
| `ADR-0015` | Atomic publication в основном есть, но остаются явные cleanup/prefix/critical-phase follow-ups. | `ADR-TASK-005` |
| `ADR-0016` | `ExecutionOutcome<T>` есть частично; миграция к canonical outcome остается открытой. | `ADR-TASK-003`, `ADR-TASK-006` |
| `ADR-0017` | Config contract реализован частично; структура подключения ИБ уточнена отдельным ADR-0018. | `ADR-TASK-008`, `ADR-TASK-010` |
| `ADR-0018` | `infobase` становится единственным контрактом подключения, credentials и DBMS-level настроек. | `ADR-TASK-008` |

## Следующий рекомендуемый порядок

1. Закрыть `ADR-TASK-002`, потому что это конкретное расхождение с accepted ADR по EDT flow.
2. После этого планировать `ADR-TASK-003` как отдельный крупный этап: он затрагивает CLI, MCP, process execution и доменную модель результата.
3. Затем брать `ADR-TASK-004`, чтобы убрать оставшийся non-shared interactive EDT path из CLI.
