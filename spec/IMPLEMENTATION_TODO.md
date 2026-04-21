# Активный TODO реализации `v8-runner`

Этот файл является коротким рабочим source of truth для следующих задач.

Исторический TODO до очистки сохранен в [spec/archive/IMPLEMENTATION_TODO_2026-04-21.md](archive/IMPLEMENTATION_TODO_2026-04-21.md).
Подробная декомпозиция ADR-задач находится в [spec/ADR_DERIVED_BACKLOG.md](ADR_DERIVED_BACKLOG.md).
Закрытый staged record MCP rollout остается в [spec/MCP_IMPLEMENTATION_PLAN.md](MCP_IMPLEMENTATION_PLAN.md) и не используется как активный backlog без явного запроса.

## Правила ведения

- Держать здесь только открытые задачи или короткие ссылки на активные детализации.
- После закрытия задачи отмечать ее `[x]` только на время текущего delivery loop, затем переносить детали в профильный archive/spec/history-документ.
- Если задача меняет архитектурный контракт, сначала обновлять или добавлять ADR, затем синхронизировать `docs/architecture/invariants.md`, arc42 и публичную документацию.
- Для реализации брать следующий конкретный пункт сверху вниз, если пользователь не указал другой приоритет.

## P0

- [ ] `ADR-TASK-002`: Закрыть EDT two-state build pipeline по `ADR-0002` и `ADR-0012`: EDT stage после successful export коммитит `edt-*` snapshot, Designer stage всегда анализирует `designer-<sourceSetName>` context, выполняет load/apply только при изменениях и коммитит `designer-*` snapshot только после successful load/apply.
- [ ] `ADR-TASK-003`: Ввести единую transport-neutral policy таймаутов и отмены по `ADR-0013` и `ADR-0014`: общий публичный `execution_timeout`, `deadline` для каждой CLI/MCP команды, семантика terminal-state, классы interruption safety, command-boundary cancellation без state machine на каждом step, deferred warning при successful critical phase и наследование budget во вложенных сценариях.
- [ ] `ADR-TASK-004`: Свести CLI EDT interactive execution к shared interactive режиму по `ADR-0007`: перенести shared EDT actor/manager в `src/platform` или общий execution слой, а MCP оставить controller/DTO/presenter boundary без собственной execution-логики.

## P1

- [ ] `ADR-TASK-005`: Закрыть follow-up gaps атомарной публикации по `ADR-0015`: neutral/caller-specific backup prefix, metadata sidecar на cleanup unit для staging directory внешних артефактов, `CriticalNonAbortable` publication phase после общей execution policy, cleanup warning для agent-oriented output.
- [ ] `ADR-TASK-006`: Довести `ExecutionOutcome<T>` и step contract до целевого состояния по `ADR-0016`: outcome-driven serialized status/errors/metrics/artifacts, `ExecutionStatus::Cancelled` для фактической terminal cancellation, command-level interruption diagnostics, richer `ExecutionStep` или расширенный `StepResult`.
- [ ] `ADR-TASK-007`: Проработать CLI output по `ADR-0010` как единый high-signal contract для человека и AI-агента без отдельного audience-параметра: сохранить только ось `--output text|json`, применить критерии корректного output из `spec/ADR_DERIVED_BACKLOG.md`, убрать лишний шум из clean success path, явно показывать warnings/degraded/artifacts/diagnostics и покрыть rendering tests.

## P2

- [ ] `ADR-TASK-008`: Реализовать новый `infobase` config contract по `ADR-0018` и закрыть IBCMD server support: перенести `connection`/`credentials` в `infobase`, добавить `infobase.dbms`, убрать legacy top-level keys, покрыть Designer/Enterprise и IBCMD file/server mapping тестами.
- [ ] `ADR-TASK-009`: Усилить regression coverage platform locator по `ADR-0004`: exact/mask selection `8.3`, `8.3.20`, `8.3.27.1789` для `1cv8`, `1cv8c`, `ibcmd`, `tools.platform.path` как root/hint и стандартные корни поиска.
- [ ] Добавить CI workflow wiring из `spec/REAL_ENV_TEST_PLAN.md`: установка 1С на GitHub-hosted runner'ах, bootstrap файловой ИБ через `ibsrv`, trusted/fork gating и upload deploy-ready артефактов.

## P3

- [ ] `ADR-TASK-010`: Добавить архитектурные guardrails для ADR-инвариантов (`ADR-0005`, `ADR-0006`, `ADR-0008`, `ADR-0009`, `ADR-0011`, `ADR-0017`, `ADR-0018`): границы зависимостей use case, platform DSL boundary, workspace lock boundary, validation/docs для config contract, checklist изменения MCP surface.
