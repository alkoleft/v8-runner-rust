# ADR-0014: Единая timeout/cancellation policy для CLI и MCP команд

- Статус: `accepted`
- Дата: `2026-04-20`

## Контекст

Timeout и cancellation нужны не только MCP и не только `check_syntax_edt`.
CLI-команды, MCP tools и nested orchestration запускают одни и те же use cases и platform operations, поэтому разные правила прерывания на transport boundary создают некорректные ожидания.

Текущие риски:

1. timeout применяется точечно, а не ко всем public commands;
2. MCP может вернуть cancellation/timeout раньше, чем underlying platform work реально остановлен;
3. `Ctrl+C` в CLI и cancellation в MCP могут вести себя по-разному для одного use case;
4. hard kill во время обновления конфигурации ИБ или другой mutating critical phase может оставить базу в повреждённом или неизвестном состоянии;
5. use case слой не имеет единой модели deadline, cancellation request и interruption safety.

## Решение

Ввести единую command execution policy для CLI и MCP.

### Общий timeout contract

1. Каждая public CLI/MCP команда должна иметь execution deadline.
2. Должен существовать global default timeout и возможность per-command override.
3. Timeout budget покрывает весь command lifecycle: admission/queue wait, preparation, platform process, log collection, cleanup и result mapping.
4. `tools.edt_cli.command_timeout_ms` остаётся EDT-specific process timeout, но не заменяет общий command timeout.
5. Use case слой получает deadline через transport-neutral execution context, а не через MCP-specific поле.
6. Nested orchestration наследует остаток deadline от outer command.

### Общий cancellation contract

1. CLI `Ctrl+C`, MCP cancellation, server shutdown и internal timeout должны маппиться в единый cancellation/deadline signal.
2. Команда не считается cancelled/timed out наружу, пока underlying operation не доведена до terminal state.
3. Terminal state означает одно из:
- external process exited and was reaped;
- process был graceful-stopped and reaped;
- process был hard-killed and reaped;
- critical operation отказалась от unsafe interruption и дошла до собственного terminal outcome.
4. Adapter не должен возвращать "cancelled" только потому, что клиентский запрос был отменён, если platform operation всё ещё выполняется.
5. Cancellation request после начала running phase должен запускать controlled interruption flow, а не detached background work with completed response.

### Interruption safety classes

Каждая operation, которая может запускать external process или менять persistent state, должна иметь interruption safety class:

1. `Interruptible`: можно hard-kill по timeout/cancellation после короткого graceful attempt.
2. `GracefulThenKill`: сначала graceful stop, затем hard kill после grace period.
3. `CriticalNonAbortable`: после входа в critical phase hard kill запрещён по умолчанию.
4. `NoExternalProcess`: cancellation проверяется между вычислительными шагами без process kill.

### Critical phase для mutating operations

1. Операции, которые обновляют или применяют изменения к ИБ, должны явно помечать critical phase.
2. Для build/load/update DB critical phase начинается до запуска platform команды, которая мутирует ИБ, и заканчивается только после process exit, сбора логов и определения результата.
3. Если cancellation/timeout пришёл до critical phase, command может быть остановлен обычным способом.
4. Если cancellation/timeout пришёл внутри critical phase, runner не выполняет hard kill по умолчанию.
5. В critical phase cancellation/timeout становится recorded request, а команда ждёт terminal outcome platform operation.
6. Result должен явно сообщать, что cancellation/timeout был requested during critical phase, но unsafe interruption не выполнялся.
7. Если после critical phase состояние ИБ неизвестно, result должен содержать diagnostic location и actionable hint проверить/восстановить ИБ.
8. Unsafe hard kill critical operations требует отдельного явного режима и отдельного ADR или явного обновления этого ADR.

### Упрощённая модель cancellation representation

Чтобы не превращать use case pipeline в отдельный workflow engine, cancellation policy фиксируется на command boundary.
CLI/MCP adapters создают общий execution policy context, а use case слой проверяет cancellation/deadline только в safe points:

1. перед дорогим шагом;
2. между крупными platform steps;
3. перед запуском external process;
4. перед входом в publish/cleanup или другую critical phase.

Pipeline остаётся линейным. Отмена не добавляет отдельную state machine в каждый pipeline block.

Результат использует простые terminal statuses:

1. `Succeeded`;
2. `Failed`;
3. `TimedOut`;
4. `Cancelled`.

`Cancelled` используется только если command действительно завершилась отменой после terminal state underlying operation.
Если cancellation/shutdown/timeout был requested во время `CriticalNonAbortable` phase, но operation безопасно дошла до success, итог остаётся `Succeeded`; result добавляет warning/diagnostic, что interruption request был отложен до safe point.

Минимальная metadata об interruption допускается на уровне command outcome/result, а не на каждом step:

1. reason/source: CLI signal, MCP cancel, graceful shutdown или timeout;
2. `during_critical_phase`;
3. краткое message для человека и AI-агента.

## Неграницы (Non-goals)

1. Не гарантировать целостность ИБ после любой platform failure.
2. Не вводить default hard kill для update/apply DB operations.
3. Не заменять workspace lock из ADR-0011.
4. Не описывать HTTP session capacity; это MCP transport concern из ADR-0013.
5. Не требовать немедленного изменения всех команд в одном PR, но все новые команды должны следовать этому контракту.

## Последствия

1. Timeout/cancellation становятся частью transport-neutral execution contract.
2. Некоторые cancellation/timeout responses могут возвращаться позже, потому что runner обязан дождаться terminal state.
3. Для critical operations timeout становится soft deadline после входа в critical phase.
4. Platform execution layer должен поддерживать graceful stop, hard kill, process reaping и clear terminal outcome.
5. Use cases должны знать, где начинаются critical phases, но transport adapters не должны знать platform-specific details.
6. Result payloads и CLI/MCP rendering должны показывать итоговый terminal status и, если interruption был requested, минимальную metadata: reason/source, факт critical phase и actionable message.

## План реализации

Текущее состояние кода не полностью следует этому решению; ADR фиксирует целевую архитектуру.

1. Ввести transport-neutral execution policy model:
- общий публичный `execution_timeout` как стартовый timeout contract;
- per-command timeout override только после появления подтверждённой необходимости;
- cancellation token;
- deadline/remaining budget;
- interruption safety class.
2. Расширить `ExecutionContext` в `src/use_cases/context.rs`, чтобы CLI и MCP передавали одинаковый deadline/cancellation signal.
3. Обновить `src/cli/execute.rs`, чтобы CLI команды и `Ctrl+C` использовали общий cancellation flow.
4. Обновить `src/mcp/server.rs`, чтобы MCP cancellation/timeout не возвращали completed cancellation до terminal state.
5. Обновить platform process executor, чтобы он поддерживал controlled graceful stop, hard kill and reap semantics.
6. Разметить critical phases в mutating use cases:
- `src/use_cases/build_project.rs` для load/update DB steps;
- `src/use_cases/run_tests.rs`, если test setup запускает mutating build;
- другие use cases, которые применяют изменения к ИБ.
7. Для read-only или diagnostic операций выбрать `Interruptible` или `GracefulThenKill`.
8. Добавить tests:
- CLI timeout применяется к каждой public command;
- MCP timeout применяется к каждой public tool;
- cancellation during queued phase does not start work;
- cancellation during running interruptible phase waits for killed/reaped process;
- cancellation during critical DB update does not hard-kill by default and reports requested cancellation;
- cancellation/shutdown during critical phase may return `Succeeded` with warning when operation safely reaches success;
- nested orchestration inherits remaining deadline.
9. Обновить `README.md`, `docs/CAPABILITIES.md`, `ARCHITECTURE.md`, arc42 и invariants после реализации config/user-facing knobs.

## Верификация

- [x] ADR фиксирует, что timeout/cancellation относятся к CLI и MCP, а не только к MCP.
- [x] ADR фиксирует timeout для каждой public command.
- [x] ADR запрещает возвращать cancellation/timeout до terminal state underlying operation.
- [x] ADR вводит interruption safety classes.
- [x] ADR защищает critical mutating DB operations от default hard kill.
- [x] ADR фиксирует упрощённую cancellation representation: command-level policy, `ExecutionStatus::Cancelled` только для фактической terminal cancellation, warning для deferred cancellation внутри successful critical phase.
