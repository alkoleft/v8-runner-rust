# ADR-0017: `v8project.yaml` / `source-set` как главный конфигурационный контракт

- Статус: `accepted`
- Дата: `2026-04-20`

## Контекст

`v8-runner` получает проектный контекст из `v8project.yaml`.
Из этой конфигурации строятся пути, runtime state, platform backend selection, source-set orchestration, MCP guardrails и test/build behavior.

Кодовая модель уже задаёт typed contract через `AppConfig`, `SourceSetConfig`, `SourceSetPurpose`, `BuildConfig`, `ToolsConfig`, `McpConfig` и связанные validators.
Если считать YAML свободной схемой или поддерживать устаревшие ключи без явного решения, появляются риски:

1. разные документы начнут описывать разные варианты одного config contract;
2. `source-set.name` перестанет быть стабильным ключом для runtime state;
3. EDT generated output может пересечься с пользовательскими исходниками;
4. external artifacts будут вызваны через backend, который их не поддерживает;
5. platform ошибки будут возникать позже, хотя комбинацию можно отклонить на config validation boundary.

## Решение

Считать `v8project.yaml`, загруженный в `AppConfig` и прошедший `config::validate`, главным конфигурационным контрактом проекта.

Правила:

1. `v8project.yaml` является primary project config; альтернативные config entrypoints должны явно маппиться в `AppConfig` или иметь отдельный ADR.
2. Список `source-set` является обязательной частью supported project config.
3. Для `source-set` используется ключ `type`, а не legacy `purpose`.
4. Поддержанные значения `source-set[].type`: `CONFIGURATION`, `EXTENSION`, `EXTERNAL_DATA_PROCESSORS`, `EXTERNAL_REPORTS`.
5. `source-set.name` является stable identity для ordering, diagnostics, runtime contexts, generated directories и extension/source-set selection.
6. `source-set.name` должен быть уникальным и безопасным path segment.
7. Resolved `source-set.path` должны быть уникальны после normalization.
8. Для `format=EDT` имена `source-set` не должны конфликтовать с reserved work dirs: `hash-storages`, `logs`, `temp`, `edt-workspace`, `designer`.
9. EDT/external source-set paths должны существовать и соответствовать ожидаемому layout.
10. External source sets требуют `builder=DESIGNER`.
11. External EDT source-set должен содержать хотя бы один child project с `.project`.
12. `EXTENSION` source-set требует наличия хотя бы одного `CONFIGURATION` source-set.
13. EDT source-set path не должен пересекаться с generated work target под `workPath/designer/<sourceSetName>`.
14. `workPath` является owned runtime root для logs, temp files, hash storages, EDT workspace и generated Designer output; его нельзя трактовать как произвольный scratch без контракта.
15. Контракт подключения информационной базы описан в [ADR-0018](0018-perenesti-kontrakt-informatsionnoy-bazy-v-infobase.md): `infobase.connection` заменяет top-level `connection`, а `infobase.user/password` заменяют top-level `credentials`.
16. Config validation должна отклонять неподдерживаемые или unsafe combinations до вызова platform DSL.

## Неграницы (Non-goals)

1. Не поддерживать legacy `source-set[].purpose` как публичный контракт без отдельного решения.
2. Не выполнять автоматическую миграцию произвольных старых YAML-схем.
3. Не валидировать полную 1С-семантику конфигурации, которую может проверить только платформа.
4. Не превращать `v8project.yaml` в transport-specific contract для CLI или MCP.
5. Не разрешать пользовательским source paths пересекаться с generated runtime directories.

## Последствия

1. Документация, примеры и tests должны использовать `source-set[].type`.
2. Переименование `source-set.name` является изменением runtime identity и может сбросить/change persisted state.
3. Изменение supported source-set types, path safety rules или reserved names требует обновления этого ADR или нового ADR.
4. Новые сценарии должны добавлять config fields в typed model и validation boundary, а не читать ad-hoc YAML ниже по стеку.
5. Ошибки unsupported config combinations должны быть user-facing validation errors, а не поздние platform failures.
6. Изменение структуры `infobase` требует обновления ADR-0018 или нового ADR.

## План реализации

Текущее состояние кода уже следует этому решению:

1. `src/config/model.rs` описывает `AppConfig`, `SourceSetConfig`, `SourceSetPurpose`, `BuildConfig`, `ToolsConfig` и `McpConfig`.
2. `src/config/loader.rs` загружает YAML в typed model и поддерживает текущие имена ключей.
3. `src/config/validate.rs` проверяет source-set presence, path uniqueness, name safety, EDT/external layout, builder compatibility и reserved names.
4. `src/change_detection/source_sets.rs`, `src/support/temp.rs` и build/dump/artifacts use cases используют `source-set.name` как identity для paths и diagnostics.
5. `src/use_cases/config_init.rs` генерирует starter config с `source-set[].type`.

При дальнейших изменениях:

1. новые config fields должны иметь typed model, defaults и validation tests;
2. примеры в `README.md`, `docs/CAPABILITIES.md`, `docs/DEEP_DIVE.md` и generated config должны оставаться синхронизированными;
3. новые source-set types должны обновлять validation, runtime selection, docs и tests;
4. изменения naming/path rules должны обновлять ADR-0002, ADR-0012 и этот ADR.

## Верификация

- [x] ADR фиксирует `v8project.yaml` / `AppConfig` как главный config contract.
- [x] ADR фиксирует `source-set[].type` и supported source-set types.
- [x] ADR фиксирует `source-set.name` как stable runtime identity.
- [x] ADR фиксирует early validation для unsafe или unsupported combinations.
- [x] ADR фиксирует `workPath` как owned runtime root.
