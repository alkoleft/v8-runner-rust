# ConfigurationRepositoryClearGlobalCache

## Назначение

Очистка глобального кэша версий конфигурации в хранилище.

## Синтаксис

```text
/ConfigurationRepositoryClearGlobalCache [-Extension <имя расширения>]
```

## Параметры

- `-Extension <имя расширения>` — имя расширения. Если параметр не указан, выполняется попытка соединения с хранилищем основной конфигурации, и команда выполняется для основной конфигурации. Если параметр указан, выполняется попытка соединения с хранилищем указанного расширения, и команда выполняется для этого хранилища.
- Пример для конфигурации, не присоединенной к текущему хранилищу: DESIGNER `/F` "D:\V8\Cfgs8\ИБ8" `/ConfigurationRepositoryF` "D:\V8\Cfgs8" `/ConfigurationRepositoryN` "Администратор" `/ConfigurationRepositoryP` xxx `/ConfigurationRepositoryClearGlobalCache`
- Пример для конфигурации, присоединенной к хранилищу конфигурации: DESIGNER `/F` "D:\V8\Cfgs8\ИБ8" `/ConfigurationRepositoryClearGlobalCache`
