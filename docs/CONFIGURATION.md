# Конфигурация

Этот документ описывает все поддержанные ключи `v8project.yaml`, их текущий статус и ограничения реализации.

Цель документа:

- дать единое место со всеми настройками;
- отделить реально работающие настройки от задела на будущее;
- явно ответить на вопросы про интерактивный EDT и дополнительные параметры запуска клиента 1С.

## Автонастройка

Создать стартовый конфиг можно командой:

```bash
v8-runner config init
```

Команда работает без существующего `v8project.yaml`, создаёт файл в текущем каталоге и заполняет `source-set` найденными исходниками:

- Designer-исходники находятся по файлу `Configuration.xml`;
- расширения Designer распознаются по маркерам расширения внутри `Configuration.xml`;
- EDT-проекты находятся по файлу `.project`;
- существующий файл не перезаписывается без `--force`.

После чтения конфига относительные `basePath`, `workPath`, пути Vanessa Automation и файловая строка подключения `File=...` / `/F ...` приводятся к абсолютным путям относительно каталога, где находится `v8project.yaml`. Серверная строка подключения должна сохраняться как строка подключения, а не трактоваться как путь.

Полезные параметры:

```bash
v8-runner config init --connection "File=/path/to/ib"
v8-runner --config custom.yaml config init
v8-runner config init --file custom.yaml --force
v8-runner config init --format edt
```

## Полный пример

```yaml
basePath: /path/to/project
workPath: build
format: EDT
builder: DESIGNER
infobase:
  connection: "File=build/ib"

  user: Admin
  password: secret

source-set:
  - name: main
    type: CONFIGURATION
    path: main
  - name: ext
    type: EXTENSION
    path: ext

build:
  partialLoadThreshold: 20

tools:
  platform:
    path: /opt/1cv8/x86_64
    version: 8.3.27.1859
  enterprise:
    additional-launch-keys:
      - /TESTMANAGER
  edt-cli:
    path: 2025.2.3
    version: 2025.2.3
    interactive-mode: false
    auto-start: false
    startup-timeout-ms: 300000
    command-timeout-ms: 300000

mcp:
  http:
    bind_address: 127.0.0.1:3000
    path: /mcp
    stateful_sessions: true
    max_sessions: 64
    idle_ttl_secs: 900
  execution:
    max_concurrent_calls: 1
    shutdown_grace_period_secs: 30

tests:
  execution_timeout_seconds: 300
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

## Обязательные ключи

### `basePath`

- Тип: путь
- Обязателен: да
- Значение: корень исходников проекта

Поведение:

- должен существовать и быть каталогом.

### `workPath`

- Тип: путь
- Обязателен: да
- Значение: рабочий каталог для временных файлов, логов, hash storage и EDT workspace

Поведение:

- будет создан автоматически, если отсутствует;
- используется как корень для:
  - `workPath/hash-storages`
  - `workPath/logs`
  - `workPath/temp`
  - `workPath/edt-workspace`
  - `workPath/designer`

### `infobase.connection`

- Тип: строка
- Обязателен: да

Поведение:

- передаётся в платформенные утилиты как строка подключения;
- для `builder=DESIGNER` используется как обычная строка подключения 1С;
- для `builder=IBCMD` file connection маппится в `--db-path`, а server connection использует `infobase.dbms`;
- для `init` server connection с `builder=IBCMD` выполняет `ensure` через `ibcmd infobase create --create-database`; при `builder=DESIGNER` server create step по-прежнему пропускается.

### `infobase.user` / `infobase.password`

- Тип: строка
- Обязательны: нет

Поведение:

- используются как логин/пароль подключения к информационной базе;
- передаются в `1cv8`, `1cv8c` и `ibcmd` как credentials самой ИБ;
- пароль редактируется в логах и диагностике команд.

### `infobase.dbms`

- Тип: объект
- Обязателен: нет

Используется только для server connection с `builder=IBCMD`.

Поддержанные поля:

- `infobase.dbms.kind`
- `infobase.dbms.server`
- `infobase.dbms.name`
- `infobase.dbms.user`
- `infobase.dbms.password`

Поведение:

- `kind/server/name` обязательны для server connection с `builder=IBCMD`;
- `user/password` опциональны и передаются как credentials физической БД;
- для file connection секция `infobase.dbms` запрещена;
- legacy top-level `connection` и `credentials` loader отклоняет до валидации use case.

### `source-set`

- Тип: список
- Обязателен: да

Каждый элемент:

- `name`: логическое имя набора исходников
- `type`: `CONFIGURATION`, `EXTENSION`, `EXTERNAL_DATA_PROCESSORS` или `EXTERNAL_REPORTS`
- `path`: путь к исходникам

Поведение:

- должен быть хотя бы один `CONFIGURATION`;
- `name` должен быть уникальным;
- для `format=EDT` путь должен существовать;
- для `format=EDT` generated Designer copy идёт в `workPath/designer/<name>`.

## Базовые режимы

### `format`

- Тип: enum
- Значения: `DESIGNER`, `EDT`
- По умолчанию: `DESIGNER`

### `builder`

- Тип: enum
- Значения: `DESIGNER`, `IBCMD`
- По умолчанию: `DESIGNER`

Ограничения:

- `builder=IBCMD` поддерживает file и server ИБ для сценариев `init`, `build`, `dump`, `extensions`, но для server connection требует полный `infobase.dbms.kind/server/name`.
- Для `format=EDT` команда `build` сначала экспортирует EDT-проект в Designer-файлы под `workPath/designer/<name>`, затем загружает результат выбранным backend.

## Опциональные секции

### `build`

- `build.partialLoadThreshold`
- Тип: integer
- По умолчанию: `20`
- Минимум: `1`

Используется для решения между partial и full load.

### `tests`

- `tests.execution_timeout_seconds`
- Тип: integer
- По умолчанию: `300`
- Допустимый диапазон: `1..=86400`
- `tests.yaxunit.timeouts.total_ms` и `tests.va.timeouts.total_ms`
- Тип: integer
- Используется как активный пользовательский таймаут для `test yaxunit` и `test va`
- `startup_ms` и `run_ms` в `tests.*.timeouts` зарезервированы и не влияют на запуск
- `tests.va.epf_path`, `tests.va.params_path`, `tests.va.profile`
- Обязательны для Vanessa Automation
- `tests.va.fail_fast`
- Передаётся в runtime params как `stoponerror`
- `tests.va.profiles.<name>.feature_path`
- Обязателен для каждого профиля Vanessa
- `tests.va.profiles.<name>.features_to_run`, `filter_tags`, `ignore_tags`, `scenario_filter`
- Дополнительные фильтры VA, передаваемые в runtime params

### `mcp.http`

- `bind_address`: адрес HTTP listener, по умолчанию `127.0.0.1:3000`
- `path`: HTTP path, по умолчанию `/mcp`
- `stateful_sessions`: `true` по умолчанию
- `max_sessions`: `64` по умолчанию
- `idle_ttl_secs`: `900` по умолчанию

### `mcp.execution`

- `max_concurrent_calls`: по умолчанию `1`
- `shutdown_grace_period_secs`: по умолчанию `30`

## `tools.platform`

### `tools.platform.path`

- Тип: путь
- Обязателен: нет

Может указывать:

- на конкретный бинарь `1cv8`, `1cv8c` или `ibcmd`;
- на каталог `bin`;
- на корень установки с версиями.

### `tools.platform.version`

- Тип: строка
- Обязателен: нет
- Формат: `major.minor`, `major.minor.patch` или `major.minor.patch.build`

Пример:

```yaml
tools:
  platform:
    version: 8.3
```

По [ADR-0004](decisions/0004-avtoobnaruzhivat-komponenty-platformy-1s-po-versii-maske.md) `v8-runner` должен сам искать установленные компоненты платформы 1С по версии или версии-маске:

- `8.3.27.1789`: точное совпадение;
- `8.3.20`: выбрать максимальную найденную сборку `8.3.20.*`;
- `8.3`: выбрать максимальную найденную версию `8.3.*.*`.

Если указаны четыре части, например `8.3.27.1859`, автопоиск требует точное совпадение.
Если `path` не указан, будет идти автопоиск по стандартным корням установки.

## `tools.edt_cli`

### `tools.edt_cli.path`

- Тип: путь или version-like hint
- Обязателен: нет

Поддержанные варианты:

- абсолютный путь к `1cedtcli`;
- путь к каталогу установки EDT;
- version-like hint, например `2025.2.3`.

Пример:

```yaml
tools:
  edt-cli:
    path: 2025.2.3
```

Это находит установленный EDT вида `1c-edt-2025.2.3+30-x86_64`.

### `tools.edt_cli.version`

- Тип: строка
- Обязателен: нет

Отдельная version-like подсказка для автопоиска EDT.

Пример:

```yaml
tools:
  edt-cli:
    version: 2025.2.3
```

### `tools.edt_cli.startup_timeout_ms`

- Тип: integer
- По умолчанию: `300000`
- Также принимает: `startup-timeout-ms`

Используется при старте интерактивной EDT session и ожидании prompt.

### `tools.edt_cli.command_timeout_ms`

- Тип: integer
- По умолчанию: `300000`
- Также принимает: `command-timeout-ms`

Используется как timeout для интерактивных EDT-команд.

### `tools.edt_cli.interactive_mode`

- Тип: boolean
- По умолчанию: `false`
- Также принимает: `interactive-mode`

Если включён:

- поддержанные EDT-сценарии используют interactive `1cedtcli` вместо one-shot вызовов;
- для MCP это означает shared actor/manager и одну shared interactive session;
- в текущем CLI interactive EDT стартует лениво при первом EDT-вызове и живёт только в рамках текущего процесса команды;
- `auto-start` влияет только на shared MCP EDT session.

Если выключен:

- все EDT-операции идут через обычные one-shot вызовы `1cedtcli -command ...`;
- `auto-start` игнорируется.

### `tools.edt_cli.auto-start`

- Тип: boolean
- По умолчанию: `false`

Работает только вместе с `tools.edt_cli.interactive_mode=true` и только для long-lived host process с shared EDT session. На текущем этапе это MCP server.

Поведение:

- для shared MCP EDT session выполняет eager prewarm на старте сервера;
- для CLI не выполняет eager prewarm: даже при `interactive_mode=true` EDT стартует лениво при первом EDT-вызове;
- при `interactive_mode=false` не оказывает эффекта.

### `tools.edt_cli.working-directory`

Текущий статус:

- не поддержан моделью конфигурации;
- будет проигнорирован как неизвестный ключ YAML;
- рабочий каталог EDT session сейчас фиксирован: `workPath/edt-workspace`.

## Интерактивный EDT: что реально работает

Реально поддержано:

- автопоиск `1cedtcli`;
- отдельное переключение через `tools.edt_cli.interactive_mode`;
- интерактивный backend для всех EDT-операций;
- ленивый старт shared MCP session;
- eager prewarm через `auto-start`, если включён interactive-mode;
- timeout старта через `tools.edt_cli.startup_timeout_ms`;
- timeout команды через `tools.edt_cli.command_timeout_ms`;
- workspace в `workPath/edt-workspace`.

Пока не поддержано как отдельная настраиваемая функция:

- произвольный `working-directory`;
- дополнительные аргументы для старта `1cedtcli` сверх `-data <workPath/edt-workspace>`.

## Запуск клиента 1С: что реально поддержано

Команда `launch` поддерживает выбор режима позиционным аргументом:

- `designer`
- `thin`
- `thick`
- `ordinary`

Старый вариант `--mode <designer|thin|thick|ordinary>` сохранён для совместимости.

Внутри формируется запуск:

- `designer` -> `1cv8 DESIGNER`
- `thin` -> `1cv8c ENTERPRISE`
- `thick` -> `1cv8 ENTERPRISE`

Дополнительно автоматически передаются только:

- аргументы из `infobase.connection`;
- `infobase.user/password`, если они заданы.

### Дополнительные параметры клиента 1С

Поддержано:

- `tools.enterprise.additional-launch-keys` как список строк;
- ключи дописываются только к `thin`/`thick` запуску клиента (`ENTERPRISE`);
- `designer`-запуск эти ключи не получает;
- MCP `launch_app` использует те же настройки, потому что опирается на тот же use case.

Пример:

```yaml
tools:
  enterprise:
    additional-launch-keys:
      - /TESTMANAGER
      - /RunModeOrdinaryApplication
```

Если нужен запуск с чем-то вроде:

- `/RunModeOrdinaryApplication`
- `/UsePrivilegedMode`
- `/C <payload>`
- `/Execute <epf>`
- `/DisableStartupDialogs`

то это сейчас потребует доработки use case и конфигурационной модели.

## Что стоит помнить

- `docs/CAPABILITIES.md` описывает пользовательские возможности и матрицу сценариев.
- Этот файл описывает именно конфигурацию и её текущие runtime-ограничения.
