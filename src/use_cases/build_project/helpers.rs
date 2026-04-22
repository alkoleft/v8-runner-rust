use std::path::Path;
use std::time::Instant;

use crate::change_detection::analyzer::{self, PreparedStateUpdate};
use crate::change_detection::hash_storage::{HashStorage, StorageError};
use crate::config::model::{AppConfig, SourceSetConfig, SourceSetPurpose};
use crate::domain::build::{BuildMode, BuildResult, BuildStep};
use crate::domain::source_set::SourceSetContext;
use crate::platform::designer::DesignerDsl;
use crate::platform::ibcmd::{IbcmdConnection, IbcmdDsl, IbcmdError};
use crate::platform::process::ProcessRunner;
use crate::platform::result::PlatformCommandResult;
use crate::support::error::AppError;
use crate::support::temp::platform_logs_dir;
use crate::use_cases::context::{ExecutionContext, InterruptionSafetyClass};
use crate::use_cases::ibcmd_diagnostics::format_ibcmd_failure_details;
use tracing::info;

use super::TimelineStageStatus;

pub(super) enum StepCommit {
    Prepared(PreparedStateUpdate),
    RescanFull { recover_storage: bool },
}

pub(super) enum StepPlan {
    Skip {
        message: String,
        ok: bool,
    },
    Execute {
        mode: BuildMode,
        message: String,
        partial_paths: Option<Vec<std::path::PathBuf>>,
        commit: StepCommit,
    },
}

pub(super) fn log_change_analysis(source_set_name: &str, changes: &[analyzer::FileChange]) {
    let mut added = 0usize;
    let mut modified = 0usize;
    let mut deleted = 0usize;

    for change in changes {
        match change.kind {
            analyzer::ChangeKind::Added => added += 1,
            analyzer::ChangeKind::Modified => modified += 1,
            analyzer::ChangeKind::Deleted => deleted += 1,
        }
    }

    log_timeline_stage(
        source_set_name,
        "changes",
        &format!(
            "Изменения: найдено {} (новых {added}, изменено {modified}, удалено {deleted})",
            changes.len()
        ),
        TimelineStageStatus::Succeeded,
    );
}

pub(super) fn push_build_step(
    steps: &mut Vec<BuildStep>,
    source_set_name: &str,
    mode: BuildMode,
    ok: bool,
    message: String,
    duration_ms: u64,
) {
    let step = BuildStep {
        source_set: source_set_name.to_owned(),
        mode,
        ok,
        message: Some(message),
        duration_ms,
    };
    log_build_step_timeline(&step);
    steps.push(step);
}

fn log_build_step_timeline(step: &BuildStep) {
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

pub(super) fn log_timeline_stage(
    source_set_name: &str,
    _stage: &str,
    message: &str,
    status: TimelineStageStatus,
) {
    let label = format!("{source_set_name}:");
    let detail = first_message_line(message);
    info!(
        timeline_status = status.as_str(),
        timeline_label = label.as_str(),
        timeline_detail = detail
    );
}

pub(super) fn build_mode_label(mode: &BuildMode) -> String {
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

pub(super) fn commit_full_rescan(
    context: &SourceSetContext,
    work_path: &Path,
    recover_storage: bool,
) -> Result<(), AppError> {
    match analyzer::rescan_and_commit_full(context, work_path) {
        Ok(()) => Ok(()),
        Err(_error) if recover_storage && storage_needs_recovery(context, work_path) => {
            let storage_path = context.storage_path(work_path);
            remove_storage_path(&storage_path).map_err(|remove_error| {
                AppError::Runtime(format!(
                    "failed to remove corrupt storage '{}': {remove_error}",
                    storage_path.display()
                ))
            })?;
            analyzer::rescan_and_commit_full(context, work_path)
                .map_err(|retry_error| AppError::Runtime(retry_error.to_string()))
        }
        Err(error) => Err(AppError::Runtime(error.to_string())),
    }
}

fn storage_needs_recovery(context: &SourceSetContext, work_path: &Path) -> bool {
    match HashStorage::new(context.storage_path(work_path)).current_generation() {
        Err(StorageError::Recoverable { .. }) => true,
        Err(StorageError::Hard { reason, .. }) => {
            let reason = reason.to_ascii_lowercase();
            reason.contains("invalid data") || reason.contains("corrupt")
        }
        Err(StorageError::ConcurrentStateModified { .. }) | Ok(_) => false,
    }
}

pub(super) fn remove_storage_path(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }

    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

pub(super) fn build_designer_dsl<'a>(
    context: &ExecutionContext,
    config: &AppConfig,
    binary: &Path,
    runner: &'a dyn ProcessRunner,
    source_set_name: &str,
    step_index: usize,
    action: &str,
    safety: InterruptionSafetyClass,
) -> Result<DesignerDsl<'a>, AppError> {
    let log_dir = platform_logs_dir(&config.work_path).map_err(|error| {
        AppError::Runtime(format!("failed to create platform logs dir: {error}"))
    })?;
    let log_file = log_dir.join(format!(
        "build-{step_index:02}-{source_set_name}-{action}.log"
    ));

    Ok(DesignerDsl::new(
        binary.to_path_buf(),
        config.v8_connection(),
        runner,
        Some(log_file),
    )
    .with_execution_policy(context.process_policy(safety, None)))
}

pub(super) fn build_ibcmd_dsl<'a>(
    context: &ExecutionContext,
    config: &AppConfig,
    binary: &Path,
    runner: &'a dyn ProcessRunner,
    safety: InterruptionSafetyClass,
) -> Result<IbcmdDsl<'a>, AppError> {
    let connection = IbcmdConnection::from_infobase(&config.infobase).map_err(map_ibcmd_error)?;

    Ok(IbcmdDsl::new(binary.to_path_buf(), connection, runner)
        .with_execution_policy(context.process_policy(safety, None)))
}

pub(super) fn map_ibcmd_error(error: IbcmdError) -> AppError {
    match error {
        IbcmdError::MissingServerDbmsField(_) => AppError::Validation(error.to_string()),
        IbcmdError::Spawn(_) => AppError::Platform(error.to_string()),
    }
}

pub(super) fn interruption_before_safe_point(
    context: &ExecutionContext,
    safe_point: String,
) -> Option<AppError> {
    context.interruption().map(|interruption| {
        AppError::Runtime(format!(
            "{} for command '{}' before entering {safe_point} safe point",
            interruption.message(context.command()),
            context.command().as_str()
        ))
    })
}

pub(super) fn deferred_interruption_warning(
    action: &str,
    result: &PlatformCommandResult,
) -> Option<String> {
    result.process.interruption.map(|interruption| {
        let reason = match interruption.reason {
            crate::platform::process::ProcessInterruptionReason::Cancelled => "cancellation request",
            crate::platform::process::ProcessInterruptionReason::TimedOut => "timeout",
        };
        format!(
            "{action} completed successfully after {reason} during critical phase; unsafe interruption was not performed"
        )
    })
}

pub(super) fn merge_step_message(message: String, warnings: &[String]) -> String {
    if warnings.is_empty() {
        message
    } else {
        format!("{message}; {}", warnings.join("; "))
    }
}

pub(super) fn extension_name(source_set: &SourceSetConfig) -> Option<&str> {
    match source_set.purpose {
        SourceSetPurpose::Configuration => None,
        SourceSetPurpose::Extension => Some(source_set.name.as_str()),
        SourceSetPurpose::ExternalDataProcessors | SourceSetPurpose::ExternalReports => None,
    }
}

pub(crate) fn ensure_platform_success(
    action: &str,
    source_set: &SourceSetConfig,
    result: &PlatformCommandResult,
) -> Result<(), AppError> {
    if result.process.exit_code == 0 {
        return Ok(());
    }

    Err(AppError::Platform(format_ibcmd_failure_details(
        action,
        "source-set",
        &source_set.name,
        result.process.exit_code,
        &result.process.stdout,
        &result.process.stderr,
        result.platform_log.as_deref(),
        result.platform_log_path.as_deref(),
    )))
}

pub(super) fn fail_with_remaining_steps(
    started: Instant,
    mut completed_steps: Vec<BuildStep>,
    remaining: Vec<&SourceSetConfig>,
    failed_source_set: &SourceSetConfig,
    failed_mode: BuildMode,
    message: String,
) -> BuildResult {
    push_build_step(
        &mut completed_steps,
        &failed_source_set.name,
        failed_mode,
        false,
        message,
        0,
    );

    for source_set in remaining.into_iter().skip(1) {
        push_build_step(
            &mut completed_steps,
            &source_set.name,
            BuildMode::Skipped,
            false,
            "aborted after previous failure".to_owned(),
            0,
        );
    }

    BuildResult {
        ok: false,
        steps: completed_steps,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}
