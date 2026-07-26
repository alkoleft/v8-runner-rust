# ADR-0023: Изолировать runtime state по информационной базе и использовать private shadow

- Статус: `accepted`
- Дата: `2026-07-21`
- Связанная задача: [#30](https://github.com/alkoleft/v8-runner-rust/issues/30)
- Уточняет: ADR-0002, ADR-0012, ADR-0015

## Контекст

Текущий change-detection context определяется только парой `designer|edt` и именем
`source-set`. Поэтому две разные информационные базы, собранные из одного checkout,
разделяют один snapshot и вторая ИБ может получить ложный `Skipped`.

Designer load всегда передает `-updateConfigDumpInfo`, а путь загрузки указывает на
живое дерево исходников. Платформа тем самым может создать или изменить
`ConfigDumpInfo.xml` в source tree. Incremental/partial dump также пишет прямо в
исходники и не способен безопасно отличить изменения пользователя от изменений ИБ.

## Решение

### Версионированная runtime identity

Runtime state хранится только под:

```text
workPath/ib-state/v1/<infobase-fingerprint>/<source-set>-<context-fingerprint>/
  hash-storage.redb
  ConfigDumpInfo.xml
  generations/<generation>/
    ib-baseline/<role>/
  transactions/
  runtime-state.lock
```

`SourceObservation` логически входит в ту же generation, но физически хранится в
`hash-storage.redb`; отдельный `source-observation/` каталог зарезервирован типовой моделью и
сейчас не материализуется.

`infobase-fingerprint` — SHA-256 от нормализованной, не содержащей секретов identity.
Ее строит один fallible typed normalizer, а тип identity реализует `Debug` только через
готовый fingerprint:

1. для файловой ИБ — от канонического пути ИБ;
2. для серверной ИБ — от отсортированных case-normalized параметров сервера и ссылки;
3. для IBCMD — дополнительно от `dbms.kind`, `dbms.server`, `dbms.name`;
4. plain semicolon form, raw `/F`/`-F`, `/S`/`-S` и
   `/IBConnectionString` приводятся к одной tagged model;
5. `Usr`/`Pwd`, `/N`/`-N`, `/P`/`-P`, config credentials и DB credentials
   исключаются до хеширования; отсутствующее значение после address/auth flag является ошибкой;
6. неизвестная raw connection form отклоняется, а не хешируется как потенциальный secret.

Пути нормализуются через nearest-existing canonical ancestor с проверенным lexical
suffix. Это дает одинаковую identity до и после создания отсутствующего target.
Symlink в уже существующей части разрешается; появление нового symlink в suffix считается
осознанной сменой target и приводит к новой identity.

`context-fingerprint` — SHA-256 от raw identity source-set, канонического source root,
`purpose`, `format`, backend и logical context kind (`edt` или `designer`). Поля
хешируются в versioned length-prefixed representation, чтобы исключить неоднозначную
конкатенацию. В логах fingerprints допустимы, исходные секреты — нет.

Старый `workPath/hash-storages` не мигрируется и не переиспользуется: в нем нет
достаточной информации для надежной привязки к ИБ. Первое обращение к новому layout
является bootstrap и обязательно выполняет full operation, даже для пустого source-set.

### Private CDFI для build

Designer никогда не получает живое source tree как load directory:

1. исходники копируются в private transaction directory с сохранением relative paths;
2. symlink не обходятся, `ConfigDumpInfo.xml` и вложенный `workPath` исключаются;
3. валидный private `ConfigDumpInfo.xml` добавляется в staging; source CDFI игнорируется;
4. missing/corrupt private CDFI переводит план в full bootstrap без синтеза UUID/version;
5. partial list строится относительно оригинального root и применим к идентичным путям staging;
6. после успешных load и apply созданный платформой CDFI валидируется и stage-ится;
   CDFI, baseline и hash snapshot становятся видимыми одним recoverable generation commit;
7. ошибка или отмена удаляет staging и не меняет source tree и успешный baseline.

Валидация CDFI проверяет только well-formed XML, ожидаемый local root name и наличие
непустых platform-owned identity/version values, которые реально присутствуют в файле.
Она не объявляет файл совместимым с конкретной версией платформы. Ошибка platform load
с seed CDFI не ретраится автоматически как full: без надежного diagnostic code такой
retry способен скрыть ошибку исходников. Старый CDFI сохраняется, а явный full rebuild
создает transaction без seed и после успеха заменяет private CDFI. UUID/version никогда
не синтезируются и не переписываются runner-ом.

### Private shadow и трехсторонняя публикация dump

Full, incremental и partial dump выполняются только в private shadow. При отсутствии
baseline incremental/partial request повышается до full shadow dump. Для каждого файла
отсутствующий `B` безопасно разрешает создание нового target (`S` отсутствует) или
convergence (`S == D`). Комбинация `B=absent, S=present, D=absent` является конфликтом:
runner не удаляет локальный файл, которым ещё не владел baseline.

Для обратной публикации сравниваются последний успешный baseline `B`, текущий source
`S` и новый shadow `D` по каждому файлу:

- `S == B && D != B` — публиковать `D`;
- `D == B && S != B` — сохранить локальный `S`;
- `S == D` — уже согласовано;
- `S != B && D != B && S != D` — conflict;
- остальные комбинации — no-op.

При любом conflict весь source-set остается неизменным. Перед publication source
повторно хешируется для защиты от TOCTOU. `ConfigDumpInfo.xml` никогда не публикуется
в source tree.

Publication работает по manifest только управляемых файлов, а не заменяет source root:

1. symlink, nested `workPath`, `.git` и прочие ignored/unmanaged entries не обходятся и
   не входят в manifest;
2. до первой записи создаются byte-exact backups затронутых target и fsync journal с
   generation, expected hashes и ordered actions;
3. при любой ошибке journal откатывается; незавершенный journal восстанавливается до
   новой операции;
4. commit marker пишется только после всех actions и повторной проверки; cleanup
   выполняется отдельно и идемпотентно.

Source-publication journal и private state commit связывает один канонический UUID
`DumpTransactionId`. Restart recovery считает state видимым и может завершить publication
вперёд только при точном совпадении пары `(generation, transaction id)`. Совпавший номер
generation с другим token не доказывает успех этой dump-транзакции и приводит к rollback
управляемых source changes.

Это recoverable source-set transaction: процессный сбой не оставляет неопределимое
состояние, а следующий запуск обязан завершить rollback до анализа. Она не заявляет
недостижимую multi-file filesystem atomicity в момент аварийного завершения процесса.

### Точная квитанция

Каждый `BuildStep` и единственный `DumpResult` содержит детерминированную,
отсортированную квитанцию следующей JSON-формы:

```json
{
  "status": "applied|skipped|failed|conflict",
  "requested": [{ "path": "...", "preHash": null, "postHash": "..." }],
  "processed": [],
  "skipped": [],
  "conflicted": []
}
```

Поля структуры private; exhaustive smart constructors не позволяют создать
противоречивые комбинации. `failed`/cancelled и `conflict` всегда имеют пустой
`processed`; `applied` не имеет `conflicted`; `skipped` не имеет `processed`.

Эти списки являются независимыми audit dimensions, а не строгим partition. Для
`applied` одна и та же запись с одинаковыми hashes может одновременно находиться в
`processed` и `skipped`, если файл входил в effective platform scope, но B/S/D merge сохранил
локальную версию или обнаружил no-op. Для full effective scope включает полный managed D и
baseline deletions; для incremental/partial `processed` строится по наблюдаемым записям private
shadow. Другие overlap и несовпадающие hashes отклоняются.

Списки имеют смысл:

- `requested` — исходно обнаруженная пользовательская дельта;
- `processed` — точный effective scope: полный managed result для full либо наблюдаемый write-set
  private shadow для incremental/partial, включая неизменные переписанные файлы;
- `skipped` — файлы, намеренно сохраненные/no-op;
- `conflicted` — файлы, заблокировавшие публикацию.

Каждая запись содержит нормализованный relative path и raw SHA-256 `preHash` /
`postHash`, когда соответствующая версия существует. Для build `preHash` — last
successfully applied source observation, `postHash` — requested/staged content. Для dump
`preHash` — source перед операцией, `postHash` — proposed shadow content; поэтому conflict
также наблюдаем без публикации. Add имеет `preHash=null`, delete — `postHash=null`, а
неизменный файл partial closure — одинаковые hashes. Failed/cancelled operation хранит
дельту только в `requested`, если она уже была вычислена, не объявляет файлы processed и не
продвигает state; при более раннем failure все списки могут быть пустыми.

### Два независимых состояния и их commit

`IbBaseline` — полное private зеркало последнего успешно наблюдавшегося результата ИБ,
а `SourceObservation` (`redb`) — содержимое, которое последний раз было успешно
применено к ИБ. Они представлены разными типами и одной state generation.

После dump applied/converged paths продвигаются к `D`; retained-local paths сохраняют
прежние baseline/observation `B`, чтобы следующий build снова увидел локальную дельту;
при conflict не продвигается ничего. CDFI, baseline manifest/files и redb snapshot
готовятся в private state transaction и коммитятся с journal/recovery generation.
Безопасный порядок делает новое состояние видимым только после source publication;
после state commit source journal помечается видимым точной парой `(generation,
DumpTransactionId)`. Незавершенный state journal восстанавливается до следующего planning pass.

## Последствия

1. Смена ИБ, source root, формата или backend создает независимый state и full bootstrap.
2. Две ИБ в одном checkout не могут влиять на skip/partial decision друг друга.
3. Clean clone не требует `ConfigDumpInfo.xml`; private CDFI появляется только как
   результат успешной операции платформы.
4. Runtime layout становится диагностическим контрактом, но fingerprints являются
   opaque: пользователи не должны вычислять или редактировать их вручную.
5. Копирование в private staging увеличивает локальный I/O, но делает build/dump
   транзакционными относительно пользовательских исходников.
6. Вложенный `workPath` должен исключаться не по имени каталога, а по разрешенному root.

## Верификация

- [x] A build -> repeat skip; B build -> full; return to A -> skip; restart сохраняет результат.
- [x] Credentials не влияют на fingerprint и не встречаются в диагностике.
- [x] Missing/corrupt private CDFI всегда дает full bootstrap.
- [x] Designer build failure не создает и не изменяет source `ConfigDumpInfo.xml`.
- [x] Build receipt различает requested/processed/skipped/conflicted с raw hashes и допустимый processed/skipped overlap.
- [ ] Отдельные orchestration-тесты incremental/partial dump подтверждают, что conflict не изменяет ни одного source-файла; общая B/S/D и manifest-механика уже покрыта unit-тестами.
- [x] Scanner и private copy исключают CDFI, symlinks и вложенный custom `workPath`.
- [x] Crash recovery требует exact transaction token и не затрагивает unmanaged entries.
- [ ] `windows-latest` contract CI подтверждает Windows-specific claim/replace/remove semantics; локальная Unix-проверка не заменяет этот gate.
- [ ] Disposable real file-IB acceptance подтверждает lifecycle private CDFI; без нее PR
      не объявляет задачу закрытой.
