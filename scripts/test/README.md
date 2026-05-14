# `scripts/test`

Набор helper-скриптов для локального smoke/UAT запуска и для CI entrypoint-слоя вокруг `v8-runner`.

Здесь разделены четыре роли:

- CI orchestration: выбор и запуск нужного контура проверки.
- Live CLI smoke: запуск реального сценария через CLI `v8-runner`.
- UAT wrappers: запуск "с нуля" с подготовкой бинаря и очисткой артефактов.
- MCP smoke: проверка HTTP-поднятия MCP сервера и вызовов MCP tools.

## Карта вызовов

```text
.github/workflows/ci.yml
  -> ci-platform-install.sh
  -> ci-designer-config.sh
  -> ci-ibsrv.sh
  -> ci-rust.sh
    -> ci-happy-path.sh
      -> live-cli-fixture.sh

ci-rust.sh
  -> ci-happy-path.sh
    -> live-cli-fixture.sh

live-cli-designer.sh
  -> live-cli-fixture.sh

live-cli-ibcmd.sh
  -> live-cli-fixture.sh

uat-cli-ibcmd.sh
  -> live-cli-ibcmd.sh
    -> live-cli-fixture.sh

live-mcp-http.py
  -> v8-runner mcp serve http
  -> MCP initialize/tools/list/tools/call smoke
```

## Скрипты

| Файл | Роль | Назначение | Зона ответственности |
| --- | --- | --- | --- |
| `ci-rust.sh` | CI entrypoint | Диспетчер CI-контуров по `V8_RUNNER_CI_SCOPE` | Выбрать нужный scope и передать управление в `cargo test` или `ci-happy-path.sh` |
| `ci-happy-path.sh` | CI helper | Canonical happy-path для trusted CI | Собрать бинарь, выполнить `cargo check/test`, затем запустить обязательный packaging/live contour |
| `ci-platform-install.sh` | CI helper | Установить 1С platform bundle на GitHub-hosted runner | Скачать secret-backed bundle, проверить checksum, распаковать и отдать `tools.platform.path`/`ibsrv` paths |
| `ci-designer-config.sh` | CI helper | Материализовать dedicated live config для mandatory CI smoke | Подготовить `format=DESIGNER`, `builder=DESIGNER`, file `infobase.connection`, required source-set'ы и `tools.platform.path` |
| `ci-ibsrv.sh` | CI helper | Поднять/остановить standalone `ibsrv` sidecar для trusted happy-path | Запустить `ibsrv` с `--data` и `--db-path`, синхронизированным с `V8TR_DESIGNER_REAL_CONFIG`, и корректно завершить процесс |
| `live-cli-fixture.sh` | Общий harness | Универсальный fixture-based smoke для `format=DESIGNER` с `builder=DESIGNER` или `builder=IBCMD` | Валидировать config, развернуть workspace, выполнить последовательность `init/build/extensions/...`, проверить JSON/output артефакты |
| `live-cli-designer.sh` | Live entrypoint | Удобный ручной запуск smoke для `builder=DESIGNER` | Подготовить designer live-config и передать его в `live-cli-fixture.sh` |
| `live-cli-ibcmd.sh` | Live entrypoint | Удобный ручной запуск smoke для `builder=IBCMD` | Сгенерировать IBCMD-конфиг из designer fixture, убрать неподходящие source-set и передать управление в `live-cli-fixture.sh` |
| `uat-cli-ibcmd.sh` | UAT wrapper | Полный запуск IBCMD smoke "с нуля" | Собрать бинарь, очистить старые артефакты и вызвать `live-cli-ibcmd.sh` |
| `live-mcp-http.py` | MCP smoke | Live-проверка MCP HTTP сервера | Поднять `mcp serve http`, выполнить `initialize`, `tools/list` и `tools/call` smoke-последовательность |

## Границы ответственности

### `ci-rust.sh`

- Отвечает только за выбор CI-scope.
- Не знает деталей live fixture и не должен дублировать live smoke-логику.

### `ci-happy-path.sh`

- Отвечает за canonical blocking helper chain для happy-path.
- Должен собирать Rust-бинарь и запускать обязательные Rust-проверки до live contour.
- Не должен дублировать шаги `init/build/make/dump`; за них отвечает `live-cli-fixture.sh`.

### `ci-platform-install.sh`

- Используется только в GitHub Actions trusted live path.
- Принимает OS-specific secret URL bundle-а и обязательный SHA256, затем извлекает из bundle минимум `1cv8` и `ibsrv`.
- Не должен знать про `build/test/make`; его задача заканчивается на установке platform root/hint и путей до утилит.

### `ci-designer-config.sh`

- Используется только в GitHub Actions trusted live path.
- Материализует explicit `V8TR_DESIGNER_REAL_CONFIG`, чтобы mandatory smoke не зависел от implicit fallback.
- Выносит file infobase path в dedicated runtime directory вне canonical artifact output root, потому что `live-cli-fixture.sh` очищает `target/manual-tests/live-cli-designer` перед запуском.

### `ci-ibsrv.sh`

- Используется только в GitHub Actions trusted live path.
- Поднимает standalone `ibsrv` как sidecar на том же file database path, который зашит в dedicated CI config.
- Не меняет сам happy-path chain: его задача ограничивается bootstrap/health/teardown отдельного процесса вокруг вызова `ci-rust.sh`.

### `live-cli-fixture.sh`

- Это главный исполнитель реального fixture-based smoke.
- Он владеет порядком шагов, валидацией config, подготовкой workspace и проверкой результата.
- Для `builder=DESIGNER` выполняет `syntax`, opt-in `test`, упаковку `.cf/.cfe/.epf/.erf` и проверку deploy-ready артефактов.
- Для `builder=IBCMD` выполняет `dump full/incremental/partial` smoke вместо designer-specific packaging.
- Именно этот скрипт является общим contract layer для `live-cli-designer.sh` и `live-cli-ibcmd.sh`.

### `live-cli-designer.sh`

- Это тонкая обёртка над `live-cli-fixture.sh`.
- Если `V8TR_DESIGNER_REAL_CONFIG` не задан, скрипт сам материализует временный config из `live-cli-designer.fixture.yaml`.
- Не должен повторять шаги smoke-harness; его зона ответственности заканчивается на подготовке designer-config и env.

### `live-cli-ibcmd.sh`

- Это тонкая IBCMD-специализация поверх общего fixture harness.
- Если `V8TR_IBCMD_REAL_CONFIG` не задан, скрипт материализует временный config на основе `live-cli-designer.fixture.yaml`.
- Скрипт меняет `builder` на `IBCMD` и удаляет `EXTERNAL_DATA_PROCESSORS` и `EXTERNAL_REPORTS`, потому что этот contour ориентирован на IBCMD-compatible сценарий.
- Не должен заниматься сборкой бинаря и общей очисткой стенда; это ответственность UAT wrapper или вызывающего окружения.

### `uat-cli-ibcmd.sh`

- Это сценарий "запустить IBCMD UAT с чистого листа".
- Он отвечает за `cargo build`, удаление `target/manual-tests/live-cli-ibcmd` и `target/manual-tests/live-cli-ibcmd.generated.yaml`, а затем за запуск `live-cli-ibcmd.sh`.
- Он не должен дублировать материализацию IBCMD-config или шаги smoke-runner.

### `live-mcp-http.py`

- Отвечает за smoke-проверку MCP HTTP protocol path.
- Использует реальный config из `V8TR_REAL_CONFIG`, поднимает сервер, ждёт порт, затем как клиент вызывает MCP lifecycle и tools.
- Не заменяет CLI smoke; это отдельный контур, который проверяет именно MCP transport и tool contract.

## Вспомогательные файлы

| Файл | Назначение | Ответственность |
| --- | --- | --- |
| `live-cli-designer.fixture.yaml` | Базовый шаблон fixture-based live config | Описывает `format`, `builder`, `infobase`, `source-set` и placeholders для изолированного запуска |
| `live-cli-designer.va-params.json` | Шаблон параметров Vanessa Automation | Используется в opt-in `V8TR_DESIGNER_TEST_MODE=va` |
| `features/live-cli-designer/smoke.feature` | Минимальный VA smoke-feature | Держит простой сценарий для проверки интеграции VA |

## Основные переменные окружения

| Переменная | Где используется | Назначение |
| --- | --- | --- |
| `V8_RUNNER_CI_SCOPE` | `ci-rust.sh` | Выбор CI-контура: `contract`, `full`, `runtime-locks`, `happy-path` |
| `V8TR_BIN` | Почти все entrypoint-скрипты | Явный путь к бинарю `v8-runner` |
| `V8TR_PLATFORM_BUNDLE_URL` | `ci-platform-install.sh` | Secret-backed URL platform bundle для текущей ОС |
| `V8TR_PLATFORM_BUNDLE_SHA256` | `ci-platform-install.sh` | Обязательный SHA256 platform bundle для trusted install path |
| `V8TR_DESIGNER_REAL_CONFIG` | `live-cli-fixture.sh`, `live-cli-designer.sh` | Явный live-config для designer contour |
| `V8TR_IBCMD_REAL_CONFIG` | `live-cli-ibcmd.sh` | Явный live-config для IBCMD contour |
| `V8TR_LIVE_CLI_OUTPUT_ROOT` | `live-cli-designer.sh`, `live-cli-ibcmd.sh`, `live-cli-fixture.sh` | Корень выходных артефактов под `target/manual-tests/*` |
| `V8TR_CI_RUNTIME_ROOT` | `ci-designer-config.sh`, `ci-ibsrv.sh` | Отдельный runtime root для CI infobase, `ibsrv` data/log/pid и generated config |
| `V8TR_INFOBASE_PATH` | `ci-designer-config.sh`, `ci-ibsrv.sh` | Явный file infobase path, синхронизированный между config и `ibsrv --db-path` |
| `V8TR_IBSRV_PATH` | `ci-platform-install.sh`, `ci-ibsrv.sh` | Путь до standalone `ibsrv`, извлечённый из platform bundle |
| `V8TR_DESIGNER_TEST_MODE` | `live-cli-fixture.sh` | Opt-in запуск реального 1С test-stage: `none`, `va`, `yaxunit-all`, `module` |
| `V8TR_DESIGNER_ALLOW_MISSING_CONFIG` | `live-cli-fixture.sh`, `.github/workflows/ci.yml` | Trusted/fork gating hook для soft-skip mandatory live contour на untrusted контексте |
| `V8TR_REAL_CONFIG` | `live-mcp-http.py` | Реальный config для MCP HTTP smoke |

## Типовые сценарии запуска

### Contract / Rust CI

```bash
bash scripts/test/ci-rust.sh
```

GitHub Actions currently runs this blocking contract on `ubuntu-latest`.
Re-enabling Windows as blocking requires hardening the existing Unix-assumptive
unit/helper tests first.

### Happy-path CI helper

```bash
V8_RUNNER_CI_SCOPE=happy-path bash scripts/test/ci-rust.sh
```

### Trusted CI wiring helpers

```bash
bash scripts/test/ci-platform-install.sh
bash scripts/test/ci-designer-config.sh
bash scripts/test/ci-ibsrv.sh start
V8_RUNNER_CI_SCOPE=happy-path bash scripts/test/ci-rust.sh
bash scripts/test/ci-ibsrv.sh stop
```

### Ручной live smoke для Designer

```bash
bash scripts/test/live-cli-designer.sh
```

### Ручной live smoke для IBCMD

```bash
bash scripts/test/live-cli-ibcmd.sh
```

### IBCMD UAT "с нуля"

```bash
bash scripts/test/uat-cli-ibcmd.sh
```

### MCP HTTP smoke

```bash
python3 scripts/test/live-mcp-http.py
```

## Нота по legacy entrypoint

Legacy-скрипт `live-cli.sh` удалён. Вместо него нужно использовать специализированные entrypoint'ы:

- `live-cli-designer.sh` для `builder=DESIGNER`
- `live-cli-ibcmd.sh` для `builder=IBCMD`
