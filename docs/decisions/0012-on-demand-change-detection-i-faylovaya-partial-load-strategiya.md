# ADR-0012: On-demand change detection и файловая partial-load стратегия

- Статус: `accepted`
- Дата: `2026-04-20`

## Контекст

`v8-runner` должен ускорять повторные build/load сценарии, но не имеет права выполнять неполную загрузку, если нет уверенности в корректности набора изменённых файлов.
Система уже хранит per-source-set состояние в `redb`, различает Designer и EDT contexts и принимает partial/full decision перед platform load.

Ключевые риски:

1. background watcher может пропустить изменения, если процесс не был запущен;
2. timestamp-only detection может дать ложные решения на файловых системах с coarse mtime;
3. EDT source of truth и generated Designer output требуют разных snapshots;
4. partial load опасен при удалениях, изменении `Configuration.xml` и небезопасном expansion набора файлов;
5. persisted state нельзя коммитить до успешного platform export/load.

## Решение

Использовать on-demand change detection и консервативную файловую partial-load стратегию.

Правила:

1. Анализ изменений запускается только во время команды, которой нужен build/export/load decision; background watcher не используется.
2. Persistent state хранится под `workPath/hash-storages` в отдельном `redb` context на логический source-set context.
3. Для `format=DESIGNER` используется один context на `source-set`: `designer-<sourceSetName>`.
4. Для `format=EDT` используется два context на `source-set`: `edt-<sourceSetName>` для решения об export и `designer-<sourceSetName>` для решения о load generated Designer output.
5. Scanner использует watermark/mtime filter с coarse margin и проверкой хеша для candidate files.
6. Scanner игнорирует runtime/build каталоги и файлы, которые не должны участвовать в source snapshot, например `.git`, `build`, `target`, `temp`, `tmp`, `.yaxunit`, `ConfigDumpInfo.xml`.
7. Recoverable scan/storage ошибки приводят к safe fallback: full execution или full rescan вместо partial decision.
8. Hard storage ошибки и concurrent generation mismatch surfaced как failures, а не silently ignored.
9. `--full-rebuild` означает bypass текущего анализа и последующий full rescan/commit после успешной platform операции; это не отдельный backend mode.
10. Partial load является file-level стратегией, а не semantic object dependency graph.
11. Partial load запрещён и заменяется full load, если изменён `Configuration.xml`, есть удаления, expansion небезопасен, expanded set пустой или превышает `build.partialLoadThreshold`.
12. Изменения `.bsl` расширяются до sibling XML и object directory, чтобы Designer получил достаточно файлов для безопасной загрузки.
13. Prepared snapshot коммитится только после успешного соответствующего export/load step.

## Неграницы (Non-goals)

1. Не вводить background file watcher.
2. Не строить semantic dependency graph объектов 1С.
3. Не делать incremental state глобальным на весь проект.
4. Не обещать атомарность multi-source-set build.
5. Не пытаться выполнить partial load при удалениях или изменениях, которые требуют full load.

## Последствия

1. Повторные build/load команды могут пропускать неизменённые source-set или выполнять partial load.
2. При сомнениях система должна выбирать full execution, а не потенциально неполную загрузку.
3. `source-set.name` и layout `workPath/hash-storages` являются частью runtime contract.
4. Изменения ignored paths, context naming или partial decision rules являются архитектурно значимыми и требуют обновления этого ADR или нового ADR.
5. EDT flow обязан анализировать и коммитить EDT export context отдельно от Designer load context.

## План реализации

Целевое состояние реализации:

1. `src/change_detection/source_sets.rs` создаёт Designer и EDT contexts.
2. `src/change_detection/scanner.rs` реализует on-demand scan, ignored paths, mtime/hash strategy и coarse margin.
3. `src/change_detection/hash_storage.rs` хранит `redb` snapshots и generation metadata.
4. `src/change_detection/analyzer.rs` разделяет concrete changes, no changes, fallback и hard failures.
5. `src/change_detection/partial_load.rs` принимает `Partial`/`Full` decision и пишет list files.
6. `src/use_cases/build_project.rs` применяет analysis перед platform operations и коммитит snapshots после успешного шага.
7. Для `format=EDT` build pipeline разделён на независимые последовательные стадии:
   - `edt-*` analysis управляет только export decision;
   - successful export коммитит `edt-*` snapshot;
   - `designer-*` analysis запускается всегда после успешной или skipped EDT stage;
   - Designer/IBCMD load/apply запускается только при изменениях в generated Designer output;
   - `designer-*` snapshot коммитится только после successful load/apply;
   - ошибка на предыдущей стадии останавливает pipeline до следующей стадии.

При дальнейших изменениях:

1. новые build/export flows должны использовать `ChangeAnalysis` вместо самостоятельного обхода файлов;
2. новые partial load rules должны покрываться unit tests в `change_detection::partial_load`;
3. любые изменения context naming или storage layout должны обновлять ADR-0002 и этот ADR;
4. failure handling должен сохранять safe fallback semantics для recoverable ошибок.

## Верификация

- [x] ADR фиксирует on-demand, а не watcher-based change detection.
- [x] ADR фиксирует per-context `redb` storage под `workPath/hash-storages`.
- [x] ADR фиксирует разные contexts для EDT source и generated Designer output.
- [x] ADR фиксирует conservative full-load fallback для unsafe partial cases.
- [x] ADR фиксирует commit snapshot только после successful platform step.
- [ ] EDT build выполняет Designer analysis всегда после successful или skipped EDT stage.
- [ ] Отсутствие изменений в generated Designer output приводит к skip без load/apply.
- [ ] Изменения в generated Designer output проходят через обычное partial/full decision.
- [ ] `designer-*` snapshot не коммитится до successful load/apply.
