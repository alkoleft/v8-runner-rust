# TODO реализации Rust CLI

> Примечание: это внутренний рабочий документ. Английские названия разделов и терминов оставлены там, где они совпадают с именами стадий, команд или устоявшимися техническими идентификаторами.

## Foundation

- [x] Создать `Cargo.toml` для бинаря `v8-test-runner`
- [x] Добавить базовые зависимости: `clap`, `serde`, `serde_yaml`, `serde_json`, `thiserror`, `tracing`, `walkdir`, `sha2`, `quick-xml`, `tempfile`
- [x] Создать `src/main.rs`
- [x] Создать `src/app.rs`
- [x] Завести модульную структуру `cli`, `config`, `domain`, `use_cases`, `change_detection`, `platform`, `parsers`, `output`, `support`

## CLI и output

- [x] Описать CLI API на `clap`
- [x] Добавить глобальные флаги `--config`, `--output`, `--log-level`, `--clean-before-execution`, `--no-color`, `--workdir`
- [x] Добавить subcommands `build`, `test`, `dump`, `syntax`, `launch`
- [x] Реализовать text presenter
- [x] Реализовать json presenter с единым envelope
- [x] Нормализовать exit codes для validation/runtime/platform errors

## Конфигурация

- [x] Описать модель конфигурации приложения
- [x] Реализовать загрузку YAML-конфига
- [x] Валидировать `basePath`
- [x] Валидировать `workPath`
- [x] Валидировать `source-set`
- [x] Валидировать `format`
- [x] Валидировать `builder`
- [x] Валидировать строку подключения
- [x] Подготовить `examples/application.yaml`

## Процессы и утилиты платформы

- [x] Реализовать `ProcessExecutor`
- [x] Реализовать захват `stdout/stderr/exit code`
- [x] Реализовать запись и чтение log-файлов платформы
- [x] Реализовать временные файлы для partial lists и YaXUnit config
- [x] Реализовать поиск бинарников `1cv8`, `1cv8c`, `ibcmd`, `1cedtcli`
- [x] Развести ответственность между locator и platform DSL

## Change detection

- [x] Реализовать `Scanner` с рекурсивным обходом
- [x] Игнорировать `.git/`, `.gradle/`, `build/`, `target/`, `temp/`, `tmp/`, `ConfigDumpInfo.xml`, `.yaxunit/`
- [x] Реализовать отбор кандидатов по `lastModified`
- [x] Реализовать hashing содержимого для кандидатов
- [x] Реализовать `redb` storage в `workPath/hash-storages/*.redb` (tables: `FILES_MTIME`, `FILES_HASH`, `META`)
- [x] Реализовать fallback на "все изменено" при recoverable проблемах хранения/сканирования
- [x] Реализовать группировку изменений по `source-set`
- [x] Реализовать `SourceSetsService`: выдавать `SourceSetContext` для EDT и Designer, прикреплять отдельное hash storage к каждому логическому источнику; в EDT-режиме — два независимых контекста (исходный EDT и временный Designer в `workPath`)

## Волна 1: Designer MVP

### Build

- [x] Реализовать `DesignerDsl`
- [x] Реализовать `build_project` use case
- [x] Реализовать `--full-rebuild` как forced full execution без destructive cache cleanup
- [x] Реализовать выбор затронутых `source-set`
- [x] Реализовать `PartialLoadListGenerator`
- [x] Для `.bsl`-файлов добавлять в list связанные XML и каталог объекта
- [x] Запретить partial при изменении `Configuration.xml`
- [x] Запретить partial при превышении порога числа файлов
- [x] Сохранять state только после успешного build

### Tests

- [x] Реализовать `EnterpriseDsl`
- [x] Реализовать генерацию временного JSON-конфига YaXUnit
- [x] Реализовать `test all`
- [x] Реализовать `test module <MODULE_NAME>`
- [x] Гарантировать обязательный `build` перед тестами
- [x] Реализовать parser JUnit XML
- [x] Реализовать parser `[ERR]` блоков YaXUnit-лога
- [x] Вернуть summary, suites, cases и extracted errors
- [x] Реализовать compact/full режим тестового ответа: compact скрывает passed-тесты и урезает stack trace

### Dump

- [x] Реализовать `dump --mode full`
- [x] Реализовать `dump --mode incremental`
- [x] Реализовать `dump --mode partial`
- [x] Валидировать, что `partial` требует минимум один `--object`
- [x] Поддержать выбор `source-set`
- [x] Поддержать выбор `extension`

### Syntax

- [x] Реализовать `syntax designer-config`
- [x] Реализовать `syntax designer-modules`
- [x] Валидировать, что для `syntax designer-modules` включён хотя бы один режим проверки
- [x] Реализовать parser designer validation logs
- [x] Вернуть structured issues вместо raw stdout

### Launch

- [x] Реализовать `launch --mode designer`
- [x] Реализовать `launch --mode thin`
- [x] Реализовать `launch --mode thick`
- [x] Возвращать статус запуска и доступные process details

## Волна 1: тестирование и документация

- [x] Добавить unit-тесты на change detection
- [x] Добавить unit-тесты на `PartialLoadListGenerator`
- [x] Добавить unit-тесты на JUnit parser
- [x] Добавить unit-тесты на YaXUnit log parser
- [x] Добавить unit-тесты на designer validation parser
- [x] Добавить integration tests для CLI команд
- [x] 2026-03-20: Подготовить fixture-наборы логов и XML
- [x] Обновить `README.md`
- [x] 2026-03-22: Перепаковать public docs: новый onboarding `README.md`, `docs/CAPABILITIES.md`, `docs/DEEP_DIVE.md`, и явно развести public docs vs internal reference docs
- [x] 2026-03-22: Перевести public docs layer на русский (`README.md`, `docs/CAPABILITIES.md`, `docs/DEEP_DIVE.md`)

## Волна 2: EDT

- [ ] Расширить config-модель для `format = EDT`
- [ ] Ввести отдельные state storage для `edt` и `designer`
- [x] Реализовать `InteractiveProcessExecutor`
- [x] Реализовать ожидание prompt `1C:EDT>`
- [ ] Реализовать мониторинг живости EDT-процесса и автоперезапуск при сбое
- [x] Реализовать single-flight инициализацию EDT-сессии
- [ ] Реализовать `EdtDsl`
- [ ] Реализовать экспорт только измененных EDT `source-set`
- [ ] Реализовать временный Designer-каталог в `workPath/<sourceSetName>/`
- [ ] Встроить export как первую фазу EDT build
- [ ] Реализовать `syntax edt`
- [ ] Реализовать parser EDT validation logs (TSV-подобный формат: каждая строка — отдельный issue)

## Волна 2: IBCMD

- [x] Реализовать `IbcmdDsl`
- [x] Реализовать build через `config import` и `config apply`
- [x] Реализовать dump `FULL` через `--force`
- [x] Реализовать dump `INCREMENTAL` через `--sync`
- [x] Явно задокументировать: partial dump по объектам в IBCMD backend не поддерживается
- [x] Валидировать ограничения IBCMD для connection type (только файловая ИБ)
- [x] Задокументировать ограничения IBCMD backend

## Подготовка к следующему этапу

- [x] Расширить config-модель новыми `mcp.*` и `tools.edt_cli.*` настройками для MCP stage
- [ ] Нормализовать result types для build/test/dump/syntax/launch
- [x] Добавить `steps` в output envelope
- [x] Добавить `warnings` в output envelope
- [x] Убедиться, что use cases не зависят от CLI parsing
- [x] Добавить MCP-facing service layer с business/internal failure boundary и отдельными MCP DTO
- [x] Нормализовать MCP contract mapping defaults, aliases и pre-validation в service layer
- [x] Оставить место для будущего transport adapter слоя

## MCP

- [x] Добавить `v8-test-runner mcp serve stdio`
- [x] Поднять rmcp stdio tool server с tools-only capability и опубликовать 8 MCP tools
- [x] Добавить bounded execution через semaphore и per-call timeout/cancel semantics для MCP path
- [x] Подключить shared EDT actor к live MCP `check_syntax_edt`
- [x] Добавить baseline/reset pre-dispatch для shared EDT session (`cd <workspace>` + probe `cd`)
- [x] Добавить `v8-test-runner mcp serve http`
- [x] Поднять `axum` + `rmcp` streamable HTTP transport с stateful/stateless session semantics
- [x] Переиспользовать shared EDT actor и общий execution semaphore для HTTP MCP sessions
- [x] Добавить MCP runtime telemetry: semaphore wait time, EDT queue depth, restart count и shutdown/restart drain stats
- [x] Расширить MCP regression/stress suite: `tools/list` contract, все 8 stdio tools, HTTP admission/tool-call regressions и `dump_config(PARTIAL)` matrix для `DESIGNER`/`IBCMD`
- [x] 2026-03-21: `spec/MCP_IMPLEMENTATION_PLAN.md` сохранён как canonical staged MCP rollout history/reference; `spec/IMPLEMENTATION_TODO.md` остаётся активным backlog для follow-up и немигрированных EDT-задач.
