# Архитектурные решения (ADR)

Этот каталог хранит архитектурные решения проекта в формате ADR.

## Индекс

- [ADR-0001: Границы поддержки IBCMD как ограниченного backend](0001-granitsy-podderzhki-ibcmd-kak-ogranichennogo-backend.md) — `accepted`, `2026-04-02`
- [ADR-0002: Изолировать runtime state по source-set под workPath](0002-izolirovat-runtime-state-po-source-set-pod-workpath.md) — `accepted`, `2026-04-20`
- [ADR-0003: Поддерживать серверные ИБ для всех инструментов](0003-podderzhivat-servernye-ib-dlya-vseh-instrumentov.md) — `accepted`, `2026-04-20`
- [ADR-0004: Автообнаруживать компоненты платформы 1С по версии-маске](0004-avtoobnaruzhivat-komponenty-platformy-1s-po-versii-maske.md) — `accepted`, `2026-04-20`
- [ADR-0005: Разделить CLI и MCP публичные поверхности](0005-razdelit-cli-i-mcp-publichnye-poverhnosti.md) — `accepted`, `2026-04-20`
- [ADR-0006: Сохранять транспортно-нейтральный use case слой](0006-sohranyat-transportno-neytralnyy-use-case-sloy.md) — `accepted`, `2026-04-20`
- [ADR-0007: Свести EDT execution к one-shot и shared interactive режимам](0007-vydelit-otdelnyy-pereklyuchatel-dlya-shared-edt.md) — `accepted`, `2026-04-20`
- [ADR-0008: Держать платформенные backend DSL отдельно от orchestration](0008-derzhat-platformennye-backend-dsl-otdelno-ot-orchestration.md) — `accepted`, `2026-04-20`
- [ADR-0009: Разделить structured business failures и transport/runtime failures](0009-razdelit-business-i-transport-runtime-failures.md) — `accepted`, `2026-04-20`
- [ADR-0010: Единый CLI output для человека и AI-агента](0010-razdelit-cli-output-dlya-cheloveka-i-ai-agenta.md) — `accepted`, `2026-04-20`
- [ADR-0011: Эксклюзивное владение `workPath` на время команды](0011-eksklyuzivnoe-vladenie-workpath-na-vremya-komandy.md) — `accepted`, `2026-04-20`
- [ADR-0012: On-demand change detection и файловая partial-load стратегия](0012-on-demand-change-detection-i-faylovaya-partial-load-strategiya.md) — `accepted`, `2026-04-20`
- [ADR-0013: MCP execution admission, timeout/cancellation routing и HTTP session capacity](0013-mcp-execution-admission-timeout-cancellation-routing-i-http-session-capacity.md) — `accepted`, `2026-04-20`
- [ADR-0014: Единая timeout/cancellation policy для CLI и MCP команд](0014-edinaya-timeout-cancellation-policy-dlya-cli-i-mcp-komand.md) — `accepted`, `2026-04-20`
- [ADR-0015: Атомарная публикация dump/artifacts через staging/backup](0015-atomarnaya-publikatsiya-dump-artifacts-cherez-staging-backup.md) — `accepted`, `2026-04-21`
- [ADR-0016: Единый `ExecutionOutcome` и pipeline steps для runner-like сценариев](0016-edinyy-executionoutcome-i-pipeline-steps-dlya-runner-like-stsenariev.md) — `accepted`, `2026-04-21`
- [ADR-0017: `v8project.yaml` / `source-set` как главный конфигурационный контракт](0017-v8project-yaml-source-set-kak-glavnyy-konfiguratsionnyy-kontrakt.md) — `accepted`, `2026-04-20`
- [ADR-0018: Перенести контракт информационной базы в `infobase`](0018-perenesti-kontrakt-informatsionnoy-bazy-v-infobase.md) — `accepted`, `2026-04-21`
- [ADR-0019: Обеспечивать наличие серверной ИБ через `ibcmd` в `init`](0019-sozdavat-servernuyu-infobazu-cherez-ibcmd-pri-init-pri-otsutstvii.md) — `accepted`, `2026-04-22`
- [ADR-0020: Упростить CLI-only `convert` до repo-aware конвертации текущих исходников проекта](0020-dobavit-cli-only-convert-dlya-dvustoronney-konvertatsii-edt-i-designer.md) — `accepted`, `2026-04-22`
- [ADR-0021: Ввести локальный overlay для `v8project.yaml`](0021-lokalnyy-overlay-config.md) — `accepted`, `2026-05-02`
- [ADR-0022: Ввести общий механизм подготовки расширений и использовать его для `client_mcp`](0022-universalnyy-mehanizm-podgotovki-rasshireniy-i-client-mcp-extension.md) — `accepted`, `2026-05-02`

## Правила обновления

- Для изменений архитектурных ограничений добавляйте новый ADR или обновляйте существующий с явным указанием статуса.
- При обновлении публичного контракта синхронизируйте связанные документы (`README.md`, `docs/CAPABILITIES.md`, `docs/DEEP_DIVE.md`, `ARCHITECTURE.md`).
- Архитектурные инварианты, которые должны соблюдаться агентами и контрибьюторами, перечислены в [spec/architecture/invariants.md](../architecture/invariants.md).
