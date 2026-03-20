use std::time::Duration;

/// Identifies the logical command being executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandName {
    Build,
    Test,
    Dump,
    Syntax,
    Launch,
}

impl CommandName {
    /// Returns the stable command label used in logs and CLI envelopes.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Test => "test",
            Self::Dump => "dump",
            Self::Syntax => "syntax",
            Self::Launch => "launch",
        }
    }
}

/// Describes the transport that invoked the use case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionTransport {
    /// Invocation from the existing CLI surface.
    Cli,
    /// Invocation from MCP over stdio.
    McpStdio,
    /// Invocation from MCP over HTTP.
    McpHttp,
}

/// Per-invocation metadata passed into transport-neutral use cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionContext {
    command: CommandName,
    transport: ExecutionTransport,
    edt_timeout: Option<Duration>,
}

impl ExecutionContext {
    /// Creates an execution context for the specified command and transport.
    pub const fn new(command: CommandName, transport: ExecutionTransport) -> Self {
        Self {
            command,
            transport,
            edt_timeout: None,
        }
    }

    /// Creates a CLI execution context for the specified command.
    pub const fn cli(command: CommandName) -> Self {
        Self::new(command, ExecutionTransport::Cli)
    }

    /// Creates an MCP stdio execution context for the specified command.
    pub const fn mcp_stdio(command: CommandName) -> Self {
        Self::new(command, ExecutionTransport::McpStdio)
    }

    /// Creates an MCP HTTP execution context for the specified command.
    pub const fn mcp_http(command: CommandName) -> Self {
        Self::new(command, ExecutionTransport::McpHttp)
    }

    /// Returns the command being executed.
    pub const fn command(self) -> CommandName {
        self.command
    }

    /// Returns the transport that initiated this execution.
    pub const fn transport(self) -> ExecutionTransport {
        self.transport
    }

    /// Attaches an EDT subprocess timeout budget to the execution context.
    pub const fn with_edt_timeout(self, edt_timeout: Option<Duration>) -> Self {
        Self {
            command: self.command,
            transport: self.transport,
            edt_timeout,
        }
    }

    /// Returns the EDT subprocess timeout budget for this execution.
    pub const fn edt_timeout(self) -> Option<Duration> {
        self.edt_timeout
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{CommandName, ExecutionContext, ExecutionTransport};

    #[test]
    fn constructs_mcp_contexts() {
        let stdio = ExecutionContext::mcp_stdio(CommandName::Build)
            .with_edt_timeout(Some(Duration::from_secs(5)));
        let http = ExecutionContext::mcp_http(CommandName::Test);

        assert_eq!(stdio.command(), CommandName::Build);
        assert_eq!(stdio.transport(), ExecutionTransport::McpStdio);
        assert_eq!(stdio.edt_timeout(), Some(Duration::from_secs(5)));
        assert_eq!(http.command(), CommandName::Test);
        assert_eq!(http.transport(), ExecutionTransport::McpHttp);
        assert_eq!(http.edt_timeout(), None);
    }
}
