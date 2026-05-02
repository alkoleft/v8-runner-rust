use crate::domain::build::{BuildMode, BuildStep};
use crate::use_cases::progress::log_live_stage;
use tracing::info;

#[derive(Clone, Copy)]
pub(crate) enum TimelineStageStatus {
    Running,
    Succeeded,
    Failed,
    Skipped,
}

impl TimelineStageStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

pub(crate) fn log_build_step_timeline(step: &BuildStep) {
    if step.ok
        && matches!(step.mode, BuildMode::Skipped)
        && step.message.as_deref() == Some("no changes")
    {
        return;
    }

    let status = if !step.ok {
        TimelineStageStatus::Failed
    } else if matches!(step.mode, BuildMode::Skipped) {
        TimelineStageStatus::Skipped
    } else {
        TimelineStageStatus::Succeeded
    };
    let message = step
        .message
        .as_deref()
        .map(first_message_line)
        .filter(|value| !value.is_empty())
        .unwrap_or("ok");
    let detail = build_step_completion_detail(step, status, message);
    log_timeline_stage(
        &step.source_set,
        &build_mode_label(&step.mode),
        &detail,
        status,
    );
}

fn build_step_completion_detail(
    step: &BuildStep,
    status: TimelineStageStatus,
    message: &str,
) -> String {
    match status {
        TimelineStageStatus::Succeeded => match step.mode {
            BuildMode::EdtExport => "✓ completed".to_owned(),
            _ => format!("✓ {message}"),
        },
        TimelineStageStatus::Failed => format!("✗ {message}"),
        TimelineStageStatus::Skipped => format!("○ {message}"),
        TimelineStageStatus::Running => message.to_owned(),
    }
}

pub(crate) fn log_timeline_stage(
    source_set_name: &str,
    _stage: &str,
    message: &str,
    status: TimelineStageStatus,
) {
    let label = format!("{source_set_name}:");
    let detail = first_message_line(message);
    if matches!(status, TimelineStageStatus::Running) {
        log_live_stage(&label, detail);
        return;
    }

    info!(
        timeline_status = status.as_str(),
        timeline_label = label.as_str(),
        timeline_detail = detail
    );
}

pub(crate) fn build_mode_label(mode: &BuildMode) -> String {
    match mode {
        BuildMode::EdtExport => "edt_export".to_owned(),
        BuildMode::Full => "full".to_owned(),
        BuildMode::Partial { file_count } => format!("partial ({file_count} files)"),
        BuildMode::Skipped => "skipped".to_owned(),
    }
}

fn first_message_line(message: &str) -> &str {
    message.lines().next().unwrap_or(message).trim()
}
