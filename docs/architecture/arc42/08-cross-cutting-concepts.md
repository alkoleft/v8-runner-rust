## 8. Сквозные концепции

Свод правил, которые должны оставаться верными при развитии проекта, вынесен в [архитектурные инварианты](../invariants.md).

### 8.1 Модель конфигурации

- `v8project.yaml` — главный входной контракт.
- Валидация конфигурации заранее отклоняет неподдерживаемые комбинации.
- `source-set` — базовая единица оркестрации.
- Поддержанные `source-set[].type`: `CONFIGURATION`, `EXTENSION`, `EXTERNAL_DATA_PROCESSORS`, `EXTERNAL_REPORTS`.
- `source-set.name` — stable identity для runtime state, generated directories, diagnostics и selection logic.
- Для EDT/external source-set validation должна проверять layout, reserved names и пересечение пользовательских paths с generated work targets.
- Поддержанный config contract описан в ADR-0017; legacy YAML keys не должны становиться публичным контрактом без отдельного решения.
- `ExecutionContext` дополняет конфигурацию invocation-метаданными: команда, transport, correlation metadata и transport-specific flags.

### 8.2 Анализ изменений

- Анализ изменений выполняется on-demand во время build/export/load decision.
- Сканирование файлов использует фильтрацию по timestamp с последующей проверкой хеша.
- Состояние изолировано по логическим `source-set`.
- При сбоях система предпочитает безопасную деградацию, а не тихую потерю данных.
- Персистентное состояние хранится в отдельных `redb`-файлах на source-set, а не в едином глобальном индексе.
- Для EDT исходный project context и generated Designer context анализируются отдельно; partial load decision принимается по Designer context.
- Правила on-demand detection и partial load описаны в ADR-0012.

### 8.3 Обработка ошибок и результаты

- Use case возвращают структурированные результаты или `UseCaseFailure<T>` с transport-neutral error metadata.
- Runner-like/pipeline-like сценарии используют `ExecutionOutcome<T>` как canonical domain outcome для статуса, structured errors, diagnostics, metrics, artifacts и typed payload.
- Команды рассматриваются как pipeline из стандартных блоков: validation, resolve target, prepare workspace, platform command, parse output, publish, cleanup and diagnostics.
- Pipeline composition находится в use case слое; adapters не собирают blocks, а только мапят request/response.
- Blocks должны обмениваться typed context/input/output и оставлять step/outcome trail для skipped/degraded/failure behavior.
- `ExecutionStatus::TimedOut` и `ExecutionStatus::Cancelled` допустимы только после terminal-state semantics из ADR-0014.
- Если cancellation/shutdown/timeout был requested внутри successful `CriticalNonAbortable` phase, итог остаётся `Succeeded`, а result содержит warning/diagnostic о deferred interruption.
- Degraded success, например cleanup warning после успешного publish, не должен маскироваться как полностью чистый success.
- CLI решает на адаптерной границе, печатать ли `Envelope<T>`, text rendering или top-level error.
- CLI output дополнительно разделяет human-oriented highlights и agent-oriented minimal signal; это presentation concern, а не use-case behavior.
- MCP дополнительно разделяет `McpBusinessFailure<T>` и `McpInternalError`, чтобы агент видел предсказуемые business failures, но не получал как business-response ошибки неправильного transport/runtime usage.
- Это разделение является ключевым architectural invariant: orchestration не должна знать про конкретный transport payload format.
- Outcome/step contract описан в ADR-0016.

### 8.4 Наблюдаемость

- Логи и сгенерированные артефакты хранятся под `workPath`.
- Телеметрия MCP публикуется как структурированные tracing-события, а не через отдельный metrics-backend.
- `output::Presenter` и JSON `Envelope` являются частью CLI presentation, но не observability backend.

### 8.4.1 Публикация dump/artifacts

- Full replacement dump/artifacts сначала пишутся в staging path рядом с target.
- При замене существующего target старое состояние временно переносится в backup и используется для rollback при publish failure.
- Cleanup backup/staging после успешной публикации выполняется best-effort и может вернуться как warning.
- Staging/backup cleanup опирается на metadata sidecar: `tool`, `kind`, `run_id`, `target_path`, `target_identity`, `created_at`.
- Orphan cleanup не должен удалять malformed, foreign или recent temp paths.
- Incremental/partial dump остаются non-atomic update modes.
- Правила staging/backup publication описаны в ADR-0015.

### 8.5 Параллелизм и таймауты

- MCP tool-вызовы используют общие admission-лимиты.
- CLI/MCP команды, которые работают с `workPath`, сериализуются через workspace lock по canonical `workPath`.
- Lock sidecar является diagnostic-only metadata; ошибка sidecar не разрешает конкурентное выполнение.
- MCP admission limits не заменяют workspace lock: они ограничивают общую нагрузку, а не ownership конкретного рабочего каталога.
- HTTP MCP session capacity является отдельным transport guardrail и не равна execution admission.
- Timeout/cancellation являются общим CLI/MCP целевым command contract; result должен возвращаться только после terminal state underlying operation.
- Mutating DB operations должны иметь critical phase, где hard kill запрещён по умолчанию.
- Timeout budget покрывает очередь/admission, подготовку, platform process, log collection, cleanup и result mapping.
- Queued cancellation может завершиться до запуска work; running cancellation должна идти через controlled interruption flow.
- Cancellation policy применяется на command boundary и safe points, без отдельной cancellation state machine на каждом pipeline step.
- HTTP MCP-сессии ограничены по ёмкости и управляются через TTL.
- Для интерактивного EDT-исполнения заданы отдельные ограничения на startup и command timeout.
- Серверные отмены и shutdown строятся вокруг cooperative cancellation и bounded drain, а не вокруг мгновенного прерывания любой внешней работы.
- Workspace lock contract описан в ADR-0011.
- MCP admission/session capacity описаны в ADR-0013.
- Общая timeout/cancellation policy описана в ADR-0014.
