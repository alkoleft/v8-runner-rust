# Backlog реализации Rust CLI

Документ опирается на:

- `spec/FUNCTIONAL_CAPABILITIES.md`
- `spec/KEY_COMPONENTS.md`

Текущий scope: реализуем CLI-решение на Rust. MCP-слой в этот этап не входит.

## 1. Цель первой и второй волны

### Волна 1: MVP для повседневной работы

Цель: получить рабочий Rust CLI, который покрывает основной сценарий локальной разработки без MCP:

- запуск `build`;
- запуск YaXUnit-тестов;
- `dump` из ИБ;
- синтаксические проверки через Designer;
- запуск приложений 1С;
- on-demand change detection;
- JSON/text output для интеграции с IDE, shell и CI.

Ограничения волны 1:

- `DESIGNER` backend, плюс `IBCMD` для build/dump;
- без поддержки `EDT`;
- без фоновых процессов уровня приложения;
- без совместимости с MCP DTO один в один, но со структурированным CLI output.

### Волна 2: расширение до полного актуального spec

Цель: довести CLI почти до текущих реальных возможностей продукта:

- поддержка `EDT`;
- интерактивная `1cedtcli`-сессия внутри CLI-процесса;
- `syntax edt`;
- двухстадийный `EDT -> Designer -> build`;
- выравнивание структуры output под будущую возможную адаптацию к MCP.

## 2. Рекомендуемая структура Rust-проекта

### Корневые файлы

- `Cargo.toml`
- `Cargo.lock`
- `README.md`
- `.gitignore`
- `examples/v8project.yaml`

### Исходники

```text
src/
  main.rs
  app.rs
  cli/
    mod.rs
    args.rs
    output.rs
  config/
    mod.rs
    loader.rs
    model.rs
    validate.rs
  domain/
    mod.rs
    source_set.rs
    build.rs
    test.rs
    dump.rs
    syntax.rs
    launch.rs
    issue.rs
  use_cases/
    mod.rs
    build_project.rs
    run_tests.rs
    dump_config.rs
    check_syntax.rs
    launch_app.rs
  change_detection/
    mod.rs
    scanner.rs
    file_state.rs
    hash_storage.rs
    analyzer.rs
    partial_load.rs
  platform/
    mod.rs
    process.rs
    locator.rs
    connection.rs
    designer.rs
    enterprise.rs
    edt.rs
    ibcmd.rs
    interactive.rs
  parsers/
    mod.rs
    junit.rs
    yaxunit_log.rs
    designer_validation.rs
    edt_validation.rs
  output/
    mod.rs
    presenter.rs
    json.rs
    text.rs
  support/
    mod.rs
    fs.rs
    temp.rs
    time.rs
```

### Тесты

```text
tests/
  cli_build.rs
  cli_test.rs
  cli_dump.rs
  fixtures/
    junit/
    logs/
    designer/
    edt/
```

## 3. Рекомендуемые зависимости

Минимальный набор:

- `clap` для CLI;
- `serde`, `serde_json`, `serde_yaml` для конфигурации и output;
- `thiserror` или `anyhow` для ошибок;
- `tracing`, `tracing-subscriber` для логирования;
- `walkdir` для обхода файлов;
- `sha2` для hashing;
- `quick-xml` для JUnit XML;
- `tempfile` для временных файлов;
- `camino` для более безопасной работы с путями;
- `chrono` или `time` для timestamps.

Опционально:

- `tokio` только если будет нужен удобный async для subprocess orchestration;
- `assert_cmd`, `insta`, `predicates` для CLI-тестов;
- `parking_lot` для упрощения синхронизации интерактивных executor-ов.

## 4. Рекомендуемый CLI API

Имя бинаря:

- `v8-runner`

Глобальные флаги:

- `--config <PATH>` путь к YAML-конфигу;
- `--output <text|json>` формат вывода;
- `--log-level <error|warn|info|debug|trace>`;
- `--clean-before-execution` очистить лог-файлы перед выполнением;
- `--no-color` отключить ANSI;
- `--workdir <PATH>` опционально переопределить рабочую директорию.

### Команды волны 1

```bash
v8-runner build [--full-rebuild]

v8-runner test all
v8-runner test module <MODULE_NAME>

v8-runner dump --mode <full|incremental|partial> \
  [--source-set <NAME>] \
  [--extension <NAME>] \
  [--object <TYPE:NAME> ...]

v8-runner syntax designer-config
v8-runner syntax designer-modules

v8-runner launch --mode <designer|thin|thick>
```

### Команды волны 2

```bash
v8-runner syntax edt --project <EDT_PROJECT_NAME> ...

v8-runner build --format edt [--full-rebuild]
```

Примечания по API:

- `test all` и `test module` должны всегда вызывать `build` до запуска тестов;
- `dump --mode partial` пока не реализован (планируется требование минимум одного `--object`);
- `syntax edt` должен работать по именам EDT-проектов;
- `build --full-rebuild` должен сбрасывать state cache, а не менять platform commands.

### Рекомендуемый формат JSON output

Единый envelope:

```json
{
  "ok": true,
  "command": "test module",
  "duration_ms": 1234,
  "data": {},
  "warnings": [],
  "steps": []
}
```

Поля envelope:

- `ok`: итоговый статус;
- `command`: выполненная команда;
- `duration_ms`: длительность;
- `data`: полезная нагрузка;
- `warnings`: нефатальные замечания;
- `steps`: шаги пайплайна с длительностями и статусами.

## 5. Волна 1: прикладной backlog

### Эпик 1. Базовый каркас CLI и конфигурация

Задачи:

1. Создать `Cargo.toml` и базовый bin crate.
2. Поднять `clap`-модель команд и глобальных флагов.
3. Реализовать загрузку YAML-конфига.
4. Реализовать валидацию конфигурации проекта.
5. Ввести единый `AppError` и коды завершения CLI.
6. Добавить JSON/text presenter.

Модули и файлы:

- `src/main.rs`
- `src/app.rs`
- `src/cli/args.rs`
- `src/cli/output.rs`
- `src/config/loader.rs`
- `src/config/model.rs`
- `src/config/validate.rs`
- `src/output/presenter.rs`
- `src/output/json.rs`
- `src/output/text.rs`

Definition of done:

- CLI печатает `--help`;
- конфиг читается из `--config`;
- невалидный конфиг дает понятную ошибку;
- все команды возвращают единый envelope в `json`.

### Эпик 2. Инфраструктура процессов и файлов

Задачи:

1. Реализовать универсальный `ProcessExecutor`.
2. Реализовать работу с временными файлами и каталогами.
3. Реализовать очистку логов перед выполнением.
4. Реализовать поиск бинарников платформы.
5. Выделить общий слой запуска platform commands.
6. Реализовать управляемую схему рабочих директорий: `workPath/<sourceSetName>/...` для EDT-экспорта.

Модули и файлы:

- `src/platform/process.rs`
- `src/platform/locator.rs`
- `src/platform/connection.rs`
- `src/support/fs.rs`
- `src/support/temp.rs`

Definition of done:

- можно запустить внешний процесс с захватом `stdout`, `stderr`, exit code;
- можно создать временный файл для list file и YaXUnit config;
- можно получить путь к `1cv8` и смежным утилитам из конфига или auto-discovery;
- рабочая директория EDT создаётся по схеме `workPath/<sourceSetName>/` и управляется предсказуемо.

### Эпик 3. Change detection и persistent state

Задачи:

1. Реализовать `Scanner` с игнорированием служебных путей.
2. Реализовать двухфазный алгоритм `mtime -> hash`.
3. Реализовать `redb` storage для hashes/timestamps и метаданных watermark/generation.
4. Реализовать группировку изменений по `source-set`.
5. Реализовать safe fallback: при ошибке сканирования считать все измененным.
6. Реализовать `SourceSetsService`: выдавать `SourceSetContext` для EDT и Designer, прикреплять отдельное hash storage к каждому логическому источнику. В режиме `DESIGNER` — один контекст; в режиме `EDT` — два независимых контекста (исходный EDT source-set и временный Designer source-set в `workPath`).

Модули и файлы:

- `src/change_detection/scanner.rs`
- `src/change_detection/file_state.rs`
- `src/change_detection/hash_storage.rs`
- `src/change_detection/analyzer.rs`
- `src/domain/source_set.rs`

Definition of done:

- state хранится в `workPath/hash-storages/*.redb`, изолированно по имени логического source-set;
- после успешного build state обновляется;
- если изменений нет, build pipeline может быть пропущен;
- `SourceSetsService` корректно разделяет EDT и Designer контексты в EDT-режиме.

### Эпик 4. Designer backend и build pipeline

Задачи:

1. Реализовать `DesignerDsl` поверх `ProcessExecutor`.
2. Реализовать `build_project` use case.
3. Реализовать partial/full decision logic.
4. Реализовать `PartialLoadListGenerator`.
5. Реализовать последовательную загрузку конфигурации и расширений.
6. Сохранять state только после успешной загрузки.

Модули и файлы:

- `src/platform/designer.rs`
- `src/use_cases/build_project.rs`
- `src/change_detection/partial_load.rs`
- `src/domain/build.rs`

Definition of done:

- команда `build` работает для `DESIGNER`;
- `--full-rebuild` очищает change cache;
- partial load запрещается при `Configuration.xml` и при слишком большом числе изменений.

### Эпик 5. YaXUnit test pipeline

Задачи:

1. Реализовать генерацию временного JSON-конфига YaXUnit.
2. Реализовать `EnterpriseDsl` для запуска `RunUnitTests=...`.
3. Реализовать `run_tests` use case с обязательным предварительным build.
4. Реализовать JUnit XML parser.
5. Реализовать потоковый parser `[ERR]` блоков YaXUnit-лога.
6. Сформировать summary и подробный test result.

Модули и файлы:

- `src/platform/enterprise.rs`
- `src/use_cases/run_tests.rs`
- `src/domain/test.rs`
- `src/parsers/junit.rs`
- `src/parsers/yaxunit_log.rs`

Definition of done:

- `test all` и `test module` всегда запускают build;
- провал build не позволяет начать тесты;
- JSON output содержит summary, suites, failed cases и extracted errors.

### Эпик 6. Dump pipeline для Designer

Задачи:

1. Реализовать `dump_config` use case.
2. Реализовать режимы `FULL`, `INCREMENTAL`, `PARTIAL`.
3. Реализовать генерацию списка объектов для partial dump.
4. Валидировать комбинацию `source-set`, `extension`, `mode`, `objects`.

Модули и файлы:

- `src/use_cases/dump_config.rs`
- `src/domain/dump.rs`
- `src/platform/designer.rs`

Definition of done:

- `dump --mode full|incremental|partial` работает для Designer;
- partial без `--object` возвращает validation error;
- output содержит режим, target path и summary по выгрузке.

### Эпик 7. Syntax checks для Designer

Задачи:

1. Реализовать `CheckConfig`.
2. Реализовать `CheckModules`.
3. Реализовать parser designer validation logs.
4. Реализовать единый `check_syntax` use case.

Модули и файлы:

- `src/use_cases/check_syntax.rs`
- `src/domain/syntax.rs`
- `src/domain/issue.rs`
- `src/parsers/designer_validation.rs`
- `src/platform/designer.rs`

Definition of done:

- `syntax designer-config` возвращает structured issues;
- `syntax designer-modules` возвращает structured issues;
- CLI не деградирует до raw stdout-only ответа.

### Эпик 8. Launch приложений 1С

Задачи:

1. Реализовать запуск `designer`, `thin`, `thick`.
2. Реализовать асинхронный fire-and-forget режим.
3. Вернуть pid и статус запуска, если они доступны.

Модули и файлы:

- `src/use_cases/launch_app.rs`
- `src/domain/launch.rs`
- `src/platform/enterprise.rs`
- `src/platform/designer.rs`

Definition of done:

- `launch --mode designer|thin|thick` возвращает структурированный ответ;
- старт не блокирует CLI дольше, чем нужно на spawn процесса.

### Эпик 9. Тестирование и эксплуатация волны 1

Задачи:

1. Покрыть unit-тестами parsers и change detection.
2. Добавить integration tests для CLI surface.
3. Подготовить `examples/v8project.yaml`.
4. Обновить `README.md` по запуску CLI.

Модули и файлы:

- `tests/cli_build.rs`
- `tests/cli_tests.rs`
- `tests/cli_dump.rs`
- `tests/fixtures/...`
- `README.md`
- `examples/v8project.yaml`

Definition of done:

- парсеры покрыты fixture-based тестами;
- CLI smoke-тесты проходят локально;
- есть минимальная инструкция запуска.

## 6. Волна 2: прикладной backlog

### Эпик 10. Поддержка формата EDT

Задачи:

1. Расширить config-модель полями `format = EDT`.
2. Реализовать `SourceSetsService`, который выдает `SourceSetContext` для EDT и Designer.
3. Реализовать два логических контекста source-set:
   - EDT source-set;
   - временный Designer source-set в `workPath`.
4. Реализовать change detection отдельно для EDT и Designer представления.

Модули и файлы:

- `src/config/model.rs`
- `src/domain/source_set.rs`
- `src/use_cases/build_project.rs`
- `src/change_detection/*`

Definition of done:

- build pipeline корректно различает исходный EDT source-set и сгенерированный Designer source-set.
- `SourceSetsService` корректно разделяет EDT и Designer контексты в EDT-режиме;
- у каждого логического source-set свое hash storage.

### Эпик 11. Интерактивный EDT executor

Задачи:

1. Реализовать `InteractiveProcessExecutor`.
2. Реализовать ожидание prompt `1C:EDT>`.
3. Реализовать отправку команды и чтение ответа до следующего prompt.
4. Реализовать штатное завершение и forced kill.

Модули и файлы:

- `src/platform/interactive.rs`
- `src/platform/edt.rs`

Definition of done:

- один CLI-процесс может многократно использовать одну интерактивную `1cedtcli`-сессию;
- команды export/validate не стартуют новый EDT-процесс на каждый вызов внутри одного сценария.

### Эпик 12. EDT export pipeline

Задачи:

1. Реализовать `EdtDsl`.
2. Реализовать экспорт только измененных EDT-проектов.
3. Выгружать экспорт в `workPath/<sourceSetName>/...`.
4. Встроить export как первую фазу EDT build.

Модули и файлы:

- `src/platform/edt.rs`
- `src/use_cases/build_project.rs`
- `src/domain/build.rs`

Definition of done:

- `build` в EDT-режиме сначала делает `EDT -> Designer export`, затем обычный Designer build;
- экспортные каталоги создаются и переиспользуются по схеме `workPath/<sourceSetName>/...`.

### Эпик 13. EDT syntax validate

Задачи:

1. Реализовать `syntax edt`.
2. Реализовать parser EDT validation log.
3. Нормализовать EDT issues в общую модель issue/result.

Модули и файлы:

- `src/use_cases/check_syntax.rs`
- `src/parsers/edt_validation.rs`
- `src/platform/edt.rs`
- `src/domain/issue.rs`

Definition of done:

- `syntax edt --project ...` возвращает structured issues.

### Эпик 14. IBCMD backend

Задачи:

1. Реализовать `IbcmdDsl`.
2. Реализовать `build` через `config import` и `config apply`.
3. Реализовать `dump` через `--force` и `--sync`.
4. Явно ограничить или валидировать сценарии, завязанные на файловую ИБ.

Модули и файлы:

- `src/platform/ibcmd.rs`
- `src/use_cases/build_project.rs`
- `src/use_cases/dump_config.rs`
- `src/platform/connection.rs`

Definition of done:

- backend выбирается через config;
- ограничения IBCMD на connection type отражены в validation и docs.

### Эпик 15. Выравнивание output и будущая готовность к transport-слою

Задачи:

1. Нормализовать result types для build/test/dump/syntax/launch.
2. Добавить step-level telemetry.
3. Подготовить mapping, пригодный для будущего MCP adapter слоя.

Модули и файлы:

- `src/domain/*.rs`
- `src/output/*.rs`
- `src/use_cases/*.rs`

Definition of done:

- прикладное ядро не зависит от CLI parsing;
- transport adapter можно будет добавить поверх use cases.

## 7. Порядок реализации

Рекомендуемая последовательность:

1. Эпики 1-3.
2. Эпик 4.
3. Эпик 5.
4. Эпики 6-8.
5. Эпик 9.
6. Эпики 10-13.
7. Эпик 14.
8. Эпик 15.

## 8. Что не включать в первые итерации

Не включать в волну 1:

- MCP transport;
- background daemon для file watching;
- постоянный monitor `1cedtcli` между запусками CLI;
- команды, отсутствующие в актуальном spec;
- premature abstraction под все возможные backend-ы до появления работающего Designer MVP.

## 9. Критерии готовности волны 1

Волна 1 считается завершенной, если:

- есть один бинарь `v8-runner`;
- команда `build` работает для `DESIGNER`;
- команды `test all` и `test module` работают и возвращают структурированный output;
- `dump`, `syntax designer-config`, `syntax designer-modules`, `launch` доступны;
- реализован persistent state и partial/full decision logic;
- есть README и пример конфига;
- есть базовые unit/integration tests.

## 10. Критерии готовности волны 2

Волна 2 считается завершенной, если:

- CLI поддерживает `EDT`;
- `syntax edt` работает;
- `EDT -> Designer -> build` pipeline реализован;
- `IBCMD` backend реализован с явными ограничениями;
- структура output пригодна для последующего добавления transport layer.
