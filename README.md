# v8-runner

Локальная автоматизация 1С для разработчиков и AI-ассистентов.

`v8-runner` — это CLI-приложение на Rust и MCP-сервер для рутинных операций в разработке на 1С: загрузки исходников в информационную базу, запуска YaXUnit- и Vanessa Automation-тестов, выгрузки конфигурации обратно в файлы, сборки и загрузки релизных артефактов, синтаксических проверок и запуска инструментов 1С.

Инструмент закрывает сразу два типа сценариев:

- локальный цикл разработки из терминала;
- автоматизацию через ассистента по MCP.

## Зачем использовать

- Один инструмент для `config init`, `init`, `extensions`, `build`, `load`, `test`, `dump`, `make`/`artifacts`, `syntax`, `launch` и доступа по MCP.
- Инкрементальные сценарии вместо полной пересборки на каждое изменение.
- Удобная работа и с основной конфигурацией, и с расширениями.
- Структурированные результаты, понятные и человеку, и MCP-клиенту.
- Более узкая и удобная для автоматизации поверхность, чем прямой вызов утилит 1С.

## Что умеет

- `build`: загружать изменённые исходники в ИБ, выбирая частичное или полное выполнение в зависимости от формата исходников и бэкенда.
- `config init`: создавать `v8project.yaml` в текущем каталоге и добавлять найденные исходники в `source-set`.
- `init`: первично создавать файловую ИБ и, для EDT-проектов, инициализировать workspace импортом всех настроенных `source-set`.
- `extensions`: обновлять свойства расширений в информационной базе по настроенным `source-set`.
- `test yaxunit`: сначала выполнять `build`, затем запускать все YaXUnit-тесты или один модуль.
- `test va`: сначала выполнять `build`, затем запускать Vanessa Automation по выбранному профилю.
- `load`: загружать готовые `.cf` и `.cfe` артефакты в ИБ через Designer в режимах `load` и `merge`.
- `dump`: выгружать состояние конфигурации или расширения обратно в файлы в режимах `full`, `incremental` и `partial`.
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

Команда сканирует текущий каталог, ищет Designer-исходники по `Configuration.xml`, EDT-проекты по `.project`, добавляет найденные источники в `source-set` и не перезаписывает существующий файл без `--force`.

Или создайте минимальный `v8project.yaml` вручную:

```yaml
basePath: /path/to/project/sources
workPath: build
format: DESIGNER
builder: DESIGNER
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
./target/release/v8-runner mcp serve stdio
```

Если конфиг валиден, но локальные утилиты 1С не установлены, вы должны получить ошибку поиска платформенной утилиты или ошибку времени выполнения, а не ошибку парсинга YAML или CLI. Это тоже полезная первая проверка: она означает, что связка настроена корректно, а не хватает только локального 1С-окружения.

## Матрица поддержки

| Сценарий | Текущая поддержка |
| --- | --- |
| `config init` | Создание `v8project.yaml` в текущем каталоге; автопоиск Designer/EDT source-set; `--force` для перезаписи |
| `init` | `format=DESIGNER` или `format=EDT` с `builder=DESIGNER` или `IBCMD`; для `IBCMD` только файловая ИБ |
| `extensions` | Обновление свойств расширений для EDT и Designer-проектов по настроенным extension `source-set`; только файловая ИБ |
| `build` | `format=DESIGNER` или `format=EDT` с `builder=DESIGNER` или `IBCMD`; EDT сначала экспортируется в Designer-файлы |
| `load` | `.cf` и `.cfe` артефакты; только `format=DESIGNER` с `builder=DESIGNER`; `--mode load` и `--mode merge` |
| `test yaxunit` | Следует матрице `build` и всегда сначала запускает `build` |
| `test va` | `tests.va` с выбранным профилем, `epf_path` и `params_path`; всегда сначала запускает `build` |
| `dump` | `format=DESIGNER` с `builder=DESIGNER` или `IBCMD` |
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
- [docs/decisions/0001-granitsy-podderzhki-ibcmd-kak-ogranichennogo-backend.md](docs/decisions/0001-granitsy-podderzhki-ibcmd-kak-ogranichennogo-backend.md): принятая граница поддержки `IBCMD` как ограниченного backend.

<details>
<summary>Текущие ограничения и оговорки</summary>

- `IBCMD` требует файловое подключение к информационной базе.
- `IBCMD` поддерживается как ограниченный backend для сценариев `init`, `build`, `dump`, `extensions`.
- `load` не поддерживает `IBCMD`, EDT-формат, `.epf` и `.erf`.
- `load --mode update` зарезервирован CLI-интерфейсом, но текущая реализация его отклоняет; используйте `load` или `merge`.
- MCP-поверхность намеренно уже CLI: `init`, `extensions`, `load` и `make`/`artifacts` не опубликованы как MCP-инструменты.
- `init` считает файловую ИБ существующей только по наличию файла `1Cv8.1CD` в каталоге базы и не валидирует содержимое глубже.
- `init` для EDT считает workspace завершённым только после успешного полного импорта; незавершённый каталог без внутреннего marker-файла будет импортирован повторно.
- Точечная частичная выгрузка по объектам нативно не реализована для `IBCMD`; запрос `partial` деградирует в инкрементальную выгрузку с предупреждением.
- При деградации `partial` для `IBCMD` запрошенный режим `PARTIAL` сохраняется в результирующем payload.
- `syntax designer-modules` требует как минимум один флаг режима.
- Интерактивный EDT теперь включается явно через `tools.edt_cli.interactive-mode`; без него EDT работает в one-shot режиме.
- Внутренние документы в `spec/*` по-прежнему полезны как источник фактов, но публичный справочник теперь живёт в `README.md`, `docs/CAPABILITIES.md` и `docs/DEEP_DIVE.md`.

</details>
