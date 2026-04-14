# v8-test-runner

Локальная автоматизация 1С для разработчиков и AI-ассистентов.

`v8-test-runner` — это CLI-приложение на Rust и MCP-сервер для рутинных операций в разработке на 1С: загрузки исходников в информационную базу, запуска YaXUnit-тестов, выгрузки конфигурации обратно в файлы, синтаксических проверок и запуска инструментов 1С.

Инструмент закрывает сразу два типа сценариев:

- локальный цикл разработки из терминала;
- автоматизацию через ассистента по MCP.

## Зачем использовать

- Один инструмент для `init`, `extensions`, `build`, `test`, `dump`, `syntax`, `launch` и доступа по MCP.
- Инкрементальные сценарии вместо полной пересборки на каждое изменение.
- Удобная работа и с основной конфигурацией, и с расширениями.
- Структурированные результаты, понятные и человеку, и MCP-клиенту.
- Более узкая и удобная для автоматизации поверхность, чем прямой вызов утилит 1С.

## Что умеет

- `build`: загружать изменённые исходники в ИБ, выбирая частичное или полное выполнение в зависимости от формата исходников и бэкенда.
- `init`: первично создавать файловую ИБ и, для EDT-проектов, инициализировать workspace импортом всех настроенных `source-set`.
- `extensions`: обновлять свойства расширений в информационной базе по настроенным `source-set`.
- `test`: сначала выполнять `build`, затем запускать все YaXUnit-тесты или один модуль.
- `dump`: выгружать состояние конфигурации или расширения обратно в файлы в режимах `full`, `incremental` и `partial`.
- `syntax`: запускать проверки через Designer для Designer-исходников и `1cedtcli validate` для EDT-проектов.
- `launch`: открывать Designer, тонкий клиент или толстый клиент.
- `mcp serve`: отдавать те же сценарии MCP-клиентам по stdio или по протоколу `streamable HTTP`.

## Быстрый старт

Соберите бинарь:

```bash
cargo build --release
```

Создайте минимальный `application.yaml`:

```yaml
basePath: /path/to/project/sources
workPath: /tmp/v8-test-runner/my-project
format: DESIGNER
builder: DESIGNER
connection: "File=/path/to/infobase"

source-set:
  - name: main
    purpose: CONFIGURATION
    path: .
```

Запустите первые команды:

```bash
./target/release/v8-test-runner --config ./application.yaml build
./target/release/v8-test-runner --config ./application.yaml init
./target/release/v8-test-runner --config ./application.yaml test all
./target/release/v8-test-runner --config ./application.yaml mcp serve stdio
```

Если конфиг валиден, но локальные утилиты 1С не установлены, вы должны получить ошибку поиска платформенной утилиты или ошибку времени выполнения, а не ошибку парсинга YAML или CLI. Это тоже полезная первая проверка: она означает, что связка настроена корректно, а не хватает только локального 1С-окружения.

## Матрица поддержки

| Сценарий | Текущая поддержка |
| --- | --- |
| `init` | `format=DESIGNER` с `builder=DESIGNER` или `IBCMD`; `format=EDT` с `builder=DESIGNER` |
| `extensions` | Обновление свойств расширений для EDT и Designer-проектов по настроенным extension `source-set`; только файловая ИБ |
| `build` | `format=DESIGNER` с `builder=DESIGNER` или `IBCMD`; `format=EDT` с `builder=DESIGNER` |
| `test` | Следует матрице `build` и всегда сначала запускает `build` |
| `dump` | `format=DESIGNER` с `builder=DESIGNER` или `IBCMD` |
| `syntax` | Проверки через Designer для `DESIGNER`-исходников и валидация EDT для `EDT` |
| `launch` | Designer, тонкий клиент, толстый клиент |
| MCP | stdio и HTTP-транспорты с 8 опубликованными инструментами |

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
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md): полный справочник по `application.yaml` и всем поддержанным ключам конфигурации.
- [docs/DEEP_DIVE.md](docs/DEEP_DIVE.md): объяснение внутренних эксплуатационных потоков без дублирования полного справочника команд.
- [examples/application.yaml](examples/application.yaml): полный пример конфига с опциональными секциями и значениями по умолчанию.
- [ARCHITECTURE.md](ARCHITECTURE.md): карта модулей и внутренних границ для контрибьюторов.
- [docs/decisions/0001-granitsy-podderzhki-ibcmd-kak-ogranichennogo-backend.md](docs/decisions/0001-granitsy-podderzhki-ibcmd-kak-ogranichennogo-backend.md): принятая граница поддержки `IBCMD` как ограниченного backend.

<details>
<summary>Текущие ограничения и оговорки</summary>

- `IBCMD` требует файловое подключение к информационной базе.
- `IBCMD` поддерживается как ограниченный backend для сценариев `init`, `build`, `dump`, `extensions`.
- `init` считает файловую ИБ существующей только по наличию файла `1Cv8.1CD` в каталоге базы и не валидирует содержимое глубже.
- `init` для EDT считает workspace завершённым только после успешного полного импорта; незавершённый каталог без внутреннего marker-файла будет импортирован повторно.
- Точечная частичная выгрузка по объектам нативно не реализована для `IBCMD`; запрос `partial` деградирует в инкрементальную выгрузку с предупреждением.
- При деградации `partial` для `IBCMD` запрошенный режим `PARTIAL` сохраняется в результирующем payload.
- `syntax designer-modules` требует как минимум один флаг режима.
- Интерактивный EDT теперь включается явно через `tools.edt_cli.interactive-mode`; без него EDT работает в one-shot режиме.
- Внутренние документы в `spec/*` по-прежнему полезны как источник фактов, но публичный справочник теперь живёт в `README.md`, `docs/CAPABILITIES.md` и `docs/DEEP_DIVE.md`.

</details>
