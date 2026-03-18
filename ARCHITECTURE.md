# Architecture

## Overview

`v8-test-runner` is a Rust CLI for orchestrating local 1C platform operations. The current codebase is organized into six main layers:

1. `cli` parses arguments and defines the public command surface.
2. `config` loads and validates YAML configuration.
3. `domain` defines structured result types for commands.
4. `platform` contains process execution, utility discovery, connection argument building, and low-level 1C adapters.
5. `use_cases` coordinates command execution and presentation.
6. `change_detection`, `parsers`, and `support` provide shared subsystems and utilities.

## Current Platform Layer

The platform layer is intentionally split so responsibilities do not bleed into use cases:

- `platform::process` defines `ProcessRunner`, `ProcessExecutor`, `ProcessRequest`, `ProcessResult`, and `SpawnResult`.
- `platform::locator` resolves concrete executables (`1cv8`, `1cv8c`, `ibcmd`, `1cedtcli`) and caches results per `Locator` instance.
- `platform::connection` builds reusable V8 connection/auth arguments from the raw config connection string.
- `platform::utilities` is the current facade used by use cases. It owns the stateful `Locator` and exposes the standard execution path.
- `platform::designer` is the low-level batch DSL for `1cv8 DESIGNER`, returning `PlatformCommandResult` so `/Out` logs stay separate from runner-captured stdio.

This boundary is designed so Wave 2 can add an EDT-specific interactive runner without replacing the locator API or the standard execution path.

## Output Flow

Use cases produce structured domain results and hand them to the presenter:

- JSON mode emits the common `Envelope<T>`.
- Text mode stays command-specific and is currently formatted directly in the use case when richer human-readable output is needed, such as `launch`.

## Working Directories

`workPath` is the root for runtime artifacts:

- `workPath/logs/platform/` stores platform log files.
- `workPath/temp/partial-lists/` stores partial load list files.
- `workPath/temp/yaxunit/` stores temporary YaXUnit config files.
- `workPath/hash-storages/` remains reserved for change detection state.
- `workPath/<sourceSetName>/` is reserved for the future EDT export flow and is not created yet.
