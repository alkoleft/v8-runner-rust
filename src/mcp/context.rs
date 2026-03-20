use std::time::Duration;

use crate::use_cases::context::ExecutionTransport;

/// Per-call metadata passed into the MCP service layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpCallContext {
    transport: ExecutionTransport,
    edt_timeout: Option<Duration>,
}

impl McpCallContext {
    /// Creates a new MCP call context for the specified transport.
    pub const fn new(transport: ExecutionTransport) -> Self {
        Self {
            transport,
            edt_timeout: None,
        }
    }

    /// Creates a stdio MCP call context.
    pub const fn stdio() -> Self {
        Self::new(ExecutionTransport::McpStdio)
    }

    /// Creates an HTTP MCP call context.
    pub const fn http() -> Self {
        Self::new(ExecutionTransport::McpHttp)
    }

    /// Returns the originating transport.
    pub const fn transport(self) -> ExecutionTransport {
        self.transport
    }

    /// Attaches an EDT subprocess timeout budget for this call.
    pub const fn with_edt_timeout(self, edt_timeout: Option<Duration>) -> Self {
        Self {
            transport: self.transport,
            edt_timeout,
        }
    }

    /// Returns the EDT subprocess timeout budget for this call, if any.
    pub const fn edt_timeout(self) -> Option<Duration> {
        self.edt_timeout
    }
}
