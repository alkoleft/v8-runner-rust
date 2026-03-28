# UpdateDBCfg

## Назначение

Обновляет конфигурацию базы данных или указанное расширение. Поддерживает динамическое обновление, фоновые этапы и выполнение на сервере.

## Синтаксис

```text
/UpdateDBCfg [–Dynamic<Режим>] [-BackgroundStart] [-BackgroundCancel]
[-BackgroundFinish [-Visible]] [-BackgroundSuspend] [-BackgroundResume]
[-WarningsAsErrors] [-Server [-v1|-v2]][-Extension <имя расширения>]
[-SessionTerminate <Режим>]
```

## Параметры

- `-Dynamic+` — сначала пробует динамическое обновление; при неудаче переходит к фоновому обновлению.
- `-Dynamic-` — запрещает динамическое обновление.
- `-BackgroundStart` — запускает фоновое обновление и завершает текущий сеанс.
- `-BackgroundCancel` — отменяет фоновое обновление.
- `-BackgroundFinish` — переводит фоновое обновление в финальную фазу с монопольной блокировкой.
- `-Visible` — показывает диалог управления завершением фонового обновления.
- `-BackgroundSuspend` — ставит фоновое обновление на паузу.
- `-BackgroundResume` — продолжает ранее приостановленное фоновое обновление.
- `-WarningsAsErrors` — трактует предупреждения как ошибки.
- `-Server` — выполняет обновление на сервере.
- `-v1`, `-v2` — выбирают версию механизма реструктуризации.
- `-Extension <имя расширения>` — обновляет только указанное расширение.
- `-SessionTerminate disable` — не завершает активные сеансы.
- `-SessionTerminate force` — принудительно завершает активные сеансы, если это нужно для эксклюзивной блокировки.

## Связи

- Значение по умолчанию для режима динамического обновления: `-Dynamic+`.
- `-BackgroundFinish` можно использовать вместе с `-Visible`.
- При `-Server` фаза актуализации всегда выполняется на сервере.
- При `-Server` параметр `-v2` игнорируется.
- Если `-v1` или `-v2` не указаны, версия механизма берется из `conf.cfg`.
- Если `-v2` конфликтует с остальными параметрами, платформа откатывается к `-v1`.
- Значение по умолчанию для `-SessionTerminate`: `disable`.
- `/UpdateDBCfg` можно ставить после `/LoadCfg`, `/UpdateCfg`, `/ConfigurationRepositoryUpdateCfg`, `/LoadConfigFiles`, `/LoadConfigFromFiles`, `/MobileAppUpdatePublication`, `/MobileAppWriteFile`, `/MobileClientWriteFile`, `/MobileClientDigiSign`.

## Примечания

- `-BackgroundStart` завершится ошибкой, если фоновое обновление уже идет.
- `-BackgroundCancel`, `-BackgroundSuspend`, `-BackgroundResume` и `-BackgroundFinish` завершатся ошибкой, если фонового обновления нет.
- `-Extension <имя расширения>` возвращает `0` при успехе и `1`, если расширение не найдено или операция завершилась ошибкой.
