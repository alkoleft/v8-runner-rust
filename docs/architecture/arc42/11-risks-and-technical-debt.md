## 11. Риски и технический долг

- Журнал ADR уже введён, но его нужно поддерживать синхронно с кодом и публичной документацией.
- Публичная и внутренняя документация могут расходиться, если их не обновлять вместе с кодом.
- Общий shared interactive EDT-path покрывает только MCP syntax, а CLI пока создаёт non-shared interactive EDT sessions; это implementation gap к ADR-0007 и увеличивает число execution paths.
- Поддержка `IBCMD` остаётся уже, чем поддержка Designer.
- Provisioning contract из ADR-0019 реализован только для `builder=IBCMD`; `builder=DESIGNER` по-прежнему пропускает server infobase create step и это остаётся документированным ограничением.
- Общая timeout/cancellation policy из ADR-0014 является целевой архитектурой и ещё не полностью реализована во всех public commands.
- MCP running cancellation/timeout с detached completion считается переходным механизмом до terminal-state semantics из ADR-0014.
- External artifacts staging cleanup ещё нужно привести к ADR-0015: metadata должен ставиться на cleanup unit, чтобы stale staging directory удалялся безопасно.
- `replace_dir_atomically` использует `.dump-backup-*` prefix даже для artifacts directory publication; имя internal, но префикс лучше сделать neutral/caller-specific.
- `ExecutionOutcome<T>` уже используется частично, но legacy top-level fields и минимальный `StepResult` ещё создают риск расхождения outcome, diagnostics и presentation.
- Система сильно зависит от локальных внешних инструментов и корректности окружения, что ограничивает герметичное тестирование.
- Многошаговые сценарии вроде build по нескольким `source-set` намеренно не являются атомарными.
- Workspace lock является local advisory lock и не защищает от некорректной семантики блокировок на сетевых файловых системах или от команд на разных машинах.
- Переименование `source-set.name` меняет runtime identity и может сбросить persisted state; это нужно явно подсвечивать в документации и release notes.
- Если новые архитектурные границы не фиксировать в ADR и [инвариантах](../invariants.md), AI-агенты или новые контрибьюторы могут переинтерпретировать важные решения как случайные детали реализации.
