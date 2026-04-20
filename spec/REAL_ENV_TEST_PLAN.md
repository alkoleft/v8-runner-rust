# План тестирования на реальном окружении

## Цель

Начиная с `2026-04-17`, source of truth для real-env happy-path должен быть будущий GitHub Actions matrix contract на `ubuntu-latest` и `windows-latest`, а локальные скрипты в `scripts/test/*` остаются helper/entrypoint-слоем для этого workflow.

Обязательный smoke-контур для обеих ОС один и тот же:

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

Default mandatory `live-cli-designer` не запускает интерактивный 1С test runner: в фикстурной ИБ репозитория нет надежного headless раннера, который обязан завершаться на всех ОС. Реальная 1С test-stage остается явным opt-in через `V8TR_DESIGNER_TEST_MODE=va|yaxunit-all|module`.

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

### 2. Mandatory Linux/Windows happy-path

Назначение: одинаково обязательный smoke для `Linux` и `Windows` на trusted контексте.

Canonical entrypoint:

```bash
V8_RUNNER_CI_SCOPE=happy-path bash scripts/test/ci-rust.sh
```

Реальный helper chain:

1. `cargo build --locked --bin v8-runner`
2. `cargo check --locked --all-targets`
3. `cargo test --locked`
4. `bash scripts/test/live-cli-designer.sh`

`scripts/test/live-cli-designer.sh` в mandatory профиле обязан выполнить одинаковые стадии для обеих ОС:

1. `init/setup infobase`
2. `build --full-rebuild`
3. `syntax designer-config`
4. `syntax designer-modules`
5. `test`
6. `make` для `.cf/.cfe/.epf/.erf`
7. проверку, что все deploy-ready артефакты существуют и не пусты

### 3. Non-blocking live contours

Эти сценарии сохраняются отдельно и не являются частью mandatory matrix happy-path:

- `bash scripts/test/live-cli.sh`
- `python3 scripts/test/live-mcp-http.py`
- `bash scripts/test/live-cli-ibcmd.sh`

Они остаются полезными для расширенной диагностики, но не определяют blocking-успех canonical Linux/Windows chain.

## Gating contract

Mandatory happy-path должен быть blocking только для:

- `master`
- trusted branches
- same-repo PR

Для fork PR live jobs не должны становиться blocking. В этом репозитории это пока выражено не workflow-файлом, а env hook-контрактом:

- mandatory designer smoke требует `V8TR_DESIGNER_REAL_CONFIG`
- workflow может разрешить soft-skip только через `V8TR_DESIGNER_ALLOW_MISSING_CONFIG=1`
- без этого hook `scripts/test/live-cli-designer.sh` падает, если `V8TR_DESIGNER_REAL_CONFIG` не задан

## Контракт `live-cli-designer`

Команда:

```bash
bash scripts/test/live-cli-designer.sh
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
- `basePath`, резолвящийся в `tests/fixtures/designer`
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
python3 scripts/test/live-mcp-http.py
bash scripts/test/live-cli-ibcmd.sh
```

### Будущий GitHub Actions matrix

Для `ubuntu-latest` и `windows-latest` blocking должен использоваться один и тот же entrypoint:

```bash
V8_RUNNER_CI_SCOPE=happy-path bash scripts/test/ci-rust.sh
```

Install/bootstrap шаги для 1С, `ibsrv`, trusted/fork gating, artifact upload и branch filters пользователь добавит позже в workflow wiring.

Windows runner contract for this helper layer is explicit:

- invoke the entrypoint through `bash`
- provide `python3` in PATH
- allow the helper to use `python3` for config normalization, JSON checks, and platform detection

## Матрица покрытия

| Контур | Linux | Windows | Blocking | Build | Syntax/check | Test | Package | Deploy-ready artifacts |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `ci-rust contract` | yes | yes | yes | Rust | Rust | Rust | no | no |
| `ci-rust happy-path` | yes | yes | yes on trusted | Rust + real 1C | real | Rust by default; real 1C opt-in | real | real |
| `live-mcp-http` | optional | optional | no | real via MCP | real via MCP | real via MCP | n/a | n/a |
| `live-cli-ibcmd` | optional | optional | no | real (`IBCMD`) | n/a | n/a | diagnostic dump/export only | n/a |
| `live-cli` | optional | optional | no | real | real | real | n/a | n/a |

## Ограничения и TODO hooks

- В репозитории пока нет новых `.github/workflows/*.yml`; здесь зафиксированы только matrix/contract/TODO hooks.
- Установка 1С, bootstrap файловой ИБ через `ibsrv`, artifact upload и branch/fork gating остаются внешним workflow wiring.
- `live-cli-designer` по умолчанию не запускает 1С test-stage; `va`, `yaxunit-all` и `module` остаются opt-in режимами для стендов, где установлен и проверен соответствующий headless runner.
- `live-mcp-http` и `live-cli-ibcmd` остаются отдельными non-blocking контурами.
- Mandatory designer smoke requires `V8TR_DESIGNER_REAL_CONFIG`; `V8TR_DESIGNER_ALLOW_MISSING_CONFIG=1` is reserved for fork/non-blocking soft-skip contexts.
