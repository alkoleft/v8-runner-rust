use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::platform::process::{ProcessExecutionPolicy, ProcessInterruptionSafety};

/// Identifies the logical command being executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandName {
    ToolsDownload,
    Init,
    Extensions,
    Build,
    Load,
    Test,
    Dump,
    Convert,
    Artifacts,
    Syntax,
    Launch,
}

impl CommandName {
    /// Returns the stable command label used in logs and CLI envelopes.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ToolsDownload => "tools download",
            Self::Init => "init",
            Self::Extensions => "extensions",
            Self::Build => "build",
            Self::Load => "load",
            Self::Test => "test",
            Self::Dump => "dump",
            Self::Convert => "convert",
            Self::Artifacts => "make",
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

/// Command-boundary interruption signal observed at safe points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionInterruption {
    Cancelled,
    TimedOut,
}

impl ExecutionInterruption {
    pub const fn message(self, command: CommandName) -> &'static str {
        match (self, command) {
            (Self::Cancelled, _) => "execution cancelled before reaching a safe completion point",
            (Self::TimedOut, _) => {
                "execution timeout expired before reaching a safe completion point"
            }
        }
    }
}

/// Command-level interruption safety contract from ADR-0014.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptionSafetyClass {
    Interruptible,
    GracefulThenKill,
    CriticalNonAbortable,
    NoExternalProcess,
}

impl InterruptionSafetyClass {
    /// Returns the subset of process-runner safety semantics for commands that spawn a child.
    pub const fn process_safety(self) -> ProcessInterruptionSafety {
        match self {
            Self::Interruptible => ProcessInterruptionSafety::Interruptible,
            Self::GracefulThenKill => ProcessInterruptionSafety::GracefulThenKill,
            Self::CriticalNonAbortable | Self::NoExternalProcess => {
                ProcessInterruptionSafety::CriticalNonAbortable
            }
        }
    }
}

/// Result of a critical in-process phase that must complete without mid-phase aborts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CriticalPhaseResult<T> {
    pub value: T,
    pub deferred_interruption: Option<ExecutionInterruption>,
}

/// Per-invocation metadata passed into transport-neutral use cases.
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    command: CommandName,
    transport: ExecutionTransport,
    edt_timeout: Option<Duration>,
    deadline: Option<Instant>,
    cancellation: CancellationToken,
}

impl ExecutionContext {
    /// Creates an execution context for the specified command and transport.
    pub fn new(command: CommandName, transport: ExecutionTransport) -> Self {
        Self {
            command,
            transport,
            edt_timeout: None,
            deadline: None,
            cancellation: CancellationToken::new(),
        }
    }

    /// Creates a CLI execution context for the specified command.
    pub fn cli(command: CommandName) -> Self {
        Self::new(command, ExecutionTransport::Cli)
    }

    /// Creates an MCP stdio execution context for the specified command.
    #[cfg(test)]
    pub fn mcp_stdio(command: CommandName) -> Self {
        Self::new(command, ExecutionTransport::McpStdio)
    }

    /// Creates an MCP HTTP execution context for the specified command.
    #[cfg(test)]
    pub fn mcp_http(command: CommandName) -> Self {
        Self::new(command, ExecutionTransport::McpHttp)
    }

    /// Returns the command being executed.
    pub const fn command(&self) -> CommandName {
        self.command
    }

    /// Returns the transport that initiated this execution.
    pub const fn transport(&self) -> ExecutionTransport {
        self.transport
    }

    /// Attaches an EDT subprocess timeout budget to the execution context.
    pub fn with_edt_timeout(mut self, edt_timeout: Option<Duration>) -> Self {
        self.edt_timeout = edt_timeout;
        self
    }

    /// Attaches an absolute execution deadline to the context.
    pub fn with_deadline(mut self, deadline: Option<Instant>) -> Self {
        self.deadline = deadline;
        self
    }

    /// Attaches a cancellation token shared with the caller transport.
    pub fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }

    /// Returns the EDT subprocess timeout budget for this execution.
    pub const fn edt_timeout(&self) -> Option<Duration> {
        self.edt_timeout
    }

    /// Returns the remaining command budget from the current moment.
    pub fn remaining_budget(&self) -> Option<Duration> {
        self.deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }

    /// Returns the shared cancellation token for this execution.
    pub fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    /// Builds a process policy capped by the remaining command budget.
    pub fn process_policy(
        &self,
        safety: InterruptionSafetyClass,
        timeout_cap: Option<Duration>,
    ) -> ProcessExecutionPolicy {
        let timeout = match (timeout_cap, self.remaining_budget()) {
            (Some(cap), Some(remaining)) => Some(cap.min(remaining)),
            (Some(cap), None) => Some(cap),
            (None, Some(remaining)) => Some(remaining),
            (None, None) => None,
        };

        ProcessExecutionPolicy::new(timeout, self.cancellation(), safety.process_safety())
    }

    /// Returns the pending command-boundary interruption, if any.
    pub fn interruption(&self) -> Option<ExecutionInterruption> {
        if self
            .deadline
            .is_some_and(|deadline| deadline <= Instant::now())
        {
            Some(ExecutionInterruption::TimedOut)
        } else if self.cancellation.is_cancelled() {
            Some(ExecutionInterruption::Cancelled)
        } else {
            None
        }
    }

    /// Runs a non-process critical phase and reports whether interruption was deferred until
    /// after the operation completed.
    pub fn run_no_process_critical_phase<T, E>(
        &self,
        operation: impl FnOnce() -> Result<T, E>,
    ) -> Result<CriticalPhaseResult<T>, E> {
        let value = operation()?;
        Ok(CriticalPhaseResult {
            value,
            deferred_interruption: self.interruption(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use tokio_util::sync::CancellationToken;

    use crate::platform::process::ProcessInterruptionSafety;

    use super::{
        CommandName, ExecutionContext, ExecutionInterruption, ExecutionTransport,
        InterruptionSafetyClass,
    };

    #[test]
    fn constructs_mcp_contexts() {
        let cancellation = CancellationToken::new();
        let stdio = ExecutionContext::mcp_stdio(CommandName::Build)
            .with_edt_timeout(Some(Duration::from_secs(5)))
            .with_cancellation(cancellation.clone());
        let http = ExecutionContext::mcp_http(CommandName::Test);

        assert_eq!(stdio.command(), CommandName::Build);
        assert_eq!(stdio.transport(), ExecutionTransport::McpStdio);
        assert_eq!(stdio.edt_timeout(), Some(Duration::from_secs(5)));
        assert_eq!(http.command(), CommandName::Test);
        assert_eq!(http.transport(), ExecutionTransport::McpHttp);
        assert_eq!(http.edt_timeout(), None);
        assert_eq!(stdio.interruption(), None);
        cancellation.cancel();
        assert_eq!(stdio.interruption(), Some(ExecutionInterruption::Cancelled));
    }

    #[test]
    fn deadline_wins_over_cancellation_when_both_are_present() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let context = ExecutionContext::cli(CommandName::Test)
            .with_cancellation(cancellation)
            .with_deadline(Some(Instant::now() - Duration::from_millis(1)));

        assert_eq!(
            context.interruption(),
            Some(ExecutionInterruption::TimedOut)
        );
    }

    #[test]
    fn process_policy_caps_timeout_by_remaining_budget() {
        let context = ExecutionContext::cli(CommandName::Build)
            .with_deadline(Some(Instant::now() + Duration::from_millis(25)));

        let policy = context.process_policy(
            InterruptionSafetyClass::GracefulThenKill,
            Some(Duration::from_millis(100)),
        );

        assert!(policy.timeout.expect("timeout") <= Duration::from_millis(25));
        assert_eq!(policy.safety, ProcessInterruptionSafety::GracefulThenKill);
    }

    #[test]
    fn no_process_critical_phase_reports_deferred_cancellation() {
        let cancellation = CancellationToken::new();
        let context =
            ExecutionContext::cli(CommandName::Artifacts).with_cancellation(cancellation.clone());

        let result = context
            .run_no_process_critical_phase(|| {
                cancellation.cancel();
                Ok::<_, ()>("published")
            })
            .expect("critical phase");

        assert_eq!(result.value, "published");
        assert_eq!(
            result.deferred_interruption,
            Some(ExecutionInterruption::Cancelled)
        );
    }
}
