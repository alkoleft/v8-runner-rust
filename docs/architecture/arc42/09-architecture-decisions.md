## 9. Архитектурные решения

Существующие ADR-файлы:

- [ADR-0001: Границы поддержки IBCMD как ограниченного backend](../../decisions/0001-granitsy-podderzhki-ibcmd-kak-ogranichennogo-backend.md)
- [ADR-0002: Изолировать runtime state по source-set под workPath](../../decisions/0002-izolirovat-runtime-state-po-source-set-pod-workpath.md)
- [ADR-0003: Поддерживать серверные ИБ для всех инструментов](../../decisions/0003-podderzhivat-servernye-ib-dlya-vseh-instrumentov.md)
- [ADR-0004: Автообнаруживать компоненты платформы 1С по версии-маске](../../decisions/0004-avtoobnaruzhivat-komponenty-platformy-1s-po-versii-maske.md)
- [ADR-0005: Разделить CLI и MCP публичные поверхности](../../decisions/0005-razdelit-cli-i-mcp-publichnye-poverhnosti.md)
- [ADR-0006: Сохранять транспортно-нейтральный use case слой](../../decisions/0006-sohranyat-transportno-neytralnyy-use-case-sloy.md)
- [ADR-0007: Свести EDT execution к one-shot и shared interactive режимам](../../decisions/0007-vydelit-otdelnyy-pereklyuchatel-dlya-shared-edt.md)
- [ADR-0008: Держать платформенные backend DSL отдельно от orchestration](../../decisions/0008-derzhat-platformennye-backend-dsl-otdelno-ot-orchestration.md)
- [ADR-0009: Разделить structured business failures и transport/runtime failures](../../decisions/0009-razdelit-business-i-transport-runtime-failures.md)
- [ADR-0010: Разделить CLI output для человека и AI-агента](../../decisions/0010-razdelit-cli-output-dlya-cheloveka-i-ai-agenta.md)
- [ADR-0011: Эксклюзивное владение `workPath` на время команды](../../decisions/0011-eksklyuzivnoe-vladenie-workpath-na-vremya-komandy.md)
- [ADR-0012: On-demand change detection и файловая partial-load стратегия](../../decisions/0012-on-demand-change-detection-i-faylovaya-partial-load-strategiya.md)
- [ADR-0013: MCP execution admission, timeout/cancellation routing и HTTP session capacity](../../decisions/0013-mcp-execution-admission-timeout-cancellation-routing-i-http-session-capacity.md)
- [ADR-0014: Единая timeout/cancellation policy для CLI и MCP команд](../../decisions/0014-edinaya-timeout-cancellation-policy-dlya-cli-i-mcp-komand.md)
- [ADR-0015: Атомарная публикация dump/artifacts через staging/backup](../../decisions/0015-atomarnaya-publikatsiya-dump-artifacts-cherez-staging-backup.md)
- [ADR-0016: Единый `ExecutionOutcome` и pipeline steps для runner-like сценариев](../../decisions/0016-edinyy-executionoutcome-i-pipeline-steps-dlya-runner-like-stsenariev.md)
- [ADR-0017: `v8project.yaml` / `source-set` как главный конфигурационный контракт](../../decisions/0017-v8project-yaml-source-set-kak-glavnyy-konfiguratsionnyy-kontrakt.md)

Архитектурные инварианты для агентов и контрибьюторов зафиксированы в [docs/architecture/invariants.md](../invariants.md).

Важные уже реализованные решения, которые сейчас зафиксированы кодом и внутренними архитектурными заметками:

- транспортно-нейтральные контракты use case, общие для CLI и MCP, формализованы в ADR-0006;
- отдельные платформенные адаптеры для Designer, Enterprise, IBCMD и EDT формализованы в ADR-0008;
- централизованный поиск компонентов платформы 1С по версии или версии-маске;
- общий интерактивный EDT actor ограничен MCP EDT syntax, а не всеми EDT-операциями;
- CLI и MCP intentionally expose different public surfaces: MCP не зеркалит CLI полностью, см. ADR-0005;
- текущая поддержка `builder=IBCMD` ограничена файловыми ИБ, но целевой контракт требует server infobase support для всех инструментов;
- сохранённое инкрементальное состояние хранится в per-source-set `redb` contexts под `workPath`;
- presentation concerns (`Presenter`, `Envelope`, text formatting) остаются вне use case;
- разделение business failures и transport/runtime failures формализовано в ADR-0009;
- EDT execution имеет два целевых режима: one-shot и shared interactive, см. ADR-0007;
- CLI output должен различать human highlights и agent minimal signal, см. ADR-0010;
- команды над одним canonical `workPath` должны сериализоваться через workspace lock, см. ADR-0011;
- change detection выполняется on-demand, а partial load остаётся conservative file-level strategy, см. ADR-0012;
- MCP execution admission и HTTP session capacity разделены, см. ADR-0013;
- CLI/MCP timeout/cancellation имеют общий terminal-state contract и critical DB phase protection, см. ADR-0014;
- full replacement dump/artifacts publication проходит через staging/backup, см. ADR-0015;
- runner-like сценарии используют `ExecutionOutcome<T>` и стандартную pipeline vocabulary, см. ADR-0016;
- `v8project.yaml` / `source-set` является главным typed config contract, см. ADR-0017.

Рекомендуемое развитие:

- фиксировать эти решения в явных ADR, когда они меняются или когда добавляются новые backend/transport;
- при изменении любого инварианта сначала обновлять соответствующий ADR.
