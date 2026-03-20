/// Per-call MCP service context.
pub mod context;
/// Shared EDT actor reserved for MCP transports.
pub mod edt_session;
/// MCP-specific EDT syntax execution over the shared actor.
pub mod edt_syntax;
/// MCP-facing error and result contracts.
pub mod error;
/// Thin port used by the MCP service layer to call use cases.
pub mod port;
/// MCP-facing request DTOs.
pub mod request;
/// MCP-facing response DTOs.
pub mod response;
/// MCP stdio and HTTP transport adapters.
pub mod server;
/// MCP-facing service layer over transport-neutral use cases.
pub mod service;
/// Structured tool payloads used by MCP transports.
pub mod tool_result;
