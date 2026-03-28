# CompareCfg

## Назначение

Строит отчет сравнения между двумя конфигурациями, расширениями, версиями хранилища или файлами.

## Синтаксис

```text
/CompareCfg -FirstConfigurationType [-FirstName] [-FirstFile] [-FirstVersion]
-SecondConfigurationType [-SecondName] [-SecondFile] [-SecondVersion] [-MappingRule]
[-Objects] -ReportType [-IncludeChangedObjects] [-IncludeDeletedObjects] [-IncludeAddedObjects] -ReportFormat -ReportFile
```

## Параметры

- `-FirstConfigurationType`, `-SecondConfigurationType` — типы сравниваемых источников.
- `-FirstName`, `-SecondName` — имя конфигурации или расширения для соответствующего источника.
- `-FirstFile`, `-SecondFile` — путь к файлу, если тип источника равен `File`.
- `-FirstVersion`, `-SecondVersion` — версия хранилища для типов `ConfigurationRepository` и `ExtensionConfigurationRepository`.
- `-MappingRule` — правило сопоставления объектов.
- `-Objects` — XML-файл со списком объектов для выборочного сравнения.
- `-ReportType` — тип отчета: `Brief` или `Full`.
- `-IncludeChangedObjects`, `-IncludeDeletedObjects`, `-IncludeAddedObjects` — включают подчиненные измененные, удаленные и добавленные объекты.
- `-ReportFormat` — формат результата: `txt` или `mxl`.
- `-ReportFile` — путь к результирующему файлу.

## Связи

- Для `-FirstConfigurationType` и `-SecondConfigurationType` поддерживаются значения `MainConfiguration`, `DBConfiguration`, `VendorConfiguration`, `ExtensionConfiguration`, `ExtensionDBConfiguration`, `ConfigurationRepository`, `ExtensionConfigurationRepository`, `File`.
- `-FirstName` и `-SecondName` используются только для типов, где требуется имя поставщика или расширения.
- `-FirstFile` и `-SecondFile` используются только вместе с типом `File`.
- `-FirstVersion` и `-SecondVersion` используются только для источников из хранилища.
- Если `-MappingRule` не указан, используется `ByObjectNames`.
- Если `-Objects` не указан, сравнивается вся конфигурация.

## Примечания

- `ByObjectNames` сопоставляет объекты по именам.
- `ByObjectIDs` сопоставляет объекты по идентификаторам.
