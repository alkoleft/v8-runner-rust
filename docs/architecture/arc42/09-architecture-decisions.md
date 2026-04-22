## 9. Архитектурные решения

Источник истины для архитектурных решений — [docs/decisions](../../decisions/README.md). Этот раздел даёт обзор решений и показывает, какие контракты arc42 обязан отражать в остальных разделах.

| ADR | Статус / дата | Краткое значение для архитектуры |
| --- | --- | --- |
| [ADR-0001: Границы поддержки IBCMD как ограниченного backend](../../decisions/0001-granitsy-podderzhki-ibcmd-kak-ogranichennogo-backend.md) | `accepted`, `2026-04-02` | `IBCMD` принят как ограниченный сейчас backend для `init`, `build`, `dump`, `extensions`; file-only и partial-dump ограничения считаются gaps, а не целевой нормой. |
| [ADR-0002: Изолировать runtime state по source-set под workPath](../../decisions/0002-izolirovat-runtime-state-po-source-set-pod-workpath.md) | `accepted`, `2026-04-20` | `source-set` является минимальной единицей оркестрации, `workPath` — owned runtime root; EDT flow разделяет `edt-*` и `designer-*` change-detection contexts. |
| [ADR-0003: Поддерживать серверные ИБ для всех инструментов](../../decisions/0003-podderzhivat-servernye-ib-dlya-vseh-instrumentov.md) | `accepted`, `2026-04-20` | Server infobase support является целевым контрактом всех публичных инструментов, а file-only поведение допускается только как явно описанный gap. |
| [ADR-0004: Автообнаруживать компоненты платформы 1С по версии-маске](../../decisions/0004-avtoobnaruzhivat-komponenty-platformy-1s-po-versii-maske.md) | `accepted`, `2026-04-20` | Use case и platform adapters получают `1cv8`, `1cv8c`, `ibcmd` через общий locator/facade с поддержкой exact version и version masks. |
| [ADR-0005: Разделить CLI и MCP публичные поверхности](../../decisions/0005-razdelit-cli-i-mcp-publichnye-poverhnosti.md) | `accepted`, `2026-04-20` | MCP не зеркалит CLI; текущая MCP-поверхность состоит из 8 tool-операций, а расширение MCP требует отдельного решения или обновления ADR. |
| [ADR-0006: Сохранять транспортно-нейтральный use case слой](../../decisions/0006-sohranyat-transportno-neytralnyy-use-case-sloy.md) | `accepted`, `2026-04-20` | `src/use_cases` не зависит от `clap`, CLI `Presenter`/`Envelope`, MCP DTO и concrete transport payload format. |
| [ADR-0007: Свести EDT execution к one-shot и shared interactive режимам](../../decisions/0007-vydelit-otdelnyy-pereklyuchatel-dlya-shared-edt.md) | `accepted`, `2026-04-20` | Целевые EDT modes — только one-shot и shared interactive; direct non-shared interactive sessions считаются implementation gap. |
| [ADR-0008: Держать платформенные backend DSL отдельно от orchestration](../../decisions/0008-derzhat-platformennye-backend-dsl-otdelno-ot-orchestration.md) | `accepted`, `2026-04-20` | Designer, IBCMD, EDT, Enterprise DSL и process/locator details остаются в `src/platform`, а use case работают с domain-level operations/results. |
| [ADR-0009: Разделить structured business failures и transport/runtime failures](../../decisions/0009-razdelit-business-i-transport-runtime-failures.md) | `accepted`, `2026-04-20` | Use case возвращают `UseCaseFailure<T>`; MCP различает `McpBusinessFailure<T>` и `McpInternalError`, не смешивая business payload и runtime faults. |
| [ADR-0010: Единый CLI output для человека и AI-агента](../../decisions/0010-razdelit-cli-output-dlya-cheloveka-i-ai-agenta.md) | `accepted`, `2026-04-20` | CLI использует единый high-signal output contract для обеих ролей; единственная публичная ось — `--output text|json`, а JSON не меняется только из-за различения ролей. |
| [ADR-0011: Эксклюзивное владение `workPath` на время команды](../../decisions/0011-eksklyuzivnoe-vladenie-workpath-na-vremya-komandy.md) | `accepted`, `2026-04-20` | Public CLI/MCP commands над `workPath` должны брать advisory lock по canonical path; nested flows используют explicit unlocked entrypoints. |
| [ADR-0012: On-demand change detection и файловая partial-load стратегия](../../decisions/0012-on-demand-change-detection-i-faylovaya-partial-load-strategiya.md) | `accepted`, `2026-04-20` | Change detection запускается on-demand, хранит per-context `redb` snapshots и деградирует в full execution при unsafe partial cases. |
| [ADR-0013: MCP execution admission, timeout/cancellation routing и HTTP session capacity](../../decisions/0013-mcp-execution-admission-timeout-cancellation-routing-i-http-session-capacity.md) | `accepted`, `2026-04-20` | MCP execution admission, timeout/cancellation routing и HTTP session capacity являются разными guardrails; MCP cancellation/deadline должны идти в общую execution policy. |
| [ADR-0014: Единая timeout/cancellation policy для CLI и MCP команд](../../decisions/0014-edinaya-timeout-cancellation-policy-dlya-cli-i-mcp-komand.md) | `accepted`, `2026-04-20` | Целевой контракт: каждая public command имеет deadline, cancellation ждёт terminal state, mutating critical phases не hard-kill by default. |
| [ADR-0015: Атомарная публикация dump/artifacts через staging/backup](../../decisions/0015-atomarnaya-publikatsiya-dump-artifacts-cherez-staging-backup.md) | `accepted`, `2026-04-21` | Full replacement dump/artifacts публикуются через sibling staging/backup, rollback context и metadata-based orphan cleanup; incremental/partial остаются non-atomic. |
| [ADR-0016: Единый `ExecutionOutcome` и pipeline steps для runner-like сценариев](../../decisions/0016-edinyy-executionoutcome-i-pipeline-steps-dlya-runner-like-stsenariev.md) | `accepted`, `2026-04-21` | Runner-like и pipeline-like сценарии используют `ExecutionOutcome<T>` как canonical domain outcome и общую vocabulary pipeline blocks/steps. |
| [ADR-0017: `v8project.yaml` / `source-set` как главный конфигурационный контракт](../../decisions/0017-v8project-yaml-source-set-kak-glavnyy-konfiguratsionnyy-kontrakt.md) | `accepted`, `2026-04-20` | `v8project.yaml` -> `AppConfig` -> `config::validate` является главным config contract; `source-set[].type`, `source-set.name` и `workPath` задают runtime identity. |
| [ADR-0018: Перенести контракт информационной базы в `infobase`](../../decisions/0018-perenesti-kontrakt-informatsionnoy-bazy-v-infobase.md) | `accepted`, `2026-04-21` | `infobase.connection` и `infobase.user/password` заменяют top-level `connection`/`credentials`; `infobase.dbms` задаёт DBMS-level contract для `IBCMD` server connection. |
| [ADR-0019: Обеспечивать наличие серверной ИБ через `ibcmd` в `init`](../../decisions/0019-sozdavat-servernuyu-infobazu-cherez-ibcmd-pri-init-pri-otsutstvii.md) | `accepted`, `2026-04-22` | Для `builder=IBCMD` + server connection `init` использует `ibcmd infobase create --create-database` как ensure-step; отдельный pre-check наличия не обязателен, а benign `already exists` нормализуется как non-error outcome. |
| [ADR-0020: Упростить CLI-only `convert` до repo-aware конвертации текущих исходников проекта](../../decisions/0020-dobavit-cli-only-convert-dlya-dvustoronney-konvertatsii-edt-i-designer.md) | `accepted`, `2026-04-22` | Фиксирует и уже реализует repo-aware `convert [--source-set <name>]`, который работает от `v8project.yaml`, выводит направление из `format`, публикует output только под `workPath/convert/out` и не выносит low-level EDT flags в public surface. |

Архитектурные инварианты для агентов и контрибьюторов зафиксированы в [docs/architecture/invariants.md](../invariants.md).

### Сквозные выводы из ADR

- Public surface changes нужно оценивать отдельно для CLI и MCP: наличие CLI-команды не означает доступность MCP tool.
- `convert` является осознанной CLI-only командой и не должен трактоваться как автоматический кандидат в MCP tool.
- Use case layer остаётся общей транспортно-нейтральной orchestration boundary, а adapters отвечают за presentation, DTO и transport/runtime failures.
- `source-set.name` и canonical `workPath` являются runtime identity. Изменения naming/path rules затрагивают config validation, change detection, generated directories и workspace lock.
- `infobase` является единственным config contract для строки подключения, пользователя ИБ и DBMS-level доступа; top-level `connection`/`credentials` не поддерживаются.
- Полный `infobase.dbms` contract при `builder=IBCMD` достаточно явно разрешает server infobase provisioning в `init`; отдельный top-level provisioning flag для этого не требуется.
- Repo-aware `convert` и reverse sync из ИБ в файлы — разные сценарии; `dump format=EDT` реализован как отдельный flow поверх internal Designer snapshot и EDT import, а не как alias или скрытый sub-step `convert`.
- MCP concurrency имеет два независимых контура: execution admission для tool calls и HTTP session capacity для stateful transport lifecycle.
- Target publication safety не обеспечивается workspace lock: full replacement outputs требуют staging/backup contract рядом с target.
- ADR-0014 и ADR-0016 описывают целевую архитектуру с известными migration gaps. Новые команды должны следовать этим контрактам, даже если часть старых сценариев ещё находится в переходном состоянии.

### Правила актуализации

- При добавлении или изменении ADR синхронизировать этот раздел и затронутые arc42-разделы, а не только список ссылок.
- При изменении любого инварианта сначала обновлять соответствующий ADR или добавлять новый ADR, который явно заменяет старое решение.
- Если реализация временно расходится с принятым ADR, фиксировать это как implementation gap в разделе 11 и в профильной публичной документации, когда gap виден пользователю.
