## 4. Стратегия решения

Архитектура следует слоистой модели оркестрации.

Ключевые решения и целевые контракты:

- CLI и MCP остаются тонкими адаптерами над транспортно-нейтральными use case.
- `v8project.yaml` и typed `AppConfig` являются главным конфигурационным контрактом; unsafe/unsupported combinations должны отклоняться на validation boundary.
- Прямое взаимодействие с инструментами 1С инкапсулировано в выделенных платформенных адаптерах.
- Анализ изменений используется для предпочтения инкрементальной работы вместо полного rebuild.
- Структурированные типы результатов сохраняются до границы адаптера, а затем рендерятся отдельно для CLI и MCP.
- MCP рассматривается не только как транспорт: он добавляет сессии, параллелизм, нормализацию, admission control и обработку транспортных ошибок.
- Публичные команды над одним canonical `workPath` сериализуются через workspace lock; nested flows используют явные unlocked entrypoints только под внешним lock.
- Timeout/cancellation реализуются поверх общего execution core по host-specific policy: MCP может отсоединять caller от running EDT работы и удерживать capacity до terminal state, а CLI blocking flows ждут terminal cleanup или принудительно закрывают свой shared EDT manager перед возвратом.
- Full replacement `dump` и `artifacts` публикуются через staging/backup, чтобы platform failure до publish сохранял старый target.
- `tools download` остаётся CLI-only bootstrap-сценарием: он материализует внешние release assets в рабочие каталоги проекта и обновляет local overlay, но не расширяет MCP surface и не выполняет platform load в ИБ.
- Runner-like сценарии используют общий execution grammar: pipeline vocabulary, step entries и `ExecutionOutcome<T>` как canonical domain outcome.
- Общий интерактивный EDT actor вынесен в `platform::edt_session` и переиспользуется и MCP `check_syntax_edt`, и CLI interactive EDT use cases; различается только host policy (MCP может prewarm shared host, CLI остаётся lazy и short-lived).
- Архитектура оптимизирована под agent-friendly contracts: use case возвращают transport-neutral DTO и структурированные failure payload, а логика представления остаётся на границе адаптера.

Эта стратегия удерживает публичную поверхность стабильной и позволяет независимо развивать платформенное поведение и транспортные правила.
