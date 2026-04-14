/// Build orchestration use case.
pub mod build_project;
/// Syntax-check orchestration use case.
pub mod check_syntax;
/// Extension properties orchestration use case.
pub mod configure_extensions;
/// Per-invocation execution metadata shared across transports.
pub mod context;
/// Dump orchestration use case.
pub mod dump_config;
/// Shared formatting helpers for IBCMD diagnostics.
pub mod ibcmd_diagnostics;
/// Init orchestration use case.
pub mod init_project;
/// Launch orchestration use case.
pub mod launch_app;
/// Transport-neutral request DTOs consumed by use cases.
pub mod request;
/// Transport-neutral use-case error and failure contracts.
pub mod result;
/// Test orchestration use case.
pub mod run_tests;
/// Shared locking for commands that mutate the same workspace.
pub mod workspace_lock;
