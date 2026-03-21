# Agent Rules for v8-test-runner-rust

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

Hard limits:
- Plan review: max 2 rounds
- Code review: max 2 rounds
- If disagreements remain, record them explicitly and stop for user decision

## After each implementation stage:

1. **Review** — run `/rust-expert-best-practices-code-review` skill on changed code before committing
2. **Compile check** — each stage must produce compilable code (`cargo check` must pass)
3. **Mark progress** — update task status in the task list (in_progress → completed)
4. **Update docs** — update the active task list (`spec/IMPLEMENTATION_TODO.md` by default, and `spec/MCP_IMPLEMENTATION_PLAN.md` only when explicitly maintaining the closed MCP rollout history/reference), and add/update doc comments (`///`) on all public types, functions, and modules introduced in the stage
5. **Update architecture** — if new modules or significant components are added, update `ARCHITECTURE.md` to reflect the current structure
6. **Commit** — create a git commit with a clear message describing the stage

## Stage definition

A stage is complete when:
- All tasks for that epic/group are marked completed
- `cargo check` passes with no errors
- Rust best practices review has been applied and issues fixed
- The governing task list is updated: `spec/IMPLEMENTATION_TODO.md` by default, or `spec/MCP_IMPLEMENTATION_PLAN.md` for explicit closed MCP rollout history/reference tasks; completed items marked with `[x]`
- Public types and functions have `///` doc comments
- `ARCHITECTURE.md` reflects any new modules or components

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
