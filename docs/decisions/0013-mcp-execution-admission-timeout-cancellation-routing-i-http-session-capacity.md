# ADR-0013: MCP execution admission, timeout/cancellation routing и HTTP session capacity

- Статус: `accepted`
- Дата: `2026-04-20`

## Контекст

MCP transport имеет две независимые нагрузки:

1. tool execution: вызовы `run_all_tests`, `build_project`, `dump_config`, syntax tools и другие операции, которые могут запускать blocking platform work;
2. HTTP session lifecycle: stateful Streamable HTTP sessions, которые занимают server-side session capacity независимо от того, выполняют ли они сейчас tool call.

Кроме того, MCP получает transport-level cancellation и может иметь timeout/deadline на вызов.
Эти сигналы нельзя трактовать только как MCP-specific behavior: фактическое прерывание platform command должно подчиняться общей CLI/MCP execution policy, особенно для операций, которые могут изменить или повредить ИБ при небезопасном kill.

## Решение

Разделить MCP resource guardrails на два независимых контура и маршрутизировать timeout/cancellation в общую command execution policy.

### MCP tool execution admission

1. Все MCP tool calls проходят через общий execution admission boundary.
2. `mcp.execution.max_concurrent_calls` задаёт максимальное число MCP tool calls, одновременно допущенных к execution.
3. Лимит применяется одинаково к stdio и HTTP transports.
4. Ошибки admission и execution должны различать stage:
- `queued`: вызов ещё ждёт execution slot;
- `running`: вызов уже допущен к execution.
5. Runtime telemetry должна фиксировать outcome ожидания execution slot: acquired, cancelled, timeout, internal error.
6. MCP admission не заменяет workspace lock: admission ограничивает MCP нагрузку, workspace lock из ADR-0011 сериализует доступ к конкретному `workPath`.

### Timeout/cancellation routing

1. MCP cancellation и MCP deadline являются входными сигналами для общей execution policy, а не отдельным MCP-only контрактом.
2. Семантика "когда команда считается отменённой/завершённой" определяется ADR-0014.
3. MCP adapter не должен возвращать клиенту результат, утверждающий, что running operation отменена или остановлена, пока underlying operation не доведена до terminal state согласно ADR-0014.
4. Если текущая implementation использует detached completion для running cancellation/timeout, это считается transition mechanism, а не целевой semantic contract.
5. Bounded timeout не должен оставаться special case только для `check_syntax_edt`; общий deadline должен применяться ко всем public CLI/MCP commands согласно ADR-0014.

### HTTP session capacity

1. `mcp.http.max_sessions` ограничивает количество tracked stateful HTTP sessions, а не число одновременно исполняемых tool calls.
2. `mcp.http.idle_ttl_secs` управляет eviction idle HTTP sessions только при `stateful_sessions=true`.
3. HTTP initialize должен резервировать session capacity до delegation в rmcp service.
4. Reservation должна завершаться одним из исходов:
- `confirm(session_id)`, если initialize успешно вернул session id;
- `release()`, если initialize failed или не вернул session id;
- automatic release on drop, если обработка прервалась.
5. Если session capacity исчерпана, HTTP transport возвращает `503 Service Unavailable`.
6. В stateful HTTP mode POST без `Mcp-Session-Id`, который не является `initialize`, возвращает deterministic `400 Bad Request`.
7. `DELETE` успешной session должен eagerly освобождать tracked capacity.
8. Lazy pruning expired rmcp sessions должна учитываться при новых initialize reservations.

## Неграницы (Non-goals)

1. Не определять command-level interruption safety; это делает ADR-0014.
2. Не заменять `workPath` lock из ADR-0011.
3. Не делать HTTP session capacity глобальным лимитом platform processes.
4. Не гарантировать, что MCP cancellation может мгновенно остановить platform operation.
5. Не смешивать MCP session lifecycle с business failures use case слоя.

## Последствия

1. MCP tool calls получают предсказуемую admission model независимо от transport.
2. HTTP session overload не должен блокировать stdio MCP и не должен интерпретироваться как command execution overload.
3. Клиенты могут различать queued/running cancellation/timeout по structured error data.
4. Реализация running cancellation/timeout должна быть приведена к terminal-state semantics из ADR-0014.
5. При изменении MCP concurrency, telemetry, HTTP session reservation или overload responses нужно обновлять этот ADR.

## План реализации

Текущее состояние кода частично следует этому решению:

1. `src/config/model.rs` описывает `mcp.http` и `mcp.execution`.
2. `src/config/validate.rs` валидирует ненулевые MCP limits и корректный HTTP path/address.
3. `src/mcp/server.rs` создаёт общий semaphore на `mcp.execution.max_concurrent_calls`.
4. `src/mcp/server.rs` различает queued/running cancellation/timeout в MCP errors.
5. `src/mcp/server.rs` реализует HTTP session reservation/confirm/release и overload responses.
6. `src/mcp/edt_session.rs` использует `max_concurrent_calls` как queue capacity shared EDT actor.

Дальнейшие изменения:

1. привести MCP running cancellation/timeout к terminal-state semantics из ADR-0014;
2. убрать special-case модель, где bounded command timeout есть только у `check_syntax_edt`;
3. передавать MCP cancellation/deadline в общий transport-neutral execution context;
4. сохранить отдельные tests для execution admission и HTTP session capacity;
5. обновить `ARCHITECTURE.md`, arc42 и invariants при изменении MCP guardrails.

## Верификация

- [x] ADR разделяет MCP execution admission и HTTP session capacity.
- [x] ADR фиксирует `mcp.execution.max_concurrent_calls` как общий MCP admission limit.
- [x] ADR фиксирует `mcp.http.max_sessions` как лимит stateful HTTP sessions.
- [x] ADR запрещает считать MCP admission заменой `workPath` lock.
- [x] ADR передаёт command timeout/cancellation semantics в общий ADR-0014.
