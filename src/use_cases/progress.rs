use tracing::info;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveStageStatus {
    Succeeded,
    Failed,
}

impl LiveStageStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }
}

/// Emits a text-mode live progress timeline event before a blocking stage starts.
///
/// Callers must pass fixed, sanitized vocabulary only: no rendered commands, raw
/// arguments, stdout/stderr, environment values, connection strings, or secrets.
pub(crate) fn log_live_stage(label: &str, detail: &str) {
    info!(
        target: "v8_runner::live_progress",
        timeline_status = "running",
        timeline_label = label,
        timeline_detail = detail
    );
}

/// Emits a sanitized live progress status after a blocking stage finishes.
pub(crate) fn log_live_stage_status(label: &str, status: LiveStageStatus, detail: &str) {
    info!(
        target: "v8_runner::live_progress",
        timeline_status = status.as_str(),
        timeline_label = label,
        timeline_detail = detail
    );
}
