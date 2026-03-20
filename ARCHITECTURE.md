# Architecture

## Overview

`v8-test-runner` is a Rust CLI for orchestrating local 1C platform operations. The current codebase is organized into eight main layers:

1. `cli` parses arguments, maps them into transport-neutral requests, and owns command-level text/json rendering.
2. `config` loads and validates YAML configuration.
3. `domain` defines structured result types for commands plus shared execution step structs.
4. `use_cases` owns transport-neutral requests, `ExecutionContext`, structured failures, and business orchestration.
5. `mcp` now contains both the MCP-facing service boundary and the stdio transport adapter: it maps raw tool inputs into use-case requests, returns MCP-specific DTOs plus structured business/internal failures, and publishes the live stdio tool server.
6. `platform` contains process execution, utility discovery, connection argument building, and low-level 1C adapters.
7. `output` contains CLI presentation primitives such as `Presenter` and `Envelope`.
8. `change_detection`, `parsers`, and `support` provide shared subsystems and utilities.

## Current Platform Layer

The platform layer is intentionally split so responsibilities do not bleed into use cases:

- `platform::process` defines `ProcessRunner`, `ProcessExecutor`, `ProcessRequest`, `ProcessResult`, and `SpawnResult`.
- `platform::locator` resolves concrete executables (`1cv8`, `1cv8c`, `ibcmd`, `1cedtcli`) and caches results per `Locator` instance.
- `platform::connection` builds reusable V8 connection/auth arguments from the raw config connection string.
- `platform::utilities` is the current facade used by use cases. It owns the stateful `Locator` and exposes the standard execution path.
- `platform::designer` is the low-level batch DSL for `1cv8 DESIGNER`, returning `PlatformCommandResult` so `/Out` logs stay separate from runner-captured stdio.
- `platform::ibcmd` is the low-level DSL for `ibcmd`, returning `PlatformCommandResult` with stdout/stderr diagnostics (no `/Out` log).
- `platform::interactive` now contains the low-level `InteractiveProcessExecutor` for `1cedtcli`: it starts one child in its own process group, waits for the `1C:EDT>` prompt on stdout or stderr, executes prompt-delimited commands, kills/poisons the session on timeout, and supports graceful shutdown with forced-kill escalation.

This boundary is designed so Wave 2 can add an EDT-specific interactive runner without replacing the locator API or the standard execution path.

## Command Boundary

The CLI/runtime boundary is now split explicitly:

- `app.rs` owns bootstrap concerns only: config loading, logging setup, log cleanup, and top-level error envelopes for pre-command failures.
- `app.rs` now also branches early for `mcp serve stdio`, because that path must bypass CLI presenters and keep `stdout` reserved for protocol traffic.
- `cli::execute` converts `clap` args into transport-neutral request structs and renders command success/failure output.
- `use_cases::{request,context,result}` define the transport-neutral contract that both CLI and future MCP adapters can consume.
- `use_cases/*.rs` no longer depend on `clap`, `Presenter`, or `Envelope`.

This keeps current CLI behavior intact while reserving a stable internal API for MCP stdio/HTTP adapters.

## Configuration Surface

The typed config model now splits MCP knobs into already-wired stdio guardrails and future HTTP/session settings:

- `mcp.http` still defines listener/session defaults reserved for the future HTTP transport (`bind_address`, `path`, `stateful_sessions`, `max_sessions`, `idle_ttl_secs`).
- `mcp.execution` already defines active stdio guardrails (`max_concurrent_calls`, `shutdown_grace_period_secs`).
- `tools.edt_cli` now also carries `startup_timeout_ms` and `command_timeout_ms`; Stage 2 already uses `command_timeout_ms` as the bounded MCP deadline for `check_syntax_edt`, while the new interactive executor module establishes the startup/command timeout contract that the future shared EDT actor will reuse.

This keeps the config surface stable while letting Stage 2 stdio semantics ship without waiting for the later shared EDT actor.

## MCP Boundary

The MCP adapter no longer needs to talk to `cli::execute` or to reuse domain serialization directly.

- `mcp::request` defines raw tool-facing request DTOs.
- `mcp::service::McpService` maps those requests into `use_cases::request::*` and attaches per-call MCP transport metadata.
- `mcp::response` defines MCP-specific response DTOs, including nested step/test/issue structs that are decoupled from domain serialization details.
- `mcp::error` splits failures into `McpBusinessFailure<T>` for structured tool responses and `McpInternalError` for adapter/runtime misuse that must not be surfaced as business payloads.
- `mcp::tool_result` defines the structured transport payload returned by MCP tools for success vs business failure outcomes.
- `mcp::server::McpStdioServer` is the live rmcp stdio adapter. It exposes tools-only capabilities, maps incoming `camelCase` params into MCP DTOs, gates every tool call through a global semaphore, calls the synchronous `McpService` via `tokio::task::spawn_blocking`, and converts business/internal outcomes into MCP tool results.
- The stdio adapter now enforces an absolute deadline for bounded Stage 2 EDT syntax calls: queue wait and execution both consume the same `tools.edt_cli.command_timeout_ms` budget.
- Client cancellation returns early for queued and running MCP requests, but already-started blocking work is detached rather than killed; the detached task retains the semaphore permit until it completes, so a hung one-shot subprocess can keep capacity occupied until shutdown.
- MCP normalization is finalized in the service layer: dump-mode defaulting, launch alias mapping, `allExtensions` tri-state inference, and MCP-only pre-validation for syntax flag dependencies all live there instead of leaking into transport-neutral use cases.

Important staging note:

- The new interactive EDT executor is intentionally not wired into `EdtDsl`, `PlatformUtilities`, or the MCP server yet. Current runtime behavior for EDT syntax/build remains on the existing one-shot path until `EdtSessionManager` lands in the next Stage 3 tasks.

## Backend Dispatch

`build` and `dump` use cases dispatch by `builder`:

- `builder=DESIGNER` uses the existing `DesignerDsl`.
- `builder=IBCMD` uses `IbcmdDsl` with `config import/apply` for build and `config export` for dump.

Constraints to keep in mind:

- IBCMD requires file-based infobase connections.
- `builder=DESIGNER` supports object-level partial dump via `/DumpConfigToFiles -partial -listFile`.
- `builder=IBCMD` does not support object-scoped partial dump directly; `PARTIAL` degrades to
  incremental export for the resolved target and returns a warning while preserving the requested
  mode in the result payload.

## Output Flow

Use cases now return transport-neutral payloads or structured failures.

- `cli::execute` converts successful command payloads into `Envelope<T>` for JSON mode.
- `cli::execute` preserves command-specific text formatting for build, test, dump, syntax, and launch.
- Failure payload emission is also decided at the adapter boundary, which keeps `launch --output json` failure semantics unchanged while allowing other commands to keep structured JSON failures.
- `mcp::service` returns MCP-specific DTOs and never reuses CLI `Envelope` or presenter logic.

## Working Directories

`workPath` is the root for runtime artifacts:

- `workPath/logs/platform/` stores platform log files.
- `workPath/temp/partial-lists/` stores partial load and partial dump list files.
- `workPath/temp/yaxunit/` stores temporary YaXUnit config files.
- `workPath/hash-storages/` remains reserved for change detection state.
- `workPath/<sourceSetName>/` is reserved for the future EDT export flow and is not created yet.
