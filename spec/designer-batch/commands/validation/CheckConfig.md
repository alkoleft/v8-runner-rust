# CheckConfig

## Назначение

Выполняет централизованную проверку конфигурации и расширений: целостность, ссылки, режимы исполнения модулей и специальные диагностические проверки.

## Синтаксис

```text
/CheckConfig [-ConfigLogIntegrity] [-IncorrectReferences] [-ThinClient]
[-WebClient] [-MobileClient] [-Server] [-ExternalConnection] [-ExternalConnectionServer]
[-MobileAppClient][-MobileAppServer] [-ThickClientManagedApplication]
[-ThickClientServerManagedApplication] [-ThickClientOrdinaryApplication]
[-ThickClientServerOrdinaryApplication] [-MobileClientDigiSign] [-DistributiveModules]
[-UnreferenceProcedures] [-HandlersExistence] [-EmptyHandlers]
[-ExtendedModulesCheck] [-CheckUseSynchronousCalls] [-CheckUseModality] [-UnsupportedFunctional]
[-Extension <имя расширения>] [-AllExtensions]
```

## Параметры

- `-ConfigLogIntegrity` — проверяет логическую целостность конфигурации.
- `-IncorrectReferences` — ищет некорректные и удаленные ссылки.
- `-ThinClient`, `-WebClient`, `-Server`, `-ExternalConnection`, `-ExternalConnectionServer` — синтаксический контроль модулей в соответствующих режимах.
- `-MobileAppClient`, `-MobileAppServer`, `-MobileClient` — проверка для мобильных режимов исполнения.
- `-ThickClientManagedApplication`, `-ThickClientServerManagedApplication`, `-ThickClientOrdinaryApplication`, `-ThickClientServerOrdinaryApplication` — проверка для толстого клиента.
- `-MobileClientDigiSign` — проверяет корректность подписи мобильного клиента.
- `-DistributiveModules` — проверяет возможность поставки модулей без исходного текста.
- `-UnreferenceProcedures` — ищет неиспользуемые локальные процедуры и функции.
- `-HandlersExistence` — проверяет существование назначенных обработчиков.
- `-EmptyHandlers` — ищет пустые обработчики.
- `-ExtendedModulesCheck` — включает расширенную проверку обращений через точку и строковых литералов.
- `-CheckUseSynchronousCalls` — ищет синхронные вызовы в модулях.
- `-CheckUseModality` — ищет использование модальности.
- `-UnsupportedFunctional` — ищет функциональность, не поддерживаемую мобильной платформой.
- `-Extension <имя расширения>` — проверяет только указанное расширение.
- `-AllExtensions` — проверяет все расширения.

## Связи

- `-CheckUseSynchronousCalls` работает только вместе с `-ExtendedModulesCheck`.
- `-CheckUseModality` работает только вместе с `-ExtendedModulesCheck`.
- `-Extension <имя расширения>` ограничивает проверку одним расширением.
- `-AllExtensions` расширяет проверку на все расширения.

## Примечания

- `-UnsupportedFunctional` ориентирован на диагностику мобильных ограничений: неподдерживаемые метаданные, типы, формы, элементы управления и сложный состав рабочего стола.
- `-Extension <имя расширения>` возвращает `0` при успехе и `1`, если расширение не найдено или операция завершилась ошибкой.
