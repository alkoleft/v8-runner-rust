# План тестирования на реальном окружении

## Цель

Закрыть два разных контура проверки:

1. `contract/regression` для локального запуска и обычного CI без реальной 1С-инфраструктуры.
2. `live smoke` для реального окружения с установленными `1cv8`, `1cv8c`, `1cedtcli`, рабочей файловой ИБ и выделенными конфигами для EDT и IBCMD сценариев.

На `2026-03-21` базовый автоматизированный прогон `cargo test` в этом репозитории проходит полностью: `342` теста.

## Эталонное реальное окружение

Текущий референс для live-проверок:

- проект: `/home/alko/develop/open-source/ai/mcp/onec-client-mcp-devkit`
- конфиг: `/home/alko/develop/open-source/ai/mcp/onec-client-mcp-devkit/.agents/tools/onec-client-mcp-devkit.edt.yaml`

Этот конфиг уже задает:

- `format: EDT`
- `builder: DESIGNER`
- файловую ИБ `File=/home/alko/develop/onec_file_db/client-mcp`
- три `source-set`: `configuration`, `ClientMcp`, `tests`
- платформу `8.5.1.1150`
- `1cedtcli` `2025.2.3`

## Разделение наборов тестов

### 1. Generic local/CI

Назначение: быстрый сигнал по контрактам CLI/MCP и внутренней логике без реальных бинарей 1С.

Команда:

```bash
bash scripts/test/ci-rust.sh
```

Что проверяет:

- unit-тесты use case-ов, парсеров, конфигурации и platform DSL
- интеграционные CLI-тесты на стабах процессов
- интеграционные MCP stdio/http тесты на стабах процессов

Где запускать:

- локально перед коммитом
- в обычном CI на generic Linux runner

Критерий успеха:

- `cargo test --locked` завершился без ошибок

Перед запуском live-сценариев нужно явно экспортировать конфиг. Рекомендуемое значение на текущем стенде:

```bash
export V8TR_REAL_CONFIG=/home/alko/develop/open-source/ai/mcp/onec-client-mcp-devkit/.agents/tools/onec-client-mcp-devkit.edt.yaml
```

Скрипты намеренно не держат этот путь как default, чтобы не привязывать запуск к одному файловому дереву и не рисковать чужой реальной ИБ.

### 2. Live CLI smoke

Назначение: проверить, что CLI работает на реальном EDT-проекте и реальной ИБ.

Команда:

```bash
bash scripts/test/live-cli.sh
```

Что выполняется по умолчанию:

1. `build`
2. `syntax edt`
3. `test module <smoke module>`

Опционально:

- `launch --mode thin`, только при `V8TR_ENABLE_LAUNCH=1`
- designer-only проверки, если появится отдельный `DESIGNER` конфиг

Почему `launch` не включен по умолчанию:

- на headless self-hosted runner шаг часто нестабилен из-за GUI/desktop-зависимостей

Переменные окружения:

- `V8TR_REAL_CONFIG` - обязателен; путь к live YAML-конфигу
- `V8TR_BIN` - путь к бинарю `v8-test-runner`
- `V8TR_SMOKE_MODULE` - smoke-модуль YaXUnit, по умолчанию `ЮТДымовыеТесты`
- `V8TR_ENABLE_LAUNCH=1` - включить шаг `launch`

Критерий успеха:

- все CLI-команды завершились с `exit code 0`
- для `test module` есть зеленый прогон smoke-модуля

Артефакты для анализа при падении:

- `workPath/logs/**`
- `workPath/temp/**`
- stdout/stderr конкретной команды

Почему в этот smoke не включен `syntax designer-*`:

- референсный devkit-конфиг находится в `format: EDT`
- `syntax designer-config` и `syntax designer-modules` поддерживаются только для `builder=DESIGNER` и `format=DESIGNER`
- для этих проверок нужен отдельный live `DESIGNER`-конфиг

### 3. Live MCP HTTP smoke

Назначение: проверить живой MCP transport и бизнес-интеграцию поверх того же EDT-конфига.

Команда:

```bash
python3 scripts/test/live-mcp-http.py
```

Что выполняется:

1. старт `v8-test-runner mcp serve http`
2. `initialize`
3. `notifications/initialized`
4. `tools/list`
5. `tools/call build_project`
6. `tools/call check_syntax_edt`
7. `tools/call run_module_tests`

Переменные окружения:

- `V8TR_REAL_CONFIG` - обязателен; путь к live YAML-конфигу
- `V8TR_BIN` - путь к бинарю `v8-test-runner`
- `V8TR_MCP_URL` - URL MCP HTTP endpoint, по умолчанию `http://127.0.0.1:3000/mcp`
- `V8TR_SMOKE_MODULE` - smoke-модуль YaXUnit, по умолчанию `ЮТДымовыеТесты`
- `V8TR_EDT_PROJECT` - EDT project для `check_syntax_edt`, по умолчанию `configuration`
- `V8TR_HTTP_TIMEOUT_SECONDS` - timeout одного HTTP вызова
- `V8TR_SERVER_STARTUP_TIMEOUT_SECONDS` - ожидание старта MCP HTTP сервера

Технические требования:

- `python3 >= 3.8`

Критерий успеха:

- transport-level HTTP статусы корректны: `200` и `202`
- `tools/list` возвращает как минимум обязательное подмножество инструментов: `build_project`, `check_syntax_edt`, `run_module_tests`
- `build_project`, `check_syntax_edt`, `run_module_tests` возвращают `structuredContent.status=success`
- для `build_project` и `run_module_tests` поле `result.success=true`
- для `check_syntax_edt` поле `result.check_result` находится в `clean|issues_found`

Артефакты для анализа при падении:

- `target/manual-tests/live-mcp-http/server.stderr.log`
- `workPath/logs/mcp/actions.log`
- `workPath/temp/**`
- JSON-RPC/SSE payload текущего упавшего шага

### 4. Live CLI IBCMD smoke

Назначение: проверить реальный CLI path для `format=DESIGNER + builder=IBCMD` на файловой ИБ.

Команда:

```bash
bash scripts/test/live-cli-ibcmd.sh
```

Что выполняется:

1. `init`
2. `build`
3. `dump --mode full`
4. `dump --mode incremental`
5. `dump --mode partial --object Catalog.Items`
6. `extensions`

Что не выполняется намеренно:

- реальный `partial-fail` сценарий в live-контуре, чтобы не добавлять недетерминированные или потенциально опасные для рабочей ИБ условия;
- `syntax edt` и `test module`, так как этот smoke предназначен для IBCMD-конфига `DESIGNER`-формата и проверяет только целевой IBCMD path.

Skip policy:

- если `V8TR_IBCMD_REAL_CONFIG` не задан, скрипт завершает прогон со статусом `SKIPPED` и `exit code 0`;
- это считается корректным поведением для окружений без выделенного IBCMD live-стенда;
- если `V8TR_IBCMD_REAL_CONFIG` задан, но файл отсутствует, не соответствует `format: DESIGNER` + `builder: IBCMD` или использует не файловую строку подключения (`File=...` / raw `/F ...`), скрипт завершает прогон с ранней понятной ошибкой.

Переменные окружения:

- `V8TR_IBCMD_REAL_CONFIG` - путь к отдельному live YAML-конфигу для IBCMD smoke (`format: DESIGNER`, `builder: IBCMD`, файловая ИБ)
- `V8TR_BIN` - путь к бинарю `v8-test-runner`

Критерий успеха:

- при заданном валидном `V8TR_IBCMD_REAL_CONFIG` все команды smoke завершаются с `exit code 0`;
- при незаданном `V8TR_IBCMD_REAL_CONFIG` сценарий завершается как `SKIPPED` (`exit code 0`).

Артефакты для анализа при падении:

- `workPath/logs/**`
- `workPath/temp/**`
- stdout/stderr конкретной команды

## Рекомендуемый порядок запуска

### Локально

1. `bash scripts/test/ci-rust.sh`
2. `bash scripts/test/live-cli.sh`
3. `python3 scripts/test/live-mcp-http.py`
4. `bash scripts/test/live-cli-ibcmd.sh`

### CI

#### Generic CI

Запускать только:

```bash
bash scripts/test/ci-rust.sh
```

#### Self-hosted CI с 1С/EDT

После generic CI или в отдельном job запускать:

```bash
bash scripts/test/live-cli.sh
python3 scripts/test/live-mcp-http.py
bash scripts/test/live-cli-ibcmd.sh
```

Рекомендация:

- держать live smoke в отдельном job/stage
- не делать его обязательным для любого внешнего PR, если runner и ИБ недоступны

## Матрица покрытия

| Контур | Build | Syntax EDT | Syntax Designer | YaXUnit | Launch | MCP initialize/list/tools |
| --- | --- | --- | --- | --- | --- | --- |
| `ci-rust` | mock | mock | mock | mock | mock | mock |
| `live-cli` | real | real | requires separate DESIGNER config | real | optional real | n/a |
| `live-mcp-http` | real via MCP | real via MCP | n/a | real via MCP | n/a | real |
| `live-cli-ibcmd` | real (`IBCMD`) | n/a | n/a | n/a | n/a | n/a |

## Ограничения и риски

- Live smoke меняет состояние реальной ИБ и рабочего каталога.
- `launch` зависит от GUI-окружения и поэтому оставлен opt-in.
- Smoke-модуль привязан к devkit-проекту; при переименовании нужно обновить `V8TR_SMOKE_MODULE`.
- В обычный CI нельзя переносить live smoke без self-hosted runner и установленной 1С-инфраструктуры.
- Для `live-cli-ibcmd` обязательный реальный стенд может отсутствовать; в этом случае сценарий штатно завершает прогон как `SKIPPED`.
