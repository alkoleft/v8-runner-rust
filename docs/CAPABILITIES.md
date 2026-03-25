# Возможности

Публичный справочник по тому, что `v8-test-runner` поддерживает на текущий момент.

Последняя факт-проверка: `2026-03-25` по свежей локальной сборке `cargo build`, актуальному CLI `--help`, `src/config/model.rs` и реальной MCP-поверхности запросов в `src/mcp/request.rs` / `src/mcp/service.rs`.

Если этот документ расходится со старыми внутренними заметками в `spec/*`, доверяйте текущему коду и CLI-интерфейсу.

## Матрица сценариев

| Сценарий | Поддерживаемые комбинации | Примечания |
| --- | --- | --- |
| `init` | `format=DESIGNER` + `builder=DESIGNER` | Создаёт файловую ИБ через `1cv8 CREATEINFOBASE`, если отсутствует |
| `init` | `format=DESIGNER` + `builder=IBCMD` | Создаёт файловую ИБ через `ibcmd infobase create`, если отсутствует |
| `init` | `format=EDT` + `builder=DESIGNER` | Создаёт файловую ИБ и, если workspace отсутствует, импортирует все EDT `source-set` в `workPath/edt-workspace` |
| `build` | `format=DESIGNER` + `builder=DESIGNER` | Инкрементальная или полная загрузка через Designer |
| `build` | `format=DESIGNER` + `builder=IBCMD` | Использует `ibcmd config import` + `config apply`; только файловая ИБ |
| `build` | `format=EDT` + `builder=DESIGNER` | Определяет EDT-изменения, экспортирует затронутые `source-set`, затем загружает Designer-вывод |
| `test` | Та же матрица, что и у `build` | Всегда сначала запускает `build`, затем YaXUnit через Enterprise |
| `dump` | `format=DESIGNER` + `builder=DESIGNER` | Полная, инкрементальная или точечная частичная выгрузка объектов |
| `dump` | `format=DESIGNER` + `builder=IBCMD` | Полная и инкрементальная выгрузка; запрос `partial` деградирует в инкрементальную выгрузку с предупреждением |
| `syntax` | `syntax designer-config` и `syntax designer-modules` требуют `builder=DESIGNER`, `format=DESIGNER` | Проверки через Designer |
| `syntax` | `syntax edt` требует `builder=DESIGNER`, `format=EDT` | Проверка через EDT `validate` |
| `launch` | У команды нет отдельного деления по форматам | Требует соответствующую локальную утилиту 1С |
| MCP | stdio и транспорт по протоколу `streamable HTTP` | Оба публикуют один и тот же набор из 8 инструментов |

## Общие CLI-опции

Все команды разделяют следующие глобальные опции:

| Опция | Значение |
| --- | --- |
| `--config <CONFIG>` | Путь к YAML-конфигу; также доступен через `V8TR_CONFIG` |
| `--output <OUTPUT>` | `text` или `json` |
| `--log-level <LOG_LEVEL>` | `error`, `warn`, `info`, `debug`, `trace` |
| `--clean-before-execution` | Очистить лог-файлы перед запуском команды |
| `--no-color` | Отключить ANSI-цвета |
| `--workdir <WORKDIR>` | Переопределить `workPath` из конфига |

## Команда `build`

```bash
v8-test-runner build [--full-rebuild]
```

Поведение:

- Всегда обрабатывает `CONFIGURATION` первой, затем расширения в порядке из конфига.
- Использует механизм отслеживания изменений по каждому `source-set`, чтобы пропускать нетронутую работу.
- Может выбрать частичное или полное выполнение в зависимости от изменённых файлов и `build.partialLoadThreshold`.
- Намеренно не является атомарной по нескольким `source-set`: если поздний шаг упал, ранние успешные шаги остаются применёнными.

Примечания по режимам:

- `format=DESIGNER`, `builder=DESIGNER`: загружает изменённые Designer-исходники напрямую через бэкенд Designer.
- `format=DESIGNER`, `builder=IBCMD`: загружает исходники в Designer-формате через `ibcmd`.
- `format=EDT`, `builder=DESIGNER`: экспортирует изменённые EDT `source-set` во временные Designer-файлы под `workPath/designer`, затем запускает обычный конвейер Designer.

Важные детали:

- `--full-rebuild` форсирует полное выполнение текущего запуска и не зависит от ручного удаления состояния отслеживания изменений.
- Изменения в `Configuration.xml` принудительно переключают выполнение в режим полной загрузки.
- При выборе частичной загрузки реальный набор файлов может расширяться относительно исходного списка изменений.

## Команда `init`

```bash
v8-test-runner init
```

Поведение:

- Всегда выполняет два независимых шага: создание файловой ИБ и инициализацию EDT workspace.
- Падение шага создания ИБ не блокирует EDT-шаг; общий результат остаётся неуспешным, если любой шаг завершился ошибкой.
- Файловая ИБ считается существующей только при наличии файла `1Cv8.1CD` в каталоге базы.
- Для `builder=DESIGNER` создание ИБ идёт через `1cv8 CREATEINFOBASE`.
- Для `builder=IBCMD` создание ИБ идёт через `ibcmd infobase create`.
- Для `format=EDT` workspace создаётся в `workPath/edt-workspace`, а импорт проектов идёт в порядке `CONFIGURATION`, затем `EXTENSION`.
- Если `workPath/edt-workspace` уже существует и содержит внутренний marker успешной инициализации, EDT-шаг пропускается.
- Если каталог workspace уже есть, но marker успешной инициализации отсутствует, `init` повторяет импорт всех EDT-проектов.

## Команда `test`

```bash
v8-test-runner test [--full] all
v8-test-runner test [--full] module <NAME>
```

Поведение:

- Всегда сначала запускает `build`.
- `test module <NAME>` требует непустое имя модуля.
- Компактный режим скрывает успешно прошедшие кейсы и сокращает трассы стека.
- `--full` сохраняет успешно прошедшие кейсы и полные трассы стека.
- YaXUnit должен быть уже установлен и доступен из целевой информационной базы.

Артефакты и сохранение:

- Для каждого запуска генерируется временный JSON-конфиг YaXUnit.
- JUnit XML и YaXUnit-логи разбираются в структурированный вывод.
- Если выполнение упало или JUnit-отчёт не удалось распарсить, сохранённые артефакты остаются под `workPath/temp/yaxunit/runs/<run-id>/`.

Связанный конфиг:

- `tests.execution_timeout_seconds` управляет жёстким тайм-аутом для запуска Enterprise.
- Флаг `--full` относится именно к команде `test`, поэтому его нужно ставить до `all` или `module`.

## Команда `dump`

```bash
v8-test-runner dump --mode <full|incremental|partial> [--source-set <NAME>] [--extension <EXTENSION>] [--object <TYPE:NAME>...]
```

Поведение:

- Поддерживает режимы `full`, `incremental` и `partial`.
- `partial` требует хотя бы один `--object`.
- Пустые значения объектов и управляющие символы отклоняются.
- `--source-set` явно выбирает целевой `source-set`.
- `--extension` нацеливает выгрузку на конкретное расширение.

Особенности бэкендов:

- `builder=DESIGNER`: `partial` выполняет точечную выгрузку объектов через частичную выгрузку Designer.
- `builder=IBCMD`: прямая точечная частичная выгрузка по объектам недоступна. Запрос `partial` деградирует в инкрементальную выгрузку для разрешённой цели и возвращает предупреждение, сохраняя запрошенный режим как `PARTIAL` в результирующем ответе.

## Команда `syntax`

```bash
v8-test-runner syntax designer-config [FLAGS]
v8-test-runner syntax designer-modules [FLAGS]
v8-test-runner syntax edt [--project <PROJECT>...]
```

### `syntax designer-config`

Поддерживается только при `builder=DESIGNER` и `format=DESIGNER`.

Доступные группы флагов:

- Базовые проверки: `--config-log-integrity`, `--incorrect-references`, `--unsupported-functional`
- Селекторы контекста: `--thin-client`, `--web-client`, `--mobile-client`, `--server`, `--external-connection`, `--external-connection-server`, `--mobile-app-client`, `--mobile-app-server`
- Варианты толстого клиента: `--thick-client-managed-application`, `--thick-client-server-managed-application`, `--thick-client-ordinary-application`, `--thick-client-server-ordinary-application`
- Дополнительные флаги валидации: `--mobile-client-digi-sign`, `--distributive-modules`, `--unreference-procedures`, `--handlers-existence`, `--empty-handlers`, `--extended-modules-check`, `--check-use-synchronous-calls`, `--check-use-modality`
- Область применения по расширениям: `--extension <EXTENSION>` или `--all-extensions`

Ограничения:

- `--extension` конфликтует с `--all-extensions`.
- `--check-use-synchronous-calls` требует `--extended-modules-check`.
- `--check-use-modality` требует `--extended-modules-check`.

### `syntax designer-modules`

Поддерживается только при `builder=DESIGNER` и `format=DESIGNER`.

Доступные флаги:

- Селекторы режима: `--thin-client`, `--web-client`, `--server`, `--external-connection`, `--thick-client-ordinary-application`, `--mobile-app-client`, `--mobile-app-server`, `--mobile-client`
- Дополнительные флаги области применения: `--extended-modules-check`, `--extension <EXTENSION>`, `--all-extensions`

Ограничения:

- Нужен хотя бы один флаг режима.
- `--extension` конфликтует с `--all-extensions`.

### `syntax edt`

Поддерживается только при `builder=DESIGNER` и `format=EDT`.

Поведение:

- `--project <PROJECT>` можно передавать несколько раз.
- Если проекты не переданы, команда использует дефолтный набор EDT-проектов из конфига.
- После разбора вывода `validate` команда возвращает структурированные EDT-проблемы.

## Команда `launch`

```bash
v8-test-runner launch --mode <designer|thin|thick>
```

Поведение:

- `designer` запускается через `1cv8`.
- `thin` запускается через `1cv8c`.
- `thick` запускается через `1cv8`.
- Успешный результат включает статус запуска и сведения о процессе, например PID и определённый путь к бинарю.

## MCP

```bash
v8-test-runner mcp serve stdio
v8-test-runner mcp serve http
```

Общее поведение транспортов:

- MCP-сервер объявляет только возможность работы с инструментами.
- Один и тот же набор из 8 инструментов на обоих транспортах.
- Поля запросов используют `camelCase`.
- Бизнес-ошибки возвращаются как структурированные ошибки инструмента, а ошибки неправильного использования, адаптера и рантайма остаются на транспортном уровне.
- Все вызовы инструментов разделяют `mcp.execution.max_concurrent_calls`.

### Опубликованные инструменты

| Инструмент | Основные поля запроса | Примечания |
| --- | --- | --- |
| `build_project` | `fullRebuild` | По умолчанию `false` |
| `run_all_tests` | `full` | По умолчанию компактный вывод |
| `run_module_tests` | `moduleName`, `full` | Отклоняет пустой `moduleName` |
| `dump_config` | `mode`, `extension`, `objects` | `mode=null` или пустой `mode` по умолчанию превращается в `INCREMENTAL` |
| `launch_app` | `utilityType` | Принимает алиасы вроде `designer`, `1cv8`, `thin`, `thin_client`, `1cv8c`, `thick`, `thick_client`, а также поддерживаемые русские алиасы |
| `check_syntax_edt` | `projectName` | Пустой или отсутствующий `projectName` означает "проверить все настроенные EDT-проекты" |
| `check_syntax_designer_config` | Флаги `designer-config` в `camelCase` | `allExtensions` имеет три состояния; область расширений нормализуется в сервисном слое |
| `check_syntax_designer_modules` | Флаги `designer-modules` в `camelCase` | `allExtensions` имеет три состояния; область расширений нормализуется в сервисном слое |

Дополнительные правила MCP-нормализации:

- Значение по умолчанию для `allExtensions` выводится из того, передан ли `extension`.
- `checkUseSynchronousCalls` и `checkUseModality` отклоняются, когда `extendedModulesCheck=false`.
- `check_syntax_edt` — единственный MCP-инструмент, который сейчас выполняется через общую живую EDT-сессию.

### Особенности HTTP-транспорта

Ключи конфига:

- `mcp.http.bind_address`
- `mcp.http.path`
- `mcp.http.stateful_sessions`
- `mcp.http.max_sessions`
- `mcp.http.idle_ttl_secs`

Поведение:

- Режим с состоянием включён по умолчанию.
- Создание новых HTTP-сессий ограничено `max_sessions`.
- В режиме без состояния отключается жизненный цикл MCP-сессий на `GET` и `DELETE`.

## Ключи конфигурации

| Ключ | Обязателен | Значение по умолчанию / примечания |
| --- | --- | --- |
| `basePath` | Да | Корневой путь к исходникам проекта |
| `workPath` | Да | Корень для артефактов выполнения, временных файлов, логов и хранилищ хешей |
| `format` | Нет | `DESIGNER` |
| `builder` | Нет | `DESIGNER` |
| `connection` | Да | Строка подключения к информационной базе в сыром виде |
| `credentials.user` | Нет | Опциональный пользователь ИБ |
| `credentials.password` | Нет | Опциональный пароль ИБ |
| `source-set[]` | Да | Каждый элемент содержит `name`, `purpose`, `path`; используйте `CONFIGURATION` и `EXTENSION` |
| `build.partialLoadThreshold` | Нет | `20` |
| `tests.execution_timeout_seconds` | Нет | `300` |
| `tools.platform.path` | Нет | Может указывать на бинарь, `bin`-директорию или корень платформы с версиями |
| `tools.platform.version` | Нет | Точная подсказка по версии платформы |
| `tools.edt_cli.path` | Нет | Опциональный путь к `1cedtcli`, корню установки EDT или version-like подсказка для автопоиска |
| `tools.edt_cli.version` | Нет | Отдельная version-like подсказка для автопоиска EDT, например `2025.2.3` |
| `tools.edt_cli.auto-start` | Нет | `false` |
| `tools.edt_cli.startup_timeout_ms` | Нет | `300000`; также принимает `startup-timeout-ms` |
| `tools.edt_cli.command_timeout_ms` | Нет | `300000`; также принимает `command-timeout-ms` |
| `tools.edt_cli` | Нет | Также принимает обратносовместимый алиас ключа `edt-cli` |
| `mcp.http.bind_address` | Нет | `127.0.0.1:3000` |
| `mcp.http.path` | Нет | `/mcp` |
| `mcp.http.stateful_sessions` | Нет | `true` |
| `mcp.http.max_sessions` | Нет | `64` |
| `mcp.http.idle_ttl_secs` | Нет | `900` |
| `mcp.execution.max_concurrent_calls` | Нет | `1` |
| `mcp.execution.shutdown_grace_period_secs` | Нет | `30` |

## Артефакты выполнения

Важные служебные пути под `workPath`:

- `hash-storages/*.redb`: состояние отслеживания изменений
- `logs/platform/`: логи команд платформы
- `logs/mcp/actions.log`: трассировка MCP
- `temp/partial-lists/`: сгенерированные списки частичной загрузки
- `temp/yaxunit/runs/<run-id>/`: сохранённые YaXUnit-артефакты при падении или проблемах парсинга

## Пока не поддерживается

- Нет публичного MCP-инструмента для `list_modules`.
- Нет публичного MCP-инструмента для `get_configuration`.
- Нет публичного MCP-инструмента для `check_platform`.
- `IBCMD` не предоставляет нативную точечную частичную выгрузку по объектам.
- Общая интерактивная EDT-сессия не является путём выполнения для всех EDT-сценариев; текущая долгоживущая сессия сосредоточена на MCP `check_syntax_edt`.
