# Поэтапный план реализации MCP

## Summary

- Добавить в текущий бинарь отдельные режимы `v8-test-runner mcp serve stdio` и `v8-test-runner mcp serve http`.
- Реализовать MCP поверх `rmcp`, сохранив 8 tool-методов и входную семантику из `kotlin-example/.../McpServer.kt`.
- Не переиспользовать CLI `Envelope` и `clap`-типы в MCP. Ввести transport-neutral request/service слой и отдельные MCP result structs.
- Включить в MCP-этап недостающий функциональный gap: `dump_config` в режиме `PARTIAL`.
- Для MCP path добавить shared EDT interactive session как bounded actor; CLI path оставить на текущем one-shot поведении.

## Stage 1. Foundation And Contract Layer

- [x] 2026-03-20: Ввести transport-neutral request/result слой и `ExecutionContext`, чтобы use case-ы не зависели от `clap`, `Presenter` и `Envelope`.
  - Добавлены transport-neutral request DTO, `ExecutionContext`, shared use-case failure contract и CLI adapter boundary.
  - CLI rendering сохранен в `cli::execute`, а bootstrap error rendering оставлен в `app.rs`.
- [x] 2026-03-20: Добавить MCP-facing service layer с явными structured business failures.
  - Добавлен внутренний модуль `src/mcp` с request/response DTO, `McpCallContext`, `McpUseCasePort` и `McpService`.
  - MCP boundary теперь возвращает либо typed success payload, либо `McpBusinessFailure<T>` с machine-readable error code, либо отдельный `McpInternalError`.
  - Raw MCP defaults и alias normalization изолированы в service-layer mapper-ах с явными `TODO(mcp-normalization-stage)`.
  - MCP response structs отвязаны от domain nested DTO, чтобы будущий transport adapter не зависел от внутренних сериализационных деталей.
- [x] 2026-03-20: Расширить config для MCP transport defaults и EDT timeout knobs.
  - Добавлены typed config-секции `mcp.http` и `mcp.execution` с defaults `127.0.0.1:3000`, `/mcp`, `stateful_sessions=true`, `max_sessions=64`, `idle_ttl_secs=900`, `max_concurrent_calls=1`, `shutdown_grace_period_secs=30`.
  - `tools.edt_cli` теперь поддерживает `startup_timeout_ms` и `command_timeout_ms` с default `300_000 ms`; сохранена совместимость с `startup-timeout-ms` и `command-timeout-ms`.
  - Добавлена pre-runtime validation для bind address, MCP HTTP path, positive session/concurrency/grace/timeout limits и обновлены example/docs.
- [x] 2026-03-20: Реализовать contract normalization в MCP service layer.
  - Зафиксирован tri-state для `allExtensions`: blank `extension` трактуется как отсутствие значения, default зависит от наличия `extension`, явный `allExtensions=true` сохраняется как Kotlin-compatible behavior.
  - Добавлен pre-validation для `checkUseSynchronousCalls` и `checkUseModality`: при `extendedModulesCheck=false` MCP boundary возвращает `InvalidArgument` до вызова use case.
  - Зафиксирован alias set для `launch_app`, включая Kotlin aliases и ранее принятые `thin` / `thick`; trim + lowercase normalization сохранены.
  - Осознанное product-расхождение теперь задокументировано и покрыто тестами: `dump_config(mode=null|blank)` в MCP трактуется как `INCREMENTAL`.
- [x] 2026-03-20: Закрыть functional gap: `dump_config(PARTIAL)` для `DESIGNER`; для `IBCMD` сделать degraded fallback с warning и сохранением requested mode `PARTIAL`.
  - `DESIGNER` partial dump теперь использует `DumpConfigToFiles -partial -listFile ... -updateConfigDumpInfo` для configuration и extension targets.
  - `IBCMD` partial dump теперь валидирует тот же object list contract, затем деградирует в incremental export (`--sync`) по resolved target и возвращает warning, сохраняя `mode=PARTIAL` в use-case/MCP payload.
  - Добавлены regression tests на object normalization, temp-list cleanup, missing-target creation и `IBCMD` success/failure fallback semantics.

## Stage 2. MCP stdio MVP

- [x] 2026-03-20: Добавить `v8-test-runner mcp serve stdio`.
  - CLI surface расширен nested-командой `mcp serve stdio`, а `app.rs` получил отдельный bootstrap path без CLI presenter/json envelope.
  - MCP bootstrap errors теперь печатаются в `stderr`, а обычный CLI bootstrap остался без изменений.
- [x] 2026-03-20: Поднять `rmcp` tool server только с tools-capability, без resources и prompts.
  - Добавлен `src/mcp/server.rs` с rmcp stdio transport adapter и `ServerCapabilities::enable_tools()` без resources/prompts.
  - Реальный `tools/list` contract покрыт интеграционным тестом через child-process rmcp client.
- [x] 2026-03-20: Опубликовать все 8 tools через MCP adapter поверх нового service layer.
  - Через stdio публикуются `run_all_tests`, `run_module_tests`, `build_project`, `dump_config`, `launch_app`, `check_syntax_edt`, `check_syntax_designer_config`, `check_syntax_designer_modules`.
  - Tool inputs теперь сериализуются через MCP DTO с `camelCase` schema/serde mapping, а business failures возвращаются как structured tool error payloads поверх `McpToolResult<T>`.
- [x] 2026-03-20: Зафиксировать правило `stdout reserved for MCP`.
  - MCP stdio path инициализирует file-only action logging через `workPath/logs/mcp/actions.log`, не пишет tracing в `stdout` и ставит explicit panic hook в `stderr`.
  - Integration test на реальном stdio transport подтверждает, что initialize/tools-list/tool-call handshake проходит без загрязнения MCP stdout.
- [x] 2026-03-20: Добавить bounded execution через semaphore и per-call timeout/cancel semantics для MCP path.
  - Все MCP tool calls теперь проходят через глобальный semaphore на базе `mcp.execution.max_concurrent_calls`; queued и running client cancellation возвращаются раньше завершения blocking job для всех tools.
  - Для bounded Stage 2 call-ов дедлайн считается от момента входа запроса в MCP wrapper, поэтому queue wait и execution делят один budget; на этом этапе budget применяется только к `check_syntax_edt` через `tools.edt_cli.command_timeout_ms`.
  - Transport error contract зафиксирован как `reason=cancelled|timeout`, `stage=queued|running`, `timeoutMs=null|<budget>`; ранний возврат не убивает уже стартовавший one-shot subprocess, а detached worker удерживает permit до фактического завершения.
  - Для rmcp cancellable request handles клиентский API может завершаться локальным `Cancelled { reason }` после отправки `notifications/cancelled`; зафиксированная выше матрица описывает server-side transport payload.
  - Добавлены unit и stdio integration tests на mixed-tool admission, queued/running timeout, running cancellation через rmcp cancellable request и regression, что обычные non-EDT tools не получают running timeout от EDT config knob.
- На этом этапе EDT tools могут работать через текущий one-shot path, но уже через новый MCP adapter.

## Stage 3. Shared EDT Session For MCP

- [x] 2026-03-20: Stage 3 завершен целиком.
  - Весь scoped объем этапа закрыт: low-level interactive executor, shared EDT actor, baseline/reset contract, restart/shutdown drain semantics и переключение live MCP `check_syntax_edt` на shared session при сохранении CLI EDT path в one-shot режиме.
- [x] 2026-03-20: Реализовать `InteractiveProcessExecutor` для `1cedtcli`.
  - Добавлен `src/platform/interactive.rs` с prompt-delimited executor, который стартует отдельный process group, ждёт `1C:EDT>` на `stdout` или `stderr`, посылает команды в `stdin` и читает ответ до следующего prompt.
  - Зафиксирован lifecycle contract: startup/command timeout убивает process group и poison-ит executor, mid-command child exit возвращается сразу как `ProcessExited`, graceful shutdown закрывает `stdin` и эскалирует в forced kill при таймауте.
  - Добавлены unit tests на startup prompt, stderr prompt, split prompt, prompt с завершающим newline, repeated command reuse, timeout/poison semantics, prompt-then-exit detection, stdio disconnect cleanup, shutdown escalation и process-group kill для дочерних процессов.
- [x] 2026-03-20: Добавить `EdtSessionManager` как single shared actor для MCP mode.
  - Добавлен `src/mcp/edt_session.rs` с async facade над выделенным worker thread, который лениво поднимает один `InteractiveProcessExecutor` и тем самым обеспечивает single-flight startup и FIFO execution.
  - Bounded admission привязан к уже валидируемому `mcp.execution.max_concurrent_calls`; queue wait и execution делят один absolute deadline от enqueue-time, queued cancel/timeout физически удаляет запись из внутренней очереди, а running cancellation остаётся cooperative early-return без прерывания уже стартовавшей команды.
  - Зафиксирован typed error model: `queue_full`, queued/running `cancelled`, queued/running `timeout`, `startup_failed`, `session_failed`, `drained_by_restart_or_shutdown`, `internal_failure`.
  - Timeout/hang и другие fatal session errors poison/restart-ят shared session: текущий запрос получает typed failure, pending queue drain-ится единым `drained_by_restart_or_shutdown(reason=restart)`, а lazy restart происходит только на следующем запросе.
  - Добавлены unit tests на single-flight startup, FIFO, bounded admission, queued/running cancel+timeout, startup/session failure restart, non-retry semantics и shutdown drain behavior.
- [x] 2026-03-20: Перед каждой EDT-командой делать baseline/reset check, чтобы не было межсессионной утечки интерактивного состояния.
  - Shared EDT actor теперь делает двухшаговый reset+probe перед каждым user command: `cd <edt-workspace>` и затем `cd`, который обязан вернуть тот же workspace path.
  - Зафиксирован success contract baseline-check: reset/probe не должны вернуть `stderr`, а probe обязан подтвердить ожидаемый workspace с path-normalized сравнением; mismatch или любой process/protocol failure считаются fatal session fault с restart+queue drain.
  - Зафиксирован split timeout semantics: request-budget exhaustion во время baseline даёт `QueuedTimeout` без restart, а timeout baseline под внутренним cap при ещё живом request budget считается fatal session fault.
  - Добавлены unit tests на reset-before-each-command, normalized probe success, probe mismatch, fatal-cap timeout, budget timeout и cancellation during baseline.
- [x] 2026-03-20: Во время shutdown или restart queued jobs отменять сразу единым business error.
  - Shared EDT actor дренирует pending queue единым typed failure `drained_by_restart_or_shutdown(reason=restart|shutdown)`; это покрыто unit tests и теперь используется live MCP EDT path.
- [x] 2026-03-20: MCP path переключить на shared EDT actor; CLI path оставить без изменений.
  - Добавлен `src/mcp/edt_syntax.rs`, через который live MCP `check_syntax_edt` выполняется поверх shared `EdtSessionManager`, сохраняя текущий CLI EDT path на one-shot `EdtDsl`.
  - `src/mcp/server.rs` теперь special-case-ит `check_syntax_edt`, инициализирует shared actor через typed bootstrap error path и переиспользует MCP request normalization/response mapping из service layer без дублирования contract logic.
  - Running cancel/timeout для live EDT path теперь удерживают не только actor admission, но и внешний MCP permit до фактического завершения интерактивной команды; follow-up requests больше не проваливаются в spurious `QueueFull`.
  - Actor-backed EDT status mapping больше не считает `stdout + parseable issues` фатальной ошибкой: `--file` issues сохраняют `issues_found`, а `stdout` без parseable issues и любой `stderr` остаются сигналом tool/session failure.
  - Добавлены stdio integration tests на live actor-backed EDT syntax path, transport timeout/queue timeout regressions, reset между двумя последовательными вызовами, running cancel capacity retention, `stdout`+issues classification и `stdout`-only fallback failure.

## Stage 4. HTTP Transport

- [x] 2026-03-20: Добавить `v8-test-runner mcp serve http`.
  - CLI surface расширен nested-командой `mcp serve http`, а `app.rs` получил общий MCP bootstrap path для stdio/HTTP без CLI presenter/json envelope.
  - MCP HTTP bootstrap errors, как и stdio path, печатаются в `stderr`, а action logging остаётся file-only в `workPath/logs/mcp/actions.log`.
- [x] 2026-03-20: Поднять `axum` + `rmcp` streamable HTTP transport.
  - `src/mcp/server.rs` теперь содержит transport-aware `McpToolServer`, который используется и для stdio, и для HTTP, сохраняя общий `McpService`, global semaphore и shared `EdtSessionManager`.
  - Поверх `rmcp::transport::StreamableHttpService` добавлен thin HTTP wrapper, который перехватывает только transport-level admission cases: `stateful` non-`initialize` POST без `Mcp-Session-Id` возвращает deterministic `400`, а переполнение `max_sessions` на новом `initialize` возвращает `503 Service Unavailable`.
- [x] 2026-03-20: Явно зафиксировать HTTP defaults.
  - Defaults остаются источником правды для live HTTP transport и задокументированы в typed config/model validation, `examples/application.yaml` и `README.md`.
  - `bind_address=127.0.0.1:3000`
  - `path=/mcp`
  - `stateful_sessions=true`
  - `max_sessions=64`
  - `idle_ttl_secs=900`
- [x] 2026-03-20: Проверить session semantics.
  - Stateful HTTP contract зафиксирован тестами на живом binary: `POST initialize` выдаёт `Mcp-Session-Id`, `notifications/initialized` возвращает `202`, `GET/DELETE` без session дают `400`, а unknown/expired/deleted session IDs дают `404`.
  - `stateful_sessions=false` теперь покрыт integration tests как POST-only mode без session header; `GET/DELETE` в этом режиме возвращают `405`, а `Accept`/`Content-Type` preconditions валидируются transport layer-ом.
  - Session capacity теперь держится через atomic reservation lifecycle `reserve -> delegate initialize -> confirm/release` с lazy pruning expired rmcp sessions, поэтому `max_sessions` корректно освобождается после `DELETE`, TTL expiry и failed initialize.
- [x] 2026-03-20: Shared EDT actor используется и для HTTP, без создания отдельных EDT processes per MCP session.
  - Две разные HTTP MCP sessions теперь переиспользуют один интерактивный `1cedtcli` process и общий `mcp.execution.max_concurrent_calls` budget; это подтверждено integration test-ом на actor reuse и shared capacity serialization.

## Stage 5. Hardening And Docs

- [x] 2026-03-20: Добавить stress и regression suite.
  - MCP `list_tools` contract теперь дополнительно фиксируется schema-level assertions на ключевые `camelCase` поля и required args поверх stdio/HTTP listing tests.
  - Stdio integration suite теперь покрывает все 8 опубликованных tools: `run_all_tests`, `run_module_tests`, `build_project`, `dump_config`, `launch_app`, `check_syntax_edt`, `check_syntax_designer_config`, `check_syntax_designer_modules`.
  - `dump_config` regression matrix теперь покрывает MCP `PARTIAL` для `DESIGNER` и degraded success/failure semantics для `IBCMD` с сохранением `mode=PARTIAL`.
  - HTTP suite дополнительно покрывает live non-EDT tool call и burst admission/recovery scenario, а существующие tests остаются источником правды для session lifecycle и EDT timeout/restart/isolation semantics.
- [x] 2026-03-20: Добавить runtime metrics и tracing.
  - Добавлен `src/mcp/telemetry.rs` с единым telemetry state для MCP semaphore admission и shared EDT actor lifecycle без новых внешних metrics-зависимостей.
  - В action log теперь пишутся stable structured events: `mcp_execution_semaphore_wait` (`transport`, `tool`, `outcome`, `bounded`, `timeout_ms`, `wait_ms`), `mcp_edt_queue_depth` (`action`, `queue_depth`, `reason`), `mcp_edt_session_restart`, `mcp_edt_startup_failure`, `mcp_edt_shutdown_drain`.
  - Semaphore wait теперь измеряется для всех outcome-веток acquire path: `acquired`, queued `cancelled`, queued `timeout`, `internal_error`.
  - Shared EDT actor теперь фиксирует queue depth на enqueue/dequeue/remove/drain, отдельно считает `startup_failure_total`, strict `restart_total` только для реального kill/drop живой session и раздельные drain totals для `restart` / `shutdown`.
  - Добавлены unit/integration tests на semaphore telemetry outcomes, queued cancel/timeout queue-depth updates, startup-failure-vs-restart semantics и наличие telemetry events в live HTTP MCP action log.
- [x] 2026-03-20: Оформить migration note для расхождения `dump_config(mode=null)` с текущим Kotlin code path.
  - Расхождение задокументировано в `README.md`: `dump_config(mode=null|blank)` в MCP трактуется как `INCREMENTAL`.
- [x] 2026-03-20: Зафиксировать, что этот staged MCP plan был полностью закрыт к началу следующего task workflow.
  - Следующая работа продолжена из общего backlog в `spec/IMPLEMENTATION_TODO.md`, без переоткрытия закрытых Stage 1-5 задач.
- [x] 2026-03-21: Сохранить этот документ как canonical staged MCP rollout history/reference.
  - `spec/MCP_IMPLEMENTATION_PLAN.md` остаётся основным staged record для закрытого MCP rollout Stage 1-5, а новая реализационная работа продолжается из общего backlog в `spec/IMPLEMENTATION_TODO.md`.

## Public Changes

- Новый CLI surface:
  - `v8-test-runner mcp serve stdio`
  - `v8-test-runner mcp serve http`
- Новые config keys в `mcp.*` и `tools.edt_cli.*`.
- Новый внутренний transport-neutral API между CLI/MCP и use case-слоем.
- Новый shared EDT actor только для MCP path.

## Test Plan

- Stage 1: unit tests на use-case boundary, CLI request mapping, bootstrap error parity, launch JSON failure parity, normalization, validation, alias matrix, partial dump semantics.
- Stage 2: MCP stdio integration tests, protocol-clean stdout tests, business-vs-transport error boundary tests.
- Stage 3: EDT actor tests, timeout/restart tests, queue cancellation tests, A->B isolation tests.
- Stage 4: HTTP session tests, TTL/eviction tests, stateful behavior tests.
- Stage 5: full regression and stress suite.

## Assumptions

- Сохраняется Kotlin tool surface, но не byte-for-byte DTO parity.
- `dump_config(mode=null -> INCREMENTAL)` остается осознанным продуктовым решением, а не случайной несовместимостью.
- HTTP transport локально-ориентирован; remote auth layer в этот этап не входит.
- Если baseline test suite остается частично красным из-за уже существующего unrelated дефекта, MCP acceptance не блокируется на нем.
