# v8-runner

Простой и удобный интерфейс для сборки и проверки исходников 1С-решений человеком и AI-агентом.

`v8-runner` — это CLI-приложение на Rust и MCP-сервер для рутинных операций в разработке на 1С: загрузки исходников в информационную базу, выгрузки конфигурации обратно в файлы, repo-aware конвертации текущих `source-set` между EDT- и Designer-форматами, запуска YaXUnit- и Vanessa Automation-тестов, сборки и загрузки релизных артефактов, синтаксических проверок и запуска инструментов 1С.

Инструмент закрывает сразу два типа сценариев:

- локальный цикл разработки из терминала;
- автоматизацию через ассистента по MCP.

## Зачем использовать

- Главная цель — дать человеку и AI-агенту простой, предсказуемый интерфейс для локального цикла `build -> syntax/test -> diagnose`.
- Один инструмент для `config init`, `init`, `extensions`, `build`, `load`, `test`, `dump`, `convert`, `make`/`artifacts`, `syntax`, `launch` и доступа по MCP.
- Инкрементальные сценарии вместо полной пересборки на каждое изменение.
- Удобная работа и с основной конфигурацией, и с расширениями.
- Структурированные результаты, понятные и человеку, и MCP-клиенту.
- Более узкая и удобная для автоматизации поверхность, чем прямой вызов утилит 1С.

## Что умеет

- `build`: загружать изменённые исходники в ИБ, выбирая частичное или полное выполнение в зависимости от формата исходников и бэкенда.
- `config init`: создавать `v8project.yaml` в текущем каталоге, добавлять найденные Designer/EDT source-set и autodetect aggregate-root каталоги внешних обработок и отчётов.
- `init`: первично создавать файловую ИБ, а для `builder=IBCMD` при server connection выполнять `ensure` серверной ИБ через `ibcmd infobase create --create-database`; для EDT-проектов инициализировать workspace импортом всех настроенных `source-set`.
- `extensions`: обновлять свойства расширений в информационной базе по настроенным `source-set`.
- `test yaxunit`: сначала выполнять `build`, затем запускать все YaXUnit-тесты или один модуль.
- `test va`: сначала выполнять `build`, затем запускать Vanessa Automation по выбранному профилю.
- `load`: загружать готовые `.cf` и `.cfe` артефакты в ИБ через Designer в режимах `load` и `merge`.
- `dump`: выгружать состояние конфигурации или расширения обратно в Designer-формат файлы в режимах `full`, `incremental` и `partial`.
- `convert`: конвертировать текущие `source-set` из `v8project.yaml` между EDT и Designer без обращения к ИБ.
- `make`/`artifacts`: экспортировать релизные `.cf` и `.cfe`, а также публиковать внешние обработки `.epf` и отчёты `.erf`.
- `syntax`: запускать проверки через Designer для Designer-исходников и `1cedtcli validate` для EDT-проектов.
- `launch`: открывать Designer, тонкий клиент, толстый клиент или обычное приложение с типизированными и raw-параметрами запуска.
- `mcp serve`: отдавать те же сценарии MCP-клиентам по stdio или по протоколу `streamable HTTP`.

## Быстрый старт

Соберите бинарь:

```bash
cargo build --release
```

Создайте `v8project.yaml` автоматически:

```bash
./target/release/v8-runner config init
```

Команда сканирует текущий каталог, опирается на файлы-маркеры и их содержимое: ищет Designer-исходники по `Configuration.xml`, EDT-проекты по `.project`, определяет ordinary EDT `source-set[].type` по `V8ConfigurationNature` / `V8ExtensionNature`, читает runtime version из `DT-INF/PROJECT.PMF`, для `EXTENSION` дополнительно требует `Base-Project`, а native EDT layout подтверждает по `src/Configuration/Configuration.mdo`. Для Designer external root нужен однородный набор top-level XML дескрипторов, для EDT external root — однородные direct child projects одного external-kind с валидными `.project`, `DT-INF/PROJECT.PMF`, `Base-Project` и canonical `src/root.xml`. Mixed/ambiguous external roots не автогенерируются. Если autodiscovery не нашёл ни одного `CONFIGURATION` или external source-set требует `--builder DESIGNER`, команда завершится validation error вместо записи phantom-конфига.

Или создайте минимальный `v8project.yaml` вручную:

```yaml
basePath: /path/to/project/sources
workPath: build
format: DESIGNER
builder: DESIGNER
infobase:
  connection: "File=build/ib"

source-set:
  - name: main
    type: CONFIGURATION
    path: .
  # - name: ext
  #   type: EXTENSION
  #   path: ext
  # - name: tools
  #   type: EXTERNAL_DATA_PROCESSORS
  #   path: tools

tests:
  yaxunit:
    timeouts:
      total_ms: 300000
  va:
    epf_path: /path/to/vanessa.epf
    params_path: /path/to/va-params.json
    profile: smoke
    fail_fast: true
    timeouts:
      total_ms: 300000
    profiles:
      smoke:
        feature_path: /path/to/features
```

Запустите первые команды:

```bash
./target/release/v8-runner config init
./target/release/v8-runner init
./target/release/v8-runner build
./target/release/v8-runner test yaxunit all
./target/release/v8-runner test va
./target/release/v8-runner make --output dist/main.cf
./target/release/v8-runner load --path dist/main.cf
./target/release/v8-runner convert --source-set main
./target/release/v8-runner mcp serve stdio
```

Если конфиг валиден, но локальные утилиты 1С не установлены, вы должны получить ошибку поиска платформенной утилиты или ошибку времени выполнения, а не ошибку парсинга YAML или CLI. Это тоже полезная первая проверка: она означает, что связка настроена корректно, а не хватает только локального 1С-окружения.

## CLI output contract

- `--output text` показывает краткий clean success path без подробного успешного журнала на каждом шаге.
- `--output text` и `--output json` одинаково сохраняют видимыми предупреждения, degraded behavior, созданные артефакты и пути к диагностике.
- `--output json` остаётся стабильным structured contract для автоматизации; text-режим не должен быть единственным источником важного факта.

## Матрица поддержки

| Сценарий | Текущая поддержка |
| --- | --- |
| `config init` | Создание `v8project.yaml` в текущем каталоге; content-based автопоиск Designer/EDT source-set и external aggregate roots; `--force` для перезаписи |
| `init` | `format=DESIGNER` или `format=EDT` с `builder=DESIGNER` или `IBCMD`; `builder=IBCMD` поддерживает file и server ИБ, `builder=DESIGNER` автоматически создаёт только файловую ИБ |
| `extensions` | Обновление свойств расширений для EDT и Designer-проектов по настроенным extension `source-set`; file и server ИБ через IBCMD adapter |
| `build` | `format=DESIGNER` или `format=EDT` с `builder=DESIGNER` или `IBCMD`; EDT сначала экспортируется в Designer-файлы |
| `load` | `.cf` и `.cfe` артефакты; только `format=DESIGNER` с `builder=DESIGNER`; `--mode load` и `--mode merge` |
| `test yaxunit` | Следует матрице `build` и всегда сначала запускает `build` |
| `test va` | `tests.va` с выбранным профилем, `epf_path` и `params_path`; всегда сначала запускает `build` |
| `dump` | `format=DESIGNER` или `format=EDT` с `builder=DESIGNER` или `IBCMD`; при `format=EDT` reverse sync идёт через internal Designer snapshot и EDT import |
| `convert` | CLI-only repo-aware конвертация текущих `source-set` через EDT CLI; direction выводится из `format`, output публикуется под `workPath/convert/out`, не требует `builder` или подключения к ИБ |
| `make` / `artifacts` | Экспорт `.cf` и `.cfe` через Designer; публикация `.epf`/`.erf` для внешних `source-set`; требуется `builder=DESIGNER` |
| `syntax` | Проверки через Designer для `DESIGNER`-исходников и валидация EDT для `EDT` |
| `launch` | Designer, тонкий клиент, толстый клиент, обычное приложение; поддерживает `--c`, `--execute`, `--use-privileged-mode`, `--out`, `--raw-key` |
| MCP | stdio и HTTP-транспорты с 8 опубликованными инструментами |

## Новые CLI-сценарии

### Загрузка релизных артефактов

```bash
v8-runner load --path dist/main.cf
v8-runner load --path dist/sales.cfe --extension SalesAddon
v8-runner load --path dist/sales.cfe --mode merge --settings merge.xml --extension SalesAddon
```

`load` работает через Designer и сейчас поддерживает только `.cf` и `.cfe`. Для `.cfe` обязательно указывается имя расширения, а режим `merge` требует файл настроек слияния. После загрузки команда выполняет обновление конфигурации базы данных.

### Экспорт артефактов

```bash
v8-runner make --output dist/main.cf
v8-runner make --output dist/sales.cfe --source-set ext-sales --extension SalesAddon
v8-runner artifacts --output dist/tools --source-set tools
```

`make` и видимый alias `artifacts` используют одну команду. Тип экспорта выводится из `--output` и выбранного `source-set`: `.cf` для основной конфигурации, `.cfe` для расширений, каталог публикации для внешних обработок и отчётов. Для внешних артефактов `source-set` должен иметь `type=EXTERNAL_DATA_PROCESSORS` или `type=EXTERNAL_REPORTS`.

### Repo-aware конвертация EDT и Designer

```bash
v8-runner convert
v8-runner convert --source-set main
```

`convert` работает от текущего `v8project.yaml`: без аргументов обрабатывает все `source-set` в конфигурационном порядке, а `--source-set` ограничивает выполнение одним именем. Направление выводится из `format`: `EDT -> Designer` для `format=EDT` и `Designer -> EDT` для `format=DESIGNER`. Результат всегда публикуется только в `workPath/convert/out/<source-set>/<designer|edt>/`, команда использует отдельный workspace `workPath/convert/edt-workspace`, не зависит от `builder`, не использует `infobase.connection` и не опубликована как MCP-инструмент. Это отдельный сценарий от `dump`: `convert` конвертирует текущие project files, а `dump` делает reverse sync из ИБ.

### Расширенный запуск 1С

```bash
v8-runner launch designer
v8-runner launch ordinary --execute tool.epf --c DoWork --use-privileged-mode
v8-runner launch thin --raw-key /WA- --raw-key /DisplayAllFunctions
```

`launch` также принимает старый вариант `--mode <designer|thin|thick|ordinary>`. `launch` и `test` используют общий набор дополнительных параметров запуска: `--c`, `--execute`, `--use-privileged-mode`, `--out` и повторяемый `--raw-key`. Для команды `test` значения `--c` и `--execute` зарезервированы под внутренний runner payload и будут отклонены.

## Опубликованные MCP-инструменты

Текущий MCP-сервер публикует следующие инструменты:

- `run_all_tests`
- `run_module_tests`
- `build_project`
- `dump_config`
- `launch_app`
- `check_syntax_edt`
- `check_syntax_designer_config`
- `check_syntax_designer_modules`

На стороне MCP запросы используют поля в `camelCase`, а CLI сохраняет обычный интерфейс с флагами.

## Карта документации

- [docs/CAPABILITIES.md](docs/CAPABILITIES.md): основной пользовательский справочник по командам, MCP-инструментам, матрицам поддержки и ограничениям.
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md): полный справочник по `v8project.yaml` и всем поддержанным ключам конфигурации.
- [docs/DEEP_DIVE.md](docs/DEEP_DIVE.md): объяснение внутренних эксплуатационных потоков без дублирования полного справочника команд.
- [examples/v8project.yaml](examples/v8project.yaml): полный пример конфига с опциональными секциями и значениями по умолчанию.
- [ARCHITECTURE.md](ARCHITECTURE.md): карта модулей и внутренних границ для контрибьюторов.
- [docs/decisions/0001-granitsy-podderzhki-ibcmd-kak-ogranichennogo-backend.md](docs/decisions/0001-granitsy-podderzhki-ibcmd-kak-ogranichennogo-backend.md): текущая граница поддержки `IBCMD` и целевой принцип взаимозаменяемости builder backend.
- [docs/decisions/0003-podderzhivat-servernye-ib-dlya-vseh-instrumentov.md](docs/decisions/0003-podderzhivat-servernye-ib-dlya-vseh-instrumentov.md): целевой контракт поддержки серверных ИБ для всех инструментов.
- [docs/decisions/0004-avtoobnaruzhivat-komponenty-platformy-1s-po-versii-maske.md](docs/decisions/0004-avtoobnaruzhivat-komponenty-platformy-1s-po-versii-maske.md): автопоиск компонентов платформы 1С по точной версии или версии-маске.
- [docs/decisions/0020-dobavit-cli-only-convert-dlya-dvustoronney-konvertatsii-edt-i-designer.md](docs/decisions/0020-dobavit-cli-only-convert-dlya-dvustoronney-konvertatsii-edt-i-designer.md): repo-aware CLI-only контракт для `convert [--source-set <name>]` и явное разделение `convert` и `dump` как разных сценариев.

<details>
<summary>Текущие ограничения и оговорки</summary>

- `IBCMD` остаётся ограниченным backend, но для реализованных сценариев `init`, `build`, `dump`, `extensions` уже поддерживает и file, и server ИБ; для server connection нужен полный `infobase.dbms` contract.
- `IBCMD` поддерживается как ограниченный backend для сценариев `init`, `build`, `dump`, `extensions`.
- Builder-сценарии должны развиваться как взаимозаменяемые между `DESIGNER`, `IBCMD` и будущим Designer agent mode; временные отличия фиксируются как явные gaps.
- Все инструменты должны развиваться с поддержкой серверных ИБ; текущие ограничения на файловую ИБ считаются gaps, а не целевой архитектурной нормой.
- `load` не поддерживает `IBCMD`, EDT-формат, `.epf` и `.erf`.
- `load --mode update` зарезервирован CLI-интерфейсом, но текущая реализация его отклоняет; используйте `load` или `merge`.
- MCP-поверхность намеренно уже CLI: `init`, `extensions`, `load`, `convert` и `make`/`artifacts` не опубликованы как MCP-инструменты.
- `init` считает файловую ИБ существующей только по наличию файла `1Cv8.1CD` в каталоге базы и не валидирует содержимое глубже.
- Для server connection `builder=IBCMD` выполняет `ensure` через `ibcmd infobase create --create-database` и трактует benign `already exists` как non-fatal outcome; `builder=DESIGNER` по-прежнему пропускает server create step.
- `init` для EDT считает workspace завершённым только после успешного полного импорта; незавершённый каталог без внутреннего marker-файла будет импортирован повторно.
- Точечная частичная выгрузка по объектам нативно не реализована для `IBCMD`; запрос `partial` деградирует в инкрементальную выгрузку с предупреждением.
- При деградации `partial` для `IBCMD` запрошенный режим `PARTIAL` сохраняется в результирующем payload.
- `dump format=EDT` использует internal Designer snapshot под `workPath/designer/<source-set>`, затем импортирует его в EDT-проект и публикует результат атомарной заменой target-каталога; для repo-aware файловой конвертации текущих исходников по-прежнему используйте `convert`.
- `syntax designer-modules` требует как минимум один флаг режима.
- Интерактивный EDT теперь включается явно через `tools.edt_cli.interactive-mode`; без него EDT работает в one-shot режиме.
- Внутренние документы в `spec/*` по-прежнему полезны как источник фактов, но публичный справочник теперь живёт в `README.md`, `docs/CAPABILITIES.md` и `docs/DEEP_DIVE.md`.

</details>
