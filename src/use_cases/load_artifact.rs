use std::path::{Path, PathBuf};
use std::time::Instant;

use tracing::debug;

use crate::config::model::{AppConfig, BuilderBackend, SourceFormat};
use crate::domain::artifact::{ArtifactKind, ArtifactRef, ArtifactSet, ARTIFACT_ROLE_PLATFORM_LOG};
use crate::domain::artifacts::ArtifactBuildMode;
use crate::domain::execution::{ExecutionError, ExecutionOutcome, ExecutionStatus};
use crate::domain::load::{
    CompatibilityState, LoadExecutionMetadata, LoadMode, LoadResult, LoadTargetKind,
};
use crate::platform::designer::DesignerDsl;
use crate::platform::locator::UtilityType;
use crate::platform::process::ProcessRunner;
use crate::platform::result::PlatformCommandResult;
use crate::platform::utilities::PlatformUtilities;
use crate::support::error::AppError;
use crate::support::temp::platform_logs_dir;
use crate::use_cases::context::{ExecutionContext, ExecutionInterruption, InterruptionSafetyClass};
use crate::use_cases::ibcmd_diagnostics::format_ibcmd_failure_details;
use crate::use_cases::interruption::{
    command_interruption_details, command_interruption_status,
    deferred_process_interruption_details, deferred_process_interruption_warning,
    interruption_before_safe_point_message,
};
use crate::use_cases::progress::log_live_stage;
use crate::use_cases::request::LoadRequest;
use crate::use_cases::result::{UseCaseFailure, UseCaseResult};

const SUPPORTED_LOAD_ERROR: &str =
    "load currently supports only builder=DESIGNER and format=DESIGNER";
const UNSUPPORTED_EXTERNAL_ARTIFACTS_ERROR: &str =
    "load currently supports only .cf and .cfe artifacts";
const UNSUPPORTED_UPDATE_MODE_ERROR: &str =
    "load --mode update is not supported; use --mode load or --mode merge";

pub fn execute(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &LoadRequest,
) -> UseCaseResult<LoadResult> {
    debug!(
        command = context.command().as_str(),
        transport = ?context.transport(),
        mode = ?args.mode,
        artifact = args.artifact_path.as_str(),
        extension = args.extension.as_deref().unwrap_or("<none>"),
        "executing load use case"
    );
    run_load(context, config, args)
}

type LoadExecutionFailure = UseCaseFailure<LoadResult>;

#[derive(Debug, Clone)]
struct ResolvedLoadRequest {
    mode: LoadMode,
    artifact_path: PathBuf,
    artifact_type: ArtifactBuildMode,
    target_kind: LoadTargetKind,
    settings_path: Option<PathBuf>,
    extension: Option<String>,
}

fn run_load(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &LoadRequest,
) -> UseCaseResult<LoadResult> {
    let started = Instant::now();
    let request_snapshot = request_snapshot_for_failure_payload(args);

    if let Some(error) = validate_supported_matrix(config) {
        return Err(LoadExecutionFailure::with_payload(
            error,
            empty_result(
                args.mode,
                PathBuf::from(&args.artifact_path),
                request_snapshot.artifact_type,
                request_snapshot.target_kind,
                CompatibilityState::Unknown,
                request_snapshot.extension,
                started,
                Some(SUPPORTED_LOAD_ERROR.to_owned()),
                None,
                false,
            ),
        ));
    }

    let resolved = match resolve_request(config, args) {
        Ok(resolved) => resolved,
        Err(error) => {
            let message = error.to_string();
            return Err(LoadExecutionFailure::with_payload(
                error,
                empty_result(
                    args.mode,
                    PathBuf::from(&args.artifact_path),
                    request_snapshot.artifact_type,
                    request_snapshot.target_kind,
                    CompatibilityState::Unknown,
                    request_snapshot.extension,
                    started,
                    Some(message),
                    None,
                    false,
                ),
            ));
        }
    };

    if let Some(interruption) = context.interruption() {
        let message = interruption_before_safe_point_message(context, interruption, "load probe");
        return Err(LoadExecutionFailure::with_payload(
            AppError::Runtime(message.clone()),
            interrupted_result_from_resolved(
                &resolved,
                CompatibilityState::Unknown,
                started,
                interruption,
                message,
                None,
            ),
        ));
    }

    let mut utilities = PlatformUtilities::from_config(config);
    let location = match utilities.locate(UtilityType::V8) {
        Ok(location) => location,
        Err(error) => {
            let message = error.to_string();
            return Err(LoadExecutionFailure::with_payload(
                AppError::from(error),
                empty_result_from_resolved(
                    &resolved,
                    CompatibilityState::Unknown,
                    started,
                    Some(message),
                    None,
                    false,
                ),
            ));
        }
    };

    log_live_stage(
        "load: compatibility probe",
        "[Конфигуратор] comparing infobase compatibility",
    );
    let probe_result = match probe_compatibility(
        context,
        config,
        location.path.as_path(),
        utilities.runner_for(UtilityType::V8),
        &resolved,
    ) {
        Ok(result) => result,
        Err((error, platform_log_path)) => {
            let message = error.to_string();
            return Err(LoadExecutionFailure::with_payload(
                error,
                empty_result_from_resolved(
                    &resolved,
                    CompatibilityState::Unknown,
                    started,
                    Some(message),
                    platform_log_path,
                    false,
                ),
            ));
        }
    };

    let compatibility_state = probe_result.state;
    let probe_log_path = probe_result.platform_log_path;
    if let Some(error) = validate_probe_mode_compatibility(&resolved, compatibility_state) {
        let message = error.to_string();
        return Err(LoadExecutionFailure::with_payload(
            error,
            empty_result_from_resolved(
                &resolved,
                compatibility_state,
                started,
                Some(message),
                probe_log_path,
                false,
            ),
        ));
    }

    let apply_dsl = match build_designer_dsl(
        context,
        config,
        location.path.as_path(),
        utilities.runner_for(UtilityType::V8),
        match resolved.mode {
            LoadMode::Load => "load",
            LoadMode::Merge => "merge",
            LoadMode::Update => unreachable!("update mode is rejected during validation"),
        },
        InterruptionSafetyClass::CriticalNonAbortable,
    ) {
        Ok(dsl) => dsl,
        Err((error, platform_log_path)) => {
            let message = error.to_string();
            return Err(LoadExecutionFailure::with_payload(
                error,
                empty_result_from_resolved(
                    &resolved,
                    compatibility_state,
                    started,
                    Some(message),
                    platform_log_path.or(probe_log_path),
                    false,
                ),
            ));
        }
    };

    let apply_stage = match resolved.mode {
        LoadMode::Load => "load: apply",
        LoadMode::Merge => "load: merge",
        LoadMode::Update => unreachable!("update mode is rejected during validation"),
    };
    log_live_stage(apply_stage, "[Конфигуратор] applying artifact");
    let apply_result = match resolved.mode {
        LoadMode::Load => apply_dsl
            .load_cfg(&resolved.artifact_path, resolved.extension.as_deref())
            .map_err(|error| (AppError::from(error), None)),
        LoadMode::Merge => apply_dsl
            .merge_cfg(
                &resolved.artifact_path,
                resolved
                    .settings_path
                    .as_deref()
                    .expect("merge settings were validated"),
                resolved.extension.as_deref(),
            )
            .map_err(|error| (AppError::from(error), None)),
        LoadMode::Update => unreachable!("update mode is rejected during validation"),
    };

    let apply_result = match apply_result {
        Ok(result) => result,
        Err((error, platform_log_path)) => {
            let message = error.to_string();
            return Err(LoadExecutionFailure::with_payload(
                error,
                empty_result_from_resolved(
                    &resolved,
                    compatibility_state,
                    started,
                    Some(message),
                    platform_log_path.or(probe_log_path),
                    false,
                ),
            ));
        }
    };

    let apply_action = match resolved.mode {
        LoadMode::Load => "load",
        LoadMode::Merge => "merge",
        LoadMode::Update => unreachable!("update mode is rejected during validation"),
    };

    if let Err(error) = ensure_platform_success(apply_action, &resolved, &apply_result) {
        let message = error.to_string();
        return Err(LoadExecutionFailure::with_payload(
            error,
            empty_result_from_resolved(
                &resolved,
                compatibility_state,
                started,
                Some(message),
                apply_result.platform_log_path.or(probe_log_path),
                false,
            ),
        ));
    }

    if let Some(interruption) = context.interruption() {
        let message =
            interruption_before_safe_point_message(context, interruption, "update_db_cfg");
        return Err(LoadExecutionFailure::with_payload(
            AppError::Runtime(message.clone()),
            interrupted_result_from_resolved(
                &resolved,
                compatibility_state,
                started,
                interruption,
                message,
                apply_result.platform_log_path.or(probe_log_path),
            ),
        ));
    }

    let update_dsl = match build_designer_dsl(
        context,
        config,
        location.path.as_path(),
        utilities.runner_for(UtilityType::V8),
        "update-db-cfg",
        InterruptionSafetyClass::CriticalNonAbortable,
    ) {
        Ok(dsl) => dsl,
        Err((error, platform_log_path)) => {
            let message = error.to_string();
            return Err(LoadExecutionFailure::with_payload(
                error,
                empty_result_from_resolved(
                    &resolved,
                    compatibility_state,
                    started,
                    Some(message),
                    platform_log_path
                        .or(apply_result.platform_log_path)
                        .or(probe_log_path),
                    false,
                ),
            ));
        }
    };

    log_live_stage(
        "load: update_db_cfg",
        "[Конфигуратор] updating database configuration",
    );
    // `load` mirrors the historical static update — DBA approves the artifact, the runner
    // applies the locked change. Dynamic mode is scoped to `build` per TASK-124.
    let update_result = update_dsl
        .update_db_cfg(resolved.extension.as_deref(), false)
        .map_err(AppError::from);

    let update_result = match update_result {
        Ok(result) => result,
        Err(error) => {
            let message = error.to_string();
            return Err(LoadExecutionFailure::with_payload(
                error,
                empty_result_from_resolved(
                    &resolved,
                    compatibility_state,
                    started,
                    Some(message),
                    apply_result.platform_log_path.or(probe_log_path),
                    false,
                ),
            ));
        }
    };

    if let Err(error) = ensure_platform_success("update_db_cfg", &resolved, &update_result) {
        let message = error.to_string();
        return Err(LoadExecutionFailure::with_payload(
            error,
            empty_result_from_resolved(
                &resolved,
                compatibility_state,
                started,
                Some(message),
                update_result
                    .platform_log_path
                    .or(apply_result.platform_log_path)
                    .or(probe_log_path),
                false,
            ),
        ));
    }

    let deferred_warnings = [
        deferred_interruption_warning("apply", &apply_result),
        deferred_interruption_warning("update_db_cfg", &update_result),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let deferred_interruptions = [
        deferred_process_interruption_details(
            "apply",
            "apply completed successfully",
            &apply_result,
        ),
        deferred_process_interruption_details(
            "update_db_cfg",
            "update_db_cfg completed successfully",
            &update_result,
        ),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let mut execution =
        ExecutionOutcome::new(ExecutionStatus::Succeeded).with_payload(LoadExecutionMetadata {
            applied: true,
            target_kind: resolved.target_kind,
            compatibility_state,
            update_db_cfg_ran: true,
        });
    if !deferred_warnings.is_empty() {
        execution = execution.with_diagnostics(deferred_warnings);
    }
    if !deferred_interruptions.is_empty() {
        execution = execution.with_interruptions(deferred_interruptions);
    }
    Ok(LoadResult {
        mode: resolved.mode,
        artifact_path: resolved.artifact_path,
        artifact_type: resolved.artifact_type,
        extension: resolved.extension,
        duration_ms: started.elapsed().as_millis() as u64,
        execution: with_platform_log_artifact(
            execution,
            update_result
                .platform_log_path
                .or(apply_result.platform_log_path)
                .or(probe_log_path),
        ),
    })
}

struct ProbeResult {
    state: CompatibilityState,
    platform_log_path: Option<PathBuf>,
}

fn probe_compatibility(
    context: &ExecutionContext,
    config: &AppConfig,
    binary: &Path,
    runner: &dyn ProcessRunner,
    resolved: &ResolvedLoadRequest,
) -> Result<ProbeResult, (AppError, Option<PathBuf>)> {
    let report_dir = config.work_path.join("load-probe");
    std::fs::create_dir_all(&report_dir).map_err(|error| {
        (
            AppError::Runtime(format!("failed to prepare load probe dir: {error}")),
            None,
        )
    })?;
    let report_file = report_dir.join(match resolved.target_kind {
        LoadTargetKind::Configuration => "configuration-compare.txt",
        LoadTargetKind::Extension => "extension-compare.txt",
        LoadTargetKind::Unknown => "unknown-compare.txt",
    });

    let dsl = build_designer_dsl(
        context,
        config,
        binary,
        runner,
        "probe",
        InterruptionSafetyClass::GracefulThenKill,
    )?;
    let result = match resolved.target_kind {
        LoadTargetKind::Configuration => dsl.compare_cfg(
            "MainConfiguration",
            None,
            "VendorConfiguration",
            None,
            &report_file,
        ),
        LoadTargetKind::Extension => dsl.compare_cfg(
            "ExtensionConfiguration",
            resolved.extension.as_deref(),
            "ExtensionDBConfiguration",
            resolved.extension.as_deref(),
            &report_file,
        ),
        LoadTargetKind::Unknown => unreachable!("unknown targets are rejected during validation"),
    }
    .map_err(|error| (AppError::from(error), None))?;

    if result.process.exit_code == 0 {
        return Ok(ProbeResult {
            state: CompatibilityState::Supported,
            platform_log_path: result.platform_log_path,
        });
    }

    let combined = format!(
        "{}\n{}\n{}",
        result.process.stdout,
        result.process.stderr,
        result.platform_log.as_deref().unwrap_or_default()
    );
    let state = classify_probe_failure(resolved.target_kind, &combined);
    if state == CompatibilityState::Unknown {
        let error = AppError::Validation(format!(
            "failed to determine infobase compatibility state for {}",
            target_label(resolved)
        ));
        Err((error, result.platform_log_path))
    } else {
        Ok(ProbeResult {
            state,
            platform_log_path: result.platform_log_path,
        })
    }
}

fn classify_probe_failure(
    target_kind: LoadTargetKind,
    combined_output: &str,
) -> CompatibilityState {
    let lower = combined_output.to_ascii_lowercase();
    match target_kind {
        LoadTargetKind::Configuration => {
            if lower.contains("vendorconfiguration")
                || lower.contains("vendor configuration")
                || lower.contains("поддерж")
                || lower.contains("поставщик")
            {
                CompatibilityState::NotSupported
            } else {
                CompatibilityState::Unknown
            }
        }
        LoadTargetKind::Extension => {
            if (lower.contains("extension") || lower.contains("расширен"))
                && (lower.contains("not found") || lower.contains("не найден"))
            {
                CompatibilityState::NotSupported
            } else if (lower.contains("extension") || lower.contains("расширен"))
                && (lower.contains("not supported")
                    || lower.contains("unsupported")
                    || lower.contains("не поддерж"))
            {
                CompatibilityState::NotSupported
            } else {
                CompatibilityState::Unknown
            }
        }
        LoadTargetKind::Unknown => CompatibilityState::Unknown,
    }
}

fn validate_probe_mode_compatibility(
    resolved: &ResolvedLoadRequest,
    state: CompatibilityState,
) -> Option<AppError> {
    match (resolved.mode, state) {
        (_, CompatibilityState::Unknown) => Some(AppError::Validation(format!(
            "failed to determine infobase compatibility state for {}",
            target_label(resolved)
        ))),
        (LoadMode::Load, CompatibilityState::Supported) => Some(AppError::Validation(format!(
            "{} is already compatible with merge; use --mode merge instead",
            target_label(resolved)
        ))),
        (LoadMode::Merge, CompatibilityState::NotSupported) => Some(AppError::Validation(format!(
            "{} is not ready for merge; use --mode load instead",
            target_label(resolved)
        ))),
        _ => None,
    }
}

fn validate_supported_matrix(config: &AppConfig) -> Option<AppError> {
    if config.builder == BuilderBackend::Designer && config.format == SourceFormat::Designer {
        None
    } else {
        Some(AppError::Validation(SUPPORTED_LOAD_ERROR.to_owned()))
    }
}

fn request_snapshot_for_failure_payload(args: &LoadRequest) -> ResolvedLoadRequest {
    let artifact_type =
        infer_artifact_type(&args.artifact_path).unwrap_or(ArtifactBuildMode::Unknown);
    let extension = trim_optional(args.extension.clone());
    let target_kind = match artifact_type {
        ArtifactBuildMode::ExtensionCfe => LoadTargetKind::Extension,
        ArtifactBuildMode::ConfigurationCf => LoadTargetKind::Configuration,
        ArtifactBuildMode::ExternalDataProcessorEpf | ArtifactBuildMode::ExternalReportErf => {
            LoadTargetKind::Unknown
        }
        ArtifactBuildMode::Unknown => LoadTargetKind::Unknown,
    };

    ResolvedLoadRequest {
        mode: args.mode,
        artifact_path: PathBuf::from(&args.artifact_path),
        artifact_type,
        target_kind,
        settings_path: None,
        extension,
    }
}

fn resolve_request(
    config: &AppConfig,
    args: &LoadRequest,
) -> Result<ResolvedLoadRequest, AppError> {
    if args.mode == LoadMode::Update {
        return Err(AppError::Validation(
            UNSUPPORTED_UPDATE_MODE_ERROR.to_owned(),
        ));
    }

    let artifact_path = resolve_existing_file(config, &args.artifact_path, "--path")?;
    let artifact_type = infer_artifact_type(&args.artifact_path)
        .ok_or_else(|| AppError::Validation(UNSUPPORTED_EXTERNAL_ARTIFACTS_ERROR.to_owned()))?;
    if matches!(
        artifact_type,
        ArtifactBuildMode::ExternalDataProcessorEpf | ArtifactBuildMode::ExternalReportErf
    ) {
        return Err(AppError::Validation(
            UNSUPPORTED_EXTERNAL_ARTIFACTS_ERROR.to_owned(),
        ));
    }

    let extension = trim_optional(args.extension.clone());
    let settings_path = match args.mode {
        LoadMode::Merge => Some(resolve_existing_file(
            config,
            args.settings_path.as_deref().ok_or_else(|| {
                AppError::Validation("load --mode merge requires --settings <file>".to_owned())
            })?,
            "--settings",
        )?),
        LoadMode::Load => {
            if args.settings_path.is_some() {
                return Err(AppError::Validation(
                    "--settings is supported only with --mode merge".to_owned(),
                ));
            }
            None
        }
        LoadMode::Update => None,
    };

    let (target_kind, extension) = match artifact_type {
        ArtifactBuildMode::ConfigurationCf => {
            if extension.is_some() {
                return Err(AppError::Validation(
                    ".cf artifacts do not support --extension".to_owned(),
                ));
            }
            (LoadTargetKind::Configuration, None)
        }
        ArtifactBuildMode::ExtensionCfe => {
            let extension = extension.ok_or_else(|| {
                AppError::Validation(".cfe artifacts require --extension <name>".to_owned())
            })?;
            (LoadTargetKind::Extension, Some(extension))
        }
        ArtifactBuildMode::ExternalDataProcessorEpf | ArtifactBuildMode::ExternalReportErf => {
            unreachable!("external artifacts are rejected above")
        }
        ArtifactBuildMode::Unknown => unreachable!("unknown artifacts are rejected above"),
    };

    Ok(ResolvedLoadRequest {
        mode: args.mode,
        artifact_path,
        artifact_type,
        target_kind,
        settings_path,
        extension,
    })
}

fn resolve_existing_file(
    config: &AppConfig,
    raw_path: &str,
    flag: &str,
) -> Result<PathBuf, AppError> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return Err(AppError::Validation(format!(
            "{flag} requires a non-empty file path"
        )));
    }
    let candidate = PathBuf::from(trimmed);
    let candidate = if candidate.is_absolute() {
        candidate
    } else {
        config.base_path.join(candidate)
    };
    if !candidate.exists() {
        return Err(AppError::Validation(format!(
            "{flag} file does not exist: {}",
            candidate.display()
        )));
    }
    if !candidate.is_file() {
        return Err(AppError::Validation(format!(
            "{flag} must point to a file: {}",
            candidate.display()
        )));
    }
    std::fs::canonicalize(&candidate).map_err(|error| {
        AppError::Runtime(format!(
            "failed to canonicalize '{}': {error}",
            candidate.display()
        ))
    })
}

fn infer_artifact_type(raw_path: &str) -> Option<ArtifactBuildMode> {
    let extension = Path::new(raw_path)
        .extension()
        .and_then(|value| value.to_str())?
        .to_ascii_lowercase();
    match extension.as_str() {
        "cf" => Some(ArtifactBuildMode::ConfigurationCf),
        "cfe" => Some(ArtifactBuildMode::ExtensionCfe),
        "epf" => Some(ArtifactBuildMode::ExternalDataProcessorEpf),
        "erf" => Some(ArtifactBuildMode::ExternalReportErf),
        _ => None,
    }
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

fn build_designer_dsl<'a>(
    context: &ExecutionContext,
    config: &AppConfig,
    binary: &Path,
    runner: &'a dyn ProcessRunner,
    action: &str,
    safety: InterruptionSafetyClass,
) -> Result<DesignerDsl<'a>, (AppError, Option<PathBuf>)> {
    let log_dir = platform_logs_dir(&config.work_path).map_err(|error| {
        (
            AppError::Runtime(format!("failed to create platform logs dir: {error}")),
            None,
        )
    })?;
    let log_file = log_dir.join(format!("load-{action}.log"));
    Ok(DesignerDsl::new(
        binary.to_path_buf(),
        config.v8_connection(),
        runner,
        Some(log_file),
    )
    .with_execution_policy(context.process_policy(safety, None)))
}

fn ensure_platform_success(
    action: &str,
    resolved: &ResolvedLoadRequest,
    result: &PlatformCommandResult,
) -> Result<(), AppError> {
    if result.process.exit_code == 0 {
        return Ok(());
    }
    Err(AppError::Platform(format_ibcmd_failure_details(
        action,
        match resolved.target_kind {
            LoadTargetKind::Configuration => "configuration",
            LoadTargetKind::Extension => "extension",
            LoadTargetKind::Unknown => "unknown",
        },
        resolved.extension.as_deref().unwrap_or("main"),
        result.process.exit_code,
        &result.process.stdout,
        &result.process.stderr,
        result.platform_log.as_deref(),
        result.platform_log_path.as_deref(),
    )))
}

fn target_label(resolved: &ResolvedLoadRequest) -> String {
    match resolved.target_kind {
        LoadTargetKind::Configuration => "configuration".to_owned(),
        LoadTargetKind::Extension => format!(
            "extension '{}'",
            resolved.extension.as_deref().unwrap_or("<unknown>")
        ),
        LoadTargetKind::Unknown => "unknown".to_owned(),
    }
}

fn interrupted_result_from_resolved(
    resolved: &ResolvedLoadRequest,
    compatibility_state: CompatibilityState,
    started: Instant,
    interruption: ExecutionInterruption,
    message: String,
    platform_log_path: Option<PathBuf>,
) -> LoadResult {
    LoadResult {
        mode: resolved.mode,
        artifact_path: resolved.artifact_path.clone(),
        artifact_type: resolved.artifact_type,
        extension: resolved.extension.clone(),
        duration_ms: started.elapsed().as_millis() as u64,
        execution: with_platform_log_artifact(
            ExecutionOutcome::new(command_interruption_status(interruption))
                .with_diagnostics(vec![message.clone()])
                .with_errors(vec![ExecutionError::new(
                    "artifact_load_interrupted",
                    message.clone(),
                )])
                .with_interruptions(vec![command_interruption_details(
                    interruption,
                    "update_db_cfg_safe_point",
                    message,
                )])
                .with_payload(LoadExecutionMetadata {
                    applied: true,
                    target_kind: resolved.target_kind,
                    compatibility_state,
                    update_db_cfg_ran: false,
                }),
            platform_log_path,
        ),
    }
}

fn deferred_interruption_warning(action: &str, result: &PlatformCommandResult) -> Option<String> {
    deferred_process_interruption_warning(&format!("{action} completed successfully"), result)
}

fn empty_result_from_resolved(
    resolved: &ResolvedLoadRequest,
    compatibility_state: CompatibilityState,
    started: Instant,
    message: Option<String>,
    platform_log_path: Option<PathBuf>,
    update_db_cfg_ran: bool,
) -> LoadResult {
    empty_result(
        resolved.mode,
        resolved.artifact_path.clone(),
        resolved.artifact_type,
        resolved.target_kind,
        compatibility_state,
        resolved.extension.clone(),
        started,
        message,
        platform_log_path,
        update_db_cfg_ran,
    )
}

fn empty_result(
    mode: LoadMode,
    artifact_path: PathBuf,
    artifact_type: ArtifactBuildMode,
    target_kind: LoadTargetKind,
    compatibility_state: CompatibilityState,
    extension: Option<String>,
    started: Instant,
    message: Option<String>,
    platform_log_path: Option<PathBuf>,
    update_db_cfg_ran: bool,
) -> LoadResult {
    let error_message = message
        .clone()
        .unwrap_or_else(|| "artifact load failed".to_owned());
    LoadResult {
        mode,
        artifact_path,
        artifact_type,
        extension,
        duration_ms: started.elapsed().as_millis() as u64,
        execution: with_platform_log_artifact(
            ExecutionOutcome::new(ExecutionStatus::Failed)
                .with_errors(vec![ExecutionError::new(
                    "artifact_load_failed",
                    error_message,
                )])
                .with_payload(LoadExecutionMetadata {
                    applied: false,
                    target_kind,
                    compatibility_state,
                    update_db_cfg_ran,
                }),
            platform_log_path,
        ),
    }
}

fn with_platform_log_artifact(
    execution: ExecutionOutcome<LoadExecutionMetadata>,
    platform_log_path: Option<PathBuf>,
) -> ExecutionOutcome<LoadExecutionMetadata> {
    let Some(platform_log_path) = platform_log_path else {
        return execution;
    };
    let mut artifacts = ArtifactSet::default();
    artifacts.push(
        ArtifactRef::new(ArtifactKind::PlatformLog, platform_log_path)
            .with_role(ARTIFACT_ROLE_PLATFORM_LOG),
    );
    execution.with_artifacts(artifacts)
}

#[cfg(test)]
mod tests {
    use super::{execute, resolve_request};
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, PlatformToolConfig, SourceFormat, TestsConfig,
        ToolsConfig,
    };
    use crate::domain::artifacts::ArtifactBuildMode;
    use crate::domain::execution::ExecutionStatus;
    use crate::domain::load::{
        CompatibilityState, LoadExecutionMetadata, LoadMode, LoadResult, LoadTargetKind,
    };
    use crate::use_cases::context::{CommandName, ExecutionContext};
    use crate::use_cases::request::LoadRequest;
    use crate::use_cases::result::UseCaseErrorKind;
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    fn load_payload(result: &LoadResult) -> &LoadExecutionMetadata {
        result.execution.payload.as_ref().expect("payload")
    }

    fn load_message(result: &LoadResult) -> &str {
        result
            .execution
            .errors
            .first()
            .map(|error| error.message.as_str())
            .or_else(|| result.execution.diagnostics.first().map(String::as_str))
            .expect("message")
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }

    #[cfg(unix)]
    fn write_designer_script(path: &Path, calls_log: &Path) {
        write_designer_script_with_merge_failure(path, calls_log, false);
    }

    #[cfg(unix)]
    fn write_designer_script_with_merge_failure(path: &Path, calls_log: &Path, fail_merge: bool) {
        let merge_block = if fail_merge {
            "if printf '%s' \"$*\" | grep -F -q -- '/MergeCfg'; then\n  printf 'merge failed\\n' >&2\n  exit 23\nfi\n"
        } else {
            ""
        };
        let body = format!(
            "args=\"$*\"\nout=\"\"\nreport=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  if [ \"$prev\" = \"-ReportFile\" ]; then report=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nprintf '%s\\n' \"$args\" >> \"{}\"\nif [ -n \"$out\" ]; then mkdir -p \"$(dirname \"$out\")\"; printf 'designer log for %s\\n' \"$args\" > \"$out\"; fi\nif printf '%s' \"$args\" | grep -F -q -- '/CompareCfg'; then\n  if printf '%s' \"$args\" | grep -F -q -- 'VendorConfiguration'; then\n    printf 'configuration is not on support\\n' >&2\n    exit 17\n  fi\n  if printf '%s' \"$args\" | grep -F -q -- 'ExtensionDBConfiguration'; then\n    if printf '%s' \"$args\" | grep -F -q -- 'ExistingExt'; then\n      : > \"$report\"\n      exit 0\n    fi\n    if printf '%s' \"$args\" | grep -F -q -- 'UnsupportedExt'; then\n      printf 'extension is not supported\\n' >&2\n      exit 18\n    fi\n    printf 'extension not found\\n' >&2\n    exit 19\n  fi\nfi\n{merge_block}exit 0",
            calls_log.display()
        );
        fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        make_executable(path);
    }

    fn sample_config(root: &Path, binary: &Path) -> AppConfig {
        AppConfig {
            base_path: root.to_path_buf(),
            work_path: root.join("work"),
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
            source_sets: vec![],
            build: BuildConfig::default(),
            tools: ToolsConfig {
                platform: PlatformToolConfig {
                    path: Some(binary.to_path_buf()),
                    version: None,
                },
                enterprise: Default::default(),
                edt_cli: Default::default(),
                ..Default::default()
            },
            mcp: Default::default(),
            tests: TestsConfig::default(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn resolve_request_rejects_unsupported_artifacts_and_missing_flags() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("tool.epf"), "epf").expect("write");
        fs::write(root.join("ext.cfe"), "cfe").expect("write");
        let config = sample_config(root, &root.join("1cv8"));

        let unsupported = resolve_request(
            &config,
            &LoadRequest {
                mode: LoadMode::Load,
                artifact_path: "tool.epf".to_owned(),
                settings_path: None,
                extension: None,
            },
        )
        .expect_err("epf should fail");
        assert!(unsupported.to_string().contains("only .cf and .cfe"));

        let missing_extension = resolve_request(
            &config,
            &LoadRequest {
                mode: LoadMode::Load,
                artifact_path: "ext.cfe".to_owned(),
                settings_path: None,
                extension: None,
            },
        )
        .expect_err("cfe without extension should fail");
        assert!(missing_extension
            .to_string()
            .contains("require --extension"));
    }

    #[cfg(unix)]
    #[test]
    fn execute_load_cf_runs_probe_load_and_update() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("work")).expect("work");
        let binary = root.join("1cv8");
        let calls = root.join("calls.log");
        fs::write(root.join("main.cf"), "cf").expect("artifact");
        write_designer_script(&binary, &calls);
        let config = sample_config(root, &binary);
        let request = LoadRequest {
            mode: LoadMode::Load,
            artifact_path: "main.cf".to_owned(),
            settings_path: None,
            extension: None,
        };

        let result = execute(&ExecutionContext::cli(CommandName::Load), &config, &request)
            .expect("load result");

        assert!(result.execution.is_ok());
        assert_eq!(result.artifact_type, ArtifactBuildMode::ConfigurationCf);
        assert_eq!(
            load_payload(&result).compatibility_state,
            CompatibilityState::NotSupported
        );
        assert_eq!(load_payload(&result).update_db_cfg_ran, true);
        let calls_text = fs::read_to_string(calls).expect("calls");
        assert!(calls_text.contains("/CompareCfg"));
        assert!(calls_text.contains("/LoadCfg"));
        assert!(calls_text.contains("/UpdateDBCfg"));
    }

    #[cfg(unix)]
    #[test]
    fn execute_rejects_unsupported_matrix_with_real_cfe_metadata() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("ext.cfe"), "cfe").expect("artifact");
        let mut config = sample_config(root, &root.join("1cv8"));
        config.builder = BuilderBackend::Ibcmd;

        let request = LoadRequest {
            mode: LoadMode::Load,
            artifact_path: "ext.cfe".to_owned(),
            settings_path: None,
            extension: Some("ExistingExt".to_owned()),
        };

        let failure = execute(&ExecutionContext::cli(CommandName::Load), &config, &request)
            .expect_err("matrix should reject");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Validation);
        let payload = failure.payload.expect("payload");
        assert!(!payload.execution.is_ok());
        assert_eq!(payload.artifact_type, ArtifactBuildMode::ExtensionCfe);
        assert_eq!(
            load_payload(&payload).target_kind,
            LoadTargetKind::Extension
        );
        assert_eq!(payload.extension.as_deref(), Some("ExistingExt"));
        assert!(load_message(&payload).contains("builder=DESIGNER and format=DESIGNER"));
    }

    #[cfg(unix)]
    #[test]
    fn execute_reports_cancelled_status_before_load_probe() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("work")).expect("work");
        let binary = root.join("1cv8");
        let calls = root.join("calls.log");
        fs::write(root.join("main.cf"), "cf").expect("artifact");
        write_designer_script(&binary, &calls);
        let config = sample_config(root, &binary);
        let request = LoadRequest {
            mode: LoadMode::Load,
            artifact_path: "main.cf".to_owned(),
            settings_path: None,
            extension: None,
        };
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let context = ExecutionContext::cli(CommandName::Load).with_cancellation(cancellation);

        let failure = execute(&context, &config, &request).expect_err("cancelled");
        let payload = failure.payload.expect("payload");

        assert_eq!(payload.execution.status, ExecutionStatus::Cancelled);
        assert_eq!(payload.execution.interruptions.len(), 1);
        assert!(payload.execution.errors[0]
            .message
            .contains("before entering load probe"));
    }

    #[cfg(unix)]
    #[test]
    fn execute_merge_cfe_runs_probe_merge_and_update() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("work")).expect("work");
        let binary = root.join("1cv8");
        let calls = root.join("calls.log");
        fs::write(root.join("ext.cfe"), "cfe").expect("artifact");
        fs::write(root.join("merge.xml"), "<settings/>").expect("settings");
        write_designer_script(&binary, &calls);
        let config = sample_config(root, &binary);
        let request = LoadRequest {
            mode: LoadMode::Merge,
            artifact_path: "ext.cfe".to_owned(),
            settings_path: Some("merge.xml".to_owned()),
            extension: Some("ExistingExt".to_owned()),
        };

        let result = execute(&ExecutionContext::cli(CommandName::Load), &config, &request)
            .expect("merge result");

        assert!(result.execution.is_ok());
        assert_eq!(
            load_payload(&result).compatibility_state,
            CompatibilityState::Supported
        );
        let calls_text = fs::read_to_string(calls).expect("calls");
        assert!(calls_text.contains("ExtensionConfiguration"));
        assert!(calls_text.contains("/MergeCfg"));
        assert!(calls_text.contains("-Settings"));
        assert!(calls_text.contains("-Extension ExistingExt"));
        assert!(calls_text.contains("/UpdateDBCfg -Extension ExistingExt"));
    }

    #[cfg(unix)]
    #[test]
    fn execute_load_cfe_with_unsupported_extension_runs_load_path() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("work")).expect("work");
        let binary = root.join("1cv8");
        let calls = root.join("calls.log");
        fs::write(root.join("ext.cfe"), "cfe").expect("artifact");
        write_designer_script(&binary, &calls);
        let config = sample_config(root, &binary);
        let request = LoadRequest {
            mode: LoadMode::Load,
            artifact_path: "ext.cfe".to_owned(),
            settings_path: None,
            extension: Some("UnsupportedExt".to_owned()),
        };

        let result = execute(&ExecutionContext::cli(CommandName::Load), &config, &request)
            .expect("load result");

        assert!(result.execution.is_ok());
        assert_eq!(
            load_payload(&result).compatibility_state,
            CompatibilityState::NotSupported
        );
        let calls_text = fs::read_to_string(calls).expect("calls");
        assert!(calls_text.contains("/CompareCfg"));
        assert!(calls_text.contains("/LoadCfg"));
        assert!(calls_text.contains("/UpdateDBCfg -Extension UnsupportedExt"));
    }

    #[cfg(unix)]
    #[test]
    fn execute_merge_cfe_rejects_unsupported_extension_state() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("work")).expect("work");
        let binary = root.join("1cv8");
        let calls = root.join("calls.log");
        fs::write(root.join("ext.cfe"), "cfe").expect("artifact");
        fs::write(root.join("merge.xml"), "<settings/>").expect("settings");
        write_designer_script(&binary, &calls);
        let config = sample_config(root, &binary);
        let request = LoadRequest {
            mode: LoadMode::Merge,
            artifact_path: "ext.cfe".to_owned(),
            settings_path: Some("merge.xml".to_owned()),
            extension: Some("UnsupportedExt".to_owned()),
        };

        let failure = execute(&ExecutionContext::cli(CommandName::Load), &config, &request)
            .expect_err("merge should reject");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Validation);
        let payload = failure.payload.expect("payload");
        assert_eq!(
            load_payload(&payload).compatibility_state,
            CompatibilityState::NotSupported
        );
        assert!(load_message(&payload).contains("use --mode load instead"));
    }

    #[cfg(unix)]
    #[test]
    fn execute_merge_failure_message_uses_merge_action_name() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("work")).expect("work");
        let binary = root.join("1cv8");
        let calls = root.join("calls.log");
        fs::write(root.join("ext.cfe"), "cfe").expect("artifact");
        fs::write(root.join("merge.xml"), "<settings/>").expect("settings");
        write_designer_script(&binary, &calls);
        fs::write(
            &binary,
            format!(
                "#!/bin/sh\nif printf '%s' \"$*\" | grep -F -q -- '/MergeCfg'; then\n  echo 'merge failed' >&2\n  exit 23\nfi\n{}\n",
                fs::read_to_string(&binary).expect("script")
            ),
        )
        .expect("rewrite script");
        make_executable(&binary);
        let config = sample_config(root, &binary);
        let request = LoadRequest {
            mode: LoadMode::Merge,
            artifact_path: "ext.cfe".to_owned(),
            settings_path: Some("merge.xml".to_owned()),
            extension: Some("ExistingExt".to_owned()),
        };

        let failure = execute(&ExecutionContext::cli(CommandName::Load), &config, &request)
            .expect_err("merge should fail");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Platform);
        let payload = failure.payload.expect("payload");
        assert!(!payload.execution.is_ok());
        assert_eq!(
            load_payload(&payload).compatibility_state,
            CompatibilityState::Supported
        );
        assert!(load_message(&payload).contains("merge failed for extension"));
    }

    #[cfg(unix)]
    #[test]
    fn execute_rejects_cf_with_extension_payload_keeps_configuration_target_kind() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("main.cf"), "cf").expect("artifact");
        let config = sample_config(root, &root.join("1cv8"));

        let request = LoadRequest {
            mode: LoadMode::Load,
            artifact_path: "main.cf".to_owned(),
            settings_path: None,
            extension: Some("Ignored".to_owned()),
        };

        let failure = execute(&ExecutionContext::cli(CommandName::Load), &config, &request)
            .expect_err("cf with extension should fail");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Validation);
        let payload = failure.payload.expect("payload");
        assert_eq!(payload.artifact_type, ArtifactBuildMode::ConfigurationCf);
        assert_eq!(
            load_payload(&payload).target_kind,
            LoadTargetKind::Configuration
        );
        assert_eq!(payload.extension.as_deref(), Some("Ignored"));
    }

    #[cfg(unix)]
    #[test]
    fn execute_rejects_cfe_without_extension_payload_keeps_extension_target_kind() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("work")).expect("work");
        fs::write(root.join("ext.cfe"), "cfe").expect("artifact");
        let config = sample_config(root, &root.join("1cv8"));

        let request = LoadRequest {
            mode: LoadMode::Load,
            artifact_path: "ext.cfe".to_owned(),
            settings_path: None,
            extension: None,
        };

        let failure = execute(&ExecutionContext::cli(CommandName::Load), &config, &request)
            .expect_err("cfe without extension should fail");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Validation);
        let payload = failure.payload.expect("payload");
        assert_eq!(payload.artifact_type, ArtifactBuildMode::ExtensionCfe);
        assert_eq!(
            load_payload(&payload).target_kind,
            LoadTargetKind::Extension
        );
        assert_eq!(payload.extension.as_deref(), None);
    }

    #[cfg(unix)]
    #[test]
    fn execute_rejects_unknown_artifact_payload_marks_unknown_metadata() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("release.zip"), "zip").expect("artifact");
        let config = sample_config(root, &root.join("1cv8"));

        let request = LoadRequest {
            mode: LoadMode::Load,
            artifact_path: "release.zip".to_owned(),
            settings_path: None,
            extension: None,
        };

        let failure = execute(&ExecutionContext::cli(CommandName::Load), &config, &request)
            .expect_err("zip should fail");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Validation);
        let payload = failure.payload.expect("payload");
        assert_eq!(payload.artifact_type, ArtifactBuildMode::Unknown);
        assert_eq!(load_payload(&payload).target_kind, LoadTargetKind::Unknown);
        assert_eq!(payload.extension.as_deref(), None);
        assert!(load_message(&payload).contains("only .cf and .cfe"));
    }

    #[cfg(unix)]
    #[test]
    fn execute_rejects_external_artifact_payload_marks_unknown_target_kind() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("tool.epf"), "epf").expect("artifact");
        let config = sample_config(root, &root.join("1cv8"));

        let request = LoadRequest {
            mode: LoadMode::Load,
            artifact_path: "tool.epf".to_owned(),
            settings_path: None,
            extension: None,
        };

        let failure = execute(&ExecutionContext::cli(CommandName::Load), &config, &request)
            .expect_err("epf should fail");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Validation);
        let payload = failure.payload.expect("payload");
        assert_eq!(
            payload.artifact_type,
            ArtifactBuildMode::ExternalDataProcessorEpf
        );
        assert_eq!(load_payload(&payload).target_kind, LoadTargetKind::Unknown);
        assert_eq!(payload.extension.as_deref(), None);
        assert!(load_message(&payload).contains("only .cf and .cfe"));
    }
}
