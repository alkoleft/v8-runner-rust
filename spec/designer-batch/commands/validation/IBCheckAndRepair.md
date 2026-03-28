# IBCheckAndRepair

## Назначение

Тестирует и при необходимости исправляет информационную базу, включая таблицы, итоги, расширения и хранилище двоичных данных.

## Синтаксис

```text
/IBCheckAndRepair [-ReIndex][-LogIntegrity [<тип объекта>[,<тип объекта>] ]
[-LogAndRefsIntegrity [<тип объекта>[,<тип объекта>] ]]
[-RecalcTotals] [-IBCompression][-Rebuild][–RebuildStandaloneCfg]
[-TestOnly | [[-BadRefCreate | -BadRefClear |-BadRefNone]
[-BadDataCreate | -BadDataDelete]]] [-UseStartPoint][-TimeLimit:hhh:mm]
[-ConfigurationExtensionsLogIntegrity][-RebuildConfigurationExtensions]
[-RefreshTableLocation]
[-BinaryDataStorageIntegrity [<тип объекта>[,<тип объекта>]]]
[-JobsCount <количество>] [-Z: "<значения разделителей>"]
```

## Параметры

- `-ReIndex` — переиндексирует таблицы.
- `-LogIntegrity [типы]` — проверяет логическую целостность, при необходимости для заданных типов таблиц.
- `-LogAndRefsIntegrity [типы]` — проверяет логическую и ссылочную целостность.
- `-RecalcTotals` — пересчитывает итоги.
- `-IBCompression` — сжимает таблицы.
- `-Rebuild` — реструктурирует таблицы базы.
- `-RebuildStandaloneCfg` — пересоздает конфигурацию для автономного мобильного клиента.
- `-TestOnly` — выполняет только проверку без исправления.
- `-BadRefCreate`, `-BadRefClear`, `-BadRefNone` — стратегия обработки ссылок на отсутствующие объекты.
- `-BadDataCreate`, `-BadDataDelete` — стратегия обработки частично потерянных данных.
- `-UseStartPoint` — продолжает операцию с ранее сохраненной точки.
- `-TimeLimit:hhh:mm` — ограничивает длительность сеанса.
- `-ConfigurationExtensionsLogIntegrity` — проверяет целостность расширений конфигурации.
- `-RebuildConfigurationExtensions` — реструктурирует таблицы расширений.
- `-RefreshTableLocation` — обновляет размещение таблиц.
- `-BinaryDataStorageIntegrity [типы]` — проверяет целостность хранилища двоичных данных.
- `-JobsCount <количество>` — количество фоновых заданий.
- `-Z: "<значения разделителей>"` — ограничивает проверку указанными областями данных.

## Связи

- `-TestOnly` взаимоисключает параметры исправления `-BadRefCreate`, `-BadRefClear`, `-BadRefNone`, `-BadDataCreate`, `-BadDataDelete`.
- Для `-LogIntegrity`, `-LogAndRefsIntegrity` и `-BinaryDataStorageIntegrity` можно указать список типов таблиц через запятую.
- `-JobsCount <количество>` работает только в клиент-серверном варианте.
- Значение по умолчанию для `-JobsCount <количество>`: `0`, то есть количество задач выбирается автоматически.
- `-Z` можно указывать несколько раз для нескольких областей данных.
- Если задан только `-ConfigurationExtensionsLogIntegrity` или `-RebuildConfigurationExtensions`, монопольный доступ к основной базе не требуется.
- При `-ConfigurationExtensionsLogIntegrity` вместе с `-TestOnly` выполняется только проверка.
- Параметры внутри каждой подгруппы выбора поведения взаимоисключают друг друга.

## Примечания

- `-RefreshTableLocation` не поддерживается в файловой базе и в дата-акселераторе.
- Списки типов таблиц используют имена групп метаданных платформы, например `Catalogs`, `Documents`, `InformationRegisters`, `Other`.
