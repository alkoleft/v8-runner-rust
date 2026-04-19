# Agent Rules for v8-runner

## Task Workflow

For every task:

1. Take the next concrete item from the active spec backlog: use `spec/IMPLEMENTATION_TODO.md` by default, and use `spec/MCP_IMPLEMENTATION_PLAN.md` only when the task explicitly targets the closed MCP rollout history/reference.
2. Extract scope, constraints, acceptance criteria, and affected files before editing code.
3. Draft an implementation plan with files, decisions, risks, and test strategy.
4. Run a plan review loop with two `reviewer` subagents in parallel.
5. Resolve findings and rerun once if major issues remain.
6. Freeze the plan before implementation.
7. Implement the task.
8. Use `worker` subagents only for disjoint file sets.
9. Run a code review loop in parallel:
   - `reviewer`
   - tests (`cargo test`)
   - Rust best-practices review
10. Fix findings and rerun once if needed.
11. Update the governing spec document for the task (`spec/IMPLEMENTATION_TODO.md` by default; `spec/MCP_IMPLEMENTATION_PLAN.md` only for explicit MCP rollout history/reference work), public docs, and `ARCHITECTURE.md`.
12. Commit only after all review and test gates pass.

## Documentation Language

- Новые документы и любые обновления существующих документов в репозитории нужно писать на русском языке.
- Это правило относится как минимум к `README`, `docs/*`, `spec/*`, `ARCHITECTURE.md` и аналогичным текстовым артефактам, которые изменяются в рамках текущей задачи.
- Английский допустим только там, где он является частью внешнего интерфейса, имени команды, формата данных, исходной цитаты или общепринятого технического идентификатора.

Hard limits:
- Plan review: max 2 rounds
- Code review: max 2 rounds
- If disagreements remain, record them explicitly and stop for user decision

## After each implementation stage:

  1. **Review** — run `/rust-expert-best-practices-code-review` skill on changed code before committing
1. **Compile check** — each stage must produce compilable code (`cargo check` must pass)
2. **Mark progress** — update task status in the task list (in_progress → completed)
3. **Update docs** — update the active task list (`spec/IMPLEMENTATION_TODO.md` by default, and `spec/MCP_IMPLEMENTATION_PLAN.md` only when explicitly maintaining the closed MCP rollout history/reference), and add/update doc comments (`///`) on all public types, functions, and modules introduced in the stage
4. **Update architecture** — if new modules or significant components are added, update `ARCHITECTURE.md` to reflect the current structure
5. **Commit** — create a separate git commit for every completed ready stage with a clear message describing exactly that stage
6. **Do not batch ready stages** — if a step is completed and passes the required checks, commit it immediately instead of accumulating multiple completed steps into one commit

## Stage definition

A stage is complete when:
- All tasks for that epic/group are marked completed
- `cargo check` passes with no errors
- Rust best practices review has been applied and issues fixed
- The governing task list is updated: `spec/IMPLEMENTATION_TODO.md` by default, or `spec/MCP_IMPLEMENTATION_PLAN.md` for explicit closed MCP rollout history/reference tasks; completed items marked with `[x]`
- Public types and functions have `///` doc comments
- `ARCHITECTURE.md` reflects any new modules or components

When a stage meets these conditions, it is considered ready and must be committed before starting the next completed-ready stage.

## ОБЯЗАТЕЛЬНО перед каждым коммитом

**ВСЕГДА** перед коммитом запускать три субагента параллельно:

1. **Review субагент** — проверить все изменения на корректность, стиль, безопасность, соответствие архитектуре проекта
2. **Tests субагент** — запустить тесты (`cargo test`) и убедиться что все проходят
3. **Rust expert субагент** — запустить скил `/rust-expert-best-practices-code-review` для глубокого ревью кода на соответствие best practices Rust

Коммит разрешён только если все три субагента завершились успешно.

## Commit message format

```
feat(scope): short description

- bullet points of what was done
```
