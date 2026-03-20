use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tracing::info;

use crate::use_cases::context::ExecutionTransport;

#[derive(Debug, Clone)]
pub(crate) struct McpTelemetry {
    execution: Arc<ExecutionTelemetry>,
    edt: Arc<EdtTelemetry>,
}

impl Default for McpTelemetry {
    fn default() -> Self {
        Self {
            execution: Arc::new(ExecutionTelemetry::default()),
            edt: Arc::new(EdtTelemetry::default()),
        }
    }
}

impl McpTelemetry {
    pub(crate) fn execution(&self) -> Arc<ExecutionTelemetry> {
        self.execution.clone()
    }

    pub(crate) fn edt(&self) -> Arc<EdtTelemetry> {
        self.edt.clone()
    }
}

#[derive(Debug, Default)]
pub(crate) struct ExecutionTelemetry {
    acquired_total: AtomicU64,
    cancelled_total: AtomicU64,
    timeout_total: AtomicU64,
    internal_error_total: AtomicU64,
}

impl ExecutionTelemetry {
    pub(crate) fn record_semaphore_wait(
        &self,
        transport: ExecutionTransport,
        tool: &'static str,
        outcome: SemaphoreWaitOutcome,
        bounded: bool,
        timeout: Option<Duration>,
        wait: Duration,
        error_kind: Option<SemaphoreWaitErrorKind>,
    ) {
        match outcome {
            SemaphoreWaitOutcome::Acquired => {
                self.acquired_total.fetch_add(1, Ordering::Relaxed);
            }
            SemaphoreWaitOutcome::Cancelled => {
                self.cancelled_total.fetch_add(1, Ordering::Relaxed);
            }
            SemaphoreWaitOutcome::Timeout => {
                self.timeout_total.fetch_add(1, Ordering::Relaxed);
            }
            SemaphoreWaitOutcome::InternalError => {
                self.internal_error_total.fetch_add(1, Ordering::Relaxed);
            }
        }

        info!(
            event = "mcp_execution_semaphore_wait",
            transport = transport_name(transport),
            tool,
            outcome = outcome.as_str(),
            bounded,
            timeout_ms = timeout.map(duration_millis),
            wait_ms = duration_millis(wait),
            error_kind = error_kind.map(SemaphoreWaitErrorKind::as_str),
            "recorded MCP execution semaphore wait"
        );
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> ExecutionTelemetrySnapshot {
        ExecutionTelemetrySnapshot {
            acquired_total: self.acquired_total.load(Ordering::Relaxed),
            cancelled_total: self.cancelled_total.load(Ordering::Relaxed),
            timeout_total: self.timeout_total.load(Ordering::Relaxed),
            internal_error_total: self.internal_error_total.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SemaphoreWaitOutcome {
    Acquired,
    Cancelled,
    Timeout,
    InternalError,
}

impl SemaphoreWaitOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Acquired => "acquired",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::InternalError => "internal_error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SemaphoreWaitErrorKind {
    SemaphoreClosed,
}

impl SemaphoreWaitErrorKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::SemaphoreClosed => "semaphore_closed",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct EdtTelemetry {
    queue_depth: AtomicUsize,
    max_queue_depth: AtomicUsize,
    startup_failure_total: AtomicU64,
    restart_total: AtomicU64,
    drain_restart_total: AtomicU64,
    drain_shutdown_total: AtomicU64,
    last_drained_jobs: AtomicUsize,
}

impl EdtTelemetry {
    pub(crate) fn record_queue_depth(
        &self,
        action: EdtQueueDepthAction,
        queue_depth: usize,
        reason: Option<EdtQueueDepthReason>,
    ) {
        self.queue_depth.store(queue_depth, Ordering::Relaxed);
        self.max_queue_depth
            .fetch_max(queue_depth, Ordering::Relaxed);
        info!(
            event = "mcp_edt_queue_depth",
            action = action.as_str(),
            queue_depth,
            reason = reason.map(EdtQueueDepthReason::as_str),
            "recorded shared EDT queue depth"
        );
    }

    pub(crate) fn record_startup_failure(&self) {
        let startup_failure_total = self.startup_failure_total.fetch_add(1, Ordering::Relaxed) + 1;
        info!(
            event = "mcp_edt_startup_failure",
            startup_failure_total, "recorded shared EDT startup failure"
        );
    }

    pub(crate) fn record_restart(&self, reason: EdtRestartReason) {
        let restart_count = self.restart_total.fetch_add(1, Ordering::Relaxed) + 1;
        info!(
            event = "mcp_edt_session_restart",
            restart_count,
            reason = reason.as_str(),
            "recorded shared EDT session restart"
        );
    }

    pub(crate) fn record_drain(&self, reason: EdtDrainTelemetryReason, drained_jobs: usize) {
        self.last_drained_jobs
            .store(drained_jobs, Ordering::Relaxed);
        let (drain_restart_total, drain_shutdown_total) = match reason {
            EdtDrainTelemetryReason::Restart => (
                self.drain_restart_total.fetch_add(1, Ordering::Relaxed) + 1,
                self.drain_shutdown_total.load(Ordering::Relaxed),
            ),
            EdtDrainTelemetryReason::Shutdown => (
                self.drain_restart_total.load(Ordering::Relaxed),
                self.drain_shutdown_total.fetch_add(1, Ordering::Relaxed) + 1,
            ),
        };
        info!(
            event = "mcp_edt_shutdown_drain",
            reason = reason.as_str(),
            drained_jobs,
            drain_restart_total,
            drain_shutdown_total,
            "recorded shared EDT queue drain"
        );
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> EdtTelemetrySnapshot {
        EdtTelemetrySnapshot {
            queue_depth: self.queue_depth.load(Ordering::Relaxed),
            max_queue_depth: self.max_queue_depth.load(Ordering::Relaxed),
            startup_failure_total: self.startup_failure_total.load(Ordering::Relaxed),
            restart_total: self.restart_total.load(Ordering::Relaxed),
            drain_restart_total: self.drain_restart_total.load(Ordering::Relaxed),
            drain_shutdown_total: self.drain_shutdown_total.load(Ordering::Relaxed),
            last_drained_jobs: self.last_drained_jobs.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdtQueueDepthAction {
    Enqueue,
    Dequeue,
    RemoveQueued,
    Drain,
}

impl EdtQueueDepthAction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Enqueue => "enqueue",
            Self::Dequeue => "dequeue",
            Self::RemoveQueued => "remove_queued",
            Self::Drain => "drain",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdtQueueDepthReason {
    QueuedCancelled,
    QueuedTimeout,
    Restart,
    Shutdown,
}

impl EdtQueueDepthReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::QueuedCancelled => "queued_cancelled",
            Self::QueuedTimeout => "queued_timeout",
            Self::Restart => "restart",
            Self::Shutdown => "shutdown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdtRestartReason {
    BaselineFailure,
    CommandTimeout,
    SessionFailure,
}

impl EdtRestartReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::BaselineFailure => "baseline_failure",
            Self::CommandTimeout => "command_timeout",
            Self::SessionFailure => "session_failure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdtDrainTelemetryReason {
    Restart,
    Shutdown,
}

impl EdtDrainTelemetryReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::Shutdown => "shutdown",
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExecutionTelemetrySnapshot {
    pub(crate) acquired_total: u64,
    pub(crate) cancelled_total: u64,
    pub(crate) timeout_total: u64,
    pub(crate) internal_error_total: u64,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EdtTelemetrySnapshot {
    pub(crate) queue_depth: usize,
    pub(crate) max_queue_depth: usize,
    pub(crate) startup_failure_total: u64,
    pub(crate) restart_total: u64,
    pub(crate) drain_restart_total: u64,
    pub(crate) drain_shutdown_total: u64,
    pub(crate) last_drained_jobs: usize,
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

const fn transport_name(transport: ExecutionTransport) -> &'static str {
    match transport {
        ExecutionTransport::Cli => "cli",
        ExecutionTransport::McpStdio => "mcp_stdio",
        ExecutionTransport::McpHttp => "mcp_http",
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        EdtDrainTelemetryReason, EdtQueueDepthAction, EdtQueueDepthReason, EdtRestartReason,
        EdtTelemetry, ExecutionTelemetry, SemaphoreWaitErrorKind, SemaphoreWaitOutcome,
    };
    use crate::use_cases::context::ExecutionTransport;

    #[test]
    fn execution_telemetry_counts_all_outcomes() {
        let telemetry = ExecutionTelemetry::default();

        telemetry.record_semaphore_wait(
            ExecutionTransport::McpHttp,
            "run_all_tests",
            SemaphoreWaitOutcome::Acquired,
            false,
            None,
            Duration::from_millis(3),
            None,
        );
        telemetry.record_semaphore_wait(
            ExecutionTransport::McpHttp,
            "run_all_tests",
            SemaphoreWaitOutcome::Cancelled,
            true,
            Some(Duration::from_millis(10)),
            Duration::from_millis(4),
            None,
        );
        telemetry.record_semaphore_wait(
            ExecutionTransport::McpHttp,
            "check_syntax_edt",
            SemaphoreWaitOutcome::Timeout,
            true,
            Some(Duration::from_millis(10)),
            Duration::from_millis(5),
            None,
        );
        telemetry.record_semaphore_wait(
            ExecutionTransport::McpHttp,
            "check_syntax_edt",
            SemaphoreWaitOutcome::InternalError,
            true,
            Some(Duration::from_millis(10)),
            Duration::from_millis(1),
            Some(SemaphoreWaitErrorKind::SemaphoreClosed),
        );

        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.acquired_total, 1);
        assert_eq!(snapshot.cancelled_total, 1);
        assert_eq!(snapshot.timeout_total, 1);
        assert_eq!(snapshot.internal_error_total, 1);
    }

    #[test]
    fn edt_telemetry_keeps_monotonic_max_depth_and_separate_totals() {
        let telemetry = EdtTelemetry::default();

        telemetry.record_queue_depth(
            EdtQueueDepthAction::Enqueue,
            2,
            Some(EdtQueueDepthReason::QueuedCancelled),
        );
        telemetry.record_queue_depth(EdtQueueDepthAction::Dequeue, 1, None);
        telemetry.record_queue_depth(
            EdtQueueDepthAction::RemoveQueued,
            0,
            Some(EdtQueueDepthReason::QueuedTimeout),
        );
        telemetry.record_startup_failure();
        telemetry.record_restart(EdtRestartReason::BaselineFailure);
        telemetry.record_restart(EdtRestartReason::CommandTimeout);
        telemetry.record_restart(EdtRestartReason::SessionFailure);
        telemetry.record_drain(EdtDrainTelemetryReason::Restart, 3);
        telemetry.record_drain(EdtDrainTelemetryReason::Shutdown, 1);

        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.queue_depth, 0);
        assert_eq!(snapshot.max_queue_depth, 2);
        assert_eq!(snapshot.startup_failure_total, 1);
        assert_eq!(snapshot.restart_total, 3);
        assert_eq!(snapshot.drain_restart_total, 1);
        assert_eq!(snapshot.drain_shutdown_total, 1);
        assert_eq!(snapshot.last_drained_jobs, 1);
    }
}
