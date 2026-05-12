# ADR-0021: Ввести локальный overlay для `v8project.yaml`

- Статус: `accepted`
- Дата: `2026-05-02`
- Связанные решения: [ADR-0011](0011-eksklyuzivnoe-vladenie-workpath-na-vremya-komandy.md), [ADR-0017](0017-v8project-yaml-source-set-kak-glavnyy-konfiguratsionnyy-kontrakt.md), [ADR-0018](0018-perenesti-kontrakt-informatsionnoy-bazy-v-infobase.md)

## Контекст

`v8-runner` должен сохранять короткий и предсказуемый запуск:

```bash
v8-runner build
v8-runner test va
v8-runner launch mcp va
```

Но локальные рабочие места отличаются путями к платформе 1С, EDT CLI, Vanessa Automation,
рабочей ИБ, credentials и `workPath`.
Передавать это флагами в каждом запуске неудобно и повышает вероятность ошибки.

`v8project.yaml` уже является primary project config по ADR-0017.
Нужно оставить его источником проектной структуры, но дать пользователю локальный слой настроек,
который применяется автоматически.

## Решение

Ввести optional local overlay `v8project.local.yaml`, расположенный рядом с основным
`v8project.yaml`.

Loader строит итоговую конфигурацию так:

1. загрузить основной `v8project.yaml`;
2. если рядом существует `v8project.local.yaml`, применить его как local overlay;
3. применить CLI overrides, например `--workdir`;
4. десериализовать итоговый YAML в typed model;
5. нормализовать paths и выполнить `config::validate`.

`v8project.local.yaml` не является самостоятельным project config.
Он существует только как overlay поверх выбранного основного config.

### Merge rules

1. `map/object` значения merge-ятся рекурсивно.
2. Scalar значения из local overlay заменяют project значение.
3. List значения заменяются целиком.
4. `null` разрешён только для optional fields и означает явный сброс значения.
5. Относительные пути из local overlay резолвятся относительно каталога основного `v8project.yaml`.
6. Внутренний project base path считается равным каталогу основного `v8project.yaml`; YAML-ключ `basePath` не является public contract.

### Supported local overlay scope

Local overlay предназначен для machine-local и user-local настроек:

1. `workPath`;
2. `infobase.*`, включая credentials;
3. `tools.*`;
4. `tests.*`;
5. `mcp.*`.

Local overlay не должен менять project identity:

1. `source-set` запрещён в `v8project.local.yaml`;
2. `format` запрещён в `v8project.local.yaml`;
3. `builder` запрещён в `v8project.local.yaml`.

## Неграницы (Non-goals)

1. Не вводить env flag `V8TR_NO_LOCAL_CONFIG` как часть начального контракта.
2. Не поддерживать chain из нескольких overlay файлов.
3. Не разрешать local overlay менять `source-set`, `format` или `builder`.
4. Не превращать `v8project.local.yaml` в альтернативный entrypoint для `--config`.

## Последствия

1. Типовой запуск остаётся коротким и не требует локальных флагов.
2. Общий `v8project.yaml` остаётся project truth, а локальные пути и credentials уходят в gitignored `v8project.local.yaml`.
3. `basePath` удалён из public config surface; внутренний `AppConfig.base_path` по умолчанию совпадает с каталогом основного config.
4. Реализация должна синхронизировать typed config model, loader, validation, docs, examples and tests.

## План реализации

1. `src/config/model.rs`:
   - удалить `basePath` с YAML boundary;
   - сохранить итоговый `AppConfig.base_path` как resolved `PathBuf`.
2. `src/config/loader.rs`:
   - читать `v8project.local.yaml` рядом с primary config, если файл существует;
   - merge-ить YAML до typed deserialization;
   - отклонять local overlay keys `source-set`, `format`, `builder`;
   - резолвить overlay paths относительно primary config directory.
3. `docs/CONFIGURATION.md`, examples:
   - описать local overlay и project root от каталога primary config;
   - указать, что `v8project.local.yaml` должен быть gitignored.
4. Tests:
   - покрыть automatic overlay discovery;
   - overlay merge rules;
   - forbidden local keys;
   - default внутреннего `AppConfig.base_path`;
   - precedence `project -> local -> CLI override`.

## Верификация

- [x] `v8-runner build` автоматически применяет `v8project.local.yaml` без дополнительных CLI-флагов.
- [x] `source-set`, `format` и `builder` в `v8project.local.yaml` отклоняются как unsupported local overlay keys.
- [x] Внутренний `AppConfig.base_path` резолвится в каталог основного `v8project.yaml`.
- [x] `--workdir` остаётся сильнее local overlay.
