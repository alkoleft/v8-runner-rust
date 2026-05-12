# v8-runner

`v8-runner` — CLI (командная строка) и MCP server (сервер Model Context Protocol) для
локального 1C development workflow (цикла разработки 1С). Он собирает исходники, готовит
информационную базу, запускает проверки и тесты, выгружает изменения обратно в файлы и дает
AI-агентам безопасную, уже ограниченную MCP-поверхность.

Проект закрывает практическую боль 1С-разработки: вместо набора хрупких shell scripts
(скриптов оболочки), ручных запусков Designer (Конфигуратора), EDT и Vanessa Automation команда
получает один воспроизводимый entrypoint (точку входа) для локального цикла и автоматизации.

## Зачем это нужно

- Быстрый feedback loop (цикл обратной связи): `build -> syntax/test -> diagnose`.
- Один config (конфиг) `v8project.yaml` для исходников, рабочей ИБ, инструментов и тестов.
- Поддержка source sets (наборов исходников) в форматах `DESIGNER` и `EDT`.
- Builder backends (сборщики) `DESIGNER` и `IBCMD` там, где это разрешает контракт 1С.
- Machine-readable output (машиночитаемый вывод) через `--json-message` для CI и агентов.
- MCP tools (MCP-инструменты) для управляемой работы AI-агентов без выдачи всей CLI-поверхности.
- Изолированный `workPath` для hash storages (хранилищ хэшей), логов, временных файлов и
  промежуточных артефактов.

![test-yaxunit](docs/assets/test-yaxunit.png)

## Быстрый старт

Соберите release binary (релизный бинарный файл):

```bash
cargo build --release
```

Команда компилирует `v8-runner` в `target/release/v8-runner`.

### Создайте стартовый config (конфиг) в текущем репозитории:

```bash
v8-runner config init
```

Команда анализирует структуру проекта, находит поддержанные `source-set` (наборы исходников),
создает `v8project.yaml`, пустой `v8project.local.yaml` со schema modeline и добавляет local
overlay в `.gitignore`, если он еще не указан.

Machine-local пути, credentials и настройки инструментов можно вынести в `v8project.local.yaml`
рядом с основным конфигом. Этот файл применяется автоматически и должен оставаться вне Git.

### Загрузите тестовые и MCP-инструменты:

```bash
v8-runner tools download yaxunit --sources
v8-runner tools download vanessa
v8-runner tools download client-mcp --sources
```

Команды берут latest releases выбранного инструмента. Для YAxUnit и onec-client-mcp-devkit
`--sources` выбирает source install; без него скачивается `.cfe` artifact в `build/tools`.
Vanessa Automation single всегда скачивается как EPF в `build/tools` и прописывается в
`v8project.local.yaml`.

### Подготовьте рабочую информационную базу:

```bash
v8-runner init
```

Команда создает или подготавливает ИБ и, для `EDT`, импортирует workspace (рабочую область).

### Загрузите исходники в ИБ:

```bash
v8-runner build
```

Команда выполняет incremental build (инкрементальную сборку) или full path (полную сборку) по
текущим изменениям и настройкам проекта.

### Проверьте синтаксис серверных модулей:

```bash
v8-runner syntax designer-modules --server
```

Команда запускает Designer syntax check (проверку синтаксиса Конфигуратором) для серверного
контекста.

### Запустите YAxUnit-тесты:

```bash
v8-runner test yaxunit all
```

### Или тесты Vanessa Automation:

```bash
v8-runner test va
```

Команда сначала выполняет `build`, затем запускает полный набор YAxUnit-тестов.

Для отладки и написания тестов Vanessa Automation запустите ее в режиме MCP

```bash
v8-runner launch mcp va
```

### Поднимите MCP transport (MCP-транспорт) для AI-агентов:

```bash
v8-runner mcp serve stdio
```

Команда запускает MCP server (сервер Model Context Protocol) поверх `stdio` transport
(транспорта стандартного ввода-вывода).

Если `config init` не покрывает вашу структуру репозитория, настройте `v8project.yaml` вручную по
[docs/CONFIGURATION.md](docs/CONFIGURATION.md).

## Что умеет

| Зона | Команды | Что делает |
| --- | --- | --- |
| Project setup (настройка проекта) | `config init`, `tools download`, `init`, `extensions`, `build` | Создает config, скачивает инструменты, готовит ИБ, обновляет расширения и загружает исходники |
| Verification (проверка) | `syntax`, `test` | Запускает syntax checks, YAxUnit и Vanessa Automation |
| File materialization (материализация файлов) | `dump`, `convert`, `load`, `make`, `artifacts` | Выгружает, конвертирует, загружает и публикует `.cf`, `.cfe`, `.epf`, `.erf` |
| Direct launch (прямой запуск) | `launch <designer|thin|thick|ordinary>`, `launch mcp [va]` | Запускает 1C clients (клиенты 1С), Designer и MCP/Vanessa сценарии |
| MCP automation (автоматизация через MCP) | `mcp serve stdio`, `mcp serve http` | Открывает 8 MCP tools для агентных workflow |

## Для кого

- 1С-разработчики, которым нужен повторяемый локальный цикл без ручного переключения между
  Designer, EDT, Vanessa Automation и тестовыми runner-ами.
- Команды, которые хотят единый command contract (контракт команд) для локальной разработки,
  CI и релизной сборки.
- AI-assisted development (разработка с AI-агентами), где агент должен строить, проверять и
  диагностировать проект через узкую управляемую поверхность.

## Карта документации

- [docs/CAPABILITIES.md](docs/CAPABILITIES.md): полный каталог команд, матрица поддержки,
  MCP tools и текущие ограничения.
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md): контракт `v8project.yaml`, поддержанные keys
  (ключи) и validation rules (правила валидации).
- [docs/DEEP_DIVE.md](docs/DEEP_DIVE.md): execution semantics (семантика выполнения), runtime
  model (модель выполнения), lock/publication behavior (поведение блокировок и публикации).
- [docs/README.md](docs/README.md): порядок чтения документации и source-of-truth (источник
  истины).
- [ARCHITECTURE.md](ARCHITECTURE.md): module map (карта модулей) и границы для контрибьюторов.
- [spec/README.md](spec/README.md): внутренние ADR, architecture rules (архитектурные правила),
  acceptance (приемка) и implementation backlog (план реализации).
- [references/1c/README.md](references/1c/README.md): сырой внешний reference corpus
  (корпус справочных материалов) по 1С, не source of truth проекта.
