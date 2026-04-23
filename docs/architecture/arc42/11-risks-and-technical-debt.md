## 11. Риски и технический долг

- Журнал ADR уже введён, но его нужно поддерживать синхронно с кодом и публичной документацией.
- Публичная и внутренняя документация могут расходиться, если их не обновлять вместе с кодом.
- Общий shared interactive EDT-path теперь вынесен в `platform`, но остаётся риск регрессии к третьему публичному execution path, если новые EDT-сценарии начнут обходить общий actor/manager или документация/tests перестанут держать инварианты ADR-0007.
- `dump format=EDT` теперь зависит от внутреннего Designer snapshot под `workPath/designer/<source-set>`; новые изменения не должны подменять этот reverse-sync path командой `convert` или обходить staged publication target-каталога.
- Поддержка `IBCMD` остаётся уже, чем поддержка Designer.
- Provisioning contract из ADR-0019 реализован только для `builder=IBCMD`; `builder=DESIGNER` по-прежнему пропускает server infobase create step и это остаётся документированным ограничением.
- Общая timeout/cancellation policy из ADR-0014 является целевой архитектурой и ещё не полностью реализована во всех public commands.
- MCP running cancellation/timeout с detached completion считается переходным механизмом до terminal-state semantics из ADR-0014.
- `ExecutionOutcome<T>` is now canonical for `test`, `artifacts`, and `load` domain results; the remaining risk is future reintroduction of duplicated result fields outside adapter projections.
- Система сильно зависит от локальных внешних инструментов и корректности окружения, что ограничивает герметичное тестирование.
- Многошаговые сценарии вроде build по нескольким `source-set` намеренно не являются атомарными.
- Workspace lock является local advisory lock и не защищает от некорректной семантики блокировок на сетевых файловых системах или от команд на разных машинах.
- Переименование `source-set.name` меняет runtime identity и может сбросить persisted state; это нужно явно подсвечивать в документации и release notes.
- Если новые архитектурные границы не фиксировать в ADR и [инвариантах](../invariants.md), AI-агенты или новые контрибьюторы могут переинтерпретировать важные решения как случайные детали реализации.
