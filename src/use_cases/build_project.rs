use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::Instant;

use crate::change_detection::analyzer::{self, AnalysisOutcome, PreparedStateUpdate};
use crate::change_detection::hash_storage::{HashStorage, StorageError};
use crate::change_detection::partial_load::{self, LoadDecision};
use crate::change_detection::source_sets::SourceSetsService;
use crate::config::model::{
    AppConfig, BuilderBackend, SourceFormat, SourceSetConfig, SourceSetPurpose,
};
use crate::domain::build::{BuildMode, BuildResult, BuildStep};
use crate::domain::source_set::SourceSetContext;
use crate::platform::designer::DesignerDsl;
use crate::platform::edt::EdtDsl;
use crate::platform::ibcmd::{DynamicUpdateMode, IbcmdConnection, IbcmdDsl, IbcmdError};
use crate::platform::locator::UtilityType;
use crate::platform::process::ProcessRunner;
use crate::platform::result::PlatformCommandResult;
use crate::platform::utilities::PlatformUtilities;
use crate::support::error::AppError;
use crate::support::temp::{partial_list_file, platform_logs_dir, reserved_source_set_dir};
use crate::use_cases::context::ExecutionContext;
use crate::use_cases::external_artifacts::{
    discover_designer_external_artifacts, prepare_edt_external_artifacts, resolve_source_set_path,
    source_set_external_kind,
};
use crate::use_cases::ibcmd_diagnostics::format_ibcmd_failure_details;
use crate::use_cases::request::BuildRequest as BuildArgs;
use crate::use_cases::result::{UseCaseFailure, UseCaseResult};
use tracing::{debug, info};

#[cfg(test)]
const BUILD_COMMAND: &str = crate::use_cases::context::CommandName::Build.as_str();
const SUPPORTED_DESIGNER_BUILD_ERROR: &str =
    "build currently supports only builder=DESIGNER or IBCMD with format=DESIGNER";
const SUPPORTED_EDT_BUILD_ERROR: &str =
    "build with format=EDT currently supports only builder=DESIGNER or IBCMD";

pub fn execute(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &BuildArgs,
) -> UseCaseResult<BuildResult> {
    debug!(
        command = context.command().as_str(),
        transport = ?context.transport(),
        "executing build use case"
    );
    run_build_unlocked(config, args)
}

pub(crate) type BuildExecutionFailure = UseCaseFailure<BuildResult>;

enum StepCommit {
    Prepared(PreparedStateUpdate),
    RescanFull { recover_storage: bool },
}

enum StepPlan {
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

#[cfg(test)]
pub(crate) fn run_build(config: &AppConfig, args: &BuildArgs) -> UseCaseResult<BuildResult> {
    run_build_unlocked(config, args)
}

/// Caller must ensure exclusive ownership of `config.work_path`.
pub(crate) fn run_build_unlocked(
    config: &AppConfig,
    args: &BuildArgs,
) -> UseCaseResult<BuildResult> {
    if config.format == SourceFormat::Edt {
        return run_build_edt(config, args);
    }

    if let Some(error) = validate_designer_supported_matrix(config) {
        return Err(BuildExecutionFailure::with_payload(
            error,
            BuildResult {
                ok: false,
                steps: vec![],
                duration_ms: 0,
            },
        ));
    }

    match config.builder {
        BuilderBackend::Designer => run_build_designer(config, args),
        BuilderBackend::Ibcmd => run_build_ibcmd(config, args),
    }
}

fn run_build_designer(
    config: &AppConfig,
    args: &BuildArgs,
) -> Result<BuildResult, BuildExecutionFailure> {
    debug!(full_rebuild = args.full_rebuild, "preparing build plan");

    let started = Instant::now();
    let service = SourceSetsService::new(config);
    let contexts = service.designer_contexts();
    let contexts_by_name: HashMap<String, SourceSetContext> = contexts
        .into_iter()
        .map(|context| (context.name().to_owned(), context))
        .collect();
    let ordered_source_sets = ordered_source_sets(config);

    let analysis_by_name = if args.full_rebuild {
        None
    } else {
        Some(analyze_contexts_by_name(
            &service,
            &contexts_by_name.values().cloned().collect::<Vec<_>>(),
        ))
    };

    let mut utilities = PlatformUtilities::from_config(config);
    let mut designer_binary: Option<PathBuf> = None;
    let mut steps = Vec::new();

    for (index, source_set) in ordered_source_sets.iter().enumerate() {
        let Some(context) = contexts_by_name.get(&source_set.name).cloned() else {
            continue;
        };

        if source_set.purpose.is_external() {
            let step_started = Instant::now();
            let result = discover_designer_external_artifacts(
                &source_set.name,
                &resolve_source_set_path(config, source_set),
                source_set_external_kind(source_set).expect("external kind"),
            );
            match result {
                Ok(descriptors) => steps.push(BuildStep {
                    source_set: source_set.name.clone(),
                    mode: BuildMode::Skipped,
                    ok: true,
                    message: Some(format!(
                        "prepared {} external artifact(s) for packaging",
                        descriptors.len()
                    )),
                    duration_ms: step_started.elapsed().as_millis() as u64,
                }),
                Err(error) => {
                    let result = fail_with_remaining_steps(
                        started,
                        steps,
                        ordered_source_sets
                            .iter()
                            .skip(index)
                            .copied()
                            .collect::<Vec<_>>(),
                        source_set,
                        BuildMode::Skipped,
                        error.to_string(),
                    );
                    return Err(BuildExecutionFailure::with_payload(error, result));
                }
            }
            continue;
        }

        let plan = if args.full_rebuild {
            StepPlan::Execute {
                mode: BuildMode::Full,
                message: "forced full rebuild".to_owned(),
                partial_paths: None,
                commit: StepCommit::RescanFull {
                    recover_storage: true,
                },
            }
        } else {
            match analysis_by_name
                .as_ref()
                .and_then(|analysis| analysis.get(&source_set.name))
                .cloned()
                .expect("every source-set must have an analysis result")
            {
                Ok(AnalysisOutcome::NoChanges) => {
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
                Ok(AnalysisOutcome::Fallback) => {
                    debug!(
                        source_set = source_set.name.as_str(),
                        "change analysis result: fallback to full load after recoverable issue"
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
                Ok(AnalysisOutcome::Changes { changes, prepared }) => {
                    log_change_analysis(source_set.name.as_str(), &changes);
                    match partial_load::decide(
                        &changes,
                        context.path(),
                        config.build.partial_load_threshold,
                    ) {
                        LoadDecision::Partial(paths) => {
                            debug!(
                                source_set = source_set.name.as_str(),
                                partial_file_count = paths.len(),
                                threshold = config.build.partial_load_threshold,
                                "change analysis decision: partial load"
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
                                threshold = config.build.partial_load_threshold,
                                "change analysis decision: full load"
                            );
                            StepPlan::Execute {
                                mode: BuildMode::Full,
                                message: "full load selected by partial-load rules".to_owned(),
                                partial_paths: None,
                                commit: StepCommit::Prepared(prepared),
                            }
                        }
                    }
                }
                Err(error) => {
                    let result = fail_with_remaining_steps(
                        started,
                        steps,
                        ordered_source_sets
                            .iter()
                            .skip(index)
                            .copied()
                            .collect::<Vec<_>>(),
                        source_set,
                        BuildMode::Skipped,
                        error.to_string(),
                    );
                    return Err(BuildExecutionFailure::with_payload(
                        AppError::Runtime(error.to_string()),
                        result,
                    ));
                }
            }
        };

        match plan {
            StepPlan::Skip { message, ok } => {
                debug!(
                    source_set = source_set.name.as_str(),
                    message = message.as_str(),
                    "skipping build step"
                );
                steps.push(BuildStep {
                    source_set: source_set.name.clone(),
                    mode: BuildMode::Skipped,
                    ok,
                    message: Some(message),
                    duration_ms: 0,
                })
            }
            StepPlan::Execute {
                mode,
                message,
                partial_paths,
                commit,
            } => {
                debug!(
                    source_set = source_set.name.as_str(),
                    mode = ?mode,
                    message = message.as_str(),
                    "executing build step"
                );
                let binary = match designer_binary.clone() {
                    Some(path) => path,
                    None => {
                        let location = match utilities.locate(UtilityType::V8) {
                            Ok(location) => location,
                            Err(error) => {
                                let result = fail_with_remaining_steps(
                                    started,
                                    steps,
                                    ordered_source_sets
                                        .iter()
                                        .skip(index)
                                        .copied()
                                        .collect::<Vec<_>>(),
                                    source_set,
                                    mode.clone(),
                                    error.to_string(),
                                );
                                return Err(BuildExecutionFailure::with_payload(
                                    AppError::Platform(error.to_string()),
                                    result,
                                ));
                            }
                        };
                        designer_binary = Some(location.path.clone());
                        location.path
                    }
                };

                let step_started = Instant::now();
                match execute_source_set_step(
                    config,
                    &binary,
                    utilities.runner_for(UtilityType::V8),
                    source_set,
                    &context,
                    &context,
                    index,
                    partial_paths.as_deref(),
                    &commit,
                ) {
                    Ok(()) => steps.push(BuildStep {
                        source_set: source_set.name.clone(),
                        mode,
                        ok: true,
                        message: Some(message),
                        duration_ms: step_started.elapsed().as_millis() as u64,
                    }),
                    Err(error) => {
                        let result = fail_with_remaining_steps(
                            started,
                            steps,
                            ordered_source_sets
                                .iter()
                                .skip(index)
                                .copied()
                                .collect::<Vec<_>>(),
                            source_set,
                            mode,
                            error.to_string(),
                        );
                        return Err(BuildExecutionFailure::with_payload(error, result));
                    }
                }
            }
        }
    }

    Ok(BuildResult {
        ok: true,
        steps,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

fn log_change_analysis(source_set_name: &str, changes: &[analyzer::FileChange]) {
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

    info!(
        "[Изменения] {source_set_name}: найдено {} (новых {added}, изменено {modified}, удалено {deleted})",
        changes.len()
    );
}

fn run_build_ibcmd(
    config: &AppConfig,
    args: &BuildArgs,
) -> Result<BuildResult, BuildExecutionFailure> {
    debug!(
        full_rebuild = args.full_rebuild,
        "preparing ibcmd build plan"
    );

    let started = Instant::now();
    let service = SourceSetsService::new(config);
    let contexts = service.designer_contexts();
    let contexts_by_name: HashMap<String, SourceSetContext> = contexts
        .into_iter()
        .map(|context| (context.name().to_owned(), context))
        .collect();
    let ordered_source_sets = ordered_source_sets(config);

    let analysis_by_name = if args.full_rebuild {
        None
    } else {
        Some(analyze_contexts_by_name(
            &service,
            &contexts_by_name.values().cloned().collect::<Vec<_>>(),
        ))
    };

    let mut utilities = PlatformUtilities::from_config(config);
    let mut ibcmd_binary: Option<PathBuf> = None;
    let mut steps = Vec::new();

    for (index, source_set) in ordered_source_sets.iter().enumerate() {
        let Some(context) = contexts_by_name.get(&source_set.name).cloned() else {
            continue;
        };

        let plan = if args.full_rebuild {
            StepPlan::Execute {
                mode: BuildMode::Full,
                message: "forced full rebuild".to_owned(),
                partial_paths: None,
                commit: StepCommit::RescanFull {
                    recover_storage: true,
                },
            }
        } else {
            match analysis_by_name
                .as_ref()
                .and_then(|analysis| analysis.get(&source_set.name))
                .cloned()
                .expect("every source-set must have an analysis result")
            {
                Ok(AnalysisOutcome::NoChanges) => {
                    info!(
                        source_set = source_set.name.as_str(),
                        found_changes = 0,
                        "change analysis result: found 0 change(s)"
                    );
                    StepPlan::Skip {
                        message: "no changes".to_owned(),
                        ok: true,
                    }
                }
                Ok(AnalysisOutcome::Fallback) => {
                    info!(
                        source_set = source_set.name.as_str(),
                        "change analysis result: fallback to full load after recoverable issue"
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
                Ok(AnalysisOutcome::Changes { changes, prepared }) => {
                    log_change_analysis(source_set.name.as_str(), &changes);
                    match partial_load::decide(
                        &changes,
                        context.path(),
                        config.build.partial_load_threshold,
                    ) {
                        LoadDecision::Partial(paths) => {
                            info!(
                                source_set = source_set.name.as_str(),
                                partial_file_count = paths.len(),
                                threshold = config.build.partial_load_threshold,
                                "change analysis decision: partial load"
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
                            info!(
                                source_set = source_set.name.as_str(),
                                threshold = config.build.partial_load_threshold,
                                "change analysis decision: full load"
                            );
                            StepPlan::Execute {
                                mode: BuildMode::Full,
                                message: "full load selected by partial-load rules".to_owned(),
                                partial_paths: None,
                                commit: StepCommit::Prepared(prepared),
                            }
                        }
                    }
                }
                Err(error) => {
                    let result = fail_with_remaining_steps(
                        started,
                        steps,
                        ordered_source_sets
                            .iter()
                            .skip(index)
                            .copied()
                            .collect::<Vec<_>>(),
                        source_set,
                        BuildMode::Skipped,
                        error.to_string(),
                    );
                    return Err(BuildExecutionFailure::with_payload(
                        AppError::Runtime(error.to_string()),
                        result,
                    ));
                }
            }
        };

        match plan {
            StepPlan::Skip { message, ok } => {
                info!(
                    source_set = source_set.name.as_str(),
                    message = message.as_str(),
                    "skipping build step"
                );
                steps.push(BuildStep {
                    source_set: source_set.name.clone(),
                    mode: BuildMode::Skipped,
                    ok,
                    message: Some(message),
                    duration_ms: 0,
                })
            }
            StepPlan::Execute {
                mode,
                message,
                partial_paths,
                commit,
            } => {
                info!(
                    source_set = source_set.name.as_str(),
                    mode = ?mode,
                    message = message.as_str(),
                    "executing ibcmd build step"
                );
                let binary = match ibcmd_binary.clone() {
                    Some(path) => path,
                    None => {
                        let location = match utilities.locate(UtilityType::Ibcmd) {
                            Ok(location) => location,
                            Err(error) => {
                                let result = fail_with_remaining_steps(
                                    started,
                                    steps,
                                    ordered_source_sets
                                        .iter()
                                        .skip(index)
                                        .copied()
                                        .collect::<Vec<_>>(),
                                    source_set,
                                    mode.clone(),
                                    error.to_string(),
                                );
                                return Err(BuildExecutionFailure::with_payload(
                                    AppError::Platform(error.to_string()),
                                    result,
                                ));
                            }
                        };
                        ibcmd_binary = Some(location.path.clone());
                        location.path
                    }
                };

                let step_started = Instant::now();
                match execute_source_set_step_ibcmd(
                    config,
                    &binary,
                    utilities.runner_for(UtilityType::Ibcmd),
                    source_set,
                    &context,
                    &context,
                    partial_paths.as_deref(),
                    &commit,
                ) {
                    Ok(()) => steps.push(BuildStep {
                        source_set: source_set.name.clone(),
                        mode,
                        ok: true,
                        message: Some(message),
                        duration_ms: step_started.elapsed().as_millis() as u64,
                    }),
                    Err(error) => {
                        let result = fail_with_remaining_steps(
                            started,
                            steps,
                            ordered_source_sets
                                .iter()
                                .skip(index)
                                .copied()
                                .collect::<Vec<_>>(),
                            source_set,
                            mode,
                            error.to_string(),
                        );
                        return Err(BuildExecutionFailure::with_payload(error, result));
                    }
                }
            }
        }
    }

    Ok(BuildResult {
        ok: true,
        steps,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

fn validate_designer_supported_matrix(config: &AppConfig) -> Option<AppError> {
    if config.format == SourceFormat::Designer
        && matches!(
            config.builder,
            BuilderBackend::Designer | BuilderBackend::Ibcmd
        )
    {
        None
    } else {
        Some(AppError::Validation(
            SUPPORTED_DESIGNER_BUILD_ERROR.to_owned(),
        ))
    }
}

fn validate_edt_supported_matrix(config: &AppConfig) -> Option<AppError> {
    if config.format == SourceFormat::Edt
        && matches!(
            config.builder,
            BuilderBackend::Designer | BuilderBackend::Ibcmd
        )
    {
        None
    } else {
        Some(AppError::Validation(SUPPORTED_EDT_BUILD_ERROR.to_owned()))
    }
}

fn run_build_edt(
    config: &AppConfig,
    args: &BuildArgs,
) -> Result<BuildResult, BuildExecutionFailure> {
    debug!(full_rebuild = args.full_rebuild, "preparing edt build plan");
    if let Some(error) = validate_edt_supported_matrix(config) {
        return Err(BuildExecutionFailure::with_payload(
            error,
            BuildResult {
                ok: false,
                steps: vec![],
                duration_ms: 0,
            },
        ));
    }

    let started = Instant::now();
    let service = SourceSetsService::new(config);
    let edt_contexts = service.edt_contexts();
    let designer_contexts = service.designer_contexts();
    let edt_contexts_by_name: HashMap<String, SourceSetContext> = edt_contexts
        .into_iter()
        .map(|context| (context.name().to_owned(), context))
        .collect();
    let designer_contexts_by_name: HashMap<String, SourceSetContext> = designer_contexts
        .into_iter()
        .map(|context| (context.name().to_owned(), context))
        .collect();
    let ordered_source_sets = ordered_source_sets(config);

    let analysis_by_name = if args.full_rebuild {
        None
    } else {
        Some(analyze_contexts_by_name(
            &service,
            &edt_contexts_by_name.values().cloned().collect::<Vec<_>>(),
        ))
    };

    let mut utilities = PlatformUtilities::from_config(config);
    let mut designer_binary: Option<PathBuf> = None;
    let mut ibcmd_binary: Option<PathBuf> = None;
    let mut edt_binary: Option<PathBuf> = None;
    let mut interactive_edt = None;
    let mut steps = Vec::new();

    for (index, source_set) in ordered_source_sets.iter().enumerate() {
        let Some(edt_context) = edt_contexts_by_name.get(&source_set.name).cloned() else {
            continue;
        };
        let Some(designer_context) = designer_contexts_by_name.get(&source_set.name).cloned()
        else {
            continue;
        };

        let plan = if args.full_rebuild {
            StepPlan::Execute {
                mode: BuildMode::Full,
                message: "full load from EDT export (--full-rebuild)".to_owned(),
                partial_paths: None,
                commit: StepCommit::RescanFull {
                    recover_storage: true,
                },
            }
        } else {
            match analysis_by_name
                .as_ref()
                .and_then(|analysis| analysis.get(&source_set.name))
                .cloned()
                .expect("every source-set must have an EDT analysis result")
            {
                Ok(AnalysisOutcome::NoChanges) => {
                    debug!(
                        source_set = source_set.name.as_str(),
                        found_changes = 0,
                        "edt change analysis result: found 0 change(s)"
                    );
                    StepPlan::Skip {
                        message: "no changes".to_owned(),
                        ok: true,
                    }
                }
                Ok(AnalysisOutcome::Fallback) => {
                    debug!(
                        source_set = source_set.name.as_str(),
                        "edt change analysis result: fallback to full export/load after recoverable issue"
                    );
                    StepPlan::Execute {
                        mode: BuildMode::Full,
                        message:
                            "fallback to full export/load after recoverable change-detection issue"
                                .to_owned(),
                        partial_paths: None,
                        commit: StepCommit::RescanFull {
                            recover_storage: false,
                        },
                    }
                }
                Ok(AnalysisOutcome::Changes { changes, prepared }) => {
                    log_change_analysis(source_set.name.as_str(), &changes);
                    StepPlan::Execute {
                        mode: BuildMode::Full,
                        message: "full load from EDT export after change detection".to_owned(),
                        partial_paths: None,
                        commit: StepCommit::Prepared(prepared),
                    }
                }
                Err(error) => {
                    let result = fail_with_remaining_steps(
                        started,
                        steps,
                        ordered_source_sets
                            .iter()
                            .skip(index)
                            .copied()
                            .collect::<Vec<_>>(),
                        source_set,
                        BuildMode::Skipped,
                        error.to_string(),
                    );
                    return Err(BuildExecutionFailure::with_payload(
                        AppError::Runtime(error.to_string()),
                        result,
                    ));
                }
            }
        };

        if source_set.purpose.is_external() {
            let edt = match edt_binary.clone() {
                Some(path) => path,
                None => {
                    let location = match utilities.locate(UtilityType::EdtCli) {
                        Ok(location) => location,
                        Err(error) => {
                            let result = fail_with_remaining_steps(
                                started,
                                steps,
                                ordered_source_sets
                                    .iter()
                                    .skip(index)
                                    .copied()
                                    .collect::<Vec<_>>(),
                                source_set,
                                BuildMode::EdtExport,
                                error.to_string(),
                            );
                            return Err(BuildExecutionFailure::with_payload(
                                AppError::Platform(error.to_string()),
                                result,
                            ));
                        }
                    };
                    edt_binary = Some(location.path.clone());
                    location.path
                }
            };
            let export_started = Instant::now();
            let export_result = if config.tools.edt_cli.interactive_mode {
                if interactive_edt.is_none() {
                    interactive_edt = Some(
                        match EdtDsl::new_interactive(
                            edt.clone(),
                            config.work_path.join("edt-workspace"),
                            Duration::from_millis(config.tools.edt_cli.startup_timeout_ms),
                            Duration::from_millis(config.tools.edt_cli.command_timeout_ms),
                        ) {
                            Ok(dsl) => dsl,
                            Err(error) => {
                                let app_error = AppError::Platform(error.to_string());
                                let result = fail_with_remaining_steps(
                                    started,
                                    steps,
                                    ordered_source_sets
                                        .iter()
                                        .skip(index)
                                        .copied()
                                        .collect::<Vec<_>>(),
                                    source_set,
                                    BuildMode::EdtExport,
                                    app_error.to_string(),
                                );
                                return Err(BuildExecutionFailure::with_payload(app_error, result));
                            }
                        },
                    );
                }
                prepare_edt_external_artifacts(
                    config,
                    source_set,
                    interactive_edt.as_ref().expect("interactive edt dsl"),
                )
            } else {
                let one_shot_edt = EdtDsl::new(
                    edt.clone(),
                    config.work_path.join("edt-workspace"),
                    utilities.runner_for(UtilityType::EdtCli),
                );
                prepare_edt_external_artifacts(config, source_set, &one_shot_edt)
            };
            match export_result {
                Ok(descriptors) => {
                    match &plan {
                        StepPlan::Execute { commit, .. } => match commit {
                            StepCommit::Prepared(prepared) => {
                                if let Err(error) = analyzer::commit_success(
                                    &edt_context,
                                    &config.work_path,
                                    prepared,
                                ) {
                                    let app_error = AppError::Runtime(error.to_string());
                                    let result = fail_with_remaining_steps(
                                        started,
                                        steps,
                                        ordered_source_sets
                                            .iter()
                                            .skip(index)
                                            .copied()
                                            .collect::<Vec<_>>(),
                                        source_set,
                                        BuildMode::EdtExport,
                                        app_error.to_string(),
                                    );
                                    return Err(BuildExecutionFailure::with_payload(
                                        app_error, result,
                                    ));
                                }
                            }
                            StepCommit::RescanFull { recover_storage } => {
                                if let Err(app_error) = commit_full_rescan(
                                    &edt_context,
                                    &config.work_path,
                                    *recover_storage,
                                ) {
                                    let result = fail_with_remaining_steps(
                                        started,
                                        steps,
                                        ordered_source_sets
                                            .iter()
                                            .skip(index)
                                            .copied()
                                            .collect::<Vec<_>>(),
                                        source_set,
                                        BuildMode::EdtExport,
                                        app_error.to_string(),
                                    );
                                    return Err(BuildExecutionFailure::with_payload(
                                        app_error, result,
                                    ));
                                }
                            }
                        },
                        StepPlan::Skip { .. } => {}
                    }
                    steps.push(BuildStep {
                        source_set: source_set.name.clone(),
                        mode: BuildMode::EdtExport,
                        ok: true,
                        message: Some(format!(
                            "exported {} external artifact(s) to designer runtime",
                            descriptors.len()
                        )),
                        duration_ms: export_started.elapsed().as_millis() as u64,
                    })
                }
                Err(error) => {
                    let result = fail_with_remaining_steps(
                        started,
                        steps,
                        ordered_source_sets
                            .iter()
                            .skip(index)
                            .copied()
                            .collect::<Vec<_>>(),
                        source_set,
                        BuildMode::EdtExport,
                        error.to_string(),
                    );
                    return Err(BuildExecutionFailure::with_payload(error, result));
                }
            }
            continue;
        }

        match plan {
            StepPlan::Skip { message, ok } => {
                steps.push(BuildStep {
                    source_set: source_set.name.clone(),
                    mode: BuildMode::Skipped,
                    ok,
                    message: Some(message),
                    duration_ms: 0,
                });
            }
            StepPlan::Execute {
                mode,
                message,
                partial_paths: _,
                commit,
            } => {
                let edt = match edt_binary.clone() {
                    Some(path) => path,
                    None => {
                        let location = match utilities.locate(UtilityType::EdtCli) {
                            Ok(location) => location,
                            Err(error) => {
                                let result = fail_with_remaining_steps(
                                    started,
                                    steps,
                                    ordered_source_sets
                                        .iter()
                                        .skip(index)
                                        .copied()
                                        .collect::<Vec<_>>(),
                                    source_set,
                                    BuildMode::EdtExport,
                                    error.to_string(),
                                );
                                return Err(BuildExecutionFailure::with_payload(
                                    AppError::Platform(error.to_string()),
                                    result,
                                ));
                            }
                        };
                        edt_binary = Some(location.path.clone());
                        location.path
                    }
                };

                let export_started = Instant::now();
                info!(
                    "[EDT] Конвертация в файлы конфигуратора: {}",
                    source_set.name
                );
                let export_result = if config.tools.edt_cli.interactive_mode {
                    if interactive_edt.is_none() {
                        interactive_edt = Some(
                            match EdtDsl::new_interactive(
                                edt.clone(),
                                config.work_path.join("edt-workspace"),
                                Duration::from_millis(config.tools.edt_cli.startup_timeout_ms),
                                Duration::from_millis(config.tools.edt_cli.command_timeout_ms),
                            ) {
                                Ok(dsl) => dsl,
                                Err(error) => {
                                    let app_error = AppError::Platform(error.to_string());
                                    let result = fail_with_remaining_steps(
                                        started,
                                        steps,
                                        ordered_source_sets
                                            .iter()
                                            .skip(index)
                                            .copied()
                                            .collect::<Vec<_>>(),
                                        source_set,
                                        BuildMode::EdtExport,
                                        app_error.to_string(),
                                    );
                                    return Err(BuildExecutionFailure::with_payload(
                                        app_error, result,
                                    ));
                                }
                            },
                        );
                    }
                    execute_edt_export_step(
                        config,
                        interactive_edt.as_ref().expect("interactive edt dsl"),
                        source_set,
                        &edt_context,
                        &designer_context,
                    )
                } else {
                    let one_shot_edt = EdtDsl::new(
                        edt.clone(),
                        config.work_path.join("edt-workspace"),
                        utilities.runner_for(UtilityType::EdtCli),
                    );
                    execute_edt_export_step(
                        config,
                        &one_shot_edt,
                        source_set,
                        &edt_context,
                        &designer_context,
                    )
                };
                if let Err(error) = export_result {
                    let result = fail_with_remaining_steps(
                        started,
                        steps,
                        ordered_source_sets
                            .iter()
                            .skip(index)
                            .copied()
                            .collect::<Vec<_>>(),
                        source_set,
                        BuildMode::EdtExport,
                        error.to_string(),
                    );
                    return Err(BuildExecutionFailure::with_payload(error, result));
                }

                steps.push(BuildStep {
                    source_set: source_set.name.clone(),
                    mode: BuildMode::EdtExport,
                    ok: true,
                    message: Some("EDT export completed".to_owned()),
                    duration_ms: export_started.elapsed().as_millis() as u64,
                });

                let load_started = Instant::now();
                let load_result = match config.builder {
                    BuilderBackend::Designer => {
                        let designer = match designer_binary.clone() {
                            Some(path) => path,
                            None => {
                                let location = match utilities.locate(UtilityType::V8) {
                                    Ok(location) => location,
                                    Err(error) => {
                                        let result = fail_with_remaining_steps(
                                            started,
                                            steps,
                                            ordered_source_sets
                                                .iter()
                                                .skip(index)
                                                .copied()
                                                .collect::<Vec<_>>(),
                                            source_set,
                                            mode.clone(),
                                            error.to_string(),
                                        );
                                        return Err(BuildExecutionFailure::with_payload(
                                            AppError::Platform(error.to_string()),
                                            result,
                                        ));
                                    }
                                };
                                designer_binary = Some(location.path.clone());
                                location.path
                            }
                        };
                        execute_source_set_step(
                            config,
                            &designer,
                            utilities.runner_for(UtilityType::V8),
                            source_set,
                            &designer_context,
                            &edt_context,
                            index,
                            None,
                            &commit,
                        )
                    }
                    BuilderBackend::Ibcmd => {
                        let ibcmd = match ibcmd_binary.clone() {
                            Some(path) => path,
                            None => {
                                let location = match utilities.locate(UtilityType::Ibcmd) {
                                    Ok(location) => location,
                                    Err(error) => {
                                        let result = fail_with_remaining_steps(
                                            started,
                                            steps,
                                            ordered_source_sets
                                                .iter()
                                                .skip(index)
                                                .copied()
                                                .collect::<Vec<_>>(),
                                            source_set,
                                            mode.clone(),
                                            error.to_string(),
                                        );
                                        return Err(BuildExecutionFailure::with_payload(
                                            AppError::Platform(error.to_string()),
                                            result,
                                        ));
                                    }
                                };
                                ibcmd_binary = Some(location.path.clone());
                                location.path
                            }
                        };
                        execute_source_set_step_ibcmd(
                            config,
                            &ibcmd,
                            utilities.runner_for(UtilityType::Ibcmd),
                            source_set,
                            &designer_context,
                            &edt_context,
                            None,
                            &commit,
                        )
                    }
                };
                match load_result {
                    Ok(()) => steps.push(BuildStep {
                        source_set: source_set.name.clone(),
                        mode,
                        ok: true,
                        message: Some(message),
                        duration_ms: load_started.elapsed().as_millis() as u64,
                    }),
                    Err(error) => {
                        let result = fail_with_remaining_steps(
                            started,
                            steps,
                            ordered_source_sets
                                .iter()
                                .skip(index)
                                .copied()
                                .collect::<Vec<_>>(),
                            source_set,
                            mode,
                            error.to_string(),
                        );
                        return Err(BuildExecutionFailure::with_payload(error, result));
                    }
                }
            }
        }
    }

    Ok(BuildResult {
        ok: true,
        steps,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

fn analyze_contexts_by_name(
    service: &SourceSetsService<'_>,
    contexts: &[SourceSetContext],
) -> HashMap<String, Result<AnalysisOutcome, analyzer::ChangeDetectionError>> {
    service
        .analyze_contexts(contexts)
        .into_iter()
        .map(|analysis| (analysis.context.name().to_owned(), analysis.outcome))
        .collect()
}

fn ordered_source_sets(config: &AppConfig) -> Vec<&SourceSetConfig> {
    let mut configuration = Vec::new();
    let mut extensions = Vec::new();
    let mut external_processors = Vec::new();
    let mut external_reports = Vec::new();

    for source_set in &config.source_sets {
        match source_set.purpose {
            SourceSetPurpose::Configuration => configuration.push(source_set),
            SourceSetPurpose::Extension => extensions.push(source_set),
            SourceSetPurpose::ExternalDataProcessors => external_processors.push(source_set),
            SourceSetPurpose::ExternalReports => external_reports.push(source_set),
        }
    }

    configuration.extend(extensions);
    configuration.extend(external_processors);
    configuration.extend(external_reports);
    configuration
}

fn execute_edt_export_step(
    config: &AppConfig,
    dsl: &EdtDsl<'_>,
    source_set: &SourceSetConfig,
    edt_context: &SourceSetContext,
    designer_context: &SourceSetContext,
) -> Result<(), AppError> {
    let export_target = reserved_source_set_dir(&config.work_path, &source_set.name);
    let project_name = resolve_edt_project_name(source_set, edt_context);
    recreate_directory(&export_target).map_err(|error| {
        AppError::Runtime(format!(
            "failed to prepare EDT export directory '{}': {error}",
            export_target.display()
        ))
    })?;
    let export_result = dsl
        .export_project(&project_name, designer_context.path())
        .map_err(|error| AppError::Platform(error.to_string()))?;
    ensure_platform_success("edt_export", source_set, &export_result)
}

fn resolve_edt_project_name(
    source_set: &SourceSetConfig,
    edt_context: &SourceSetContext,
) -> String {
    let project_file = edt_context.path().join(".project");
    std::fs::read_to_string(&project_file)
        .ok()
        .and_then(|contents| extract_xml_tag_text(&contents, "name"))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| source_set.name.clone())
}

fn extract_xml_tag_text(contents: &str, tag_name: &str) -> Option<String> {
    let open_tag = format!("<{tag_name}>");
    let close_tag = format!("</{tag_name}>");
    let start = contents.find(&open_tag)? + open_tag.len();
    let rest = &contents[start..];
    let end = rest.find(&close_tag)?;
    Some(rest[..end].trim().to_owned())
}

fn recreate_directory(path: &Path) -> std::io::Result<()> {
    remove_storage_path(path)?;
    std::fs::create_dir_all(path)
}

fn execute_source_set_step(
    config: &AppConfig,
    binary: &Path,
    runner: &dyn ProcessRunner,
    source_set: &SourceSetConfig,
    load_context: &SourceSetContext,
    commit_context: &SourceSetContext,
    step_index: usize,
    partial_paths: Option<&[PathBuf]>,
    commit: &StepCommit,
) -> Result<(), AppError> {
    if partial_paths.is_some() {
        info!(
            "[Конфигуратор] Загрузка изменений в базу: {}",
            source_set.name
        );
    } else {
        info!("[Конфигуратор] Загрузка в базу: {}", source_set.name);
    }
    let load_result = if let Some(paths) = partial_paths {
        let list_file = partial_list_file(&config.work_path).map_err(|error| {
            AppError::Runtime(format!("failed to create partial list file: {error}"))
        })?;
        partial_load::write_list_file(paths, load_context.path(), list_file.path()).map_err(
            |error| AppError::Runtime(format!("failed to write partial load list: {error}")),
        )?;
        build_designer_dsl(config, binary, runner, &source_set.name, step_index, "load")?
            .load_config_from_files_partial(
                load_context.path(),
                list_file.path(),
                extension_name(source_set),
            )
            .map_err(|error| AppError::Platform(error.to_string()))?
    } else {
        build_designer_dsl(config, binary, runner, &source_set.name, step_index, "load")?
            .load_config_from_files_full(load_context.path(), extension_name(source_set))
            .map_err(|error| AppError::Platform(error.to_string()))?
    };
    ensure_platform_success("load", source_set, &load_result)?;

    debug!(
        source_set = source_set.name.as_str(),
        "updating database configuration after load"
    );
    let update_result = build_designer_dsl(
        config,
        binary,
        runner,
        &source_set.name,
        step_index,
        "update",
    )?
    .update_db_cfg(extension_name(source_set))
    .map_err(|error| AppError::Platform(error.to_string()))?;
    ensure_platform_success("update_db_cfg", source_set, &update_result)?;

    match commit {
        StepCommit::Prepared(prepared) => {
            debug!(
                source_set = source_set.name.as_str(),
                "committing prepared change-detection state"
            );
            analyzer::commit_success(commit_context, &config.work_path, prepared)
                .map_err(|error| AppError::Runtime(error.to_string()))
        }
        StepCommit::RescanFull { recover_storage } => {
            debug!(
                source_set = source_set.name.as_str(),
                recover_storage, "rescanning source-set state after full build"
            );
            commit_full_rescan(commit_context, &config.work_path, *recover_storage)
        }
    }
}

fn execute_source_set_step_ibcmd(
    config: &AppConfig,
    binary: &Path,
    runner: &dyn ProcessRunner,
    source_set: &SourceSetConfig,
    load_context: &SourceSetContext,
    commit_context: &SourceSetContext,
    partial_paths: Option<&[PathBuf]>,
    commit: &StepCommit,
) -> Result<(), AppError> {
    if partial_paths.is_some() {
        info!("[ibcmd] Загрузка изменений в базу: {}", source_set.name);
    } else {
        info!("[ibcmd] Загрузка в базу: {}", source_set.name);
    }

    let dsl = build_ibcmd_dsl(config, binary, runner)?;
    let extension = extension_name(source_set);
    let load_result = if let Some(paths) = partial_paths {
        let rel_paths =
            partial_load::relative_paths(paths, load_context.path()).map_err(|error| {
                AppError::Runtime(format!("failed to convert partial paths: {error}"))
            })?;
        dsl.config_import_partial(load_context.path(), &rel_paths, extension)
            .map_err(map_ibcmd_error)?
    } else {
        dsl.config_import_full(load_context.path(), extension)
            .map_err(map_ibcmd_error)?
    };
    ensure_platform_success("load", source_set, &load_result)?;

    debug!(
        source_set = source_set.name.as_str(),
        "applying database configuration after ibcmd load"
    );
    let apply_result = dsl
        .config_apply(extension, DynamicUpdateMode::Auto)
        .map_err(map_ibcmd_error)?;
    ensure_platform_success("apply", source_set, &apply_result)?;

    match commit {
        StepCommit::Prepared(prepared) => {
            debug!(
                source_set = source_set.name.as_str(),
                "committing prepared change-detection state"
            );
            analyzer::commit_success(commit_context, &config.work_path, prepared)
                .map_err(|error| AppError::Runtime(error.to_string()))
        }
        StepCommit::RescanFull { recover_storage } => {
            debug!(
                source_set = source_set.name.as_str(),
                recover_storage, "rescanning source-set state after full build"
            );
            commit_full_rescan(commit_context, &config.work_path, *recover_storage)
        }
    }
}

fn commit_full_rescan(
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

fn remove_storage_path(path: &Path) -> std::io::Result<()> {
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

fn build_designer_dsl<'a>(
    config: &AppConfig,
    binary: &Path,
    runner: &'a dyn ProcessRunner,
    source_set_name: &str,
    step_index: usize,
    action: &str,
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
    ))
}

fn build_ibcmd_dsl<'a>(
    config: &AppConfig,
    binary: &Path,
    runner: &'a dyn ProcessRunner,
) -> Result<IbcmdDsl<'a>, AppError> {
    let connection =
        IbcmdConnection::from_v8_connection(&config.v8_connection()).map_err(map_ibcmd_error)?;

    Ok(IbcmdDsl::new(binary.to_path_buf(), connection, runner))
}

fn map_ibcmd_error(error: IbcmdError) -> AppError {
    match error {
        IbcmdError::ServerConnectionNotSupported => AppError::Validation(error.to_string()),
        IbcmdError::Spawn(_) => AppError::Platform(error.to_string()),
    }
}

fn extension_name(source_set: &SourceSetConfig) -> Option<&str> {
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

fn fail_with_remaining_steps(
    started: Instant,
    mut completed_steps: Vec<BuildStep>,
    remaining: Vec<&SourceSetConfig>,
    failed_source_set: &SourceSetConfig,
    failed_mode: BuildMode,
    message: String,
) -> BuildResult {
    completed_steps.push(BuildStep {
        source_set: failed_source_set.name.clone(),
        mode: failed_mode,
        ok: false,
        message: Some(message),
        duration_ms: 0,
    });

    for source_set in remaining.into_iter().skip(1) {
        completed_steps.push(BuildStep {
            source_set: source_set.name.clone(),
            mode: BuildMode::Skipped,
            ok: false,
            message: Some("aborted after previous failure".to_owned()),
            duration_ms: 0,
        });
    }

    BuildResult {
        ok: false,
        steps: completed_steps,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::{run_build, BUILD_COMMAND};
    use crate::change_detection::hash_storage::HashStorage;
    use crate::change_detection::source_sets::SourceSetsService;
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, PlatformToolConfig, SourceFormat, SourceSetConfig,
        SourceSetPurpose, TestsConfig, ToolsConfig,
    };
    use crate::domain::build::BuildMode;
    use crate::output::json::Envelope;
    use crate::use_cases::request::BuildRequest as BuildArgs;
    use crate::use_cases::result::UseCaseErrorKind;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }

    #[cfg(unix)]
    fn write_designer_script(path: &Path, calls_log: &Path, fail_pattern: Option<&str>) {
        let pattern_branch = fail_pattern
            .map(|pattern| {
                format!(
                    "if printf '%s' \"$args\" | grep -F -q -- '{}'; then exit 17; fi",
                    pattern
                )
            })
            .unwrap_or_default();
        let body = format!(
            "args=\"$*\"\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf 'designer log for %s\\n' \"$args\" > \"$out\"; fi\nprintf '%s\\n' \"$args\" >> \"{}\"\n{}\nexit 0",
            calls_log.display(),
            pattern_branch
        );

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create dirs");
        }
        fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        make_executable(path);
    }

    #[cfg(unix)]
    fn write_ibcmd_script(path: &Path, calls_log: &Path, fail_pattern: Option<&str>) {
        let pattern_branch = fail_pattern
            .map(|pattern| {
                format!(
                    "if printf '%s' \"$args\" | grep -F -q -- '{}'; then exit 17; fi",
                    pattern
                )
            })
            .unwrap_or_default();
        let body = format!(
            "args=\"$*\"\nprintf '%s\\n' \"$args\" >> \"{}\"\n{}\nexit 0",
            calls_log.display(),
            pattern_branch
        );
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create dirs");
        }
        fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        make_executable(path);
    }

    #[cfg(unix)]
    fn write_edt_script(path: &Path, calls_log: &Path, fail_pattern: Option<&str>) {
        let pattern_branch = fail_pattern
            .map(|pattern| {
                format!(
                    "if printf '%s' \"$args\" | grep -F -q -- '{}'; then exit 19; fi",
                    pattern
                )
            })
            .unwrap_or_default();
        let body = format!(
            "args=\"$*\"\nproject=\"\"\ntarget=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"--project-name\" ]; then project=\"$arg\"; fi\n  if [ \"$prev\" = \"--configuration-files\" ]; then target=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$target\" ]; then mkdir -p \"$target\"; printf 'exported from %s\\n' \"$project\" > \"$target/exported.txt\"; fi\nprintf '%s\\n' \"$args\" >> \"{}\"\n{}\nexit 0",
            calls_log.display(),
            pattern_branch
        );

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create dirs");
        }
        fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        make_executable(path);
    }

    #[cfg(unix)]
    fn write_interactive_edt_script(path: &Path, calls_log: &Path) {
        let body = format!(
            "set -eu\n\
             prompt() {{ printf '1C:EDT>'; }}\n\
             printf 'START\\n' >> '{}'\n\
             trap 'printf \"EXIT\\\\n\" >> \"{}\"' EXIT\n\
             prompt\n\
             while IFS= read -r line; do\n\
               printf '%s\\n' \"$line\" >> '{}'\n\
               eval \"set -- $line\"\n\
               cmd=\"${{1:-}}\"\n\
               if [ \"$#\" -gt 0 ]; then shift; fi\n\
               case \"$cmd\" in\n\
                 cd)\n\
                   prompt\n\
                   ;;\n\
                 export)\n\
                   project=\"\"\n\
                   target=\"\"\n\
                   prev=\"\"\n\
                   for arg in \"$@\"; do\n\
                     if [ \"$prev\" = \"--project-name\" ]; then project=\"$arg\"; fi\n\
                     if [ \"$prev\" = \"--configuration-files\" ]; then target=\"$arg\"; fi\n\
                     prev=\"$arg\"\n\
                   done\n\
                   if [ -n \"$target\" ]; then mkdir -p \"$target\"; printf 'exported from %s\\n' \"$project\" > \"$target/exported.txt\"; fi\n\
                   prompt\n\
                   ;;\n\
                 *)\n\
                   printf 'unknown:%s\\n' \"$line\"\n\
                   prompt\n\
                   ;;\n\
               esac\n\
             done\n",
            calls_log.display(),
            calls_log.display(),
            calls_log.display()
        );

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create dirs");
        }
        fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        make_executable(path);
    }

    fn build_config(
        base_path: &Path,
        work_path: &Path,
        platform_path: &Path,
        threshold: usize,
        format: SourceFormat,
        builder: BuilderBackend,
    ) -> AppConfig {
        AppConfig {
            base_path: base_path.to_path_buf(),
            work_path: work_path.to_path_buf(),
            format,
            builder,
            connection: "File=/tmp/ib".to_owned(),
            credentials: Default::default(),
            source_sets: vec![
                SourceSetConfig {
                    name: "main".to_owned(),
                    purpose: SourceSetPurpose::Configuration,
                    path: PathBuf::from("main"),
                },
                SourceSetConfig {
                    name: "ext".to_owned(),
                    purpose: SourceSetPurpose::Extension,
                    path: PathBuf::from("ext"),
                },
            ],
            build: BuildConfig {
                partial_load_threshold: threshold,
            },
            tools: ToolsConfig {
                platform: PlatformToolConfig {
                    path: Some(platform_path.to_path_buf()),
                    version: None,
                },
                ..ToolsConfig::default()
            },
            mcp: Default::default(),
            tests: TestsConfig::default(),
        }
    }

    fn build_edt_config(
        base_path: &Path,
        work_path: &Path,
        platform_path: &Path,
        edt_cli_path: &Path,
    ) -> AppConfig {
        AppConfig {
            base_path: base_path.to_path_buf(),
            work_path: work_path.to_path_buf(),
            format: SourceFormat::Edt,
            builder: BuilderBackend::Designer,
            connection: "File=/tmp/ib".to_owned(),
            credentials: Default::default(),
            source_sets: vec![
                SourceSetConfig {
                    name: "main".to_owned(),
                    purpose: SourceSetPurpose::Configuration,
                    path: PathBuf::from("main"),
                },
                SourceSetConfig {
                    name: "ext".to_owned(),
                    purpose: SourceSetPurpose::Extension,
                    path: PathBuf::from("ext"),
                },
            ],
            build: BuildConfig {
                partial_load_threshold: 20,
            },
            tools: ToolsConfig {
                platform: PlatformToolConfig {
                    path: Some(platform_path.to_path_buf()),
                    version: None,
                },
                enterprise: Default::default(),
                edt_cli: crate::config::model::EdtCliConfig {
                    path: Some(edt_cli_path.to_path_buf()),
                    auto_start: false,
                    ..Default::default()
                },
            },
            mcp: Default::default(),
            tests: TestsConfig::default(),
        }
    }

    fn create_source_tree(base_path: &Path) {
        fs::create_dir_all(base_path.join("main").join("Catalogs.Items")).expect("main dir");
        fs::create_dir_all(base_path.join("ext").join("CommonModules")).expect("ext dir");
        fs::write(
            base_path
                .join("main")
                .join("Catalogs.Items")
                .join("ObjectModule.bsl"),
            "procedure Test() endprocedure",
        )
        .expect("main bsl");
        fs::write(
            base_path
                .join("main")
                .join("Catalogs.Items")
                .join("ObjectModule.xml"),
            "<MetaDataObject />",
        )
        .expect("main xml");
        fs::write(
            base_path
                .join("ext")
                .join("CommonModules")
                .join("Module.bsl"),
            "procedure Test() endprocedure",
        )
        .expect("ext bsl");
    }

    fn prime_snapshots(config: &AppConfig) {
        let service = SourceSetsService::new(config);
        for context in service.designer_contexts() {
            crate::change_detection::analyzer::rescan_and_commit_full(&context, &config.work_path)
                .expect("prime snapshot");
        }
    }

    fn prime_edt_snapshots(config: &AppConfig) {
        let service = SourceSetsService::new(config);
        for context in service.edt_contexts() {
            crate::change_detection::analyzer::rescan_and_commit_full(&context, &config.work_path)
                .expect("prime edt snapshot");
        }
    }

    fn storage_generation(config: &AppConfig, source_set_name: &str) -> u64 {
        let service = SourceSetsService::new(config);
        let context = service
            .designer_contexts()
            .into_iter()
            .find(|context| context.name() == source_set_name)
            .expect("context");
        HashStorage::new(context.storage_path(&config.work_path))
            .load_snapshot()
            .expect("snapshot")
            .generation
    }

    fn edt_storage_generation(config: &AppConfig, source_set_name: &str) -> u64 {
        let service = SourceSetsService::new(config);
        let context = service
            .edt_contexts()
            .into_iter()
            .find(|context| context.name() == source_set_name)
            .expect("edt context");
        HashStorage::new(context.storage_path(&config.work_path))
            .load_snapshot()
            .expect("snapshot")
            .generation
    }

    #[cfg(unix)]
    #[test]
    fn ibcmd_build_dispatch_uses_ibcmd_utility() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("ibcmd");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_ibcmd_script(&script, &calls, None);
        let config = build_config(
            &base,
            &work,
            &script,
            20,
            SourceFormat::Designer,
            BuilderBackend::Ibcmd,
        );
        let result = run_build(&config, &BuildArgs { full_rebuild: true }).expect("build");

        assert!(result.ok);
        let calls_text = fs::read_to_string(&calls).expect("calls");
        assert!(calls_text.contains("config import"));
        assert!(calls_text.contains("config apply"));
    }

    #[cfg(unix)]
    #[test]
    fn ibcmd_apply_failure_does_not_commit_state() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("ibcmd");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_ibcmd_script(&script, &calls, Some("config apply"));
        let config = build_config(
            &base,
            &work,
            &script,
            20,
            SourceFormat::Designer,
            BuilderBackend::Ibcmd,
        );
        prime_snapshots(&config);
        let generation_before = storage_generation(&config, "main");

        fs::write(
            base.join("main")
                .join("Catalogs.Items")
                .join("ObjectModule.bsl"),
            "procedure Test()\n  // changed\nendprocedure",
        )
        .expect("modify main");

        let failure = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
            },
        )
        .expect_err("expected failure");
        let result = failure
            .payload
            .expect("build failures should preserve a structured payload");

        assert!(!result.ok);
        assert_eq!(generation_before, storage_generation(&config, "main"));
    }

    #[cfg(unix)]
    #[test]
    fn edt_build_exports_then_loads_from_work_path_and_commits_edt_snapshot() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform_script = dir.path().join("platform").join("bin").join("1cv8");
        let edt_script = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer-calls.log");
        let edt_calls = dir.path().join("edt-calls.log");
        create_source_tree(&base);
        write_designer_script(&platform_script, &designer_calls, None);
        write_edt_script(&edt_script, &edt_calls, None);
        let config = build_edt_config(&base, &work, &dir.path().join("platform"), &edt_script);
        prime_edt_snapshots(&config);

        fs::write(
            base.join("main")
                .join("Catalogs.Items")
                .join("ObjectModule.bsl"),
            "procedure Test()\n  // changed in edt\nendprocedure",
        )
        .expect("modify edt main");

        let result = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
            },
        )
        .expect("build");
        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");
        let designer_calls_text = fs::read_to_string(&designer_calls).expect("designer calls");

        assert!(result.ok);
        assert!(result
            .steps
            .iter()
            .any(|step| matches!(step.mode, BuildMode::EdtExport) && step.ok));
        assert!(edt_calls_text.contains("export --project-name main"));
        assert!(designer_calls_text.contains("/LoadConfigFromFiles"));
        assert!(designer_calls_text.contains(
            work.join("designer")
                .join("main")
                .display()
                .to_string()
                .as_str()
        ));
        assert_eq!(edt_storage_generation(&config, "main"), 2);
    }

    #[cfg(unix)]
    #[test]
    fn edt_build_with_ibcmd_exports_then_imports_via_ibcmd() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let ibcmd_script = dir.path().join("ibcmd");
        let edt_script = dir.path().join("edt").join("1cedtcli");
        let ibcmd_calls = dir.path().join("ibcmd-calls.log");
        let edt_calls = dir.path().join("edt-calls.log");
        create_source_tree(&base);
        write_ibcmd_script(&ibcmd_script, &ibcmd_calls, None);
        write_edt_script(&edt_script, &edt_calls, None);
        let mut config = build_edt_config(&base, &work, &ibcmd_script, &edt_script);
        config.builder = BuilderBackend::Ibcmd;
        prime_edt_snapshots(&config);

        fs::write(
            base.join("main")
                .join("Catalogs.Items")
                .join("ObjectModule.bsl"),
            "procedure Test()\n  // changed in edt\nendprocedure",
        )
        .expect("modify edt main");

        let result = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
            },
        )
        .expect("build");
        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");
        let ibcmd_calls_text = fs::read_to_string(&ibcmd_calls).expect("ibcmd calls");

        assert!(result.ok);
        assert!(result
            .steps
            .iter()
            .any(|step| matches!(step.mode, BuildMode::EdtExport) && step.ok));
        assert!(edt_calls_text.contains("export --project-name main"));
        assert!(ibcmd_calls_text.contains("infobase config import"));
        assert!(ibcmd_calls_text.contains("infobase config apply"));
        assert!(ibcmd_calls_text.contains(
            work.join("designer")
                .join("main")
                .display()
                .to_string()
                .as_str()
        ));
        assert_eq!(edt_storage_generation(&config, "main"), 2);
    }

    #[cfg(unix)]
    #[test]
    fn edt_build_prefers_project_name_from_dot_project_file() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform_script = dir.path().join("platform").join("bin").join("1cv8");
        let edt_script = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer-calls.log");
        let edt_calls = dir.path().join("edt-calls.log");
        create_source_tree(&base);
        fs::create_dir_all(base.join("exts").join("client-mcp")).expect("ext dir");
        fs::write(
            base.join("exts").join("client-mcp").join(".project"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<projectDescription>\n  <name>client_mcp</name>\n</projectDescription>\n",
        )
        .expect("write .project");
        fs::write(
            base.join("exts").join("client-mcp").join("Module.bsl"),
            "procedure Test()\n  // changed ext\nendprocedure",
        )
        .expect("write ext");
        write_designer_script(&platform_script, &designer_calls, None);
        write_edt_script(&edt_script, &edt_calls, None);

        let mut config = build_edt_config(&base, &work, &dir.path().join("platform"), &edt_script);
        config.source_sets = vec![SourceSetConfig {
            name: "client_mcp".to_owned(),
            purpose: SourceSetPurpose::Extension,
            path: PathBuf::from("exts/client-mcp"),
        }];
        prime_edt_snapshots(&config);
        fs::write(
            base.join("exts").join("client-mcp").join("Module.bsl"),
            "procedure Test()\n  // changed after snapshot\nendprocedure",
        )
        .expect("modify ext");

        let result = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
            },
        )
        .expect("build");
        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");

        assert!(result.ok);
        assert!(edt_calls_text.contains("export --project-name client_mcp"));
        assert!(!edt_calls_text.contains("export --project-name client-mcp"));
    }

    #[cfg(unix)]
    #[test]
    fn edt_build_falls_back_to_source_set_name_when_dot_project_is_missing() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform_script = dir.path().join("platform").join("bin").join("1cv8");
        let edt_script = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer-calls.log");
        let edt_calls = dir.path().join("edt-calls.log");
        fs::create_dir_all(base.join("exts").join("client-mcp")).expect("ext dir");
        fs::write(
            base.join("exts").join("client-mcp").join("Module.bsl"),
            "procedure Test()\n  // changed ext\nendprocedure",
        )
        .expect("write ext");
        write_designer_script(&platform_script, &designer_calls, None);
        write_edt_script(&edt_script, &edt_calls, None);

        let mut config = build_edt_config(&base, &work, &dir.path().join("platform"), &edt_script);
        config.source_sets = vec![SourceSetConfig {
            name: "client_mcp".to_owned(),
            purpose: SourceSetPurpose::Extension,
            path: PathBuf::from("exts/client-mcp"),
        }];
        prime_edt_snapshots(&config);
        fs::write(
            base.join("exts").join("client-mcp").join("Module.bsl"),
            "procedure Test()\n  // changed after snapshot\nendprocedure",
        )
        .expect("modify ext");

        let result = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
            },
        )
        .expect("build");
        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");

        assert!(result.ok);
        assert!(edt_calls_text.contains("export --project-name client_mcp"));
    }

    #[cfg(unix)]
    #[test]
    fn edt_build_reuses_single_interactive_session_for_multiple_source_sets() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform_script = dir.path().join("platform").join("bin").join("1cv8");
        let edt_script = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer-calls.log");
        let edt_calls = dir.path().join("edt-calls.log");
        create_source_tree(&base);
        write_designer_script(&platform_script, &designer_calls, None);
        write_interactive_edt_script(&edt_script, &edt_calls);
        let mut config = build_edt_config(&base, &work, &dir.path().join("platform"), &edt_script);
        config.tools.edt_cli.interactive_mode = true;
        prime_edt_snapshots(&config);

        fs::write(
            base.join("main")
                .join("Catalogs.Items")
                .join("ObjectModule.bsl"),
            "procedure Test()\n  // changed main\nendprocedure",
        )
        .expect("modify edt main");
        fs::write(
            base.join("ext").join("CommonModules").join("Module.bsl"),
            "procedure Test()\n  // changed ext\nendprocedure",
        )
        .expect("modify edt ext");

        let result = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
            },
        )
        .expect("build");
        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");

        assert!(result.ok);
        assert_eq!(edt_calls_text.matches("START").count(), 1);
        assert_eq!(edt_calls_text.matches("EXIT").count(), 1);
        assert_eq!(edt_calls_text.matches("export --project-name").count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn edt_export_failure_stops_pipeline_before_designer_load() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform_script = dir.path().join("platform").join("bin").join("1cv8");
        let edt_script = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer-calls.log");
        let edt_calls = dir.path().join("edt-calls.log");
        create_source_tree(&base);
        write_designer_script(&platform_script, &designer_calls, None);
        write_edt_script(&edt_script, &edt_calls, Some("export --project-name"));
        let config = build_edt_config(&base, &work, &dir.path().join("platform"), &edt_script);
        prime_edt_snapshots(&config);

        fs::write(
            base.join("main")
                .join("Catalogs.Items")
                .join("ObjectModule.bsl"),
            "procedure Test()\n  // changed in edt\nendprocedure",
        )
        .expect("modify edt main");

        let failure = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
            },
        )
        .expect_err("expected failure");
        let result = failure
            .payload
            .expect("build failures should preserve a structured payload");

        assert!(!result.ok);
        assert!(matches!(result.steps[0].mode, BuildMode::EdtExport));
        assert!(!result.steps[0].ok);
        assert!(!designer_calls.exists());
    }

    #[cfg(unix)]
    #[test]
    fn no_changes_skips_platform_invocation() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("1cv8");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_designer_script(&script, &calls, None);
        let config = build_config(
            &base,
            &work,
            &script,
            20,
            SourceFormat::Designer,
            BuilderBackend::Designer,
        );
        prime_snapshots(&config);

        let result = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
            },
        )
        .expect("build");

        assert!(result.ok);
        assert_eq!(result.steps.len(), 2);
        assert!(result
            .steps
            .iter()
            .all(|step| matches!(step.mode, BuildMode::Skipped) && step.ok));
        assert!(!calls.exists());
    }

    #[cfg(unix)]
    #[test]
    fn changed_configuration_runs_partial_load_and_commits_state() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("1cv8");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_designer_script(&script, &calls, None);
        let config = build_config(
            &base,
            &work,
            &script,
            20,
            SourceFormat::Designer,
            BuilderBackend::Designer,
        );
        prime_snapshots(&config);

        fs::write(
            base.join("main")
                .join("Catalogs.Items")
                .join("ObjectModule.bsl"),
            "procedure Test()\n  // changed\nendprocedure",
        )
        .expect("modify main");

        let result = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
            },
        )
        .expect("build");
        let calls_text = fs::read_to_string(&calls).expect("calls");

        assert!(matches!(result.steps[0].mode, BuildMode::Partial { .. }));
        assert!(result.steps[0].ok);
        assert!(calls_text.contains("/LoadConfigFromFiles"));
        assert!(calls_text.contains("-partial"));
        assert!(calls_text.contains("/UpdateDBCfg"));
        assert!(calls_text.contains("-listFile"));
        assert_eq!(storage_generation(&config, "main"), 2);

        let rerun = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
            },
        )
        .expect("rerun");
        assert!(matches!(rerun.steps[0].mode, BuildMode::Skipped));
    }

    #[cfg(unix)]
    #[test]
    fn changed_extension_only_loads_extension_and_preserves_other_storage() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("1cv8");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_designer_script(&script, &calls, None);
        let config = build_config(
            &base,
            &work,
            &script,
            20,
            SourceFormat::Designer,
            BuilderBackend::Designer,
        );
        prime_snapshots(&config);

        fs::write(
            base.join("ext").join("CommonModules").join("Module.bsl"),
            "procedure Test()\n  // ext changed\nendprocedure",
        )
        .expect("modify ext");

        let result = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
            },
        )
        .expect("build");
        let calls_text = fs::read_to_string(&calls).expect("calls");

        assert!(matches!(result.steps[0].mode, BuildMode::Skipped));
        assert!(matches!(result.steps[1].mode, BuildMode::Partial { .. }));
        assert!(calls_text.contains("-Extension ext"));
        assert_eq!(storage_generation(&config, "main"), 1);
        assert_eq!(storage_generation(&config, "ext"), 2);
    }

    #[cfg(unix)]
    #[test]
    fn full_rebuild_bypasses_analysis_and_recovers_from_corrupt_storage() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("1cv8");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_designer_script(&script, &calls, None);
        let config = build_config(
            &base,
            &work,
            &script,
            20,
            SourceFormat::Designer,
            BuilderBackend::Designer,
        );
        prime_snapshots(&config);

        let service = SourceSetsService::new(&config);
        for context in service.designer_contexts() {
            let storage_path = context.storage_path(&config.work_path);
            fs::write(storage_path, "corrupt").expect("corrupt storage");
        }

        let result = run_build(&config, &BuildArgs { full_rebuild: true }).expect("build");
        let calls_text = fs::read_to_string(&calls).expect("calls");

        assert!(result
            .steps
            .iter()
            .all(|step| matches!(step.mode, BuildMode::Full)));
        assert!(!calls_text.contains("-partial"));
        assert_eq!(storage_generation(&config, "main"), 1);
        assert_eq!(storage_generation(&config, "ext"), 1);
    }

    #[cfg(unix)]
    #[test]
    fn full_rebuild_does_not_delete_storage_for_non_corruption_errors() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("1cv8");
        create_source_tree(&base);
        write_designer_script(&script, &dir.path().join("calls.log"), None);
        let config = build_config(
            &base,
            &work,
            &script,
            20,
            SourceFormat::Designer,
            BuilderBackend::Designer,
        );
        prime_snapshots(&config);

        let service = SourceSetsService::new(&config);
        let main_context = service
            .designer_contexts()
            .into_iter()
            .find(|context| context.name() == "main")
            .expect("main context");
        let storage_path = main_context.storage_path(&config.work_path);
        std::fs::remove_file(&storage_path).expect("remove storage file");
        std::fs::create_dir_all(&storage_path).expect("replace with directory");

        let failure = run_build(&config, &BuildArgs { full_rebuild: true }).expect_err("failure");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Runtime);
        assert!(storage_path.exists());
        assert!(storage_path.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn failure_stops_pipeline_and_commits_only_successful_steps() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("1cv8");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_designer_script(&script, &calls, Some("/UpdateDBCfg -Extension ext"));
        let config = build_config(
            &base,
            &work,
            &script,
            20,
            SourceFormat::Designer,
            BuilderBackend::Designer,
        );
        prime_snapshots(&config);

        fs::write(
            base.join("main")
                .join("Catalogs.Items")
                .join("ObjectModule.bsl"),
            "procedure Test()\n  // changed\nendprocedure",
        )
        .expect("modify main");
        fs::write(
            base.join("ext").join("CommonModules").join("Module.bsl"),
            "procedure Test()\n  // changed ext\nendprocedure",
        )
        .expect("modify ext");

        let failure = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
            },
        )
        .expect_err("failure");
        let result = failure
            .payload
            .expect("build failures should preserve a structured payload");
        let calls_text = fs::read_to_string(&calls).expect("calls");

        assert!(result.steps[0].ok);
        assert!(!result.steps[1].ok);
        assert!(result.steps[1]
            .message
            .as_deref()
            .expect("message")
            .contains("update_db_cfg failed for source-set 'ext' with exit code 17"));
        assert!(calls_text.contains("/UpdateDBCfg -Extension ext"));
        assert_eq!(storage_generation(&config, "main"), 2);
        assert_eq!(storage_generation(&config, "ext"), 1);
    }

    #[test]
    fn build_result_stays_json_serializable() {
        let result = crate::domain::build::BuildResult {
            ok: true,
            steps: vec![
                crate::domain::build::BuildStep {
                    source_set: "main".to_owned(),
                    mode: BuildMode::Full,
                    ok: true,
                    message: Some("forced full rebuild".to_owned()),
                    duration_ms: 1,
                },
                crate::domain::build::BuildStep {
                    source_set: "ext".to_owned(),
                    mode: BuildMode::Skipped,
                    ok: false,
                    message: Some("aborted after previous failure".to_owned()),
                    duration_ms: 0,
                },
            ],
            duration_ms: 42,
        };

        let envelope = Envelope::ok(BUILD_COMMAND, result.duration_ms, result);
        let json = serde_json::to_string(&envelope).expect("json");
        assert!(json.contains("\"command\":\"build\""));
    }
}
