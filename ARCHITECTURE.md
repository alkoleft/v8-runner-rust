# Architecture

> Contributor note: this file is the internal module and boundary map. For the user-facing operational guide, see `docs/DEEP_DIVE.md`.

## Overview

`v8-runner` is a Rust CLI for orchestrating local 1C platform operations. The current codebase is organized into eight main layers:

Architecture decisions live in [docs/decisions](docs/decisions/README.md), and agent-facing invariants are summarized in [docs/architecture/invariants.md](docs/architecture/invariants.md).
Практический checklist для изменений MCP surface, public command boundary и config contract вынесен в [docs/architecture/change-checklist.md](docs/architecture/change-checklist.md).

1. `cli` parses arguments, maps them into transport-neutral requests, and owns command-level text/json rendering.
2. `config` loads and validates YAML configuration.
3. `domain` defines structured result types for commands plus shared execution step structs.
4. `use_cases` owns transport-neutral requests, `ExecutionContext`, structured failures, and business orchestration.
5. `mcp` now contains both the MCP-facing service boundary and the stdio/HTTP transport adapters: it maps raw tool inputs into use-case requests, returns MCP-specific DTOs plus structured business/internal failures, and publishes the live MCP tool servers.
6. `platform` contains process execution, utility discovery, connection argument building, and low-level 1C adapters.
7. `output` contains CLI presentation primitives such as `Presenter` and `Envelope`.
8. `change_detection`, `parsers`, and `support` provide shared subsystems and utilities.

## Current Platform Layer

The platform layer is intentionally split so responsibilities do not bleed into use cases:

- `platform::process` defines `ProcessRunner`, `ProcessExecutor`, `ProcessRequest`, `ProcessResult`, and `SpawnResult`.
- `platform::locator` resolves concrete executables (`1cv8`, `1cv8c`, `ibcmd`, `1cedtcli`) and caches results per `Locator` instance. Platform component discovery by version mask is governed by [ADR-0004](docs/decisions/0004-avtoobnaruzhivat-komponenty-platformy-1s-po-versii-maske.md).
- `platform::connection` builds reusable V8 connection/auth arguments from `infobase.connection`.
- `platform::utilities` is the current facade used by use cases. It owns the stateful `Locator` and exposes the standard execution path.
- `platform::designer` is the low-level batch DSL for `1cv8 DESIGNER`, returning `PlatformCommandResult` so `/Out` logs stay separate from runner-captured stdio.
- `platform::ibcmd` is the low-level DSL for `ibcmd`, returning `PlatformCommandResult` with stdout/stderr diagnostics (no `/Out` log).
- `platform::interactive` now contains the low-level `InteractiveProcessExecutor` for `1cedtcli`: it starts one child in its own process group, waits for the `1C:EDT>` prompt on stdout or stderr, executes prompt-delimited commands, applies the shared interruption policy for timeout/cancellation, and supports graceful shutdown with forced-kill escalation.
- `platform::edt_session` теперь содержит общий shared EDT actor/manager для CLI и MCP: host owner явно задаёт queue capacity, shutdown timeout, startup timeout, workspace и prewarm policy; actor держит одну lazy interactive session, снимает queued cancellation/timeout из внутренней FIFO, использует absolute deadline от enqueue-time, сохраняет baseline reset/probe, restart/shutdown drain и typed lifecycle errors.
- `mcp::edt_syntax` теперь остаётся только MCP-specific boundary над общим actor: он рендерит interactive `validate`, читает `--file` logs, сохраняет `issues_found`, когда parseable issues приходят вместе с interactive `stdout`, и маппит ошибки actor назад в существующий syntax-result/use-case boundary.
- `mcp::telemetry` now owns MCP runtime telemetry state and stable tracing contracts for semaphore admission wait, shared EDT queue depth, EDT startup failures, strict session restarts, and restart/shutdown drain stats.

This boundary is designed so Wave 2 can add an EDT-specific interactive runner without replacing the locator API or the standard execution path.

## Command Boundary

The CLI/runtime boundary is now split explicitly:

- `app.rs` owns bootstrap concerns only: config loading, logging setup, log cleanup, and top-level error envelopes for pre-command failures.
- `app.rs` now also branches early for `mcp serve stdio` and `mcp serve http`, because those paths must bypass CLI presenters and run with MCP-specific bootstrap/logging behavior.
- `cli::execute` converts `clap` args into transport-neutral request structs and renders command success/failure output.
- `cli::execute` also owns the CLI workspace lock boundary for commands that use `workPath`; nested flows call explicit unlocked internals only while the outer command owns the lock.
- CLI-only maintenance commands like `convert` live on the same adapter boundary and do not imply a matching MCP tool.
- `use_cases::{request,context,result}` define the transport-neutral contract that both CLI and future MCP adapters can consume.
- `use_cases/*.rs` no longer depend on `clap`, `Presenter`, or `Envelope`.
- Новые public CLI/MCP команды с runtime state под `workPath` должны сохранять этот boundary и проходить checklist из `docs/architecture/change-checklist.md`.

This keeps current CLI behavior intact while reserving a stable internal API for MCP stdio/HTTP adapters.
Workspace ownership is governed by [ADR-0011](docs/decisions/0011-eksklyuzivnoe-vladenie-workpath-na-vremya-komandy.md).

## Command Execution Policy

CLI and MCP commands must share the same timeout/cancellation semantics.
The target contract is that every public command has a deadline, cancellation is routed through a transport-neutral execution context, and a cancelled/timed-out operation is reported only after the underlying operation reaches a terminal state.
Mutating DB operations must mark critical phases where hard kill is not allowed by default.
Cancellation representation фиксируется на command boundary: фактическая terminal cancellation использует `ExecutionStatus::Cancelled`, а cancellation/shutdown/timeout внутри successful critical phase возвращается как `Succeeded` с warning, без per-step cancellation state machine.
This policy is governed by [ADR-0014](docs/decisions/0014-edinaya-timeout-cancellation-policy-dlya-cli-i-mcp-komand.md).

Runner-like and pipeline-like commands should be assembled in the use-case layer as transport-neutral pipelines of validation, target resolution, workspace preparation, platform execution, output parsing, publication, cleanup, and diagnostics blocks.
Those blocks exchange typed context/input/output, leave step entries for skipped/degraded/failure behavior, and report domain execution through `ExecutionOutcome<T>`.
This result grammar is governed by [ADR-0016](docs/decisions/0016-edinyy-executionoutcome-i-pipeline-steps-dlya-runner-like-stsenariev.md).

## Configuration Surface

`v8project.yaml`, loaded into `AppConfig` and accepted by `config::validate`, is the main project configuration contract.
`source-set.name` is a stable identity for runtime state, generated directories, diagnostics, and source-set selection.
The supported `source-set[].type` contract and validation boundary are governed by [ADR-0017](docs/decisions/0017-v8project-yaml-source-set-kak-glavnyy-konfiguratsionnyy-kontrakt.md).
`config init` must autodetect source-set types only from marker content: `Configuration.xml` / `.project` classify `CONFIGURATION` and `EXTENSION`, while external `.epf`/`.erf` sources are discovered only through homogeneous aggregate roots, never through per-artifact or phantom fallback source-set generation.

The typed config model now splits MCP knobs into active HTTP/session settings and shared execution guardrails:

- `mcp.http` defines the live HTTP listener and session behavior (`bind_address`, `path`, `stateful_sessions`, `max_sessions`, `idle_ttl_secs`).
- `mcp.execution` defines shared admission/shutdown limits (`max_concurrent_calls`, `shutdown_grace_period_secs`) reused by both stdio and HTTP.
- `tools.edt_cli` now also carries `startup_timeout_ms` and `command_timeout_ms`; the shared MCP EDT actor reuses these knobs for startup and bounded syntax execution.

This keeps the config surface stable while allowing both MCP transports to share the same execution/session infrastructure.
Новые public config fields, `source-set` types и `infobase` subtrees должны обновлять typed model, validation, `config init`, примеры и архитектурную документацию синхронно по checklist из `docs/architecture/change-checklist.md`.

## MCP Boundary

The MCP adapter no longer needs to talk to `cli::execute` or to reuse domain serialization directly.

- `mcp::request` defines raw tool-facing request DTOs.
- `mcp::service::McpService` maps those requests into `use_cases::request::*` and attaches per-call MCP transport metadata.
- `mcp::response` defines MCP-specific response DTOs, including nested step/test/issue structs that are decoupled from domain serialization details.
- `mcp::error` splits failures into `McpBusinessFailure<T>` for structured tool responses and `McpInternalError` for adapter/runtime misuse that must not be surfaced as business payloads.
- `mcp::tool_result` defines the structured transport payload returned by MCP tools for success vs business failure outcomes.
- `mcp::server::McpToolServer` is the shared rmcp handler used by both transports. It exposes tools-only capabilities, maps incoming `camelCase` params into MCP DTOs, gates every tool call through a global semaphore, calls the synchronous `McpService` via `tokio::task::spawn_blocking` for non-EDT tools, and routes live `check_syntax_edt` through `mcp::edt_syntax` plus the shared `EdtSessionManager`.
- `mcp::port` owns the MCP workspace lock boundary before dispatching requests into transport-neutral use cases; the global MCP semaphore remains an admission limit, not a replacement for per-`workPath` ownership.
- Изменение MCP tool surface должно оставаться явным архитектурным событием: список опубликованных tools синхронизируется между `src/mcp/server.rs`, `ADR-0005`, invariants и checklist-документом.
- MCP execution admission and HTTP session capacity are separate guardrails governed by [ADR-0013](docs/decisions/0013-mcp-execution-admission-timeout-cancellation-routing-i-http-session-capacity.md).
- MCP runtime telemetry is intentionally implemented as structured `tracing` events rather than a separate metrics backend: semaphore acquisition emits `mcp_execution_semaphore_wait`, while the shared EDT actor emits `mcp_edt_queue_depth`, `mcp_edt_startup_failure`, `mcp_edt_session_restart`, and `mcp_edt_shutdown_drain`.
- The stdio adapter still reserves `stdout` for MCP frames and enforces an absolute deadline for bounded EDT syntax calls: queue wait plus actor-side baseline/reset plus the interactive `validate` command all consume the same `tools.edt_cli.command_timeout_ms` budget.
- The HTTP adapter is built on `axum` + `rmcp::transport::StreamableHttpService`. A thin wrapper around the rmcp service enforces transport-level overload semantics for new `initialize` requests (`503` when `max_sessions` is exhausted), translates stateful non-`initialize` POSTs without `Mcp-Session-Id` into deterministic `400`, and eagerly releases tracked capacity after `DELETE`.
- HTTP session capacity is tracked via atomic reservation (`reserve -> delegate initialize -> confirm/release`) plus lazy pruning of expired rmcp sessions, so `max_sessions` remains correct across explicit close, TTL expiry, and failed initializes.
- Queued MCP cancellation/timeout still return early as transport-level admission errors. Detached one-shot work retains the server-side permit until completion, while live `check_syntax_edt` retains both the server-side permit and the shared actor's internal admission slot until the in-flight interactive command reaches terminal state and the server can return a structured tool result.
- MCP normalization is finalized in the service layer: dump-mode defaulting, launch alias mapping, `allExtensions` tri-state inference, and MCP-only pre-validation for syntax flag dependencies all live there instead of leaking into transport-neutral use cases.
- Общий shared actor применяет deterministic baseline contract перед каждой interactive EDT-командой: `cd <scenario EDT workspace>`, затем `cd`, который обязан вернуть тот же workspace path. Для `init` это обычно `workPath/edt-workspace`, для `convert` — `workPath/convert/edt-workspace`. Exhaustion request budget в этой pre-dispatch phase остаётся `QueuedTimeout`; reset/probe faults форсят session restart и queue drain.

Important staging note:

- Shared EDT actor теперь живёт в `platform` и используется всеми поддержанными interactive EDT сценариями: CLI `init`, EDT export в `build`, CLI `syntax edt` и live MCP `check_syntax_edt`.
- `tools.edt_cli.auto_start=true` остаётся eager prewarm только для long-lived host process вроде MCP server; short-lived CLI commands всегда стартуют shared EDT lazy и держат session только в рамках current command lifetime.
- `spec/MCP_IMPLEMENTATION_PLAN.md` remains the canonical staged MCP rollout history/reference for the closed Stage 1-5 MCP rollout; it is not the active backlog for follow-up EDT work.

## Backend Dispatch

`build` and `dump` use cases dispatch by `builder`:

- `builder=DESIGNER` uses the existing `DesignerDsl`.
- `builder=IBCMD` uses `IbcmdDsl` with `config import/apply` for build and `config export` for dump; for EDT build the EDT export step still produces Designer-format files first, and for EDT dump the reverse path first updates an internal Designer snapshot before EDT import/publication.
- Builder backends are expected to stay interchangeable for implemented builder scenarios. Functionality added for the Designer builder should also be available through the IBCMD builder, or the gap must be documented explicitly. Future Designer agent mode should be added behind the same use-case contract.
- Server infobase support is a target contract for all tools; file-only behavior must be documented as a current gap rather than treated as the permanent architecture.

Constraints to keep in mind:

- Граница поддержки `IBCMD` как ограниченного backend формально закреплена в [ADR-0001](docs/decisions/0001-granitsy-podderzhki-ibcmd-kak-ogranichennogo-backend.md).
- Для реализованных builder-сценариев `IBCMD` уже поддерживает file и server infobase connections; server path требует полный `infobase.dbms` contract. Оставшиеся file-only или unsupported сценарии считаются явными gaps, а не нормой архитектуры.
- `builder=DESIGNER` supports object-level partial dump via `/DumpConfigToFiles -partial -listFile`.
- `builder=IBCMD` does not support object-scoped partial dump directly; `PARTIAL` degrades to
  incremental export for the resolved target and returns a warning while preserving the requested
  mode in the result payload.
- `convert` is intentionally not a builder-dispatch scenario: it is a CLI-only repo-aware EDT-CLI conversion flow over configured `source-set` that stays independent from infobase/builder semantics.

## Dump And Artifact Publication

Full replacement outputs are published through a staging/backup contract governed by [ADR-0015](docs/decisions/0015-atomarnaya-publikatsiya-dump-artifacts-cherez-staging-backup.md).
Full dump writes to a sibling staging directory before replacing the resolved target directory.
Package artifacts write to a sibling staging file before replacing the output file, and external EPF/ERF publication stages the whole output directory before replacing it.
Incremental and partial dump modes remain direct non-atomic update modes.

## Output Flow

Use cases now return transport-neutral payloads or structured failures.

- `cli::execute` converts successful command payloads into `Envelope<T>` for JSON mode.
- `cli::execute` preserves command-specific text formatting for build, test, dump, convert, syntax, and launch.
- Failure payload emission is also decided at the adapter boundary, which keeps `launch --output json` failure semantics unchanged while allowing other commands to keep structured JSON failures.
- `mcp::service` returns MCP-specific DTOs and never reuses CLI `Envelope` or presenter logic.
- Runner-like command payloads use `ExecutionOutcome<T>` as their domain source of truth for status, diagnostics, structured errors, metrics, artifacts, and typed parsed payload; top-level command structs may keep compatibility fields while adapters migrate.

## Working Directories

`workPath` is the root for runtime artifacts:

- `workPath/logs/platform/` stores platform log files.
- `workPath/edt-workspace/` stores the shared EDT workspace used by `init`.
- `workPath/convert/edt-workspace/` stores the dedicated EDT workspace used by `convert`.
- `workPath/convert/out/<sourceSetName>/<designer|edt>/` stores deterministic generated convert outputs.
- `workPath/temp/partial-lists/` stores partial load and partial dump list files.
- `workPath/temp/yaxunit/` stores temporary YaXUnit config files.
- `workPath/hash-storages/` remains reserved for change detection state.
- `workPath/designer/<sourceSetName>/` is used by the EDT export/build flow as the generated Designer-format output area for a source-set.

The `source-set` and `workPath` state boundary is formalized in [ADR-0002](docs/decisions/0002-izolirovat-runtime-state-po-source-set-pod-workpath.md): `DESIGNER` format uses one `designer-<sourceSetName>` change-detection context, while `EDT` format uses both `edt-<sourceSetName>` for export decisions and `designer-<sourceSetName>` for load decisions.
Exclusive command ownership of `workPath` is governed by [ADR-0011](docs/decisions/0011-eksklyuzivnoe-vladenie-workpath-na-vremya-komandy.md).
On-demand change detection and conservative file-level partial load rules are governed by [ADR-0012](docs/decisions/0012-on-demand-change-detection-i-faylovaya-partial-load-strategiya.md).
