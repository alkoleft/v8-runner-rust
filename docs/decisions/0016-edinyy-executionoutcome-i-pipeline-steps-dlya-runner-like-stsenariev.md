# ADR-0016: Единый `ExecutionOutcome` и pipeline steps для runner-like сценариев

- Статус: `accepted`
- Дата: `2026-04-21`

## Контекст

Команды `v8-runner` всё чаще имеют одинаковую форму исполнения:

```text
validate request/config
  -> resolve target/profile
  -> prepare workspace/artifacts
  -> run platform or runner command
  -> collect logs/artifacts
  -> parse/normalize output
  -> publish result
  -> cleanup
  -> render through CLI/MCP
```

Сейчас часть этой модели уже появилась в коде:

1. `src/domain/execution.rs` содержит `ExecutionOutcome<T>`, `ExecutionStatus`, `ExecutionError`, `ExecutionMetrics`, `ExecutionTimeouts` и `StepResult`.
2. `src/domain/runner.rs` содержит `ScenarioExecutionRequest`, `RunnerProfile`, `RunnerKind`, output formats and retention policy.
3. `test`, `artifacts` и `load` уже используют `ExecutionOutcome<T>` частично.
4. CLI `Envelope<T>` и MCP DTO продолжают быть transport-specific presentation surfaces.

При этом разные команды всё ещё дублируют похожие поля (`ok`, `message`, `steps`, `diagnostics`, `retained_paths`, `platform_log_path`, parsed report, artifact paths) и по-разному решают, где хранить warning, degraded success, parse failure и retained artifacts.
Это усложняет добавление новых runner-like сценариев: каждый новый tool вынужден заново изобретать result shape, step accounting, artifact retention и mapping в CLI/MCP.

## Решение

Считать command execution концептуальным pipeline из стандартных блоков, а `ExecutionOutcome<T>` — canonical domain outcome для runner-like и pipeline-like сценариев.

### Pipeline composition

Pipeline — это ordered execution plan внутри use case слоя.
Он собирается из стандартных блоков и описывает, что команда собирается сделать, какие ресурсы ей нужны, какие шаги можно пропустить, где начинается external process или critical phase, какие artifacts/logs будут собраны и как итог будет нормализован.

Правила:

1. Pipeline строится после request/config validation и до запуска platform side effects, если сценарий достаточно сложный для планирования.
2. Pipeline composition остаётся transport-neutral: CLI/MCP adapter выбирает команду и request DTO, но не собирает и не исполняет pipeline blocks.
3. Pipeline block принимает typed context and typed input, а возвращает typed output, step entry, diagnostics/artifacts/errors или failure.
4. Blocks не должны обмениваться hidden global state; промежуточные paths, artifacts, runner profile, selected source-set and parsed outputs должны идти через явный typed context.
5. Pipeline может быть линейным, ветвящимся или содержать skipped steps, но любое user-visible skipped/degraded behavior должно попасть в step/outcome.
6. Resource contracts применяются на pipeline boundary: workspace lock из ADR-0011, deadline/cancellation из ADR-0014, publication safety из ADR-0015.
7. Pipeline block должен быть достаточно крупным, чтобы быть полезным для диагностики и повторного использования; не каждый private helper становится отдельным block.
8. Новая команда должна сначала попытаться собрать pipeline из существующих blocks/vocabulary, и только потом добавлять новый block kind.
9. Новый block kind должен описать input, output, failure mapping, artifacts and interruption safety.
10. Generic pipeline engine не вводится до тех пор, пока несколько команд реально не потребуют одинакового scheduling/branching/runtime layer.

### Pipeline blocks

Стандартные блоки pipeline:

1. `Validation`: проверка request/config до platform calls.
2. `ResolveTarget`: выбор source-set, extension, artifact path, runner profile или launch target.
3. `PrepareWorkspace`: создание runtime dirs, generated configs, logs, temp files, staging paths.
4. `PlatformCommand`: запуск Designer, IBCMD, Enterprise, EDT или другого внешнего runner.
5. `ParseOutput`: разбор JUnit, runner logs, Designer validation logs, EDT validation output или package metadata.
6. `Publish`: публикация dump/artifacts через supported publication contract.
7. `Cleanup`: best-effort cleanup, retention policy and orphan cleanup.
8. `Diagnostics`: сбор stderr/stdout excerpts, platform logs, actionable paths and hints.

Pipeline block не обязан быть отдельным trait/object прямо сейчас.
Публичным архитектурным контрактом является единая vocabulary, step/outcome shape и правила отображения результата.

### Outcome contract

Для runner-like/pipeline-like сценариев итоговый доменный результат должен содержать `ExecutionOutcome<T>` как source of truth для:

1. финального `ExecutionStatus`;
2. machine-readable `ExecutionError.code`;
3. human/agent diagnostics;
4. parsed metrics;
5. retained or published artifacts through `ArtifactSet`;
6. typed payload `T`, если сценарий произвёл доменный parsed result.

Top-level command result может сохранять command-specific context:

1. requested mode/scope;
2. selected source-set/extension/target;
3. duration and legacy `ok` flag for compatibility;
4. transport-facing compatibility fields, пока CLI/MCP contracts не мигрированы.

Но новые runner-like сценарии не должны создавать отдельный ad hoc набор `ok/errors/report/log_path/retained_paths`, если эти данные выражаются через `ExecutionOutcome<T>`.

### Step contract

Каждый значимый pipeline block должен иметь step entry.
Текущий `StepResult` является минимальной совместимой формой (`name`, `ok`, `duration_ms`, `message`).
Целевой step contract должен быть расширяемым и включать:

1. stable `id` или `name`;
2. `kind` из pipeline vocabulary;
3. optional target/source-set/artifact identity;
4. status (`succeeded`, `failed`, `skipped`, `degraded`);
5. duration;
6. message;
7. diagnostics and structured errors;
8. produced artifacts;
9. минимальную interruption/critical phase metadata на уровне command outcome/result, когда применимо по ADR-0014.

Пока целевой `ExecutionStep` не введён, новые сценарии должны использовать `StepResult` консервативно и не прятать значимые failures только в free-form message.
Cancellation не требует отдельного состояния на каждом step: pipeline остаётся линейным, а interruption policy применяется на command boundary и safe points.

### Status and failure semantics

1. `ExecutionStatus::Succeeded` означает, что pipeline достиг expected result.
2. `ExecutionStatus::Failed` означает business/runtime failure, который не является timeout и не сводится к malformed runner output.
3. `ExecutionStatus::TimedOut` используется только после terminal-state semantics из ADR-0014.
4. `ExecutionStatus::InvalidOutput` используется, когда external runner завершился, но обязательный report/artifact/log отсутствует, пустой или malformed.
5. `ExecutionStatus::Cancelled` должен быть введён при реализации ADR-0014 и использоваться только когда command действительно завершилась отменой после terminal state underlying operation.
6. Если cancellation/shutdown/timeout был requested в `CriticalNonAbortable` phase, но operation безопасно завершилась success, итоговый status остаётся `Succeeded`, а `ExecutionOutcome` получает diagnostic/warning о deferred interruption.
7. Degraded success не должен маскироваться как чистый success: cleanup/publish/log-read warnings должны быть доступны через diagnostics, errors with non-fatal classification или будущий explicit warning channel.

### Transport boundary

`ExecutionOutcome<T>` не заменяет:

1. CLI `Envelope<T>`;
2. MCP response DTO;
3. `UseCaseFailure<T>`;
4. command-specific top-level result structs.

CLI and MCP adapters map `ExecutionOutcome<T>` into their own presentation contracts.
Use case orchestration не зависит от `Presenter`, CLI `Envelope`, MCP DTO or JSON schema details.

## Область применения

Этот ADR применяется к:

1. `test` / YaXUnit / Vanessa;
2. `artifacts cf/cfe/epf/erf`;
3. `load` artifact scenarios;
4. будущим custom/package/runner-like сценариям, которые запускают внешний runner/tool, сохраняют artifacts/logs или парсят output;
5. новым pipeline-like сценариям, где есть повторяемые блоки prepare/run/parse/publish/cleanup.

Этот ADR не требует немедленно переводить на `ExecutionOutcome`:

1. `build`, где основная модель пока source-set steps and partial/full load decisions;
2. `dump`, где ключевой контракт publication/update mode;
3. `syntax`, где ключевой контракт issue-oriented result;
4. `launch`, `init`, `extensions`, где текущие result shapes остаются достаточными.

Эти команды могут быть мигрированы позднее, если это уменьшит duplication без потери доменной ясности.

## Неграницы (Non-goals)

1. Не вводить немедленно generic pipeline engine на trait objects для всех команд.
2. Не заменять CLI/MCP DTO единой serialized формой.
3. Не ломать существующие CLI/MCP compatibility fields одним изменением.
4. Не превращать `ExecutionOutcome<T>` в transport error model; это остаётся зоной ADR-0009.
5. Не скрывать command-specific domain context ради унификации.
6. Не требовать, чтобы каждый внутренний helper становился отдельным pipeline step.

## Последствия

1. Новые runner-like сценарии получают стандартный result grammar вместо ad hoc DTO.
2. CLI/MCP rendering can become simpler: adapters read the same status/errors/metrics/artifacts model.
3. Partial structured payloads in `UseCaseFailure<T>` can carry the same `ExecutionOutcome<T>` shape.
4. Warning/degraded behavior needs a clearer representation instead of being split randomly between `message`, `warnings`, diagnostics and text output.
5. Step representation should evolve from minimal `StepResult` to richer `ExecutionStep` before adding many new pipeline combinations.
6. `build`, `dump` and `syntax` should not be force-fit until there is a concrete benefit and a compatibility plan.
7. Use case code should become easier to extend by adding or recombining blocks instead of copying complete command implementations.

## План реализации

Текущее состояние кода частично следует этому решению:

1. `src/domain/execution.rs` already defines shared execution primitives.
2. `src/domain/runner.rs` already defines shared runner request primitives.
3. `src/domain/test.rs`, `src/domain/artifacts.rs` and `src/domain/load.rs` already carry `ExecutionOutcome<T>` in some form.
4. `src/use_cases/run_tests.rs`, `src/use_cases/artifacts.rs` and `src/use_cases/load_artifact.rs` already construct outcomes for important paths.

Дальнейшие изменения:

1. Make `ExecutionOutcome<T>` the serialized source of truth for `test` instead of keeping it only as skipped legacy field.
2. Reduce duplication between top-level `ok/message/path` fields and outcome status/errors/artifacts where compatibility allows.
3. Add `ExecutionStatus::Cancelled` and minimal command-level interruption metadata for actual cancellation outcomes and deferred interruption warnings.
4. Introduce richer `ExecutionStep` or extend `StepResult` with:
- kind;
- status beyond boolean `ok`;
- target identity;
- diagnostics/errors;
- artifacts;
- optional reference to command-level interruption/critical phase metadata when it is useful for presentation.
5. Add helper builders for common pipeline blocks in use-case/domain code without introducing a mandatory generic engine.
6. Update CLI JSON and MCP mapping tests so they assert outcome-driven status, errors, metrics and artifact paths.
7. When adding a new runner-like scenario, start from `ScenarioExecutionRequest` and `ExecutionOutcome<T>` instead of a fresh bespoke result shape.
8. Identify duplicate prepare/run/parse/publish/cleanup code in `run_tests`, `artifacts` and `load_artifact`, then extract typed helpers only where the second or third caller proves the boundary.

## Верификация

- [x] ADR fixes `ExecutionOutcome<T>` as canonical outcome for runner-like/pipeline-like scenarios.
- [x] ADR defines reusable pipeline block vocabulary.
- [x] ADR fixes pipeline as a transport-neutral use-case composition model.
- [x] ADR keeps CLI/MCP presentation separate from domain outcome.
- [x] ADR allows legacy compatibility fields during migration.
- [x] ADR explicitly rejects a premature generic pipeline engine.
- [x] ADR links timeout/cancellation terminal semantics to ADR-0014.
- [x] ADR fixes simplified cancellation representation through `ExecutionStatus::Cancelled` plus command-level interruption diagnostics instead of per-step cancellation state machines.
