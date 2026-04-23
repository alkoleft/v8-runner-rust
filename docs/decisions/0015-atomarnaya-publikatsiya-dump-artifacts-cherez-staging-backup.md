# ADR-0015: Атомарная публикация dump/artifacts через staging/backup

- Статус: `accepted`
- Дата: `2026-04-21`

## Контекст

Команды `dump` и `artifacts` публикуют результат не только во внутренний `workPath`, но и в пользовательский target path:

1. Designer/IBCMD full dump пишет Designer-format каталог source-set;
2. `artifacts cf/cfe` пишет одиночный release artifact;
3. `artifacts epf/erf` пишет набор внешних обработок или отчётов в output directory.

Если эти операции пишут прямо в target, сбой платформы, cancellation, ошибка publish или падение процесса могут оставить target в смешанном состоянии:

1. старый dump уже удалён, новый dump создан частично;
2. output directory содержит часть старых и часть новых external artifacts;
3. `.cf`/`.cfe` файл перезаписан битым или неполным payload;
4. AI-агент или CI воспринимает частично опубликованный target как успешный результат.

Workspace lock из ADR-0011 сериализует команды, но не защищает target от сбоя единственной команды.
Timeout/cancellation policy из ADR-0014 задаёт terminal-state semantics, но не описывает файловую публикацию пользовательских результатов.

## Решение

Для операций, которые полностью заменяют target, использовать staging/backup publication contract.

Правила:

1. Full replacement операция сначала пишет результат в staging path рядом с target.
2. Staging path и target должны находиться в одном parent directory, чтобы publish выполнялся через filesystem rename без crossing filesystem boundary.
3. До успешного platform step старый target не изменяется.
4. Перед publish target re-canonicalized; если resolved path изменился, publish запрещается.
5. Если target уже существует, publish переносит target в backup, затем переносит staging в target.
6. Если publish staging -> target не удался, runner пытается вернуть backup на target.
7. Если rollback тоже не удался, ошибка должна явно содержать rollback context, чтобы человек или агент понял, что нужна ручная проверка target.
8. После успешного publish backup удаляется best-effort; ошибка cleanup становится warning/degraded message, а не делает успешный publish failed.
9. Staging/backup paths должны иметь metadata sidecar с `tool`, `kind`, `run_id`, `target_path`, `target_identity`, `created_at`.
10. Orphan cleanup может удалять только stale staging/backup paths с metadata `tool=v8-runner` и matching `target_identity`.
11. Orphan cleanup не должен удалять malformed, foreign или recent temp paths.
12. Publication phase после перемещения target в backup является filesystem critical phase; cancellation/timeout не должны hard-kill этот участок по умолчанию согласно ADR-0014.

Операции, покрытые этим контрактом:

1. `dump --mode full` для `builder=DESIGNER`;
2. `dump --mode full` для `builder=IBCMD`;
3. `artifacts cf`;
4. `artifacts cfe`;
5. `artifacts epf/erf` при публикации output directory.

## Неграницы (Non-goals)

1. Не обещать атомарность `dump --mode incremental`.
2. Не обещать атомарность `dump --mode partial`.
3. Не гарантировать crash-consistency на всех файловых системах, где `rename`/`fsync` не дают нужной семантики.
4. Не вводить distributed lock для target paths.
5. Не заменять workspace lock из ADR-0011.
6. Не делать имена staging/backup публичным API; публичным является safety contract, а не точный prefix.
7. Не сохранять backup после успешного publish как user-facing rollback feature.

## Последствия

1. Full dump и artifacts publication не должны использовать `remove_dir_all(target)` перед platform export.
2. Platform failure до publish сохраняет старый target.
3. Publish failure должен пытаться восстановить старый target из backup.
4. Успешная публикация может вернуть cleanup warning, если backup или metadata не удалось удалить.
5. Инкрементальные и частичные dump-режимы остаются отдельными non-atomic update modes и должны быть описаны как такие.
6. Изменения helper-ов `replace_dir_atomically`, `replace_file_atomically`, orphan cleanup или target validation требуют обновления этого ADR.

## План реализации

Текущее состояние кода в основном следует этому решению:

1. `src/support/fs.rs` содержит `replace_dir_atomically` и `replace_file_atomically`.
2. `src/support/fs.rs` содержит `TempDirMetadata`, `TempDirKind`, metadata sidecars и best-effort parent fsync.
3. `src/use_cases/dump_config.rs` выполняет full Designer dump через `.dump-stage-*` и `replace_dir_atomically`.
4. `src/use_cases/dump_config.rs` выполняет full IBCMD dump через staging directory и `replace_dir_atomically`.
5. `src/use_cases/artifacts.rs` выполняет CF/CFE export через staging file и `replace_file_atomically`.
6. `src/use_cases/artifacts.rs` выполняет EPF/ERF publication через staging directory и `replace_dir_atomically`.
7. `src/use_cases/dump_config.rs` и `src/use_cases/artifacts.rs` используют target-specific advisory locks and target identity for stale cleanup.

Resolved follow-up к `2026-04-23`:

1. `replace_dir_atomically` принимает caller-specific backup prefix; `dump` и `artifacts`
   используют разные internal prefixes.
2. External artifacts staging directory получает metadata sidecar на cleanup unit, а staged files
   сохраняют собственные metadata для diagnostic/cleanup сценариев.
3. Full-replacement publication теперь проходит через общий `use_cases::staged_publication`
   helper, который запускает file/directory publish внутри `run_no_process_critical_phase`.
4. Cleanup warning/deferred interruption остаются в `DumpResult`/`ArtifactsResult` без изменения
   публичных result contracts.

При дальнейших изменениях:

1. новые full-replacement export/publish сценарии должны использовать общий staging/backup helper;
2. direct write в target разрешён только для явно non-atomic incremental/partial update modes;
3. tests должны проверять сохранение старого target при platform failure и rollback при publish failure;
4. orphan cleanup tests должны проверять metadata matching, TTL, foreign/malformed metadata и recent paths.

## Верификация

- [x] ADR фиксирует staging/backup publication для full replacement dump/artifacts.
- [x] ADR не обещает атомарность incremental/partial dump.
- [x] ADR фиксирует rollback semantics при publish failure.
- [x] ADR фиксирует cleanup warning как degraded success, а не failed publish.
- [x] ADR связывает publication phase с critical filesystem mutation из ADR-0014.
- [x] ADR фиксирует metadata-based orphan cleanup.
