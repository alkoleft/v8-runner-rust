# ADR-0022: Ввести общий механизм подготовки расширений и использовать его для `client_mcp`

- Статус: `accepted`
- Дата: `2026-05-02`
- Связанные решения: [ADR-0002](0002-izolirovat-runtime-state-po-source-set-pod-workpath.md), [ADR-0005](0005-razdelit-cli-i-mcp-publichnye-poverhnosti.md), [ADR-0006](0006-sohranyat-transportno-neytralnyy-use-case-sloy.md), [ADR-0012](0012-on-demand-change-detection-i-faylovaya-partial-load-strategiya.md), [ADR-0015](0015-atomarnaya-publikatsiya-dump-artifacts-cherez-staging-backup.md), [ADR-0017](0017-v8project-yaml-source-set-kak-glavnyy-konfiguratsionnyy-kontrakt.md), [ADR-0021](0021-lokalnyy-overlay-config.md)

## Контекст

Для работы `launch mcp` / `launch mcp va` нужно клиентское MCP-расширение в информационной базе.
До этого его можно было указывать как `source-set`, но `source-set` является проектной единицей
orchestration и runtime identity.
Локальное инструментальное расширение не должно менять состав project source-set.

При этом в проекте уже есть несколько близких механизмов:

1. `build` загружает project extension source-set в ИБ;
2. `load` умеет загружать `.cfe`;
3. `init` готовит EDT workspace;
4. `configure_extensions` обновляет свойства extension.

Нужно не делать частный path только для `client_mcp`, а заложить общий механизм подготовки
расширений в ИБ и использовать `client_mcp` как первый tool-extension consumer.

Уточнение `2026-05-02`: если tool extension задан через `source.path`, этот путь является
исходниками расширения и должен получать тот же on-demand change-detection подход, что и project
`source-set` path. Повторный `test -> build` не должен заново выполнять EDT export для неизменённого
tool-extension source; текущий воспроизводимый симптом зафиксирован в проекте `rat`, где
`v8project.local.yaml` указывает `tools.client_mcp.extension.source.path` на внешний EDT-каталог, а
каждый запуск тестов до доработки доходит до `build: tool extension edt export`.

## Решение

Ввести внутренний общий механизм подготовки расширения в ИБ.
Вход механизма должен описывать extension identity и один источник:

1. source path;
2. `.cfe` artifact path.

Публичная конфигурация для первого потребителя:

```yaml
tools:
  client_mcp:
    port: 9874
    extension:
      name: client_mcp
      source:
        path: /home/user/projects/onec-client-mcp/exts/client-mcp
        format: EDT
```

или:

```yaml
tools:
  client_mcp:
    port: 9874
    extension:
      name: client_mcp
      artifact:
        path: /home/user/tools/client_mcp.cfe
```

Rules:

1. `tools.client_mcp.extension.name` обязателен и является именем extension в ИБ.
2. Должен быть указан ровно один источник: `source` или `artifact`.
3. `artifact.path` поддерживает только `.cfe`.
4. `source.path` указывает на исходники расширения.
5. `source.format` может быть `DESIGNER` или `EDT`; если не задан, используется глобальный `format`.
6. `tools.client_mcp.extension` может жить в `v8project.local.yaml` из ADR-0021.
7. Tool extension не добавляется в project `source-set`, не участвует в project source-set ordering и не выбирается через `--source-set`.

### Runtime semantics

`init`:

1. готовит обычную project runtime среду;
2. если настроен tool extension с `source.format=EDT`, добавляет этот extension project в EDT workspace;
3. если extension задан как `.cfe` artifact, дополнительных workspace-действий не выполняет.

`build`:

1. строит configured project source-set по текущей build semantics;
2. после project build вызывает общий механизм подготовки extension для `tools.client_mcp.extension`, если он задан;
3. для `source` анализирует source path через stable tool-extension change-detection context и
   строит/экспортирует/загружает extension из исходников только если источник изменился, включён
   `--full-rebuild` или analysis дал conservative full-execution fallback;
4. для `artifact.path` загружает `.cfe` как extension с именем `tools.client_mcp.extension.name`;
5. не вводит отдельный `install` flag: подготовка extension является частью стадии `build`.

`launch mcp` и `launch mcp va`:

1. не загружают и не обновляют extension;
2. запускают client MCP payload только поверх уже подготовленной ИБ;
3. если extension настроен, но не подготовлен, должны давать понятный diagnostic hint: выполнить `v8-runner build`.

Validation должна отклонять unsupported combinations до platform DSL.
Если выбранный backend/config не умеет подготовить заданный source или `.cfe`, ошибка должна быть
validation error, а не поздний platform failure.

## Неграницы (Non-goals)

1. Не вводить `install`, `auto`, `always` или похожие режимы установки.
2. Не загружать extension на стадии `launch mcp`.
3. Не возвращать `client_mcp` в project `source-set`.
4. Не публиковать общий механизм подготовки расширений как отдельную MCP tool operation.
5. Не искать fallback-источник: если задан `artifact`, runner не ищет `source`; если задан `source`, runner не ищет `.cfe`.
6. Не вводить generic pipeline engine; нужен общий targeted механизм для подготовки extension.

## Последствия

1. `client_mcp` становится tool extension, а не частью project source-set.
2. `init`, `build` и `launch mcp` получают понятное разделение обязанностей:
   - `init` готовит workspace;
   - `build` загружает/обновляет project и tool extensions;
   - `launch mcp` только запускает client MCP.
3. Будущие tool extensions смогут использовать тот же extension preparation mechanism.
4. Реализация должна переиспользовать существующие build/load/configure helpers, а не дублировать platform DSL.

## План реализации

1. `src/config/model.rs`:
   - добавить typed model `tools.client_mcp.extension`;
   - добавить `source` и `artifact` variants.
2. `src/config/validate.rs`:
   - валидировать safe `extension.name`;
   - валидировать взаимоисключение `source`/`artifact`;
   - валидировать `.cfe` для `artifact.path`;
   - валидировать source layout для `DESIGNER`/`EDT`;
   - отклонять unsupported backend/config combinations.
3. Новый общий internal use case/helper:
   - принять normalized extension preparation request;
   - поддержать source-based extension preparation;
   - использовать on-demand change detection для source-backed tool extensions без превращения их в
     project `source-set`;
   - коммитить prepared tool-extension snapshot только после successful export/load step;
   - поддержать `.cfe` artifact loading through existing load semantics;
   - вернуть structured outcome/diagnostics без отдельного public CLI surface.
4. `src/use_cases/init_project.rs`:
   - при EDT source tool extension добавлять project в EDT workspace.
5. `src/use_cases/build_project.rs`:
   - после project source-set build вызывать общий extension preparation mechanism для `tools.client_mcp.extension`;
   - сохранить `--source-set` как project selector, не как selector tool extension.
6. `src/use_cases/launch_app.rs`:
   - не выполнять подготовку extension;
   - добавить понятный hint на `v8-runner build`, если client MCP launch зависит от неподготовленной ИБ.
7. Docs/tests:
   - обновить `docs/CONFIGURATION.md`, `docs/CAPABILITIES.md` and examples;
   - после реализации обновить `v8-runner/SKILL.md`, чтобы repo-local skill описывал shipped behavior;
   - добавить targeted tests для source/artifact validation, EDT workspace init, build preparation,
     source-backed no-change skip, full-rebuild refresh and no-install launch behavior.

## Верификация

- [ ] `tools.client_mcp.extension.source` и `artifact` являются взаимоисключающими.
- [ ] `.cfe` artifact загружается на стадии `build`, а не на стадии `launch mcp`.
- [ ] EDT source tool extension добавляется в EDT workspace на стадии `init`.
- [ ] `client_mcp` extension не появляется в project `source-set` и не выбирается через `--source-set`.
- [ ] Неизменённый source-backed tool extension пропускает EDT export/load при повторном `build` и
  nested `test -> build`.
- [ ] Изменение в source-backed tool extension или `--full-rebuild` приводит к refresh.
- [ ] `launch mcp` / `launch mcp va` не выполняют install/update extension и дают понятный hint при неподготовленной ИБ.
