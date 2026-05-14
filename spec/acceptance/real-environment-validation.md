# Проверка на реальном окружении

## Цель

Начиная с `2026-04-22`, source of truth для real-env happy-path является GitHub Actions workflow [`ci.yml`](../.github/workflows/ci.yml), а локальные скрипты в `scripts/test/*` остаются helper/entrypoint-слоем для этого workflow.

Обязательный smoke-контур:

1. `build`
2. `syntax/check`
3. `test` Rust/CLI/MCP-контракта
4. `package`
5. `deploy-ready artifacts`

Под `deploy-ready artifacts` в этом репозитории понимается только публикация и проверка наличия/непустоты следующих файлов:

- `.cf`
- `.cfe`
- `.epf`
- `.erf`

Ни `load`, ни обратное `apply` в mandatory happy-path не входят.

Default mandatory `live-cli-fixture` не запускает интерактивный 1С test runner: в фикстурной ИБ репозитория нет надежного headless раннера, который обязан завершаться на всех ОС. Реальная 1С test-stage остается явным opt-in через `V8TR_DESIGNER_TEST_MODE=va|yaxunit-all|module`.

Для `V8TR_DESIGNER_TEST_MODE=va` дополнительно нужны:

- `tests/fixtures/vanessa-automation-single.epf` или `V8TR_VA_EPF`
- `scripts/test/live-cli-designer.va-params.json` или `V8TR_VA_PARAMS_TEMPLATE`
- `scripts/test/features/live-cli-designer` или `V8TR_VA_FEATURE_PATH`

## Контуры

### 1. Contract / regression

Назначение: быстрый сигнал по Rust/CLI/MCP-контрактам без реальной 1С-инфраструктуры.

Команда:

```bash
bash scripts/test/ci-rust.sh
```

Поведение:

- `V8_RUNNER_CI_SCOPE=contract` или `full` запускает `cargo test --locked`
- `V8_RUNNER_CI_SCOPE=runtime-locks` запускает только lock-focused regression subset
- `V8_RUNNER_CI_SCOPE=happy-path` запускает обязательную цепочку `build -> syntax/check -> test -> package -> deploy-ready artifacts`

### 2. Mandatory happy-path

Назначение: обязательный smoke на trusted контексте. Blocking GitHub Actions runner сейчас `ubuntu-latest`; Windows full-test/live path остается TODO до hardening существующих Unix-assumptive тестов и helper-фикстур.

Canonical entrypoint:

```bash
V8_RUNNER_CI_SCOPE=happy-path bash scripts/test/ci-rust.sh
```

Реальный helper chain:

1. `cargo build --locked --bin v8-runner`
2. `cargo check --locked --all-targets`
3. `cargo test --locked`
4. `bash scripts/test/live-cli-fixture.sh`

`scripts/test/live-cli-fixture.sh` в mandatory профиле обязан выполнить стадии:

1. `init/setup infobase`
2. `build --full-rebuild`
3. `syntax designer-config`
4. `syntax designer-modules`
5. `test`
6. `make` для `.cf/.cfe/.epf/.erf`
7. проверку, что все deploy-ready артефакты существуют и не пусты

### 3. Non-blocking live contours

Эти сценарии сохраняются отдельно и не являются частью mandatory matrix happy-path:

- `bash scripts/test/live-cli-designer.sh`
- `python3 scripts/test/live-mcp-http.py`
- `bash scripts/test/live-cli-ibcmd.sh`

Они остаются полезными для расширенной диагностики, но не определяют blocking-успех canonical Linux/Windows chain.

## Gating contract

Mandatory happy-path должен быть blocking только для:

- `master`
- trusted branches
- same-repo PR

Для fork PR live jobs не должны становиться blocking. В этом репозитории это выражено workflow-файлом `.github/workflows/ci.yml` и тем же env hook-контрактом:

- mandatory designer smoke требует `V8TR_DESIGNER_REAL_CONFIG`
- workflow может разрешить soft-skip только через `V8TR_DESIGNER_ALLOW_MISSING_CONFIG=1`
- без этого hook `scripts/test/live-cli-fixture.sh` падает, если `V8TR_DESIGNER_REAL_CONFIG` не задан
- trusted path устанавливает 1С из OS-specific bundle secret, материализует dedicated `format: DESIGNER` + `builder: DESIGNER` config, запускает `ibsrv` sidecar на том же file-infobase path и только потом вызывает canonical entrypoint `V8_RUNNER_CI_SCOPE=happy-path bash scripts/test/ci-rust.sh`
- fork PR и Dependabot не получают install/bootstrap/upload path: workflow передает только `V8TR_DESIGNER_ALLOW_MISSING_CONFIG=1`, а upload deploy-ready артефактов остаётся trusted-only

## Контракт `live-cli-fixture`

Команда:

```bash
bash scripts/test/live-cli-fixture.sh
```

Обязательные переменные окружения:

- `V8TR_DESIGNER_REAL_CONFIG` - отдельный YAML-конфиг для fixture-based `format: DESIGNER` + `builder: DESIGNER`; обязателен для mandatory smoke

Опциональные hook-переменные:

- `V8TR_BIN` - путь к бинарю `v8-runner`
- `V8TR_PLATFORM_PATH` - явный override пути до `1cv8`/`1cv8.exe`
- `V8TR_DESIGNER_SMOKE_PROFILE=mandatory|extended` - mandatory по умолчанию; `extended` включает dump-only хвост
- `V8TR_DESIGNER_TEST_MODE=none|va|yaxunit-all|module` - явный запуск 1С test-stage helper-а; `none` по умолчанию
- `V8TR_DESIGNER_TEST_MODULE` - обязателен при `V8TR_DESIGNER_TEST_MODE=module`
- `V8TR_DESIGNER_ALLOW_MISSING_CONFIG=1` - разрешить `SKIPPED` вместо hard failure только для non-blocking/soft-skip контекстов

Требования к конфигу:

- `format: DESIGNER`
- `builder: DESIGNER`
- файловое подключение `File=...` или raw `/F ...`
- primary config directory, резолвящийся в `tests/fixtures/designer`
- source-set'ы для `configuration`, `extension`, `external-processor`, `external-report`
- заданный `tools.platform.path` или внешний override `V8TR_PLATFORM_PATH`

Cross-platform hardening:

- в `bash`-окружении поддерживаются и `1cv8`, и `1cv8.exe`
- платформа может быть найдена через config path, `V8TR_PLATFORM_PATH`, `PATH`, Linux `/opt/1cv8` и Windows `Program Files`
- mandatory path не зависит от GUI и не использует `launch`
- mandatory path не делает `load/apply`

Критерий успеха:

- все стадии `build -> syntax/check -> test -> package -> deploy-ready artifacts` завершаются с `exit code 0`
- существуют и не пусты:
  - `target/manual-tests/live-cli-designer/artifacts/configuration.cf`
  - `target/manual-tests/live-cli-designer/artifacts/extension.cfe`
  - `target/manual-tests/live-cli-designer/artifacts/external-processor/*.epf`
  - `target/manual-tests/live-cli-designer/artifacts/external-report/*.erf`

## Рекомендованный порядок запуска

### Локально

```bash
bash scripts/test/ci-rust.sh
V8_RUNNER_CI_SCOPE=happy-path bash scripts/test/ci-rust.sh
bash scripts/test/live-cli-designer.sh
python3 scripts/test/live-mcp-http.py
bash scripts/test/live-cli-ibcmd.sh
```

### GitHub Actions

Blocking path использует entrypoint:

```bash
V8_RUNNER_CI_SCOPE=happy-path bash scripts/test/ci-rust.sh
```

Текущая реализация workflow wiring:

- `.github/workflows/ci.yml` публикует два job: `contract` и `happy-path`
- `contract` запускает `bash scripts/test/ci-rust.sh` с `V8_RUNNER_CI_SCOPE=contract`
- `happy-path` запускает `V8_RUNNER_CI_SCOPE=happy-path bash scripts/test/ci-rust.sh`; без platform bundle secrets workflow передает `V8TR_DESIGNER_ALLOW_MISSING_CONFIG=1`, поэтому Rust build/check/test остаются blocking, а live fixture завершается soft-skip
- trusted path использует `scripts/test/ci-platform-install.sh`, `scripts/test/ci-designer-config.sh` и `scripts/test/ci-ibsrv.sh`
- upload deploy-ready артефактов делает только trusted happy-path после успешной non-empty validation в `live-cli-fixture.sh`

Workflow secrets/inputs для platform install:

- `V8TR_PLATFORM_BUNDLE_URL_LINUX`
- `V8TR_PLATFORM_BUNDLE_SHA256_LINUX`
- `V8TR_PLATFORM_BUNDLE_URL_WINDOWS`
- `V8TR_PLATFORM_BUNDLE_SHA256_WINDOWS`

Ожидается, что bundle содержит `1cv8`/`1cv8.exe` и `ibsrv`/`ibsrv.exe`; helper-скрипт резолвит `tools.platform.path` как bin-dir или root/hint для locator contract.

Windows runner contract for this helper layer is explicit:

- invoke the entrypoint through `bash`
- provide `python3` in PATH
- allow the helper to use `python3` for config normalization, JSON checks, and platform detection
- trusted Windows path использует тот же bash helper chain, что и Linux; PowerShell нужен только косвенно как системное окружение runner-а, но не как primary shell для happy-path

## Матрица покрытия

| Контур | Linux | Windows | Blocking | Build | Syntax/check | Test | Package | Deploy-ready artifacts |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `ci-rust contract` | yes | planned | yes | Rust | Rust | Rust | no | no |
| `ci-rust happy-path` | yes | planned | yes on trusted | Rust + real 1C | real | Rust by default; real 1C opt-in | real | real |
| `live-mcp-http` | optional | optional | no | real via MCP | real via MCP | real via MCP | n/a | n/a |
| `live-cli-ibcmd` | optional | optional | no | real (`IBCMD`) | n/a | n/a | diagnostic dump/export only | n/a |
| `live-cli-designer` | optional | optional | no | real (`DESIGNER`) | real | real opt-in | real | real |

## Ограничения и TODO hooks

- Workflow `.github/workflows/ci.yml` уже зафиксировал contract/gating/upload wiring, но сам не умеет скачивать vendor installer публично: trusted path ожидает готовый platform bundle по секретному URL и обязательному SHA256.
- `ibsrv` в workflow запускается как sidecar на том же `--db-path`, который зашит в dedicated file-based Designer config; сам CLI harness по текущему контракту остаётся file-connection oriented и не переключается на server connection.
- `live-cli-fixture` по умолчанию не запускает 1С test-stage; `va`, `yaxunit-all` и `module` остаются opt-in режимами для стендов, где установлен и проверен соответствующий headless runner.
- `live-mcp-http` и `live-cli-ibcmd` остаются отдельными non-blocking контурами.
- Mandatory designer smoke requires `V8TR_DESIGNER_REAL_CONFIG`; `V8TR_DESIGNER_ALLOW_MISSING_CONFIG=1` is reserved for fork/non-blocking soft-skip contexts.
- Windows GitHub Actions full-test/live path is intentionally not blocking yet; current TODO is to remove Unix-only path and fake-executable assumptions from tests before re-enabling Windows as blocking.
