# v8-test-runner

Rust CLI for local 1C development workflows.

## Build

Current `build` support is limited to `builder=DESIGNER` and `format=DESIGNER`.

- `v8-test-runner build` runs change detection and loads only affected `source-set` entries.
- `v8-test-runner build --full-rebuild` bypasses change detection and forces full load for every Designer `source-set`.
- Execution order is always the main `CONFIGURATION` first, then extensions in config order.
- Build is intentionally non-atomic across `source-set`: if a later step fails, earlier successful steps remain applied.

Optional YAML settings:

```yaml
build:
  partialLoadThreshold: 20
```

- `partialLoadThreshold` controls when partial load falls back to full load.
- `Configuration.xml` changes and deletions always force a full load.

## Dump

Current `dump` support is limited to `format=DESIGNER` with either `builder=DESIGNER` or
`builder=IBCMD`.

- `v8-test-runner dump --mode full` performs a full export.
- `v8-test-runner dump --mode incremental` exports only changes according to backend semantics.
- `v8-test-runner dump --mode partial --object <OBJECT>` performs object-scoped partial dump for
  `builder=DESIGNER`.
- `v8-test-runner dump --mode partial --object <OBJECT>` on `builder=IBCMD` degrades to
  incremental export for the resolved configuration/extension target and returns a warning while
  keeping the requested mode as `PARTIAL`.
- `partial` requires at least one `--object`; blank values and control characters are rejected.

## Tests

Current `test` support is limited to `builder=DESIGNER` and `format=DESIGNER`.

- `v8-test-runner test all` always runs `build` first, then launches YaXUnit via `1cv8c`.
- `v8-test-runner test module <MODULE_NAME>` does the same, but writes `filter.modules = ["<MODULE_NAME>"]` into the temporary YaXUnit config.
- `v8-test-runner test --full ...` keeps passed test cases and full stack traces.
- Compact mode hides passed cases and truncates stack traces.
- If the run fails or the JUnit report cannot be parsed, sanitized retained artifacts stay under `workPath/temp/yaxunit/runs/<run-id>/`.
- YaXUnit must already be installed and callable from the target infobase.

Optional YAML settings:

```yaml
tests:
  execution_timeout_seconds: 300
```

- `execution_timeout_seconds` controls the hard timeout for the Enterprise test run.

## Internal Boundary

The CLI now uses a transport-neutral use-case contract internally:

- `cli::execute` maps parsed CLI args into request DTOs and renders the final text/json output.
- `use_cases` no longer depend on `clap`, `Presenter`, or `Envelope`.
- `mcp` now adds a separate internal service boundary with MCP-specific request/response DTOs and explicit business-vs-internal failure split.
- MCP request normalization is isolated in the service layer instead of leaking into use cases.
- MCP now fixes these transport-level semantics explicitly:
  - `dump_config(mode=null|blank)` resolves to `INCREMENTAL` starting on `2026-03-20`.
  - `launch_app` accepts the Kotlin alias set plus the already supported `thin` / `thick` aliases, with trim + lowercase normalization.
  - `allExtensions` is treated as tri-state in MCP request mapping, with the default inferred from whether `extension` is present.
  - `checkUseSynchronousCalls` and `checkUseModality` are rejected at the MCP boundary when `extendedModulesCheck=false`.
- This now drives both the stdio and HTTP MCP adapters without changing the public CLI surface.

## MCP Configuration

The config model now drives both MCP transports directly.

Optional YAML settings:

```yaml
mcp:
  http:
    bind_address: 127.0.0.1:3000
    path: /mcp
    stateful_sessions: true
    max_sessions: 64
    idle_ttl_secs: 900
  execution:
    max_concurrent_calls: 1
    shutdown_grace_period_secs: 30

tools:
  edt_cli:
    startup_timeout_ms: 300000
    command_timeout_ms: 300000
```

- `mcp.http.*` now configures the live streamable HTTP listener and session semantics.
- `mcp.execution.*` still drives shared admission control and shutdown grace for both MCP transports.
- `tools.edt_cli.startup_timeout_ms` and `tools.edt_cli.command_timeout_ms` default to `300000` ms and also accept legacy `edt-cli` / kebab-case aliases for compatibility.
- `src/platform/interactive.rs` contains the low-level `InteractiveProcessExecutor` used by the shared MCP EDT session: it waits for the `1C:EDT>` prompt, executes prompt-delimited commands, and supports graceful shutdown plus forced kill.
- The shared MCP EDT session now performs a reset+probe before every live MCP `check_syntax_edt` command: `cd <workPath/edt-workspace>` followed by `cd`, which must echo the same workspace path.
- Baseline timeout semantics are split intentionally: request-budget exhaustion returns queued timeout, while a reset/probe fault under the internal baseline cap is treated as a fatal session failure that forces lazy restart on the next call.

## MCP Stdio

The binary now exposes a live stdio MCP transport:

- `v8-test-runner mcp serve stdio`
- The stdio server is built on `rmcp` and advertises tools capability only.
- The published tool set is: `run_all_tests`, `run_module_tests`, `build_project`, `dump_config`, `launch_app`, `check_syntax_edt`, `check_syntax_designer_config`, `check_syntax_designer_modules`.
- MCP requests use the Kotlin-compatible `camelCase` argument surface (`fullRebuild`, `moduleName`, `utilityType`, `projectName`, `allExtensions`, `checkUseSynchronousCalls`, `checkUseModality`, ...).
- Business failures are returned as structured MCP tool errors; internal adapter/runtime failures stay transport-level.
- `stdout` is reserved for MCP frames on this path: tracing goes to `workPath/logs/mcp/actions.log`, bootstrap failures go to `stderr`, and spawned platform processes stay captured or null-routed.
- All MCP tool calls now share a global semaphore governed by `mcp.execution.max_concurrent_calls`.
- `check_syntax_edt` is now executed through the shared interactive EDT actor instead of spawning a fresh `1cedtcli` process per call; CLI EDT execution still uses the existing one-shot path.
- Interactive EDT `stdout` no longer downgrades parseable `--file` issues into `tool_failed`: `stdout + issues` maps to `issues_found`, while `stdout` without parseable issues and any non-empty `stderr` still surface as `tool_failed`.
- For bounded EDT MCP calls (`check_syntax_edt`), the timeout deadline still starts when the request enters the MCP server wrapper, so queue wait and execution time consume the same `tools.edt_cli.command_timeout_ms` budget.
- Client cancellation now returns early for queued and already-running MCP tool calls. This early return does not kill already-started one-shot work; detached one-shot tools keep their server-side slot until they naturally finish, while live `check_syntax_edt` retains both the MCP execution slot and the actor admission slot until the in-flight interactive command finishes.
- The MCP integration suite now covers all 8 published tools on the stdio path, explicit `dump_config(mode=PARTIAL)` regressions for both `DESIGNER` and `IBCMD`, and schema-level `tools/list` contract checks.
- Transport-level error semantics are fixed as:
- `queued cancel` => `reason=cancelled`, `stage=queued`, `timeoutMs=null|<budget>`
- `running cancel` => `reason=cancelled`, `stage=running`, `timeoutMs=null|<budget>`
- `queued timeout` => bounded calls only, `reason=timeout`, `stage=queued`, `timeoutMs=<budget>`
- `running timeout` => bounded calls only, `reason=timeout`, `stage=running`, `timeoutMs=<budget>`
- With `rmcp` cancellable request handles, the client may observe local `ServiceError::Cancelled { reason }` after sending `notifications/cancelled`; the `reason/stage/timeoutMs` matrix above describes the server-side transport error payload shape.

## MCP HTTP

The binary now also exposes a live streamable HTTP MCP transport:

- `v8-test-runner mcp serve http`
- The HTTP server is built on `axum` + `rmcp` streamable HTTP and is served at `mcp.http.bind_address` plus `mcp.http.path`.
- The same 8 published tools are available over HTTP, with the same `camelCase` request surface and the same structured business-vs-transport error split as stdio.
- Stateful mode is enabled by default:
  - `POST initialize` returns `200` plus `Mcp-Session-Id`
  - `POST notifications/initialized` on a live session returns `202`
  - `GET` or `DELETE` without `Mcp-Session-Id` returns `400`
  - unknown, expired, or explicitly deleted session IDs return `404`
  - `DELETE` closes the live session immediately
- `mcp.http.max_sessions` is enforced before new session creation: a new `initialize` beyond capacity returns `503 Service Unavailable`.
- `mcp.http.idle_ttl_secs` maps to session inactivity timeout. Expired sessions are removed and their capacity becomes available to new `initialize` calls.
- `mcp.http.stateful_sessions=false` switches the transport into POST-only stateless mode: no session header is issued and `GET` / `DELETE` return `405 Method Not Allowed`.
- The HTTP path reuses the same global semaphore and the same shared interactive EDT actor as stdio. Separate HTTP MCP sessions do not spawn separate `1cedtcli` processes.
- The HTTP integration suite now also covers a live non-EDT tool call and a burst `initialize` admission/recovery scenario on top of the existing session lifecycle tests.

Current limits:

- The shared interactive EDT actor is currently wired only for MCP `check_syntax_edt`; broader EDT export/build rollout is still staged separately.
- Transport-level cancellation is still early-return only: if the underlying blocking work hangs, it can keep capacity occupied until it exits or the server shuts down.

## MCP Telemetry

MCP runtime telemetry is now emitted into the existing MCP action log (`workPath/logs/mcp/actions.log` for MCP transports) as structured tracing events:

- `mcp_execution_semaphore_wait` records global MCP semaphore admission wait with `transport`, `tool`, `outcome=acquired|cancelled|timeout|internal_error`, `bounded`, `timeout_ms`, and `wait_ms`.
- `mcp_edt_queue_depth` records shared EDT actor queue mutations with `action=enqueue|dequeue|remove_queued|drain`, `queue_depth`, and optional `reason=queued_cancelled|queued_timeout|restart|shutdown`.
- `mcp_edt_session_restart` increments only when a live EDT session is actually killed and restarted after a fatal session fault.
- `mcp_edt_startup_failure` is tracked separately so startup failures do not inflate restart metrics.
- `mcp_edt_shutdown_drain` records restart/shutdown drain totals and `drained_jobs`.

This stage intentionally keeps telemetry on top of `tracing` rather than adding a separate metrics backend, so MCP stdout semantics remain unchanged.

Planning note: `spec/MCP_IMPLEMENTATION_PLAN.md` remains the canonical staged MCP rollout history/reference for the closed Stage 1-5 MCP rollout, while active follow-up backlog items now live in `spec/IMPLEMENTATION_TODO.md`.
