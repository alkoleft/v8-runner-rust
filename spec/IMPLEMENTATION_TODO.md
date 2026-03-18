# TODO реализации Rust CLI

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

- [ ] Реализовать `ProcessExecutor`
- [ ] Реализовать захват `stdout/stderr/exit code`
- [ ] Реализовать запись и чтение log-файлов платформы
- [ ] Реализовать временные файлы для partial lists и YaXUnit config
- [ ] Реализовать поиск бинарников `1cv8`, `1cv8c`, `ibcmd`, `1cedtcli`
- [ ] Развести ответственность между locator и platform DSL

## Change detection

- [ ] Реализовать `Scanner` с рекурсивным обходом
- [ ] Игнорировать `.git/`, `.gradle/`, `build/`, `target/`, `temp/`, `tmp/`, `ConfigDumpInfo.xml`, `.yaxunit/`
- [ ] Реализовать отбор кандидатов по `lastModified`
- [ ] Реализовать hashing содержимого для кандидатов
- [ ] Реализовать JSON storage в `workPath/hash-storages/*.json`
- [ ] Реализовать fallback на "все изменено" при ошибке сканирования
- [ ] Реализовать группировку изменений по `source-set`
- [ ] Реализовать `SourceSetsService`: выдавать `SourceSetContext` для EDT и Designer, прикреплять отдельное hash storage к каждому логическому источнику; в EDT-режиме — два независимых контекста (исходный EDT и временный Designer в `workPath`)

## Волна 1: Designer MVP

### Build

- [ ] Реализовать `DesignerDsl`
- [ ] Реализовать `build_project` use case
- [ ] Реализовать `--full-rebuild` как очистку state cache
- [ ] Реализовать выбор затронутых `source-set`
- [ ] Реализовать `PartialLoadListGenerator`
- [ ] Для `.bsl`-файлов добавлять в list связанные XML и каталог объекта
- [ ] Запретить partial при изменении `Configuration.xml`
- [ ] Запретить partial при превышении порога числа файлов
- [ ] Сохранять state только после успешного build

### Tests

- [ ] Реализовать `EnterpriseDsl`
- [ ] Реализовать генерацию временного JSON-конфига YaXUnit
- [ ] Реализовать `test all`
- [ ] Реализовать `test module <MODULE_NAME>`
- [ ] Гарантировать обязательный `build` перед тестами
- [ ] Реализовать parser JUnit XML
- [ ] Реализовать parser `[ERR]` блоков YaXUnit-лога
- [ ] Вернуть summary, suites, cases и extracted errors
- [ ] Реализовать compact/full режим тестового ответа: compact скрывает passed-тесты и урезает stack trace

### Dump

- [ ] Реализовать `dump --mode full`
- [ ] Реализовать `dump --mode incremental`
- [ ] Реализовать `dump --mode partial`
- [ ] Валидировать, что `partial` требует минимум один `--object`
- [ ] Поддержать выбор `source-set`
- [ ] Поддержать выбор `extension`

### Syntax

- [ ] Реализовать `syntax designer-config`
- [ ] Реализовать `syntax designer-modules`
- [ ] Валидировать, что для `syntax designer-modules` включён хотя бы один режим проверки
- [ ] Реализовать parser designer validation logs
- [ ] Вернуть structured issues вместо raw stdout

### Launch

- [ ] Реализовать `launch --mode designer`
- [ ] Реализовать `launch --mode thin`
- [ ] Реализовать `launch --mode thick`
- [ ] Возвращать статус запуска и доступные process details

## Волна 1: тестирование и документация

- [ ] Добавить unit-тесты на change detection
- [ ] Добавить unit-тесты на `PartialLoadListGenerator`
- [ ] Добавить unit-тесты на JUnit parser
- [ ] Добавить unit-тесты на YaXUnit log parser
- [ ] Добавить unit-тесты на designer validation parser
- [ ] Добавить integration tests для CLI команд
- [ ] Подготовить fixture-наборы логов и XML
- [ ] Обновить `README.md`

## Волна 2: EDT

- [ ] Расширить config-модель для `format = EDT`
- [ ] Ввести отдельные state storage для `edt` и `designer`
- [ ] Реализовать `InteractiveProcessExecutor`
- [ ] Реализовать ожидание prompt `1C:EDT>`
- [ ] Реализовать мониторинг живости EDT-процесса и автоперезапуск при сбое
- [ ] Реализовать single-flight инициализацию EDT-сессии
- [ ] Реализовать `EdtDsl`
- [ ] Реализовать экспорт только измененных EDT `source-set`
- [ ] Реализовать временный Designer-каталог в `workPath/<sourceSetName>/`
- [ ] Встроить export как первую фазу EDT build
- [ ] Реализовать `syntax edt`
- [ ] Реализовать parser EDT validation logs (TSV-подобный формат: каждая строка — отдельный issue)

## Волна 2: IBCMD

- [ ] Реализовать `IbcmdDsl`
- [ ] Реализовать build через `config import` и `config apply`
- [ ] Реализовать dump `FULL` через `--force`
- [ ] Реализовать dump `INCREMENTAL` через `--sync`
- [ ] Явно задокументировать: partial dump по объектам в IBCMD backend не поддерживается
- [ ] Валидировать ограничения IBCMD для connection type (только файловая ИБ)
- [ ] Задокументировать ограничения IBCMD backend

## Подготовка к следующему этапу

- [ ] Нормализовать result types для build/test/dump/syntax/launch
- [ ] Добавить `steps` в output envelope
- [ ] Добавить `warnings` в output envelope
- [ ] Убедиться, что use cases не зависят от CLI parsing
- [ ] Оставить место для будущего transport adapter слоя
