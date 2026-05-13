# Возможности

Публичный каталог команд и текущих поддержанных сценариев `v8-runner`.

Документ описывает только текущий пользовательский контракт. Если он расходится с кодом или live
CLI help, доверяйте текущему коду и затем синхронизируйте docs.

## Навигация

- [Матрица поддержки](#матрица-поддержки)
- [Глобальные CLI-опции](#глобальные-cli-опции)
- [Настройка проекта](#настройка-проекта)
- [Проверка и валидация](#проверка-и-валидация)
- [Файлы и артефакты](#файлы-и-артефакты)
- [Прямой запуск и MCP](#прямой-запуск-и-mcp)
- [workPath и артефакты выполнения](#workpath-и-артефакты-выполнения)
- [Пока не поддерживается](#пока-не-поддерживается)

## Матрица поддержки

| Сценарий | Поддерживаемые комбинации | Примечания |
| --- | --- | --- |
| `config init` | Работает без существующего конфига | Создаёт `v8project.yaml`, sibling `v8project.local.yaml`, `.gitignore` entry, autodetect-ит supported `source-set` и aggregate external roots |
| `tools download <tool>` | CLI-only загрузка latest releases | Загружает выбранный YAxUnit, Vanessa Automation single или onec-client-mcp-devkit; обновляет local overlay для Vanessa/client MCP и при `yaxunit --sources` добавляет YAxUnit как `source-set` `tests` |
| `init` | `format=DESIGNER` + `builder=DESIGNER` | Создаёт файловую ИБ через Designer; server connection остаётся manual prerequisite |
| `init` | `format=DESIGNER` + `builder=IBCMD` | Выполняет `ensure` файловой или серверной ИБ через `ibcmd infobase create` |
| `init` | `format=EDT` + `builder=DESIGNER|IBCMD` | Готовит ИБ по правилам builder и импортирует EDT workspace |
| `extensions` | `format=DESIGNER` или `format=EDT` | Обновляет свойства extension `source-set` |
| `build` | `format=DESIGNER` + `builder=DESIGNER|IBCMD` | Выполняет incremental/full загрузку в ИБ |
| `build` | `format=EDT` + `builder=DESIGNER|IBCMD` | Экспортирует изменённые EDT `source-set`, затем грузит generated Designer output |
| `test` | Та же матрица, что и у `build` | Всегда сначала запускает `build` |
| `dump` | `format=DESIGNER` + `builder=DESIGNER` | Полная, инкрементальная или object-scoped partial выгрузка |
| `dump` | `format=DESIGNER` + `builder=IBCMD` | Полная и инкрементальная выгрузка; `partial` деградирует в incremental с warning |
| `dump` | `format=EDT` + `builder=DESIGNER|IBCMD` | Reverse sync из ИБ через internal Designer snapshot и EDT import |
| `convert` | CLI-only repo-aware конвертация текущих `source-set` | Не использует `builder` и не требует ИБ |
| `load` | `format=DESIGNER` + `builder=DESIGNER` | Загрузка `.cf` / `.cfe` артефактов в ИБ |
| `make` / `artifacts` | `format=DESIGNER` + `builder=DESIGNER` | Экспорт `.cf` / `.cfe` и публикация `.epf` / `.erf` |
| `syntax` | `format=DESIGNER` или `format=EDT` | Designer checks для `DESIGNER`, EDT `validate` для `EDT` |
| `launch` | Не зависит от `format` | Прямой запуск 1C utility по позиционному mode |
| MCP | `stdio` и `streamable HTTP` | Публикует 8 инструментов, уже более узкая поверхность, чем CLI |

## Глобальные CLI-опции

| Опция | Значение |
| --- | --- |
| `--config <CONFIG>` | Путь к существующему `v8project.yaml`; по умолчанию `./v8project.yaml` |
| `--json-message` | Structured JSON envelope вместо text output |
| `--log-level <LOG_LEVEL>` | `error`, `warn`, `info`, `debug`, `trace` |
| `--clean-before-execution` | Очистить лог-файлы перед запуском |
| `--no-color` | Отключить ANSI-цвета |
| `--workdir <WORKDIR>` | Переопределить `workPath` из конфига |

Если рядом с primary config лежит `v8project.local.yaml`, он применяется автоматически до CLI
overrides. Сам local overlay нельзя передавать как `--config`.

Принципы вывода:

- Без `--json-message` CLI держит clean success path кратким.
- Live progress в text output использует human-readable строки; для long-running stages время
  старта может выводиться как локальный префикс `HH:MM:SS`, без structured ключей вроде
  `started_at`.
- Важные warnings, degraded behavior, diagnostics и created artifacts должны быть видимы и в text,
  и в JSON.
- `--json-message` остаётся machine-readable contract для автоматизации.
- MCP `structured_content` использует тот же envelope core: `ok`, `command`, `duration_ms`,
  `data`, `warnings`, `steps`, optional `error`.

## Настройка проекта

### `config init`

```bash
v8-runner config init [--force] [--output <FILE>] [--connection <CONNECTION>] [--format <auto|designer|edt>] [--builder <DESIGNER|IBCMD>]
```

- Не требует существующего `v8project.yaml`.
- Пишет результат в текущий каталог или в `--output`.
- Рядом с primary config создает/обновляет пустой `v8project.local.yaml` со schema modeline и
  добавляет `v8project.local.yaml` в `.gitignore`, если подходящий pattern еще не указан.
- Не использует глобальный `--config` как shortcut output path.
- Ищет supported `DESIGNER` / `EDT` `source-set` по marker files и их содержимому.
- Для external roots создаёт aggregate `source-set` только при однородной классификации каталога.
- Не пишет synthetic `CONFIGURATION`: отсутствие конфигурационного source-set это validation error.
- Для `--builder IBCMD` найденные external roots считаются validation error.

### `init`

```bash
v8-runner init
```

- Всегда разделяет шаг подготовки ИБ и шаг EDT workspace.
- Для file connection и `builder=DESIGNER` использует `1cv8 CREATEINFOBASE`.
- Для `builder=IBCMD` использует `ibcmd infobase create`; server path добавляет
  `--create-database`.
- Для benign `already exists` при `IBCMD` возвращает non-fatal outcome.
- Для `format=EDT` использует `workPath/edt-workspace` и импортирует `CONFIGURATION`, затем
  `EXTENSION`.
- Если настроен `tools.client_mcp.extension.source.format=EDT`, импортирует этот tool extension
  project в EDT workspace, не добавляя его в project `source-set`.

### `tools download`

```bash
v8-runner tools download yaxunit [--sources] [--force]
v8-runner tools download vanessa [--force]
v8-runner tools download client-mcp [--sources] [--force]
```

- CLI-only; не публикуется как MCP tool.
- Берёт latest release из GitHub для выбранного инструмента: `bia-technologies/yaxunit`,
  `Pr-Mex/vanessa-automation-single` или `1c-neurofish/onec-client-mcp-devkit`.
- `yaxunit --sources` распаковывает source subtree в `tests` и добавляет в primary
  `v8project.yaml` `source-set` с именем `tests`; без `--sources` скачивает `.cfe` в
  `build/tools`.
- `client-mcp --sources` распаковывает source subtree в
  `build/tools/onec-client-mcp-devkit/exts/client-mcp`; без `--sources` требует
  `builder=DESIGNER` и скачивает `.cfe` в `build/tools`.
- `vanessa` всегда скачивает `build/tools/vanessa-automation-single.epf`.
- `v8project.local.yaml` обновляется только для команд, которым нужны machine-local пути:
  `vanessa` заполняет `tools.va.epf_path`, `client-mcp` заполняет
  `tools.client_mcp.extension`; повторный запуск переиспользует уже скачанные файлы, а
  `--force` перезаписывает только managed targets, созданные `tools download`.
- Managed target определяется sidecar marker-файлом `tools download`; если публикация файла или
  каталога не завершилась, новый marker очищается и target не считается управляемым.
- Каждый HTTP response body ограничен 512 MiB; превышение лимита возвращает ошибку до публикации
  target.

### `extensions`

```bash
v8-runner extensions [--name <SOURCE_SET>...]
```

- Работает только с `source-set`, у которых `type=EXTENSION`.
- Без `--name` обрабатывает все extension `source-set` из конфига.
- Возвращает пошаговый результат по каждому целевому расширению.

### `build`

```bash
v8-runner build [--source-set <NAME>] [--full-rebuild] [--dynamic]
```

- Без `--source-set` обрабатывает все configured `source-set` в canonical order.
- `--dynamic` (или `build.dynamicUpdate: true` в `v8project.yaml`) добавляет к
  `/UpdateDBCfg` флаг `-Dynamic+`. Платформа применяет изменения без захвата
  исключительной блокировки; на изменениях, требующих реструктуризации, DESIGNER возвращает
  ошибку — fallback на статический режим не выполняется.
- С `--source-set` project stage анализирует и строит только указанный `source-set`; неизвестное
  имя отклоняется как validation error.
- Для `DESIGNER` выбирает incremental, partial или full path по изменённым файлам выбранного scope.
- Для `EDT` сначала анализирует и экспортирует выбранные EDT `source-set`, затем грузит generated
  Designer files выбранным backend.
- После успешного project stage, включая scoped `--source-set`, подготавливает
  `tools.client_mcp.extension`, если оно настроено: `source` загружается как extension из
  исходников, `.cfe` `artifact` загружается как extension с именем
  `tools.client_mcp.extension.name`.
- Для source-backed `tools.client_mcp.extension` использует отдельное состояние change detection
  под `workPath/hash-storages`: неизменённый source пропускает export/load, `--full-rebuild`
  принудительно обновляет расширение.
- `tools.client_mcp.extension` не является project `source-set`; `--source-set` выбирает только
  project source-set.
- Не является атомарной multi-source-set операцией: ранние успешные шаги не откатываются, если
  поздний шаг падает.

## Проверка и валидация

### `test`

```bash
v8-runner test yaxunit [--full] all
v8-runner test yaxunit [--full] module <NAME>
v8-runner test va
v8-runner test va --feature login --filter-tag @smoke
```

- Всегда сначала запускает `build` со статическим `/UpdateDBCfg`, даже если
  `build.dynamicUpdate: true`. Для динамической подготовки перед тестами выполните отдельный
  `v8-runner build --dynamic`.
- `test yaxunit module <NAME>` требует непустое имя модуля.
- `test va` использует профиль из `tests.va.profile`; `--feature`, `--filter-tag`,
  `--ignore-tag` и `--scenario-filter` переопределяют соответствующие списки выбранного профиля
  только для текущего запуска.
- `--full` включает полный вывод успешных кейсов и расширенные stack traces.
- `tests.*.timeouts.total_ms` остаётся активным пользовательским контрактом таймаутов.

### `syntax`

```bash
v8-runner syntax designer-config [FLAGS]
v8-runner syntax designer-modules [FLAGS]
v8-runner syntax edt [--project <PROJECT>...]
```

`designer-config`:

- Только `builder=DESIGNER`, `format=DESIGNER`.
- Позволяет комбинировать config checks и client scopes.
- Поддерживает `--extension <EXTENSION>` или `--all-extensions`.

`designer-modules`:

- Только `builder=DESIGNER`, `format=DESIGNER`.
- Требует как минимум один mode flag.
- Поддерживает `--extension <EXTENSION>` или `--all-extensions`.

`edt`:

- Только `builder=DESIGNER`, `format=EDT`.
- Повторяемый `--project`.
- Без `--project` использует дефолтный набор EDT-проектов из конфига.

## Файлы и артефакты

### `dump`

```bash
v8-runner dump --mode <full|incremental|partial> [--source-set <NAME>] [--extension <EXTENSION>] [--object <TYPE:NAME>...]
```

- `partial` требует хотя бы один `--object`.
- `builder=DESIGNER` поддерживает true object-scoped partial.
- `builder=IBCMD` не умеет object-scoped partial; запрос деградирует в incremental с warning.
- `format=EDT` использует internal Designer snapshot под `workPath/designer/<sourceSetName>`,
  затем импортирует его в EDT target и публикует результат атомарной заменой target каталога.

### `convert`

```bash
v8-runner convert [--source-set <NAME>] [--output <DIR>]
```

- CLI-only; не публикуется как MCP tool.
- Работает от текущего `v8project.yaml`, а не по arbitrary source/target paths.
- Направление определяется только из `format`.
- Без `--output` публикует результат под `workPath/convert/out/<sourceSetName>/<designer|edt>/`.
- `--output` задаёт только target root и зеркалит `source-set.path` относительно каталога primary config.
- Публикация остаётся staged full replacement с overlap guardrails.

### `load`

```bash
v8-runner load --path <FILE> [--mode <load|merge>] [--settings <FILE>] [--extension <NAME>]
```

- Поддерживает `.cf` и `.cfe`.
- Работает только для `format=DESIGNER` и `builder=DESIGNER`.
- `.cfe` требует `--extension`.
- `--mode merge` требует `--settings <FILE>`.
- `load --mode update` не поддержан; используйте `load` или `merge`.

### `make` / `artifacts`

```bash
v8-runner make --output <TARGET> [--source-set <NAME>] [--extension <NAME>]
v8-runner artifacts --output <TARGET> [--source-set <NAME>] [--extension <NAME>]
```

- Это один use case с двумя CLI names.
- `.cf` используется для основной конфигурации.
- `.cfe` используется для extension export.
- Каталог output используется для external `.epf` / `.erf` publication.
- Требует `builder=DESIGNER`.

## Прямой запуск и MCP

### `launch`

```bash
v8-runner launch <designer|thin|thick|ordinary> [FLAGS]
v8-runner launch mcp [va] [--mode <thin|thick|ordinary>] [FLAGS]
```

- Для обычного запуска (`designer`/`thin`/`thick`/`ordinary`) режим задаётся позиционным
  аргументом.
- `designer` использует `1cv8`.
- `thin` использует `1cv8c`.
- `thick` и `ordinary` используют `1cv8`.
- `mcp` запускает клиентский MCP-сервер onec-client-mcp-devkit через `/C"runMcp"`.
- `launch mcp` по умолчанию использует `--mode thin` и `1cv8c`.
- `launch mcp --mode thick` использует `1cv8`; `launch mcp --mode ordinary` использует `1cv8`
  и добавляет `/RunModeOrdinaryApplication`.
- `launch mcp va` дополнительно запускает Vanessa Automation из `tools.va` через `/Execute <epf>`
  и передаёт `VAParams=<runtime params>` без `StartFeaturePlayer`.
- Любой управляемый runner payload для ключа `/C` передаётся как один аргумент
  `/C"<payload>"`: это касается `launch --c`, `launch mcp`, `test yaxunit` и `test va`.
- Для `mcp` доступны typed flags `--mcp-config <FILE>` и `--mcp-port <PORT>`;
  итоговый payload: `/C"runMcp[=<FILE>][;mcpPort=<PORT>]"`.
- Если `--mcp-port` не указан, используется `tools.client_mcp.port` из `v8project.yaml`.
- Если настроено `tools.client_mcp.extension`, `launch mcp` не устанавливает и не обновляет его;
  подготовка выполняется командой `v8-runner build`.
- `--mcp-config` не должен содержать `;`, потому что `/C` payload разделяется точкой с запятой.
- `launch mcp` не принимает `--c` и `--execute`, потому что `/C` управляется командой.
- `launch mcp` принимает общие launch flags `--use-privileged-mode`, `--output` и `--raw-key`, но
  `--raw-key` не может задавать `/C`, `/Execute` или `/Out`.
- Для `designer`/`thin`/`thick`/`ordinary` дополнительные typed flags: `--c`, `--execute`, `--use-privileged-mode`, `--output`,
  повторяемый `--raw-key`.

### `mcp serve`

```bash
v8-runner mcp serve stdio
v8-runner mcp serve http
```

- `stdio` и `streamable HTTP` публикуют один и тот же набор из 8 инструментов.
- MCP request fields используют `camelCase`.
- Business failures возвращаются внутри tool result payload.
- Transport/internal failures остаются MCP-native.
- Все tool calls разделяют `mcp.execution.max_concurrent_calls`.

### Опубликованные MCP tools

| Инструмент | Основные поля запроса | Примечания |
| --- | --- | --- |
| `build_project` | `fullRebuild`, `sourceSet`, `dynamicUpdate` | `fullRebuild=false`; `sourceSet` omitted значит все source-set; `dynamicUpdate` (опц.) переопределяет `build.dynamicUpdate` для одного вызова |
| `run_all_tests` | `full` | Компактный вывод по умолчанию |
| `run_module_tests` | `moduleName`, `full` | Отклоняет пустой `moduleName` |
| `dump_config` | `mode`, `extension`, `objects` | Пустой `mode` нормализуется в `INCREMENTAL` |
| `launch_app` | `utilityType` | Поддерживает алиасы `designer`, `thin`, `thick` и русские алиасы |
| `check_syntax_edt` | `projectName` | Пустой `projectName` значит “все EDT-проекты” |
| `check_syntax_designer_config` | Designer-config flags в `camelCase` | Область расширений нормализуется в service layer |
| `check_syntax_designer_modules` | Designer-modules flags в `camelCase` | Область расширений нормализуется в service layer |

## workPath и артефакты выполнения

Важные runtime директории:

- `workPath/hash-storages/`: persisted change-detection state.
- `workPath/edt-workspace/`: общий EDT workspace для `init`.
- `workPath/convert/edt-workspace/`: отдельный EDT workspace для `convert`.
- `workPath/logs/platform/`: platform logs.
- `workPath/logs/mcp/actions.log`: MCP action log.
- `workPath/temp/`: временные run artifacts и диагностические файлы.

## Пока не поддерживается

- Публикация CLI-only команд в MCP без отдельного ADR.
- Object-scoped partial dump для `builder=IBCMD`.
- `load` для `IBCMD`.
- Arbitrary path-based `convert source -> target` contract.
- Отдельная пользовательская настройка EDT `working-directory`.
