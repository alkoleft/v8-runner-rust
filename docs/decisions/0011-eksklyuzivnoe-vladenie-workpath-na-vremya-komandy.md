# ADR-0011: Эксклюзивное владение `workPath` на время команды

- Статус: `accepted`
- Дата: `2026-04-20`

## Контекст

Команды `v8-runner` разделяют один `workPath` между CLI и MCP сценариями.
Под этим каталогом находятся platform logs, temporary list files, YaXUnit/Vanessa artifacts, `redb` hash storages, EDT workspace и generated Designer output.

Параллельный запуск двух команд над одним `workPath` может привести к некорректному состоянию:

1. одна команда очистит или перезапишет platform logs другой команды;
2. partial load list или temporary artifacts будут смешаны между запусками;
3. `redb` snapshot будет зафиксирован не тем запуском, который реально выполнил platform load/export;
4. EDT export и Designer load начнут работать с промежуточным generated output;
5. диагностика CLI/MCP станет недостоверной, потому что разные команды пишут в одни runtime paths.

При этом MCP уже имеет admission limits для ограничения общей нагрузки, но они не являются блокировкой конкретного `workPath`.

## Решение

Ввести инвариант: любая публичная команда, которая читает или пишет runtime state под `workPath`, должна владеть workspace lock на время выполнения.

Правила:

1. CLI adapter обязан брать workspace lock перед dispatch в use case для команд, работающих с `workPath`.
2. MCP adapter обязан брать тот же workspace lock перед dispatch в use case.
3. Lock берётся по canonical `workPath`, чтобы разные строковые представления одного каталога конкурировали за один lock.
4. Lock file хранится внутри `workPath` как `.v8-runner.workspace.lock`.
5. Diagnostic sidecar `.v8-runner.workspace.lock.json` содержит metadata (`pid`, owner, command, start time, canonical path), но не является источником истины для блокировки.
6. Ошибка записи sidecar не должна снимать lock и не должна разрешать конкурентное выполнение.
7. Вложенная orchestration не должна повторно брать lock; она должна использовать явно названные internal functions, например `run_build_unlocked`, когда внешний command boundary уже владеет `workPath`.
8. `--clean-before-execution`, platform log cleanup, partial list generation, hash storage commits и generated EDT/Designer writes выполняются только внутри владения workspace lock.
9. Workspace lock является local advisory file lock, а не distributed lock для сетевых машин или разных файловых систем с невалидной семантикой блокировок.

## Неграницы (Non-goals)

1. Не вводить distributed lock для нескольких машин.
2. Не разрешать параллельную сборку нескольких `source-set` внутри одного `workPath` в рамках этого решения.
3. Не использовать workspace lock для защиты пользовательских исходников вне `workPath`.
4. Не заменять MCP admission semaphore: semaphore ограничивает общую нагрузку, workspace lock сериализует доступ к конкретному `workPath`.
5. Не делать sidecar metadata обязательным источником восстановления stale lock.

## Последствия

1. Две команды над одним `workPath` должны сериализоваться или получать предсказуемую ошибку занятости.
2. Команды над разными `workPath` могут выполняться параллельно, если не конфликтуют по внешним 1С-ресурсам.
3. Новые CLI/MCP команды, которые используют `workPath`, должны подключаться к общему lock boundary.
4. Вложенные сценарии, например `test -> build`, требуют явного разделения public locked entrypoint и internal unlocked function.
5. Ошибки workspace lock должны быть surfaced как runtime/business failure с понятным command name и, где доступно, owner metadata.

## План реализации

Текущее состояние кода уже следует этому решению:

1. `src/use_cases/workspace_lock.rs` реализует canonical path lock, lock file и diagnostic sidecar.
2. `src/cli/execute.rs` берёт workspace lock на CLI command boundary.
3. `src/mcp/port.rs` берёт workspace lock на MCP use-case port boundary.
4. `src/use_cases/build_project.rs` содержит `run_build_unlocked` для вложенного вызова из `test`.
5. `src/use_cases/run_tests.rs` вызывает build через unlocked entrypoint внутри внешнего lock.

При дальнейших изменениях:

1. новые public commands в `src/cli/execute.rs` и `src/mcp/port.rs` должны явно проходить через workspace lock;
2. новые nested flows должны получать отдельный `*_unlocked` entrypoint только если внешний boundary уже владеет lock;
3. тесты должны проверять конфликт lock до dispatch use case и отсутствие повторного lock во вложенном сценарии;
4. документация `ARCHITECTURE.md`, arc42 и invariants должны ссылаться на этот ADR.

## Верификация

- [x] ADR фиксирует, что `workPath` имеет одного владельца на время public command.
- [x] ADR разделяет workspace lock и MCP admission semaphore.
- [x] ADR фиксирует canonical `workPath` как ключ блокировки.
- [x] ADR фиксирует sidecar metadata как diagnostic-only.
- [x] ADR описывает nested `*_unlocked` pattern для сценариев вроде `test -> build`.
