# ConfigurationRepositoryCreate

## Назначение

Создает хранилище конфигурации для основной конфигурации или расширения и при необходимости сразу настраивает правила поддержки.

## Синтаксис

```text
/ConfigurationRepositoryCreate [-MinPasswordLength <Число>][-CheckPasswordComplexity]
[-Extension <имя расширения>] [-AllowConfigurationChanges
-ChangesAllowedRule <Правило поддержки> -ChangesNotRecommendedRule <Правило поддержки>] [-NoBind]
```

## Параметры

- `-MinPasswordLength <Число>` — минимальная длина пароля пользователей хранилища.
- `-CheckPasswordComplexity` — включает проверку сложности паролей.
- `-Extension <имя расширения>` — создает хранилище для указанного расширения вместо основной конфигурации.
- `-AllowConfigurationChanges` — включает возможность изменения конфигурации, если она была на поддержке без права изменения.
- `-ChangesAllowedRule <Правило поддержки>` — правило поддержки для объектов, изменения которых разрешены поставщиком.
- `-ChangesNotRecommendedRule <Правило поддержки>` — правило поддержки для объектов, изменения которых не рекомендуются поставщиком.
- `-NoBind` — не подключает созданное хранилище к текущей конфигурации.

## Связи

- `-CheckPasswordComplexity` влияет на проверку паролей, которые затем передаются через `/ConfigurationRepositoryP`.
- Если `-MinPasswordLength <Число>` не задан, нижняя граница для сложного пароля остается стандартной.
- `-Extension <имя расширения>` переключает команду с основной конфигурации на хранилище расширения.
- `-ChangesAllowedRule` и `-ChangesNotRecommendedRule` применимы только вместе с логикой настройки поддержки.

## Примечания

- Для правил поддержки используются значения `ObjectNotEditable`, `ObjectIsEditableSupportEnabled`, `ObjectNotSupported`.
