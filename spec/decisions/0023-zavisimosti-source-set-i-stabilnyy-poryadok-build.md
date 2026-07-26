# ADR-0023: Ввести зависимости source-set и стабильный порядок build

- Статус: `accepted`
- Дата: `2026-07-26`
- Связанные решения: [ADR-0002](0002-izolirovat-runtime-state-po-source-set-pod-workpath.md), [ADR-0006](0006-sohranyat-transportno-neytralnyy-use-case-sloy.md), [ADR-0010](0010-razdelit-cli-output-dlya-cheloveka-i-ai-agenta.md), [ADR-0012](0012-on-demand-change-detection-i-faylovaya-partial-load-strategiya.md), [ADR-0017](0017-v8project-yaml-source-set-kak-glavnyy-konfiguratsionnyy-kontrakt.md)

## Контекст

Project source-set имели stable identity и canonical order, но не могли выразить обязательный
порядок загрузки. Типичный test project требует цепочку `main -> yaxunit -> TESTS`: расширение
YaXUnit должно загружаться после основной конфигурации, а тестовое расширение — после YaXUnit.
Прежний `build --source-set TESTS` выбирал только `TESTS`, поэтому вызывающей стороне приходилось
знать и вручную воспроизводить prerequisites.

Нужна единая semantics для Designer, IBCMD и EDT export/build paths без нового transport surface и
без изменения structured result schema.

## Решение

Добавить optional `source-set[].dependsOn` со списком имён непосредственных зависимостей.

Validation boundary до platform DSL:

1. имя dependency должно существовать и не совпадать с dependent;
2. один dependent не может повторять dependency;
3. dependency target должен иметь `type=CONFIGURATION` или `type=EXTENSION`;
4. graph должен быть acyclic;
5. при наличии dependency graph каждое `EXTENSION` должно транзитивно разрешаться ровно в один
   `CONFIGURATION`.

Build use case формирует stable topological order. Dependency всегда выполняется раньше dependent;
среди одновременно готовых nodes сохраняется существующий canonical priority и YAML order.
Конфиги без `dependsOn` сохраняют прежний порядок.

`build --source-set <NAME>` выбирает указанный node и всё transitive dependency closure. Каждый
node выполняется не более одного раза. Dependency resolution находится выше backend dispatch,
поэтому Designer, IBCMD и EDT paths получают одну ordered selection.

При failure текущего node остальные selected nodes не вызывают platform DSL и фиксируются в
существующих build steps как `skipped` с причиной `aborted after previous failure`. Уже успешные
шаги не откатываются.

`test yaxunit` и `test va` сохраняют прежний outer workflow: сначала полный `build`, затем test
runner. Полный build теперь dependency-aware; failure prerequisites не допускает запуск runner.

## Совместимость результата

Публичные CLI/MCP result DTO не получают полей requested/expanded source-set или иной graph
metadata. Фактический порядок и skipped nodes остаются видимы через существующий список build
steps. Это сохраняет shared envelope и MCP tool surface без расширения контракта.

## Неграницы

1. Не выводить зависимости из BSL, metadata или файловой структуры.
2. Не применять graph к `tools.client_mcp.extension` и external tool preparation.
3. Не добавлять отдельную CLI/MCP команду для graph inspection.
4. Не делать multi-source-set build атомарным и не откатывать успешные prerequisites.
5. Не менять selection semantics команд, кроме project `build` и вложенного build перед tests.

## Последствия

1. `v8project.yaml` явно документирует runtime prerequisites.
2. Scoped build остаётся узким, но становится корректным: необходимые prerequisites включаются
   автоматически.
3. Backend implementations не дублируют graph traversal.
4. Existing step contract достаточно для диагностики failure/skip без result metadata migration.
5. Config examples, schema, repo-local skill и integration tests должны синхронно отражать graph
   semantics.
