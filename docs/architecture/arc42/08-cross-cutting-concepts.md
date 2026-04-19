## 8. Сквозные концепции

### 8.1 Модель конфигурации

- `v8project.yaml` — главный входной контракт.
- Валидация конфигурации заранее отклоняет неподдерживаемые комбинации.
- `source-set` — базовая единица оркестрации.
- `ExecutionContext` дополняет конфигурацию invocation-метаданными: команда, transport, correlation metadata и transport-specific flags.

### 8.2 Анализ изменений

- Сканирование файлов использует фильтрацию по timestamp с последующей проверкой хеша.
- Состояние изолировано по логическим `source-set`.
- При сбоях система предпочитает безопасную деградацию, а не тихую потерю данных.
- Персистентное состояние хранится в отдельных `redb`-файлах на source-set, а не в едином глобальном индексе.

### 8.3 Обработка ошибок и результаты

- Use case возвращают структурированные результаты или `UseCaseFailure<T>` с transport-neutral error metadata.
- CLI решает на адаптерной границе, печатать ли `Envelope<T>`, text rendering или top-level error.
- MCP дополнительно разделяет `McpBusinessFailure<T>` и `McpInternalError`, чтобы агент видел предсказуемые business failures, но не получал как business-response ошибки неправильного transport/runtime usage.
- Это разделение является ключевым architectural invariant: orchestration не должна знать про конкретный transport payload format.

### 8.4 Наблюдаемость

- Логи и сгенерированные артефакты хранятся под `workPath`.
- Телеметрия MCP публикуется как структурированные tracing-события, а не через отдельный metrics-backend.
- `output::Presenter` и JSON `Envelope` являются частью CLI presentation, но не observability backend.

### 8.5 Параллелизм и таймауты

- MCP tool-вызовы используют общие admission-лимиты.
- HTTP MCP-сессии ограничены по ёмкости и управляются через TTL.
- Для интерактивного EDT-исполнения заданы отдельные ограничения на startup и command timeout.
- Серверные отмены и shutdown строятся вокруг cooperative cancellation и bounded drain, а не вокруг мгновенного прерывания любой внешней работы.
