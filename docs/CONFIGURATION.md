# Конфигурационный контракт

Этот документ описывает поддержанный `v8project.yaml`: literal YAML keys, допустимые значения,
aliases и validation rules.

Каталог команд находится в [CAPABILITIES.md](CAPABILITIES.md), а runtime semantics и operational
nuances вынесены в [DEEP_DIVE.md](DEEP_DIVE.md).

## Навигация

- [Как получить стартовый конфиг](#как-получить-стартовый-конфиг)
- [Именование ключей](#именование-ключей)
- [Канонический пример](#канонический-пример)
- [Локальный overlay](#локальный-overlay)
- [Обязательный контракт](#обязательный-контракт)
- [Опциональные секции](#опциональные-секции)
- [`tools.platform`](#toolsplatform)
- [`tools.enterprise`](#toolsenterprise)
- [`tools.edt_cli`](#toolsedt_cli)
- [Неподдержанные ключи](#неподдержанные-ключи)

## Как получить стартовый конфиг

Базовый файл можно сгенерировать командой:

```bash
v8-runner config init
```

Что делает `config init`:

- создаёт `v8project.yaml` в текущем каталоге или по `--output <FILE>`;
- заполняет `source-set` по найденным исходникам;
- не перезаписывает существующий файл без `--force`;
- не пишет synthetic `CONFIGURATION`: если конфигурационный `source-set` не найден,
  завершается validation error;
- для `--builder IBCMD` отклоняет autodetected external roots как unsupported config combination.

Автообнаружение опирается на содержимое marker files, а не на имена каталогов:

- Designer ordinary sources находятся по `Configuration.xml`, а их тип определяется по XML;
- Designer external aggregate root создаётся как один `source-set` только при однородных
  top-level XML descriptors;
- EDT ordinary projects находятся по `.project`, `DT-INF/PROJECT.PMF` и native markers под `src`;
- EDT external root создаётся только если direct child projects однородно классифицируются как
  один external kind.

После загрузки конфига относительные пути резолвятся относительно каталога, где лежит
`v8project.yaml`.

Если рядом с основным конфигом есть `v8project.local.yaml`, он применяется автоматически после
`v8project.yaml` и до CLI overrides. Локальный файл предназначен для machine-local путей,
credentials и runtime настроек; его следует держать вне Git.

## Именование ключей

`v8project.yaml` использует не один стиль на весь документ. Это текущий loader contract, и docs
ниже повторяют именно literal YAML keys.

- top-level app keys: `basePath`, `workPath`, `source-set`;
- `build` использует `partialLoadThreshold`;
- `mcp.*` и `tests.*` используют `snake_case`;
- canonical key для EDT tool section: `tools.edt_cli`;
- у `tools.edt_cli` literal child keys смешанные:
  - `interactive-mode`
  - `auto-start`
  - `startup_timeout_ms`
  - `command_timeout_ms`

Если для поля поддержан alias, ниже он указан явно. Примеры используют canonical key, а не alias.

## Канонический пример

```yaml
# basePath можно опустить, тогда он равен каталогу v8project.yaml.
basePath: /path/to/project
workPath: build
execution_timeout: 300000
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
  client_mcp:
    port: 9874
    extension:
      name: client_mcp
      source:
        path: /path/to/onec-client-mcp/exts/client-mcp
        format: EDT
  va:
    epf_path: /path/to/vanessa.epf
  platform:
    path: /opt/1cv8/x86_64
    version: 8.3.27.1859
  enterprise:
    additional-launch-keys:
      - /TESTMANAGER
  edt_cli:
    path: 2025.2.3
    version: 2025.2.3
    interactive-mode: false
    auto-start: false
    startup_timeout_ms: 300000
    command_timeout_ms: 300000

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
    params_path: /path/to/va-params.json
    profile: smoke
    fail_fast: true
    timeouts:
      total_ms: 300000
    profiles:
      smoke:
        feature_path: /path/to/features
```

## Локальный overlay

`v8project.local.yaml` расположен рядом с выбранным primary config и применяется автоматически.
Файл не является самостоятельным config entrypoint: передавать его через `--config` нельзя.

Precedence:

1. `v8project.yaml`;
2. `v8project.local.yaml`, если существует;
3. CLI overrides, например `--workdir`.

Merge rules:

- object/map значения merge-ятся рекурсивно;
- scalar значения из local overlay заменяют project значения;
- list значения заменяются целиком;
- `null` работает как обычное YAML-значение и допустим только для optional typed fields;
- относительные пути local overlay резолвятся относительно каталога primary config.

Local overlay может задавать machine-local секции:

- `workPath`;
- `infobase.*`, включая `user`/`password`;
- `tools.*`;
- `tests.*`;
- `mcp.*`.

Другие top-level ключи в local overlay отклоняются.

Local overlay не может менять project identity:

- `source-set`;
- `format`;
- `builder`.

Пример:

```yaml
workPath: build-local

infobase:
  connection: "File=local/ib"
  user: Admin
  password: secret

tools:
  platform:
    path: /opt/1cv8/x86_64
  va:
    epf_path: /home/user/tools/vanessa.epf

tests:
  va:
    params_path: /home/user/project/.local/va-params.json
```

## Обязательный контракт

### `basePath`

- Тип: путь
- Обязателен: нет
- По умолчанию: каталог primary `v8project.yaml`

Корень исходников проекта. Должен существовать и быть каталогом.

### `workPath`

- Тип: путь
- Обязателен: да

Корень runtime state:

- `workPath/hash-storages`
- `workPath/logs`
- `workPath/temp`
- `workPath/edt-workspace`
- `workPath/designer`

Если каталога нет, он создаётся автоматически.

### `execution_timeout`

- Тип: integer
- Обязателен: нет
- По умолчанию: `300000`
- Диапазон: `1..=86400000`
- Единица: миллисекунды

Общий public budget для CLI и MCP команд. Не заменяет EDT-specific timeout для interactive
команд, а ограничивает весь command budget.

Поддержанные aliases:

- `executionTimeout`
- `execution_timeout_ms`

### `format`

- Тип: enum
- Значения: `DESIGNER`, `EDT`
- По умолчанию: `DESIGNER`

### `builder`

- Тип: enum
- Значения: `DESIGNER`, `IBCMD`
- По умолчанию: `DESIGNER`

Ограничения:

- `builder=IBCMD` поддерживает `init`, `build`, `dump`, `extensions`;
- для server connection с `builder=IBCMD` обязательны `infobase.dbms.kind`,
  `infobase.dbms.server`, `infobase.dbms.name`;
- для file connection секция `infobase.dbms` запрещена.

### `infobase`

Секция обязательна целиком.

#### `infobase.connection`

- Тип: строка
- Обязателен: да

Строка подключения к ИБ. Для file-based ИБ относительный `File=...` резолвится относительно
каталога конфига.

#### `infobase.user` / `infobase.password`

- Тип: строка
- Обязательны: нет

Credentials самой информационной базы.

#### `infobase.dbms`

- Тип: объект
- Обязателен: нет

Используется только для `builder=IBCMD` + server connection.

Поддержанные поля:

- `kind`
- `server`
- `name`
- `user`
- `password`

### `source-set`

- Тип: список
- Обязателен: да

Каждый элемент содержит:

- `name`
- `type`
- `path`

`type` поддерживает только:

- `CONFIGURATION`
- `EXTENSION`
- `EXTERNAL_DATA_PROCESSORS`
- `EXTERNAL_REPORTS`

Validation rules:

- `name` должен быть уникальным и безопасным path segment;
- `EXTENSION` требует хотя бы один `CONFIGURATION`, но external-only config допустим;
- для `format=DESIGNER` ordinary source-set должен указывать на корректный Designer root;
- для `format=DESIGNER` external source-set должен быть aggregate root с top-level XML
  descriptors matching declared `type`;
- для `format=EDT` ordinary `CONFIGURATION`/`EXTENSION` path должен быть valid EDT project root:
  каталог с `.project`, правильным nature, `DT-INF/PROJECT.PMF` и project-local native markers;
- для `format=EDT` external path должен быть каталогом direct child projects, и все найденные
  child projects должны совпадать с declared external `type`.

## Опциональные секции

### `build`

#### `build.partialLoadThreshold`

- Тип: integer
- По умолчанию: `20`
- Минимум: `1`

Порог между partial и full load.

CLI selector `v8-runner build --source-set <name>` использует `source-set[].name` как stable
runtime identity и не добавляет отдельное поле конфигурации. Если selector не задан, `build`
обрабатывает все `source-set`.

### `tests`

#### `tests.execution_timeout_seconds`

- Тип: integer
- По умолчанию: `300`
- Диапазон: `1..=86400`

#### `tests.yaxunit.timeouts.total_ms`

- Тип: integer

#### `tests.va`

Поддержанные поля:

- `params_path`
- `profile`
- `fail_fast`
- `timeouts.total_ms`
- `profiles.<name>.feature_path`
- `profiles.<name>.features_to_run`
- `profiles.<name>.filter_tags`
- `profiles.<name>.ignore_tags`
- `profiles.<name>.scenario_filter`

`v8-runner test va --feature`, `--filter-tag`, `--ignore-tag` и `--scenario-filter`
переопределяют соответствующие списки выбранного профиля только для текущего CLI-запуска.

При генерации runtime `VAParams` runner добавляет `WorkspaceRoot` со значением `basePath`,
если это поле отсутствует или равно `null` в `tests.va.params_path`.

Для Vanessa Automation обязательны:

- `tools.va.epf_path`
- `tests.va.params_path`
- `tests.va.profile`
- `tests.va.profiles.<name>.feature_path`

Поля `startup_ms` и `run_ms` внутри `tests.*.timeouts` зарезервированы и сейчас не влияют на
запуск.

### `mcp.http`

Поддержанные поля:

- `bind_address`, по умолчанию `127.0.0.1:3000`
- `path`, по умолчанию `/mcp`
- `stateful_sessions`, по умолчанию `true`
- `max_sessions`, по умолчанию `64`
- `idle_ttl_secs`, по умолчанию `900`

### `mcp.execution`

Поддержанные поля:

- `max_concurrent_calls`, по умолчанию `1`
- `shutdown_grace_period_secs`, по умолчанию `30`

### `tools.client_mcp`

Поддержанные поля:

- `port`, опциональный порт клиентского MCP-сервера onec-client-mcp-devkit.
- `extension`, опциональное tool extension для клиентского MCP-сервера.

`launch mcp` передаёт это значение как `mcpPort` внутри `/C"runMcp..."`
если CLI не указал `--mcp-port`.

`extension` поддерживает:

- `name`, обязательное безопасное имя расширения в ИБ;
- ровно один источник:
  - `source.path` и опциональный `source.format` (`DESIGNER` или `EDT`, по умолчанию global
    `format`);
  - `artifact.path` на существующий `.cfe` файл.

`tools.client_mcp.extension` не добавляется в `source-set` и не выбирается через `--source-set`.
`init` импортирует EDT `source` в workspace, `build` подготавливает расширение после project
source-set build, а `launch mcp` и `launch mcp va` расширение не устанавливают и не обновляют.

### `tools.va`

Поддержанные поля:

- `epf_path`, путь к внешней обработке Vanessa Automation.

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

Поведение:

- `8.3.27.1859`: требуется точное совпадение;
- `8.3.20`: выбирается максимальная найденная сборка `8.3.20.*`;
- `8.3`: выбирается максимальная найденная версия `8.3.*.*`.

## `tools.enterprise`

### `tools.enterprise.additional-launch-keys`

- Тип: список строк
- Обязателен: нет

Canonical key:

- `additional-launch-keys`

Поддержанные aliases:

- `additional_launch_keys`
- `additionalLaunchKeys`

Ключи добавляются к enterprise client launch.

## `tools.edt_cli`

Canonical section key:

- `tools.edt_cli`

Поддержанный alias:

- `tools.edt-cli`

### `tools.edt_cli.path`

- Тип: путь или version-like hint
- Обязателен: нет

Поддержанные варианты:

- абсолютный путь к `1cedtcli`;
- путь к каталогу установки EDT;
- version-like hint, например `2025.2.3`.

### `tools.edt_cli.version`

- Тип: строка
- Обязателен: нет

Отдельная подсказка для автопоиска EDT.

### `tools.edt_cli.interactive-mode`

- Тип: boolean
- По умолчанию: `false`

Переключает EDT execution между one-shot и shared interactive model.

### `tools.edt_cli.auto-start`

- Тип: boolean
- По умолчанию: `false`

Имеет эффект только вместе с `interactive-mode=true` и только для long-lived host process. На
текущем этапе это MCP server. CLI не делает eager prewarm и стартует EDT лениво при первом
EDT-вызове.

### `tools.edt_cli.startup_timeout_ms`

- Тип: integer
- По умолчанию: `300000`

Поддержанный alias:

- `startup-timeout-ms`

### `tools.edt_cli.command_timeout_ms`

- Тип: integer
- По умолчанию: `300000`

Поддержанный alias:

- `command-timeout-ms`

## Неподдержанные ключи

### `tools.edt_cli.working-directory`

Текущий статус:

- не входит в supported config contract;
- игнорируется как неизвестный YAML key;
- рабочий каталог EDT session сейчас фиксирован: `workPath/edt-workspace`.
