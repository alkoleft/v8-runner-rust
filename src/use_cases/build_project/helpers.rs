use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::change_detection::analyzer::{self, AnalysisOutcome, PreparedStateUpdate};
use crate::change_detection::hash_storage::{HashStorage, StorageError};
use crate::change_detection::partial_load::{self, LoadDecision};
use crate::config::model::{AppConfig, SourceSetConfig, SourceSetPurpose};
use crate::domain::build::{BuildMode, BuildResult, BuildStep};
use crate::domain::source_set::SourceSetContext;
use crate::platform::designer::DesignerDsl;
use crate::platform::ibcmd::{IbcmdConnection, IbcmdDsl, IbcmdError};
use crate::platform::process::ProcessRunner;
use crate::platform::result::PlatformCommandResult;
use crate::support::error::AppError;
use crate::support::temp::platform_logs_dir;
use crate::use_cases::build_progress::log_build_step_timeline;
use crate::use_cases::build_progress::{log_timeline_stage, TimelineStageStatus};
use crate::use_cases::context::{ExecutionContext, InterruptionSafetyClass};
use crate::use_cases::ibcmd_diagnostics::format_ibcmd_failure_details;
use crate::use_cases::interruption;
use tracing::debug;

pub(super) type AnalysisByName =
    HashMap<String, Result<AnalysisOutcome, analyzer::ChangeDetectionError>>;

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
        partial_paths: Option<Vec<PathBuf>>,
        commit: StepCommit,
    },
}

pub(super) fn plan_configurator_load_step(
    source_set: &SourceSetConfig,
    source_context: &SourceSetContext,
    full_rebuild: bool,
    analysis_by_name: Option<&AnalysisByName>,
    partial_load_threshold: usize,
) -> Result<StepPlan, analyzer::ChangeDetectionError> {
    if full_rebuild {
        return Ok(StepPlan::Execute {
            mode: BuildMode::Full,
            message: "forced full rebuild".to_owned(),
            partial_paths: None,
            commit: StepCommit::RescanFull {
                recover_storage: true,
            },
        });
    }

    let outcome = analysis_by_name
        .and_then(|analysis| analysis.get(&source_set.name))
        .cloned()
        .expect("every source-set must have an analysis result")?;
    Ok(plan_configurator_load_from_analysis(
        source_set,
        source_context.path(),
        outcome,
        partial_load_threshold,
    ))
}

pub(super) fn plan_edt_export_step(
    source_set: &SourceSetConfig,
    full_rebuild: bool,
    analysis_by_name: Option<&AnalysisByName>,
) -> Result<StepPlan, analyzer::ChangeDetectionError> {
    if full_rebuild {
        return Ok(StepPlan::Execute {
            mode: BuildMode::EdtExport,
            message: "forced EDT export (--full-rebuild)".to_owned(),
            partial_paths: None,
            commit: StepCommit::RescanFull {
                recover_storage: true,
            },
        });
    }

    match analysis_by_name
        .and_then(|analysis| analysis.get(&source_set.name))
        .cloned()
        .expect("every source-set must have an EDT analysis result")?
    {
        AnalysisOutcome::NoChanges => {
            debug!(
                source_set = source_set.name.as_str(),
                found_changes = 0,
                "edt change analysis result: found 0 change(s)"
            );
            Ok(StepPlan::Skip {
                message: "no changes".to_owned(),
                ok: true,
            })
        }
        AnalysisOutcome::Fallback => {
            debug!(
                source_set = source_set.name.as_str(),
                "edt change analysis result: fallback to full export/load after recoverable issue"
            );
            log_timeline_stage(
                &source_set.name,
                "changes",
                "fallback to full export/load after recoverable issue",
                TimelineStageStatus::Succeeded,
            );
            Ok(StepPlan::Execute {
                mode: BuildMode::EdtExport,
                message: "fallback to EDT export after recoverable change-detection issue"
                    .to_owned(),
                partial_paths: None,
                commit: StepCommit::RescanFull {
                    recover_storage: false,
                },
            })
        }
        AnalysisOutcome::Changes { changes, prepared } => {
            log_change_analysis(source_set.name.as_str(), &changes);
            Ok(StepPlan::Execute {
                mode: BuildMode::EdtExport,
                message: "EDT export after change detection".to_owned(),
                partial_paths: None,
                commit: StepCommit::Prepared(prepared),
            })
        }
    }
}

pub(super) fn plan_generated_designer_load_step(
    source_set: &SourceSetConfig,
    designer_context: &SourceSetContext,
    full_rebuild: bool,
    edt_stage_skipped: bool,
    partial_load_threshold: usize,
    work_path: &Path,
) -> Result<StepPlan, analyzer::ChangeDetectionError> {
    if edt_stage_skipped && !designer_context.path().exists() {
        return Ok(StepPlan::Skip {
            message: "no changes".to_owned(),
            ok: true,
        });
    }

    if full_rebuild {
        return Ok(StepPlan::Execute {
            mode: BuildMode::Full,
            message: "full load from EDT export (--full-rebuild)".to_owned(),
            partial_paths: None,
            commit: StepCommit::RescanFull {
                recover_storage: true,
            },
        });
    }

    let outcome = analyzer::analyze_context(designer_context, work_path).outcome?;
    Ok(plan_generated_designer_load_from_analysis(
        source_set,
        designer_context.path(),
        outcome,
        partial_load_threshold,
    ))
}

fn plan_configurator_load_from_analysis(
    source_set: &SourceSetConfig,
    context_path: &Path,
    outcome: AnalysisOutcome,
    partial_load_threshold: usize,
) -> StepPlan {
    match outcome {
        AnalysisOutcome::NoChanges => {
            debug!(
                source_set = source_set.name.as_str(),
                found_changes = 0,
                "change analysis result: found 0 change(s)"
            );
            StepPlan::Skip {
                message: "no changes".to_owned(),
                ok: true,
            }
        }
        AnalysisOutcome::Fallback => {
            debug!(
                source_set = source_set.name.as_str(),
                "change analysis result: fallback to full load after recoverable issue"
            );
            log_timeline_stage(
                &source_set.name,
                "changes",
                "fallback to full load after recoverable issue",
                TimelineStageStatus::Succeeded,
            );
            StepPlan::Execute {
                mode: BuildMode::Full,
                message: "fallback to full load after recoverable change-detection issue"
                    .to_owned(),
                partial_paths: None,
                commit: StepCommit::RescanFull {
                    recover_storage: false,
                },
            }
        }
        AnalysisOutcome::Changes { changes, prepared } => {
            log_change_analysis(source_set.name.as_str(), &changes);
            plan_partial_or_full_load(
                source_set,
                context_path,
                changes,
                prepared,
                partial_load_threshold,
                LoadPlanSource::Configurator,
            )
        }
    }
}

fn plan_generated_designer_load_from_analysis(
    source_set: &SourceSetConfig,
    context_path: &Path,
    outcome: AnalysisOutcome,
    partial_load_threshold: usize,
) -> StepPlan {
    match outcome {
        AnalysisOutcome::NoChanges => {
            debug!(
                source_set = source_set.name.as_str(),
                found_changes = 0,
                "generated designer change analysis result: found 0 change(s)"
            );
            StepPlan::Skip {
                message: "no changes".to_owned(),
                ok: true,
            }
        }
        AnalysisOutcome::Fallback => {
            debug!(
                source_set = source_set.name.as_str(),
                "generated designer change analysis result: fallback to full load after recoverable issue"
            );
            log_timeline_stage(
                &source_set.name,
                "changes",
                "fallback to full load after recoverable issue",
                TimelineStageStatus::Succeeded,
            );
            StepPlan::Execute {
                mode: BuildMode::Full,
                message: "fallback to full load after recoverable change-detection issue"
                    .to_owned(),
                partial_paths: None,
                commit: StepCommit::RescanFull {
                    recover_storage: false,
                },
            }
        }
        AnalysisOutcome::Changes { changes, prepared } => {
            log_change_analysis(source_set.name.as_str(), &changes);
            plan_partial_or_full_load(
                source_set,
                context_path,
                changes,
                prepared,
                partial_load_threshold,
                LoadPlanSource::GeneratedDesigner,
            )
        }
    }
}

#[derive(Clone, Copy)]
enum LoadPlanSource {
    Configurator,
    GeneratedDesigner,
}

impl LoadPlanSource {
    const fn forces_full_for_extension(self) -> bool {
        matches!(self, Self::GeneratedDesigner)
    }

    const fn partial_log_message(self) -> &'static str {
        match self {
            Self::Configurator => "change analysis decision: partial load",
            Self::GeneratedDesigner => "generated designer change analysis decision: partial load",
        }
    }

    const fn full_log_message(self) -> &'static str {
        match self {
            Self::Configurator => "change analysis decision: full load",
            Self::GeneratedDesigner => "generated designer change analysis decision: full load",
        }
    }
}

fn plan_partial_or_full_load(
    source_set: &SourceSetConfig,
    context_path: &Path,
    changes: Vec<analyzer::FileChange>,
    prepared: PreparedStateUpdate,
    partial_load_threshold: usize,
    source: LoadPlanSource,
) -> StepPlan {
    let decision = if source.forces_full_for_extension()
        && source_set.purpose == SourceSetPurpose::Extension
    {
        debug!(
            source_set = source_set.name.as_str(),
            "generated designer change analysis decision: forcing full load for EDT extension source-set"
        );
        LoadDecision::Full
    } else {
        partial_load::decide(&changes, context_path, partial_load_threshold)
    };

    match decision {
        LoadDecision::Partial(paths) => {
            debug!(
                source_set = source_set.name.as_str(),
                partial_file_count = paths.len(),
                threshold = partial_load_threshold,
                "{}",
                source.partial_log_message()
            );
            StepPlan::Execute {
                mode: BuildMode::Partial {
                    file_count: paths.len(),
                },
                message: format!("partial load of {} files", paths.len()),
                partial_paths: Some(paths),
                commit: StepCommit::Prepared(prepared),
            }
        }
        LoadDecision::Full => {
            debug!(
                source_set = source_set.name.as_str(),
                threshold = partial_load_threshold,
                "{}",
                source.full_log_message()
            );
            StepPlan::Execute {
                mode: BuildMode::Full,
                message: if source.forces_full_for_extension()
                    && source_set.purpose == SourceSetPurpose::Extension
                {
                    "full load required for EDT extension source-set".to_owned()
                } else {
                    "full load selected by partial-load rules".to_owned()
                },
                partial_paths: None,
                commit: StepCommit::Prepared(prepared),
            }
        }
    }
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

pub(super) fn commit_step_state(
    source_set: &SourceSetConfig,
    context: &SourceSetContext,
    work_path: &Path,
    commit: &StepCommit,
) -> Result<(), AppError> {
    match commit {
        StepCommit::Prepared(prepared) => {
            debug!(
                source_set = source_set.name.as_str(),
                "committing prepared change-detection state"
            );
            analyzer::commit_success(context, work_path, prepared)
                .map_err(|error| AppError::Runtime(error.to_string()))
        }
        StepCommit::RescanFull { recover_storage } => {
            debug!(
                source_set = source_set.name.as_str(),
                recover_storage, "rescanning source-set state after full build"
            );
            commit_full_rescan(context, work_path, *recover_storage)
        }
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
    AppError::from(error)
}

pub(super) fn interruption_before_safe_point(
    context: &ExecutionContext,
    safe_point: String,
) -> Option<AppError> {
    interruption::interruption_before_safe_point(context, safe_point)
}

pub(super) fn deferred_interruption_warning(
    action: &str,
    result: &PlatformCommandResult,
) -> Option<String> {
    interruption::deferred_process_interruption_warning(
        &format!("{action} completed successfully"),
        result,
    )
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

pub(super) fn fail_from_source_set_index(
    started: Instant,
    completed_steps: Vec<BuildStep>,
    ordered_source_sets: &[&SourceSetConfig],
    current_index: usize,
    failed_source_set: &SourceSetConfig,
    failed_mode: BuildMode,
    message: String,
) -> BuildResult {
    fail_with_remaining_steps(
        started,
        completed_steps,
        ordered_source_sets
            .iter()
            .skip(current_index)
            .copied()
            .collect(),
        failed_source_set,
        failed_mode,
        message,
    )
}
