# ADR-0018: Перенести контракт информационной базы в `infobase`

- Статус: `accepted`
- Дата: `2026-04-21`
- Связанные решения: [ADR-0003](0003-podderzhivat-servernye-ib-dlya-vseh-instrumentov.md), [ADR-0017](0017-v8project-yaml-source-set-kak-glavnyy-konfiguratsionnyy-kontrakt.md)

## Контекст

Текущий config contract хранит строку подключения на верхнем уровне как `connection`, а пользователя информационной базы в отдельной секции `credentials`.
Это было достаточно для Designer/Enterprise utilities, которым нужна обычная строка подключения 1С и параметры `/N`/`/P`.

Для `IBCMD` server connection этого недостаточно. По `spec/ibcmd-commands-full.md` режимы `config` и `infobase` получают доступ к серверной базе через DBMS-level параметры:

- `--dbms`
- `--database-server`
- `--database-name`
- `--database-user`
- `--database-password`

Обычная строка 1С вида `Srvr=...;Ref=...` не содержит полный набор этих параметров и не может быть надежно преобразована в DBMS-level contract.
При этом DBMS-параметры описывают информационную базу, а не саму утилиту `ibcmd`, поэтому размещать их в `tools.ibcmd` неправильно.

## Решение

Перенести весь контракт подключения и учетных данных информационной базы в обязательную секцию `infobase`.

Целевой YAML:

```yaml
infobase:
  connection: "Srvr=cluster:1541;Ref=demo"
  user: Админ
  password: secret
  dbms:
    kind: PostgreSQL
    server: localhost
    name: demo
    user: postgres
    password: postgres-secret
```

Для файловой ИБ:

```yaml
infobase:
  connection: "File=build/ib"
  user: Админ
  password: secret
```

Правила:

1. `infobase.connection` является обязательной строкой подключения 1С.
2. `infobase.user` и `infobase.password` являются учетными данными пользователя информационной базы.
3. `infobase.dbms` описывает физическую СУБД только для server-based информационной базы.
4. `infobase.dbms.kind`, `infobase.dbms.server` и `infobase.dbms.name` обязательны для `builder=IBCMD` при server-based `infobase.connection`.
5. `infobase.dbms.user` и `infobase.dbms.password` являются учетными данными СУБД и не заменяют `infobase.user/password`.
6. `infobase.dbms` запрещен для file-based `infobase.connection`, чтобы конфиг не выглядел server-ready при фактической файловой базе.
7. Top-level `connection` больше не является supported config key.
8. Top-level `credentials` больше не является supported config key.
9. Legacy aliases или автоматическая миграция старого YAML не вводятся.
10. `config init` должен генерировать только новый формат с `infobase`.

Mapping для платформенных adapters:

1. Designer/Enterprise получают `infobase.connection` как обычную строку подключения 1С.
2. Designer/Enterprise получают `infobase.user/password` как `/N` и `/P`.
3. `IBCMD` file connection получает `infobase.connection` как `--db-path`.
4. `IBCMD` server connection получает:
   - `infobase.dbms.kind` -> `--dbms`
   - `infobase.dbms.server` -> `--database-server`
   - `infobase.dbms.name` -> `--database-name`
   - `infobase.dbms.user` -> `--database-user`
   - `infobase.dbms.password` -> `--database-password`
5. `IBCMD` получает `infobase.user/password` как `--user` и `--password`.

Для `init` server connection по-прежнему означает "использовать уже созданную серверную ИБ": шаг создания ИБ пропускается, а EDT workspace при `format=EDT` продолжает инициализироваться.
Создание серверной БД через `ibcmd infobase create --create-database` не входит в это решение и требует отдельного explicit config field или ADR.

## Неграницы (Non-goals)

1. Не поддерживать старые `connection` и `credentials` как legacy aliases.
2. Не выводить DBMS-параметры из строки `Srvr=...;Ref=...`.
3. Не добавлять `tools.ibcmd.database`, потому что DBMS contract относится к информационной базе, а не к binary discovery.
4. Не реализовывать автоматическое создание серверной базы в `init`.
5. Не менять разделение CLI и MCP public surfaces.

## Последствия

1. Это breaking change для `v8project.yaml`.
2. Все примеры, `config init`, public docs и tests должны перейти на `infobase`.
3. `AppConfig` должен моделировать `infobase` как typed object, а не как несколько top-level fields.
4. Validation boundary должен отклонять старые top-level keys и неполные DBMS-настройки до вызова platform DSL.
5. `IBCMD` перестает считать server connection documented gap: server mode становится целевым behavior при наличии `infobase.dbms`.

## План реализации

1. Обновить config model:
   - `src/config/model.rs`: добавить `InfobaseConfig` и `InfobaseDbmsConfig`;
   - удалить top-level `connection` и `credentials`;
   - обновить `AppConfig::v8_connection()`.
2. Обновить validation:
   - `src/config/validate.rs`: требовать `infobase.connection`;
   - отклонять top-level `connection`/`credentials`;
   - требовать `infobase.dbms.kind/server/name` для `builder=IBCMD` + server connection;
   - запрещать `infobase.dbms` для file connection.
3. Обновить IBCMD adapter:
   - `src/platform/ibcmd.rs`: поддержать file и server variants в `IbcmdConnection`;
   - добавить mapping DBMS args для `config import`, `config apply`, `config export`, `infobase config extension update`;
   - сохранить разделение `infobase.user/password` и `infobase.dbms.user/password`.
4. Обновить use cases, которые создают `IbcmdDsl`:
   - `src/use_cases/build_project.rs`;
   - `src/use_cases/dump_config.rs`;
   - `src/use_cases/configure_extensions.rs`;
   - `src/use_cases/init_project.rs`.
5. Обновить generated config:
   - `src/use_cases/config_init.rs`;
   - `examples/v8project.yaml`.
6. Обновить документацию:
   - `README.md`;
   - `docs/CONFIGURATION.md`;
   - `docs/CAPABILITIES.md`;
   - `docs/DEEP_DIVE.md`;
   - `ARCHITECTURE.md`;
   - arc42 sections.
7. Обновить tests:
   - config loader/validation tests;
   - IBCMD args mapping tests для file и server variants;
   - CLI build/dump/extensions regression tests для `builder=IBCMD` + server connection;
   - negative tests для старых top-level keys.

## Верификация

- [ ] Старый top-level `connection` отклоняется с понятной validation error.
- [ ] Старый top-level `credentials` отклоняется с понятной validation error.
- [ ] `config init` генерирует `infobase.connection`.
- [ ] Designer/Enterprise commands получают `infobase.connection` и `infobase.user/password`.
- [ ] `IBCMD` file connection использует `--db-path`.
- [ ] `IBCMD` server connection использует `--dbms`, `--database-server`, `--database-name`, optional `--database-user`, optional `--database-password`.
- [ ] `IBCMD` server connection также передает `infobase.user/password` как `--user/--password`.
- [ ] `builder=IBCMD` + server connection без `infobase.dbms.kind/server/name` падает на validation boundary.
- [ ] `infobase.dbms` при file connection падает на validation boundary.
