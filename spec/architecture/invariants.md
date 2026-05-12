# Архитектурные инварианты

Этот документ фиксирует правила, которые должны оставаться верными при развитии `v8-runner`.
Если изменение нарушает инвариант, сначала нужен новый ADR, который явно заменяет или уточняет текущее решение.
Практический checklist для изменений MCP surface, public command boundary и config contract вынесен в [spec/architecture/change-checklist.md](change-checklist.md).

## Цель продукта

1. Главная цель `v8-runner` — предоставить простой и удобный интерфейс для сборки и проверки исходников 1С-решения человеком и AI-агентом.
2. Основной пользовательский цикл — `build -> syntax/test -> diagnose`.
3. Новая функциональность должна упрощать этот цикл или явно объяснять, какую диагностическую, эксплуатационную или интеграционную задачу она закрывает.
4. Низкоуровневые детали утилит 1С не должны становиться обязательным знанием для обычного пользователя или AI-агента, если их можно скрыть за стабильным CLI/MCP контрактом.
5. Удобство для человека и пригодность для AI-агента являются равноправными критериями продукта.

## Публичные поверхности

1. CLI и MCP являются разными публичными поверхностями.
2. MCP не зеркалит CLI автоматически.
3. Текущая MCP-поверхность состоит из 8 tool-операций: `run_all_tests`, `run_module_tests`, `build_project`, `dump_config`, `launch_app`, `check_syntax_edt`, `check_syntax_designer_config`, `check_syntax_designer_modules`.
4. Добавление, удаление или переименование MCP tool-операций является изменением публичного контракта и требует отдельного ADR или явного обновления действующего ADR.
5. Любое изменение MCP surface должно проходить по checklist из `spec/architecture/change-checklist.md` и синхронизировать docs/source/tests, перечисленные в `ADR-0005`.
6. CLI-only команды являются допустимой частью public surface; наличие CLI-команды не должно использоваться как аргумент для неявной публикации MCP tool.

См. [ADR-0005](../decisions/0005-razdelit-cli-i-mcp-publichnye-poverhnosti.md) и [ADR-0020](../decisions/0020-dobavit-cli-only-convert-dlya-dvustoronney-konvertatsii-edt-i-designer.md).

## Config Contract

1. `v8project.yaml`, загруженный в `AppConfig` и прошедший `config::validate`, является главным конфигурационным контрактом проекта.
2. `infobase.connection` является обязательным supported ключом строки подключения; top-level `connection` не является публичным контрактом.
3. `infobase.user/password` являются supported ключами пользователя ИБ; top-level `credentials` не является публичным контрактом.
4. `infobase.dbms` описывает DBMS-level доступ для server-based ИБ; для `builder=IBCMD` + server connection обязательны `kind`, `server` и `name`.
5. Полный `infobase.dbms` contract является достаточным explicit authorization для server infobase provisioning path в `init` при `builder=IBCMD`; отдельный `tools.*` field для этого не требуется.
6. Для `builder=DESIGNER` автоматическое создание server-based ИБ в `init` не вводится; server create step остаётся explicit gap вне `IBCMD` provisioning path.
7. `infobase.dbms` не должен задаваться для file-based ИБ.
8. `source-set[].type` является поддержанным ключом типа source-set; legacy `purpose` не является публичным контрактом.
9. `source-set.name` является stable identity для ordering, diagnostics, runtime contexts, generated directories и selection logic.
10. `source-set.name` должен быть уникальным и безопасным path segment; resolved paths должны быть уникальны после normalization.
11. EDT/external source-set paths и generated work targets не должны пересекаться; reserved work directory names нельзя использовать как EDT source-set names.
12. Unsupported или unsafe config combinations должны отклоняться на validation boundary до вызова platform DSL.
13. Новый public config field, `source-set` type или `infobase` subtree требует typed model, validation, `config init`/examples/docs sync и regression tests по checklist из `spec/architecture/change-checklist.md`.
14. `v8project.local.yaml` является optional local overlay рядом с primary config, применяется после `v8project.yaml` и до CLI overrides, не является самостоятельным `--config` entrypoint и не должен менять `source-set`, `format` или `builder`.
15. `basePath` не является public key в `v8project.yaml`; внутренний project base path считается равным каталогу primary config.
16. Tool extensions, включая `tools.client_mcp.extension`, не являются project `source-set`; их подготовка выполняется через общий механизм подготовки расширений на стадии `build`, а не на стадии `launch`.

См. [ADR-0017](../decisions/0017-v8project-yaml-source-set-kak-glavnyy-konfiguratsionnyy-kontrakt.md), [ADR-0018](../decisions/0018-perenesti-kontrakt-informatsionnoy-bazy-v-infobase.md), [ADR-0019](../decisions/0019-sozdavat-servernuyu-infobazu-cherez-ibcmd-pri-init-pri-otsutstvii.md), [ADR-0021](../decisions/0021-lokalnyy-overlay-config.md) и [ADR-0022](../decisions/0022-universalnyy-mehanizm-podgotovki-rasshireniy-i-client-mcp-extension.md).

## Workspace Lock

1. Любая CLI/MCP команда, которая читает или пишет runtime state под `workPath`, должна владеть workspace lock на время выполнения.
2. Workspace lock берётся по canonical `workPath`.
3. Lock sidecar является diagnostic-only metadata; отсутствие или ошибка записи sidecar не отменяет сам lock.
4. Вложенная orchestration использует explicit internal `*_unlocked` entrypoints только под внешним lock.
5. MCP admission limits не заменяют workspace lock: semaphore ограничивает общую нагрузку, lock сериализует доступ к конкретному `workPath`.
6. Новая public CLI/MCP команда, работающая с runtime state под `workPath`, должна брать lock на adapter boundary и иметь regression coverage на boundary conflict.

См. [ADR-0011](../decisions/0011-eksklyuzivnoe-vladenie-workpath-na-vremya-komandy.md).

## MCP Admission And HTTP Sessions

1. MCP tool calls проходят через общий execution admission boundary.
2. `mcp.execution.max_concurrent_calls` ограничивает одновременно допущенные MCP tool executions для stdio и HTTP.
3. MCP admission не заменяет workspace lock и не является HTTP session capacity.
4. `mcp.http.max_sessions` ограничивает tracked stateful HTTP sessions, а не command execution.
5. HTTP initialize должен использовать reservation/confirm/release flow; overload возвращает `503`, а stateful non-initialize POST без `Mcp-Session-Id` возвращает `400`.
6. MCP cancellation/deadline должны маршрутизироваться в общую execution policy из ADR-0014.

См. [ADR-0013](../decisions/0013-mcp-execution-admission-timeout-cancellation-routing-i-http-session-capacity.md).

## Command Timeout And Cancellation

1. Timeout/cancellation являются общим CLI/MCP command contract, а не MCP-only behavior.
2. Каждая public CLI/MCP команда должна иметь execution deadline.
3. Nested orchestration наследует оставшийся budget outer command.
4. Команда не считается cancelled/timed out наружу, пока underlying operation не доведена до terminal state.
5. Operations должны иметь interruption safety class: `Interruptible`, `GracefulThenKill`, `CriticalNonAbortable` или `NoExternalProcess`.
6. Mutating DB operations после входа в critical phase не hard-kill by default; cancellation/timeout recorded и команда ждёт terminal outcome.
7. Cancellation policy живёт на command boundary; use case pipeline проверяет cancellation/deadline в safe points и не обязан моделировать отдельное cancellation state на каждом step.
8. `ExecutionStatus::Cancelled` используется только для фактической terminal cancellation.
9. Если cancellation/shutdown/timeout пришёл в critical phase, но operation безопасно завершилась success, итог остаётся `Succeeded`, а result содержит warning/diagnostic о deferred interruption.

См. [ADR-0014](../decisions/0014-edinaya-timeout-cancellation-policy-dlya-cli-i-mcp-komand.md).

## Dump And Artifacts Publication

1. Full-replacement `dump` и `artifacts` publication не должны писать напрямую в существующий target.
2. Full dump и package/external artifacts publication должны идти через staging path рядом с target и backup старого target.
3. Platform failure до publish должен сохранять старый target.
4. Publish failure должен пытаться rollback backup -> target и surfaced rollback context, если восстановление не удалось.
5. Cleanup backup/staging после успешного publish выполняется best-effort; cleanup failure становится warning/degraded success, а не failed publish.
6. `dump incremental` и `dump partial` являются non-atomic update modes и не получают staging replacement guarantee.
7. Orphan cleanup должен удалять только stale v8-runner staging/backup paths с matching target identity.

См. [ADR-0015](../decisions/0015-atomarnaya-publikatsiya-dump-artifacts-cherez-staging-backup.md).

## Pipeline Execution Outcome

1. Runner-like и pipeline-like сценарии должны использовать `ExecutionOutcome<T>` как canonical domain outcome для статуса, structured errors, diagnostics, metrics, artifacts and typed payload.
2. Domain result structs may keep command-specific context, but legacy compatibility fields for data already expressed by `ExecutionOutcome<T>` belong at the CLI/MCP adapter boundary.
3. Pipeline composition живёт в use case слое; CLI/MCP adapters не собирают и не исполняют pipeline blocks.
4. Blocks обмениваются typed context/input/output, а не hidden global state.
5. Значимые pipeline blocks должны иметь step entry; минимальная текущая форма `StepResult` должна эволюционировать к richer `ExecutionStep` перед массовым добавлением новых combinations.
6. `ExecutionOutcome<T>` не заменяет shared command envelope, MCP request DTO/compatibility data или `UseCaseFailure<T>`.
7. Timeout/cancellation statuses in outcome должны следовать terminal-state semantics из ADR-0014.
8. Cancellation representation остаётся command-level: `ExecutionStatus::Cancelled` для фактической отмены и diagnostic/warning для deferred interruption при successful critical phase.
9. Не вводить generic pipeline engine до появления повторяемой необходимости; сначала стандартизируются vocabulary, step contract and outcome shape.

См. [ADR-0016](../decisions/0016-edinyy-executionoutcome-i-pipeline-steps-dlya-runner-like-stsenariev.md).

## Use Case Layer

1. `src/use_cases` остается транспортно-нейтральным orchestration-слоем.
2. Use case не зависят от `clap`, CLI `Presenter`, shared command envelope, MCP DTO и конкретного transport payload format.
3. CLI и MCP адаптеры преобразуют свои входные DTO/аргументы в `use_cases::request::*`.
4. Presentation, envelope rendering и MCP tool payload formatting остаются за пределами use case.

См. [ADR-0006](../decisions/0006-sohranyat-transportno-neytralnyy-use-case-sloy.md).

## Change Detection And Partial Load

1. Change detection выполняется on-demand во время build/export/load decision, без background watcher.
2. Persistent state хранится в per-context `redb` storages под `workPath/hash-storages`.
3. Для `format=DESIGNER` используется один `designer-<sourceSetName>` context на source-set.
4. Для `format=EDT` используются два context на source-set: `edt-<sourceSetName>` для export decision и `designer-<sourceSetName>` для load decision.
5. Recoverable scan/storage ошибки должны деградировать в full execution или full rescan; hard storage и concurrent generation errors должны surfaced as failures.
6. Partial load является conservative file-level strategy: `Configuration.xml`, deletions, unsafe expansion, empty expanded set или превышение threshold ведут к full load.
7. Prepared snapshot коммитится только после successful platform export/load step.

См. [ADR-0012](../decisions/0012-on-demand-change-detection-i-faylovaya-partial-load-strategiya.md).

## Shared EDT

1. EDT execution имеет два целевых режима: one-shot и shared interactive.
2. `tools.edt_cli.interactive_mode=false` (YAML key `interactive-mode`) означает one-shot `1cedtcli` execution.
3. `tools.edt_cli.interactive_mode=true` (YAML key `interactive-mode`) означает shared interactive EDT execution через общий actor/manager и общую interactive session.
4. Non-shared interactive EDT не является долгосрочным публичным режимом; если он встречается в коде, это implementation gap.
5. Shared interactive EDT должен сохранять baseline reset/probe, restart, shutdown/restart drain, typed errors and telemetry contract.
6. `tools.edt_cli.auto_start` (YAML key `auto-start`) является eager prewarm-флагом только для long-lived shared EDT host process; на текущем этапе это MCP server.
7. CLI при `tools.edt_cli.interactive_mode=true` стартует EDT лениво при первом EDT-вызове и не должен eagerly prewarm interactive session на старте процесса команды.
8. Если shared interactive временно покрывает не все EDT-сценарии, gap должен быть зафиксирован в документации или ADR.

См. [ADR-0007](../decisions/0007-vydelit-otdelnyy-pereklyuchatel-dlya-shared-edt.md).

## Platform Backends

1. Низкоуровневые DSL для платформенных инструментов остаются в `src/platform`.
2. `DesignerDsl`, `IbcmdDsl`, `EdtDsl`, `EnterpriseDsl`, `platform::edt_session`, `platform::locator`, `platform::process` и interactive executor не должны протаскивать process details в presentation или transport adapters.
3. Orchestration вызывает backend DSL через доменные операции и анализирует `PlatformCommandResult`, но не собирает сырые process arguments выше платформенного слоя.
4. Новый backend добавляется как отдельный adapter/DSL с явными gap и матрицей поддержки.

См. [ADR-0008](../decisions/0008-derzhat-platformennye-backend-dsl-otdelno-ot-orchestration.md).

## Failures

1. Business failures и transport/runtime failures разделены.
2. Use case возвращают `UseCaseFailure<T>` с transport-neutral metadata и, где возможно, структурированным payload.
3. MCP service разделяет `McpBusinessFailure<T>` и `McpInternalError`.
4. Orchestration не знает, как CLI или MCP сериализуют ошибку наружу.

См. [ADR-0009](../decisions/0009-razdelit-business-i-transport-runtime-failures.md).

## CLI Output

1. CLI output проектируется как единый high-signal contract для человека и AI-агента.
2. Единственный публичный selector structured CLI output — глобальный флаг `--json-message`; отдельный audience/profile-переключатель не вводится.
3. При отсутствии `--json-message` CLI печатает text output.
4. User-facing path флаг должен называться `--output`, если команда публикует один основной output path; не вводить параллельные public имена `--file`, `--out` или `--output-dir` для того же смысла без нового ADR.
5. Clean success должен оставаться кратким, а ошибки, предупреждения, degraded behavior, created artifacts, пути к диагностике и следующий actionable hint не должны теряться.
6. JSON остаётся стабильным structured contract для автоматизации; его schema не меняется только из-за различения ролей потребления.
7. Use case слой не знает presentation rules и не различает роли потребителя output.
8. CLI `--json-message` и MCP `structured_content` используют общий machine-readable command envelope с core fields `ok`, `command`, `duration_ms`, `data`, `warnings`, `steps`; business failures may include optional structured `error`.
9. MCP `CallToolResult`/`isError` и transport/internal `ErrorData` остаются protocol-level behavior и не заменяются command envelope.
10. Envelope `command` использует canonical CLI command identity; MCP tool/scope identity сохраняется внутри `data`, если она нужна клиенту.
11. Live text progress для long-running stages остаётся human-readable: может включать локальное время `HH:MM:SS` как краткий префикс строки, но не печатает structured field names вроде `started_at` и не вводит JSON/progress contract.

См. [ADR-0010](../decisions/0010-razdelit-cli-output-dlya-cheloveka-i-ai-agenta.md).
