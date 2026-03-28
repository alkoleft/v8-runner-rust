# Designer Batch Mode Specs

Набор спецификаций собран из локальной документации `1С:Предприятие -> ZIF3` через viewer API на `http://localhost:8080`.

## Основа

- Точка входа: `1cv8 DESIGNER [<параметры запуска>]`
- Основная страница: `ZIF3` — параметры командной строки в пакетном режиме запуска конфигуратора
- Коды возврата: `0` — успех, `1` — ошибка выполнения, `101` — обнаружены ошибки в данных
- Всего команд: `54`
- Всего вспомогательных параметров: `21`
- Машиночитаемый индекс: `manifest.json`

## Команды

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

### Удаление данных

- `EraseData` -> `commands/data-deletion/EraseData.md`

### Распределенная информационная база

- `ResetMasterNode` -> `commands/distributed-infobase/ResetMasterNode.md`

### Команды создания файла поставки и обновления

- `CreateDistributionFiles` -> `commands/distribution/CreateDistributionFiles.md`
- `CreateDistributive` -> `commands/distribution/CreateDistributive.md`
- `CreateDistributivePackage` -> `commands/distribution/CreateDistributivePackage.md`
- `CreateTemplateListFile` -> `commands/distribution/CreateTemplateListFile.md`
- `SignCfg` -> `commands/distribution/SignCfg.md`

### Журнал регистрации

- `ReduceEventLogSize` -> `commands/event-log/ReduceEventLogSize.md`

### Внешние обработки (отчеты)

- `DumpExternalDataProcessorOrReportToFiles` -> `commands/external-data-processors/DumpExternalDataProcessorOrReportToFiles.md`
- `LoadExternalDataProcessorOrReportFromFiles` -> `commands/external-data-processors/LoadExternalDataProcessorOrReportFromFiles.md`

### Восстановление структуры информационной базы

- `IBRestoreIntegrity` -> `commands/infobase-recovery/IBRestoreIntegrity.md`

### Выгрузка и загрузка информационной базы

- `DumpIB` -> `commands/infobase-transfer/DumpIB.md`
- `RestoreIB` -> `commands/infobase-transfer/RestoreIB.md`

### Мобильное приложение

- `MobileAppUpdatePublication` -> `commands/mobile-app/MobileAppUpdatePublication.md`
- `MobileAppWriteFile` -> `commands/mobile-app/MobileAppWriteFile.md`

### Мобильный клиент

- `MobileClientDigiSign` -> `commands/mobile-client/MobileClientDigiSign.md`
- `MobileClientWriteFile` -> `commands/mobile-client/MobileClientWriteFile.md`

### Предопределенные данные

- `SetPredefinedDataUpdate` -> `commands/predefined-data/SetPredefinedDataUpdate.md`

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

### Поддержка конфигурации

- `ManageCfgSupport` -> `commands/support/ManageCfgSupport.md`
- `UpdateCfg` -> `commands/support/UpdateCfg.md`

### Проверки конфигурации и расширений

- `CheckConfig` -> `commands/validation/CheckConfig.md`
- `CheckModules` -> `commands/validation/CheckModules.md`
- `IBCheckAndRepair` -> `commands/validation/IBCheckAndRepair.md`

## Вспомогательные параметры

### Параметры доступа к хранилищу

- `ConfigurationRepositoryF` -> `parameters/repository-access/ConfigurationRepositoryF.md`
- `ConfigurationRepositoryN` -> `parameters/repository-access/ConfigurationRepositoryN.md`
- `ConfigurationRepositoryP` -> `parameters/repository-access/ConfigurationRepositoryP.md`

### Параметры режима агента

- `AgentBaseDir` -> `parameters/agent-mode/AgentBaseDir.md`
- `AgentListenAddress` -> `parameters/agent-mode/AgentListenAddress.md`
- `AgentMode` -> `parameters/agent-mode/AgentMode.md`
- `AgentPort` -> `parameters/agent-mode/AgentPort.md`
- `AgentSSHHostKey` -> `parameters/agent-mode/AgentSSHHostKey.md`
- `AgentSSHHostKeyAuto` -> `parameters/agent-mode/AgentSSHHostKeyAuto.md`

### Прочие batch-параметры

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

## Примечания для реализации

- Команда `CheckCanApplyConfigurationExtensions` присутствует в двух ветках TOC; в спецификации сохранен один канонический файл с ссылкой на дополнительные вхождения.
- Часть элементов внутри `ZIF3` — не самостоятельные команды, а общие параметры пакетного режима (`Out`, `Visible`, `AgentMode`, параметры доступа к хранилищу и т.д.). Они вынесены в раздел `parameters/` для будущего reuse в DSL и валидаторах.
- Машиночитаемые исходные метаданные (`pagePath`, `sourceUrl`, `tocPath`, `syntax`) сохранены в `manifest.json`; markdown-файлы оставлены компактными для чтения.
