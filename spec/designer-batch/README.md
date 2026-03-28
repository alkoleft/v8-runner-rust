# Designer Batch Specs

Краткий индекс batch-спецификаций `1cv8 DESIGNER`, приведенных к компактному формату для AI-агентов.

## Базовый контракт

- Точка входа: `1cv8 DESIGNER [<параметры запуска>]`.
- Коды возврата: `0` — успех, `1` — ошибка выполнения, `101` — ошибки в данных.
- Машиночитаемый индекс: `manifest.json`.
- В `commands/` лежат отдельные batch-команды.
- В `parameters/` лежат общие параметры, которые переиспользуются между командами.

## Команды

### Внешние обработки (отчеты)

- `DumpExternalDataProcessorOrReportToFiles` -> `commands/external-data-processors/DumpExternalDataProcessorOrReportToFiles.md`
- `LoadExternalDataProcessorOrReportFromFiles` -> `commands/external-data-processors/LoadExternalDataProcessorOrReportFromFiles.md`

### Восстановление структуры информационной базы

- `IBRestoreIntegrity` -> `commands/infobase-recovery/IBRestoreIntegrity.md`

### Выгрузка и загрузка информационной базы

- `DumpIB` -> `commands/infobase-transfer/DumpIB.md`
- `RestoreIB` -> `commands/infobase-transfer/RestoreIB.md`

### Журнал регистрации

- `ReduceEventLogSize` -> `commands/event-log/ReduceEventLogSize.md`

### Команды работы с хранилищем конфигурации

- `ConfigurationRepositoryAddUser` -> `commands/repository/ConfigurationRepositoryAddUser.md`
- `ConfigurationRepositoryBindCfg` -> `commands/repository/ConfigurationRepositoryBindCfg.md`
- `ConfigurationRepositoryClearCache` -> `commands/repository/ConfigurationRepositoryClearCache.md`
- `ConfigurationRepositoryClearGlobalCache` -> `commands/repository/ConfigurationRepositoryClearGlobalCache.md`
- `ConfigurationRepositoryClearLocalCache` -> `commands/repository/ConfigurationRepositoryClearLocalCache.md`
- `ConfigurationRepositoryCommit` -> `commands/repository/ConfigurationRepositoryCommit.md`
- `ConfigurationRepositoryCopyUsers` -> `commands/repository/ConfigurationRepositoryCopyUsers.md`
- `ConfigurationRepositoryCreate` -> `commands/repository/ConfigurationRepositoryCreate.md`
- `ConfigurationRepositoryDumpCfg` -> `commands/repository/ConfigurationRepositoryDumpCfg.md`
- `ConfigurationRepositoryLock` -> `commands/repository/ConfigurationRepositoryLock.md`
- `ConfigurationRepositoryOptimizeData` -> `commands/repository/ConfigurationRepositoryOptimizeData.md`
- `ConfigurationRepositoryReport` -> `commands/repository/ConfigurationRepositoryReport.md`
- `ConfigurationRepositorySetLabel` -> `commands/repository/ConfigurationRepositorySetLabel.md`
- `ConfigurationRepositoryUnbindCfg` -> `commands/repository/ConfigurationRepositoryUnbindCfg.md`
- `ConfigurationRepositoryUnlock` -> `commands/repository/ConfigurationRepositoryUnlock.md`
- `ConfigurationRepositoryUpdateCfg` -> `commands/repository/ConfigurationRepositoryUpdateCfg.md`

### Команды создания файла поставки и обновления

- `CreateDistributionFiles` -> `commands/distribution/CreateDistributionFiles.md`
- `CreateDistributive` -> `commands/distribution/CreateDistributive.md`
- `CreateDistributivePackage` -> `commands/distribution/CreateDistributivePackage.md`
- `CreateTemplateListFile` -> `commands/distribution/CreateTemplateListFile.md`
- `SignCfg` -> `commands/distribution/SignCfg.md`

### Конфигурация и расширения

- `CheckCanApplyConfigurationExtensions` -> `commands/config-and-extensions/CheckCanApplyConfigurationExtensions.md`
- `CompareCfg` -> `commands/config-and-extensions/CompareCfg.md`
- `DeleteCfg` -> `commands/config-and-extensions/DeleteCfg.md`
- `DumpCfg` -> `commands/config-and-extensions/DumpCfg.md`
- `DumpConfigFiles` -> `commands/config-and-extensions/DumpConfigFiles.md`
- `DumpConfigToFiles` -> `commands/config-and-extensions/DumpConfigToFiles.md`
- `DumpDBCfg` -> `commands/config-and-extensions/DumpDBCfg.md`
- `DumpDBCfgList` -> `commands/config-and-extensions/DumpDBCfgList.md`
- `GetConfigGenerationID` -> `commands/config-and-extensions/GetConfigGenerationID.md`
- `LoadCfg` -> `commands/config-and-extensions/LoadCfg.md`
- `LoadConfigFiles` -> `commands/config-and-extensions/LoadConfigFiles.md`
- `LoadConfigFromFiles` -> `commands/config-and-extensions/LoadConfigFromFiles.md`
- `MergeCfg` -> `commands/config-and-extensions/MergeCfg.md`
- `RollbackCfg` -> `commands/config-and-extensions/RollbackCfg.md`
- `UpdateDBCfg` -> `commands/config-and-extensions/UpdateDBCfg.md`

### Мобильное приложение

- `MobileAppUpdatePublication` -> `commands/mobile-app/MobileAppUpdatePublication.md`
- `MobileAppWriteFile` -> `commands/mobile-app/MobileAppWriteFile.md`

### Мобильный клиент

- `MobileClientDigiSign` -> `commands/mobile-client/MobileClientDigiSign.md`
- `MobileClientWriteFile` -> `commands/mobile-client/MobileClientWriteFile.md`

### Поддержка конфигурации

- `ManageCfgSupport` -> `commands/support/ManageCfgSupport.md`
- `UpdateCfg` -> `commands/support/UpdateCfg.md`

### Предопределенные данные

- `SetPredefinedDataUpdate` -> `commands/predefined-data/SetPredefinedDataUpdate.md`

### Проверки конфигурации и расширений

- `CheckConfig` -> `commands/validation/CheckConfig.md`
- `CheckModules` -> `commands/validation/CheckModules.md`
- `IBCheckAndRepair` -> `commands/validation/IBCheckAndRepair.md`

### Распределенная информационная база

- `ResetMasterNode` -> `commands/distributed-infobase/ResetMasterNode.md`

### Удаление данных

- `EraseData` -> `commands/data-deletion/EraseData.md`

## Общие параметры

### Команды работы в режиме агента

- `AgentBaseDir` -> `parameters/agent-mode/AgentBaseDir.md`
- `AgentListenAddress` -> `parameters/agent-mode/AgentListenAddress.md`
- `AgentMode` -> `parameters/agent-mode/AgentMode.md`
- `AgentPort` -> `parameters/agent-mode/AgentPort.md`
- `AgentSSHHostKey` -> `parameters/agent-mode/AgentSSHHostKey.md`
- `AgentSSHHostKeyAuto` -> `parameters/agent-mode/AgentSSHHostKeyAuto.md`

### Команды работы с хранилищем конфигурации

- `ConfigurationRepositoryF` -> `parameters/repository-access/ConfigurationRepositoryF.md`
- `ConfigurationRepositoryN` -> `parameters/repository-access/ConfigurationRepositoryN.md`
- `ConfigurationRepositoryP` -> `parameters/repository-access/ConfigurationRepositoryP.md`

### Прочие параметры

- `/Visible` -> `parameters/misc/Visible.md`
- `@` -> `parameters/misc/at.md`
- `ConvertFiles` -> `parameters/misc/ConvertFiles.md`
- `DisableHomePageForms` -> `parameters/misc/DisableHomePageForms.md`
- `DisableStartupDialogs` -> `parameters/misc/DisableStartupDialogs.md`
- `DisableStartupMessages` -> `parameters/misc/DisableStartupMessages.md`
- `DisableUnrecoverableErrorMessage` -> `parameters/misc/DisableUnrecoverableErrorMessage.md`
- `DisplayUserNotificationList` -> `parameters/misc/DisplayUserNotificationList.md`
- `DumpResult` -> `parameters/misc/DumpResult.md`
- `Out` -> `parameters/misc/Out.md`
- `RunEnterprise` -> `parameters/misc/RunEnterprise.md`
- `UseHwLicenses` -> `parameters/misc/UseHwLicenses.md`

## Правила чтения

- В каждом файле сохранены только назначение, синтаксис, параметры, связи и важные ограничения.
- Секция `Связи` фиксирует зависимости между флагами, конфликтующие опции и параметры по умолчанию.
- Исходные метаданные источника (`pagePath`, `sourceUrl`, `tocPath`, `syntax`) остаются в `manifest.json`.
