use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tempfile::NamedTempFile;

use crate::change_detection::source_sets::SourceSetsService;
use crate::config::model::{
    AppConfig, BuilderBackend, SourceFormat, SourceSetConfig, SourceSetPurpose,
};
use crate::domain::dump::{DumpMode, DumpResult};
use crate::domain::source_set::SourceSetContext;
use crate::platform::designer::DesignerDsl;
use crate::platform::edt::EdtDsl;
use crate::platform::edt_session::{EdtSessionHostOptions, EdtSessionManager};
use crate::platform::ibcmd::{IbcmdConnection, IbcmdDsl, IbcmdError};
use crate::platform::locator::UtilityType;
use crate::platform::process::ProcessRunner;
use crate::platform::result::PlatformCommandResult;
use crate::platform::utilities::PlatformUtilities;
use crate::support::error::AppError;
use crate::support::fs::{
    acquire_advisory_lock, ensure_dir, is_known_tool_name, metadata_sidecar_path,
    read_temp_dir_metadata, remove_path_if_exists, replace_dir_atomically, write_temp_dir_metadata,
    TempDirKind,
};
use crate::support::path::{
    hashed_lock_path, is_filesystem_root, nearest_existing_canonical_path, stable_path_identity,
};
use crate::support::temp::{dump_object_list_file, platform_logs_dir};
use crate::use_cases::context::{ExecutionContext, InterruptionSafetyClass};
use crate::use_cases::external_artifacts::ExternalArtifactKind;
use crate::use_cases::ibcmd_diagnostics::format_ibcmd_failure_details;
use crate::use_cases::request::{DumpModeRequest, DumpRequest as DumpArgs};
use crate::use_cases::result::{UseCaseFailure, UseCaseResult};
use quick_xml::events::Event;
use quick_xml::Reader;
use tracing::debug;

#[cfg(test)]
const DUMP_COMMAND: &str = crate::use_cases::context::CommandName::Dump.as_str();
const SUPPORTED_DUMP_ERROR: &str = "dump currently supports only builder=DESIGNER or IBCMD";
const PARTIAL_OBJECTS_REQUIRED_ERROR: &str = "partial dump requires at least one object";
const PARTIAL_OBJECT_BLANK_ERROR: &str = "partial dump objects must not be blank";
const PARTIAL_OBJECT_CONTROL_ERROR: &str =
    "partial dump objects must not contain control characters";
const NON_PARTIAL_OBJECTS_ERROR: &str = "dump objects are supported only for mode 'partial'";
const ORPHAN_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const DUMP_BACKUP_PREFIX: &str = ".dump-backup";

pub fn execute(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &DumpArgs,
) -> UseCaseResult<DumpResult> {
    debug!(
        command = context.command().as_str(),
        transport = ?context.transport(),
        "executing dump use case"
    );
    run_dump_with_context(context, config, args)
}

type DumpExecutionFailure = UseCaseFailure<DumpResult>;

#[derive(Debug, Clone)]
struct ResolvedDumpTarget {
    source_set_name: String,
    extension: Option<String>,
    target_path: PathBuf,
    canonical_target_path: PathBuf,
    platform_target_path: PathBuf,
    canonical_platform_target_path: PathBuf,
    canonical_base_path: PathBuf,
    canonical_work_path: PathBuf,
    target_identity: String,
    platform_target_identity: String,
    lock_path: PathBuf,
    edt_base_project_name: Option<String>,
}

#[cfg(test)]
fn run_dump(config: &AppConfig, args: &DumpArgs) -> UseCaseResult<DumpResult> {
    let context = ExecutionContext::cli(crate::use_cases::context::CommandName::Dump);
    run_dump_with_context(&context, config, args)
}

fn run_dump_with_context(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &DumpArgs,
) -> UseCaseResult<DumpResult> {
    let started = Instant::now();
    let mode = match args.mode {
        DumpModeRequest::Full => DumpMode::Full,
        DumpModeRequest::Incremental => DumpMode::Incremental,
        DumpModeRequest::Partial => DumpMode::Partial,
    };
    debug!(
        mode = ?mode,
        source_set = args.source_set.as_deref().unwrap_or("<auto>"),
        extension = args.extension.as_deref().unwrap_or("<none>"),
        "starting dump"
    );

    if let Some(error) = validate_supported_matrix(config) {
        return Err(DumpExecutionFailure::with_payload(
            error,
            empty_result(
                mode,
                started,
                None,
                None,
                None,
                Some(SUPPORTED_DUMP_ERROR.to_owned()),
            ),
        ));
    }

    let partial_objects = match validate_dump_objects(&mode, &args.objects) {
        Ok(objects) => objects,
        Err(error) => {
            let message = error.to_string();
            return Err(DumpExecutionFailure::with_payload(
                error,
                empty_result(
                    mode,
                    started,
                    args.source_set.clone(),
                    args.extension.clone(),
                    None,
                    Some(message),
                ),
            ));
        }
    };

    let resolved = match resolve_target(config, args) {
        Ok(resolved) => resolved,
        Err(error) => {
            let message = error.to_string();
            return Err(DumpExecutionFailure::with_payload(
                error,
                empty_result(
                    mode,
                    started,
                    args.source_set.clone(),
                    args.extension.clone(),
                    None,
                    Some(message),
                ),
            ));
        }
    };

    let mut utilities = PlatformUtilities::from_config(config);
    let utility = match config.builder {
        BuilderBackend::Designer => UtilityType::V8,
        BuilderBackend::Ibcmd => UtilityType::Ibcmd,
    };
    let location = match utilities.locate(utility) {
        Ok(location) => location,
        Err(error) => {
            let message = error.to_string();
            let app_error = AppError::Platform(message.clone());
            return Err(DumpExecutionFailure::with_payload(
                app_error,
                empty_result(
                    mode,
                    started,
                    Some(resolved.source_set_name.clone()),
                    resolved.extension.clone(),
                    Some(resolved.target_path.clone()),
                    Some(message),
                ),
            ));
        }
    };
    let edt_binary = if config.format == SourceFormat::Edt {
        Some(match utilities.locate(UtilityType::EdtCli) {
            Ok(location) => location.path,
            Err(error) => {
                let message = error.to_string();
                let app_error = AppError::Platform(message.clone());
                return Err(DumpExecutionFailure::with_payload(
                    app_error,
                    empty_result(
                        mode,
                        started,
                        Some(resolved.source_set_name.clone()),
                        resolved.extension.clone(),
                        Some(resolved.target_path.clone()),
                        Some(message),
                    ),
                ));
            }
        })
    } else {
        None
    };

    let lock_guard = match acquire_advisory_lock(&resolved.lock_path) {
        Ok(lock_guard) => lock_guard,
        Err(error) => {
            let message = format!(
                "failed to acquire dump lock '{}': {error}",
                resolved.lock_path.display()
            );
            let app_error = AppError::Runtime(message.clone());
            return Err(DumpExecutionFailure::with_payload(
                app_error,
                empty_result(
                    mode,
                    started,
                    Some(resolved.source_set_name.clone()),
                    resolved.extension.clone(),
                    Some(resolved.target_path.clone()),
                    Some(message),
                ),
            ));
        }
    };

    if let Err(error) = cleanup_orphan_dirs(&resolved) {
        let message = format!("failed to cleanup stale dump temp dirs: {error}");
        let app_error = AppError::Runtime(message.clone());
        return Err(DumpExecutionFailure::with_payload(
            app_error,
            empty_result(
                mode,
                started,
                Some(resolved.source_set_name.clone()),
                resolved.extension.clone(),
                Some(resolved.target_path.clone()),
                Some(message),
            ),
        ));
    }
    if resolved.platform_target_path != resolved.target_path {
        if let Err(error) = cleanup_platform_orphan_dirs(&resolved) {
            let message = format!("failed to cleanup stale dump platform temp dirs: {error}");
            let app_error = AppError::Runtime(message.clone());
            return Err(DumpExecutionFailure::with_payload(
                app_error,
                empty_result(
                    mode,
                    started,
                    Some(resolved.source_set_name.clone()),
                    resolved.extension.clone(),
                    Some(resolved.target_path.clone()),
                    Some(message),
                ),
            ));
        }
    }

    if let Err(error) = validate_publish_target(&resolved) {
        let message = error.to_string();
        return Err(DumpExecutionFailure::with_payload(
            error,
            empty_result(
                mode,
                started,
                Some(resolved.source_set_name.clone()),
                resolved.extension.clone(),
                Some(resolved.target_path.clone()),
                Some(message),
            ),
        ));
    }
    if resolved.platform_target_path != resolved.target_path {
        if let Err(error) = validate_platform_target(&resolved) {
            let message = error.to_string();
            return Err(DumpExecutionFailure::with_payload(
                error,
                empty_result(
                    mode,
                    started,
                    Some(resolved.source_set_name.clone()),
                    resolved.extension.clone(),
                    Some(resolved.target_path.clone()),
                    Some(message),
                ),
            ));
        }
    }

    let partial_objects = partial_objects.as_deref();
    let edt_binary = edt_binary.as_deref();
    let result = match (
        config.format,
        &mode,
        &config.builder,
        partial_objects,
        edt_binary,
    ) {
        (SourceFormat::Designer, DumpMode::Incremental, BuilderBackend::Designer, _, _) => {
            run_incremental_dump_designer(
                context,
                config,
                &resolved,
                location.path.as_path(),
                utilities.runner_for(UtilityType::V8),
            )
        }
        (SourceFormat::Designer, DumpMode::Incremental, BuilderBackend::Ibcmd, _, _) => {
            run_incremental_dump_ibcmd(
                context,
                config,
                &resolved,
                location.path.as_path(),
                utilities.runner_for(UtilityType::Ibcmd),
            )
        }
        (SourceFormat::Designer, DumpMode::Full, BuilderBackend::Designer, _, _) => {
            run_full_dump_designer(
                context,
                config,
                &resolved,
                location.path.as_path(),
                utilities.runner_for(UtilityType::V8),
            )
        }
        (SourceFormat::Designer, DumpMode::Full, BuilderBackend::Ibcmd, _, _) => {
            run_full_dump_ibcmd(
                context,
                config,
                &resolved,
                location.path.as_path(),
                utilities.runner_for(UtilityType::Ibcmd),
            )
        }
        (SourceFormat::Designer, DumpMode::Partial, BuilderBackend::Designer, Some(objects), _) => {
            run_partial_dump_designer(
                context,
                config,
                &resolved,
                location.path.as_path(),
                utilities.runner_for(UtilityType::V8),
                objects,
            )
        }
        (SourceFormat::Designer, DumpMode::Partial, BuilderBackend::Ibcmd, Some(objects), _) => {
            run_partial_dump_ibcmd(
                context,
                config,
                &resolved,
                location.path.as_path(),
                utilities.runner_for(UtilityType::Ibcmd),
                objects,
            )
        }
        (
            SourceFormat::Edt,
            DumpMode::Incremental,
            BuilderBackend::Designer,
            _,
            Some(edt_binary),
        ) => run_incremental_dump_edt_designer(
            context,
            config,
            &resolved,
            location.path.as_path(),
            edt_binary,
            utilities.runner_for(UtilityType::V8),
            utilities.runner_for(UtilityType::EdtCli),
        ),
        (SourceFormat::Edt, DumpMode::Incremental, BuilderBackend::Ibcmd, _, Some(edt_binary)) => {
            run_incremental_dump_edt_ibcmd(
                context,
                config,
                &resolved,
                location.path.as_path(),
                edt_binary,
                utilities.runner_for(UtilityType::Ibcmd),
                utilities.runner_for(UtilityType::EdtCli),
            )
        }
        (SourceFormat::Edt, DumpMode::Full, BuilderBackend::Designer, _, Some(edt_binary)) => {
            run_full_dump_edt_designer(
                context,
                config,
                &resolved,
                location.path.as_path(),
                edt_binary,
                utilities.runner_for(UtilityType::V8),
                utilities.runner_for(UtilityType::EdtCli),
            )
        }
        (SourceFormat::Edt, DumpMode::Full, BuilderBackend::Ibcmd, _, Some(edt_binary)) => {
            run_full_dump_edt_ibcmd(
                context,
                config,
                &resolved,
                location.path.as_path(),
                edt_binary,
                utilities.runner_for(UtilityType::Ibcmd),
                utilities.runner_for(UtilityType::EdtCli),
            )
        }
        (
            SourceFormat::Edt,
            DumpMode::Partial,
            BuilderBackend::Designer,
            Some(objects),
            Some(edt_binary),
        ) => run_partial_dump_edt_designer(
            context,
            config,
            &resolved,
            location.path.as_path(),
            edt_binary,
            utilities.runner_for(UtilityType::V8),
            utilities.runner_for(UtilityType::EdtCli),
            objects,
        ),
        (
            SourceFormat::Edt,
            DumpMode::Partial,
            BuilderBackend::Ibcmd,
            Some(objects),
            Some(edt_binary),
        ) => run_partial_dump_edt_ibcmd(
            context,
            config,
            &resolved,
            location.path.as_path(),
            edt_binary,
            utilities.runner_for(UtilityType::Ibcmd),
            utilities.runner_for(UtilityType::EdtCli),
            objects,
        ),
        (_, DumpMode::Partial, _, None, _) => Err(AppError::Runtime(
            "partial dump objects were not validated before execution".to_owned(),
        )),
        (SourceFormat::Edt, _, _, _, None) => Err(AppError::Runtime(
            "EDT binary must be resolved before executing format=EDT dump".to_owned(),
        )),
    };
    drop(lock_guard);

    match result {
        Ok((platform_result, cleanup_message)) => Ok(DumpResult {
            ok: true,
            source_set: Some(resolved.source_set_name),
            extension: resolved.extension,
            mode,
            target_path: resolved.target_path,
            platform_log_path: platform_result.platform_log_path,
            duration_ms: started.elapsed().as_millis() as u64,
            message: cleanup_message.or_else(|| Some("dump completed successfully".to_owned())),
        }),
        Err(error) => {
            let message = error.to_string();
            Err(DumpExecutionFailure::with_payload(
                error,
                DumpResult {
                    ok: false,
                    source_set: Some(resolved.source_set_name),
                    extension: resolved.extension,
                    mode,
                    target_path: resolved.target_path,
                    platform_log_path: None,
                    duration_ms: started.elapsed().as_millis() as u64,
                    message: Some(message),
                },
            ))
        }
    }
}

fn run_incremental_dump_designer(
    context: &ExecutionContext,
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    runner: &dyn ProcessRunner,
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    debug!(
        source_set = resolved.source_set_name.as_str(),
        target = %resolved.platform_target_path.display(),
        "running incremental dump"
    );
    ensure_dir(&resolved.platform_target_path)
        .map_err(|error| AppError::Runtime(format!("failed to create target dir: {error}")))?;

    let dump_result = build_designer_dsl(
        context,
        config,
        binary,
        runner,
        &resolved.source_set_name,
        "incremental",
    )?
    .dump_config_to_files(
        &resolved.platform_target_path,
        resolved.extension.as_deref(),
    )
    .map_err(|error| AppError::Platform(error.to_string()))?;
    ensure_platform_success("dump", resolved, &dump_result)?;
    Ok((dump_result, None))
}

fn run_full_dump_designer(
    context: &ExecutionContext,
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    runner: &dyn ProcessRunner,
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    debug!(
        source_set = resolved.source_set_name.as_str(),
        target = %resolved.platform_target_path.display(),
        "running full dump via staging directory"
    );
    let target_parent = resolved.platform_target_path.parent().ok_or_else(|| {
        AppError::Runtime(format!(
            "target path has no parent: {}",
            resolved.platform_target_path.display()
        ))
    })?;
    ensure_dir(target_parent).map_err(|error| {
        AppError::Runtime(format!("failed to create target parent dir: {error}"))
    })?;

    let run_id = make_run_id();
    let staging_dir = target_parent.join(format!(".dump-stage-{run_id}"));
    if staging_dir.exists() {
        return Err(AppError::Runtime(format!(
            "staging dir already exists unexpectedly: {}",
            staging_dir.display()
        )));
    }
    std::fs::create_dir(&staging_dir)
        .map_err(|error| AppError::Runtime(format!("failed to create staging dir: {error}")))?;
    debug!(path = %staging_dir.display(), "created dump staging directory");
    write_temp_dir_metadata(
        &staging_dir,
        TempDirKind::Stage,
        &run_id,
        &resolved.platform_target_path,
        &resolved.platform_target_identity,
    )
    .map_err(|error| AppError::Runtime(format!("failed to write stage metadata: {error}")))?;

    let dump_result = match build_designer_dsl(
        context,
        config,
        binary,
        runner,
        &resolved.source_set_name,
        "full",
    )?
    .dump_config_to_files(&staging_dir, resolved.extension.as_deref())
    .map_err(|error| AppError::Platform(error.to_string()))
    {
        Ok(result) => result,
        Err(error) => return Err(cleanup_staging_on_platform_failure(&staging_dir, error)),
    };
    ensure_platform_success("dump", resolved, &dump_result)
        .map_err(|error| cleanup_staging_on_platform_failure(&staging_dir, error))?;

    validate_platform_target(resolved)?;
    if let Some(interruption) = context.interruption() {
        return Err(cleanup_staging_on_interruption(
            &staging_dir,
            AppError::Runtime(format!(
                "{} for command '{}' before entering dump publication safe point",
                interruption.message(context.command()),
                context.command().as_str()
            )),
        ));
    }

    let publish_phase = context.run_no_process_critical_phase(|| {
        replace_dir_atomically(
            &staging_dir,
            &resolved.platform_target_path,
            &run_id,
            &resolved.platform_target_identity,
            DUMP_BACKUP_PREFIX,
        )
        .map_err(|error| AppError::Runtime(format!("failed to publish staged dump: {error}")))
    })?;
    debug!(target = %resolved.platform_target_path.display(), "published staged dump");

    Ok((
        dump_result,
        merge_optional_messages(
            publish_phase.value.cleanup_warning,
            dump_publication_warning(context.command(), publish_phase.deferred_interruption),
        ),
    ))
}

fn run_incremental_dump_ibcmd(
    context: &ExecutionContext,
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    runner: &dyn ProcessRunner,
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    debug!(
        source_set = resolved.source_set_name.as_str(),
        target = %resolved.platform_target_path.display(),
        "running incremental ibcmd dump"
    );
    ensure_dir(&resolved.platform_target_path)
        .map_err(|error| AppError::Runtime(format!("failed to create target dir: {error}")))?;

    let dump_result = build_ibcmd_dsl(context, config, binary, runner)?
        .config_export_incremental(
            &resolved.platform_target_path,
            resolved.extension.as_deref(),
        )
        .map_err(map_ibcmd_error)?;
    ensure_platform_success("dump", resolved, &dump_result)?;
    Ok((dump_result, None))
}

fn run_full_dump_ibcmd(
    context: &ExecutionContext,
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    runner: &dyn ProcessRunner,
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    debug!(
        source_set = resolved.source_set_name.as_str(),
        target = %resolved.platform_target_path.display(),
        "running full ibcmd dump via staging directory"
    );
    let target_parent = resolved.platform_target_path.parent().ok_or_else(|| {
        AppError::Runtime(format!(
            "target path has no parent: {}",
            resolved.platform_target_path.display()
        ))
    })?;
    ensure_dir(target_parent).map_err(|error| {
        AppError::Runtime(format!("failed to create target parent dir: {error}"))
    })?;

    let run_id = make_run_id();
    let staging_dir = target_parent.join(format!(".dump-stage-{run_id}"));
    if staging_dir.exists() {
        return Err(AppError::Runtime(format!(
            "staging dir already exists unexpectedly: {}",
            staging_dir.display()
        )));
    }
    std::fs::create_dir(&staging_dir)
        .map_err(|error| AppError::Runtime(format!("failed to create staging dir: {error}")))?;
    debug!(path = %staging_dir.display(), "created dump staging directory");
    write_temp_dir_metadata(
        &staging_dir,
        TempDirKind::Stage,
        &run_id,
        &resolved.platform_target_path,
        &resolved.platform_target_identity,
    )
    .map_err(|error| AppError::Runtime(format!("failed to write stage metadata: {error}")))?;

    let dump_result = match build_ibcmd_dsl(context, config, binary, runner)?
        .config_export_full(&staging_dir, resolved.extension.as_deref())
        .map_err(map_ibcmd_error)
    {
        Ok(result) => result,
        Err(error) => return Err(cleanup_staging_on_platform_failure(&staging_dir, error)),
    };
    ensure_platform_success("dump", resolved, &dump_result)
        .map_err(|error| cleanup_staging_on_platform_failure(&staging_dir, error))?;

    validate_platform_target(resolved)?;
    if let Some(interruption) = context.interruption() {
        return Err(cleanup_staging_on_interruption(
            &staging_dir,
            AppError::Runtime(format!(
                "{} for command '{}' before entering dump publication safe point",
                interruption.message(context.command()),
                context.command().as_str()
            )),
        ));
    }

    let publish_phase = context.run_no_process_critical_phase(|| {
        replace_dir_atomically(
            &staging_dir,
            &resolved.platform_target_path,
            &run_id,
            &resolved.platform_target_identity,
            DUMP_BACKUP_PREFIX,
        )
        .map_err(|error| AppError::Runtime(format!("failed to publish staged dump: {error}")))
    })?;
    debug!(target = %resolved.platform_target_path.display(), "published staged dump");

    Ok((
        dump_result,
        merge_optional_messages(
            publish_phase.value.cleanup_warning,
            dump_publication_warning(context.command(), publish_phase.deferred_interruption),
        ),
    ))
}

fn run_partial_dump_designer(
    context: &ExecutionContext,
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    runner: &dyn ProcessRunner,
    objects: &[String],
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    debug!(
        source_set = resolved.source_set_name.as_str(),
        target = %resolved.platform_target_path.display(),
        object_count = objects.len(),
        "running partial designer dump"
    );
    ensure_dir(&resolved.platform_target_path)
        .map_err(|error| AppError::Runtime(format!("failed to create target dir: {error}")))?;

    let list_file = create_dump_object_list_file(&config.work_path, objects)?;
    let dump_result = build_designer_dsl(
        context,
        config,
        binary,
        runner,
        &resolved.source_set_name,
        "partial",
    )?
    .dump_config_to_files_partial(
        &resolved.platform_target_path,
        list_file.path(),
        resolved.extension.as_deref(),
    )
    .map_err(|error| AppError::Platform(error.to_string()))?;
    ensure_platform_success("dump", resolved, &dump_result)?;
    Ok((dump_result, None))
}

fn run_partial_dump_ibcmd(
    context: &ExecutionContext,
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    runner: &dyn ProcessRunner,
    _objects: &[String],
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    let warning = ibcmd_partial_warning(resolved);
    match run_incremental_dump_ibcmd(context, config, resolved, binary, runner) {
        Ok((dump_result, _)) => Ok((dump_result, Some(warning))),
        Err(error) => Err(decorate_ibcmd_partial_error(error, &warning)),
    }
}

fn ensure_interruption_clear(context: &ExecutionContext, phase: &str) -> Result<(), AppError> {
    if let Some(interruption) = context.interruption() {
        return Err(AppError::Runtime(format!(
            "{} for command '{}' {phase}",
            interruption.message(context.command()),
            context.command().as_str()
        )));
    }
    Ok(())
}

fn run_incremental_dump_edt_designer(
    context: &ExecutionContext,
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    edt_binary: &Path,
    runner: &dyn ProcessRunner,
    edt_runner: &dyn ProcessRunner,
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    let bootstrap_message = ensure_edt_platform_target_seeded(
        context,
        config,
        resolved,
        binary,
        runner,
        run_full_dump_designer,
    )?;
    ensure_interruption_clear(
        context,
        "before starting EDT follow-up dump after bootstrap publication",
    )?;
    let (dump_result, dump_message) =
        run_incremental_dump_designer(context, config, resolved, binary, runner)?;
    finalize_edt_dump(
        context,
        config,
        resolved,
        edt_binary,
        edt_runner,
        dump_result,
        merge_optional_messages(bootstrap_message, dump_message),
    )
}

fn run_full_dump_edt_designer(
    context: &ExecutionContext,
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    edt_binary: &Path,
    runner: &dyn ProcessRunner,
    edt_runner: &dyn ProcessRunner,
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    let (dump_result, dump_message) =
        run_full_dump_designer(context, config, resolved, binary, runner)?;
    finalize_edt_dump(
        context,
        config,
        resolved,
        edt_binary,
        edt_runner,
        dump_result,
        dump_message,
    )
}

fn run_partial_dump_edt_designer(
    context: &ExecutionContext,
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    edt_binary: &Path,
    runner: &dyn ProcessRunner,
    edt_runner: &dyn ProcessRunner,
    objects: &[String],
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    let bootstrap_message = ensure_edt_platform_target_seeded(
        context,
        config,
        resolved,
        binary,
        runner,
        run_full_dump_designer,
    )?;
    ensure_interruption_clear(
        context,
        "before starting EDT follow-up dump after bootstrap publication",
    )?;
    let (dump_result, dump_message) =
        run_partial_dump_designer(context, config, resolved, binary, runner, objects)?;
    finalize_edt_dump(
        context,
        config,
        resolved,
        edt_binary,
        edt_runner,
        dump_result,
        merge_optional_messages(bootstrap_message, dump_message),
    )
}

fn run_incremental_dump_edt_ibcmd(
    context: &ExecutionContext,
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    edt_binary: &Path,
    runner: &dyn ProcessRunner,
    edt_runner: &dyn ProcessRunner,
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    let bootstrap_message = ensure_edt_platform_target_seeded(
        context,
        config,
        resolved,
        binary,
        runner,
        run_full_dump_ibcmd,
    )?;
    ensure_interruption_clear(
        context,
        "before starting EDT follow-up dump after bootstrap publication",
    )?;
    let (dump_result, dump_message) =
        run_incremental_dump_ibcmd(context, config, resolved, binary, runner)?;
    finalize_edt_dump(
        context,
        config,
        resolved,
        edt_binary,
        edt_runner,
        dump_result,
        merge_optional_messages(bootstrap_message, dump_message),
    )
}

fn run_full_dump_edt_ibcmd(
    context: &ExecutionContext,
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    edt_binary: &Path,
    runner: &dyn ProcessRunner,
    edt_runner: &dyn ProcessRunner,
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    let (dump_result, dump_message) =
        run_full_dump_ibcmd(context, config, resolved, binary, runner)?;
    finalize_edt_dump(
        context,
        config,
        resolved,
        edt_binary,
        edt_runner,
        dump_result,
        dump_message,
    )
}

fn run_partial_dump_edt_ibcmd(
    context: &ExecutionContext,
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    edt_binary: &Path,
    runner: &dyn ProcessRunner,
    edt_runner: &dyn ProcessRunner,
    objects: &[String],
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    let bootstrap_message = ensure_edt_platform_target_seeded(
        context,
        config,
        resolved,
        binary,
        runner,
        run_full_dump_ibcmd,
    )?;
    ensure_interruption_clear(
        context,
        "before starting EDT follow-up dump after bootstrap publication",
    )?;
    let (dump_result, dump_message) =
        run_partial_dump_ibcmd(context, config, resolved, binary, runner, objects)?;
    finalize_edt_dump(
        context,
        config,
        resolved,
        edt_binary,
        edt_runner,
        dump_result,
        merge_optional_messages(bootstrap_message, dump_message),
    )
}

fn ensure_edt_platform_target_seeded(
    context: &ExecutionContext,
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    runner: &dyn ProcessRunner,
    full_dump_runner: fn(
        &ExecutionContext,
        &AppConfig,
        &ResolvedDumpTarget,
        &Path,
        &dyn ProcessRunner,
    ) -> Result<(PlatformCommandResult, Option<String>), AppError>,
) -> Result<Option<String>, AppError> {
    if designer_snapshot_is_ready(&resolved.platform_target_path)? {
        return Ok(None);
    }

    debug!(
        source_set = resolved.source_set_name.as_str(),
        target = %resolved.platform_target_path.display(),
        "bootstrapping missing designer dump snapshot for EDT reverse sync"
    );
    let (_, message) = full_dump_runner(context, config, resolved, binary, runner)?;
    Ok(message)
}

fn designer_snapshot_is_ready(path: &Path) -> Result<bool, AppError> {
    if !path.exists() {
        return Ok(false);
    }
    if !path.is_dir() {
        return Ok(false);
    }
    Ok(path.join("Configuration.xml").is_file())
}

fn finalize_edt_dump(
    context: &ExecutionContext,
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    edt_binary: &Path,
    edt_runner: &dyn ProcessRunner,
    platform_result: PlatformCommandResult,
    inherited_message: Option<String>,
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    ensure_interruption_clear(
        context,
        "before starting EDT reverse-sync import after designer snapshot publication",
    )?;
    let target_parent = resolved.target_path.parent().ok_or_else(|| {
        AppError::Runtime(format!(
            "target path has no parent: {}",
            resolved.target_path.display()
        ))
    })?;
    ensure_dir(target_parent).map_err(|error| {
        AppError::Runtime(format!("failed to create target parent dir: {error}"))
    })?;

    let run_id = make_run_id();
    let staging_dir = target_parent.join(format!(".dump-stage-{run_id}"));
    if staging_dir.exists() {
        return Err(AppError::Runtime(format!(
            "staging dir already exists unexpectedly: {}",
            staging_dir.display()
        )));
    }
    std::fs::create_dir(&staging_dir)
        .map_err(|error| AppError::Runtime(format!("failed to create staging dir: {error}")))?;
    write_temp_dir_metadata(
        &staging_dir,
        TempDirKind::Stage,
        &run_id,
        &resolved.target_path,
        &resolved.target_identity,
    )
    .map_err(|error| AppError::Runtime(format!("failed to write stage metadata: {error}")))?;

    let edt_dsl = build_edt_dsl(context, config, edt_binary, edt_runner)
        .map_err(|error| cleanup_staging_on_platform_failure(&staging_dir, error))?;
    let import_result = match edt_dsl
        .import_configuration_files(
            &staging_dir,
            &resolved.platform_target_path,
            normalize_config_hint(config.tools.platform.version.as_deref()),
            resolved.edt_base_project_name.as_deref(),
            false,
        )
        .map_err(|error| AppError::Platform(error.to_string()))
    {
        Ok(result) => result,
        Err(error) => return Err(cleanup_staging_on_platform_failure(&staging_dir, error)),
    };
    ensure_import_success(resolved, &import_result)
        .map_err(|error| cleanup_staging_on_platform_failure(&staging_dir, error))?;
    validate_edt_dump_staging_output(&staging_dir)
        .map_err(|error| cleanup_staging_on_platform_failure(&staging_dir, error))?;
    validate_publish_target(resolved)
        .map_err(|error| cleanup_staging_on_platform_failure(&staging_dir, error))?;

    if let Some(interruption) = context.interruption() {
        return Err(cleanup_staging_on_interruption(
            &staging_dir,
            AppError::Runtime(format!(
                "{} for command '{}' before entering dump publication safe point",
                interruption.message(context.command()),
                context.command().as_str()
            )),
        ));
    }

    let publish_phase = context.run_no_process_critical_phase(|| {
        replace_dir_atomically(
            &staging_dir,
            &resolved.target_path,
            &run_id,
            &resolved.target_identity,
            DUMP_BACKUP_PREFIX,
        )
        .map_err(|error| AppError::Runtime(format!("failed to publish staged dump: {error}")))
    })?;

    Ok((
        platform_result,
        merge_optional_messages(
            inherited_message,
            merge_optional_messages(
                publish_phase.value.cleanup_warning,
                dump_publication_warning(context.command(), publish_phase.deferred_interruption),
            ),
        ),
    ))
}

fn validate_edt_dump_staging_output(staging_dir: &Path) -> Result<(), AppError> {
    if staging_dir.join(".project").is_file() {
        Ok(())
    } else {
        Err(AppError::Validation(format!(
            "EDT dump output must contain '.project': {}",
            staging_dir.display()
        )))
    }
}

fn ensure_import_success(
    resolved: &ResolvedDumpTarget,
    result: &PlatformCommandResult,
) -> Result<(), AppError> {
    if result.process.exit_code == 0 {
        return Ok(());
    }

    let mut details = vec![format!(
        "dump EDT import failed for source-set '{}' with exit code {}",
        resolved.source_set_name, result.process.exit_code
    )];
    if !result.process.stdout.trim().is_empty() {
        details.push(format!("stdout: {}", result.process.stdout.trim()));
    }
    if !result.process.stderr.trim().is_empty() {
        details.push(format!("stderr: {}", result.process.stderr.trim()));
    }
    if let Some(log) = result
        .platform_log
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        details.push(format!("platform log: {}", log.trim()));
    }
    if let Some(path) = result.platform_log_path.as_ref() {
        details.push(format!("platform log path: {}", path.display()));
    }
    Err(AppError::Platform(details.join("; ")))
}

fn build_edt_dsl<'a>(
    context: &ExecutionContext,
    config: &AppConfig,
    binary: &Path,
    runner: &'a dyn ProcessRunner,
) -> Result<EdtDsl<'a>, AppError> {
    let workspace = config.work_path.join("edt-workspace");
    let policy = context.process_policy(InterruptionSafetyClass::GracefulThenKill, None);
    if config.tools.edt_cli.interactive_mode {
        let manager =
            EdtSessionManager::for_config(config, EdtSessionHostOptions::for_cli_command(config))
                .map_err(|error| AppError::Platform(error.to_string()))?;
        EdtDsl::new_shared_session(
            binary.to_path_buf(),
            workspace,
            Arc::new(manager),
            Duration::from_millis(config.tools.edt_cli.startup_timeout_ms),
            Duration::from_millis(config.tools.edt_cli.command_timeout_ms),
        )
        .map_err(|error| AppError::Platform(error.to_string()))
        .map(|dsl| {
            dsl.with_timeout(context.edt_timeout())
                .with_execution_policy(policy)
        })
    } else {
        Ok(EdtDsl::new(binary.to_path_buf(), workspace, runner)
            .with_timeout(context.edt_timeout())
            .with_execution_policy(policy))
    }
}

fn normalize_config_hint(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

pub(crate) fn run_external_dump_designer(
    dsl: &DesignerDsl<'_>,
    binary_path: &Path,
    root_xml_path: &Path,
    expected_kind: ExternalArtifactKind,
    expected_logical_name: &str,
) -> Result<(PlatformCommandResult, PathBuf), (AppError, Option<PathBuf>)> {
    let parent = root_xml_path.parent().ok_or_else(|| {
        (
            AppError::Runtime(format!(
                "external dump target has no parent: {}",
                root_xml_path.display()
            )),
            None,
        )
    })?;
    ensure_dir(parent).map_err(|error| {
        (
            AppError::Runtime(format!("failed to create external dump dir: {error}")),
            None,
        )
    })?;
    remove_path_if_exists(root_xml_path).map_err(|error| {
        (
            AppError::Runtime(format!("failed to clean external root xml: {error}")),
            None,
        )
    })?;
    let result = dsl
        .dump_external_data_processor_or_report_to_files(binary_path, root_xml_path)
        .map_err(|error| (AppError::Platform(error.to_string()), None))?;
    if result.process.exit_code != 0 {
        return Err((
            AppError::Platform(format!(
                "external dump failed with exit code {}",
                result.process.exit_code
            )),
            result.platform_log_path.clone(),
        ));
    }
    let contents = match std::fs::read_to_string(root_xml_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err((
                AppError::Validation(format!(
                    "external dump '{}' did not produce descriptor xml",
                    root_xml_path.display()
                )),
                result.platform_log_path.clone(),
            ));
        }
        Err(error) => {
            return Err((
                AppError::Runtime(format!(
                    "failed to read external dump root xml '{}': {error}",
                    root_xml_path.display()
                )),
                result.platform_log_path.clone(),
            ));
        }
    };
    let (root_tag, logical_name) = parse_external_dump_descriptor(&contents, root_xml_path)
        .map_err(|error| (error, result.platform_log_path.clone()))?;
    if root_tag != expected_kind.root_tag() {
        return Err((
            AppError::Validation(format!(
                "external dump '{}' has unexpected root element",
                root_xml_path.display()
            )),
            result.platform_log_path.clone(),
        ));
    }
    if logical_name != expected_logical_name {
        return Err((
            AppError::Validation(format!(
                "external dump '{}' has unexpected logical name",
                root_xml_path.display()
            )),
            result.platform_log_path.clone(),
        ));
    }
    Ok((result, root_xml_path.to_path_buf()))
}

fn parse_external_dump_descriptor(
    contents: &str,
    path: &Path,
) -> Result<(String, String), AppError> {
    let mut reader = Reader::from_str(contents);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut root_tag = None;
    let mut artifact_root_tag = None;
    let mut seen_properties = false;
    let mut seen_name = false;
    let mut logical_name = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let tag = String::from_utf8_lossy(event.name().as_ref()).into_owned();
                if root_tag.is_none() {
                    root_tag = Some(tag.clone());
                }
                if artifact_root_tag.is_none() {
                    match tag.as_str() {
                        "MetaDataObject" => {}
                        "ExternalDataProcessor" | "ExternalReport" => {
                            artifact_root_tag = Some(tag.clone());
                        }
                        _ if root_tag.as_deref() != Some("MetaDataObject") => {
                            artifact_root_tag = Some(tag.clone());
                        }
                        _ => {}
                    }
                }
                if tag == "Properties" {
                    seen_properties = true;
                } else if seen_properties && tag == "Name" {
                    seen_name = true;
                }
            }
            Ok(Event::Text(text)) if seen_name && logical_name.is_none() => {
                logical_name = Some(
                    text.unescape()
                        .map_err(|error| {
                            AppError::Validation(format!(
                                "failed to decode external dump logical name in '{}': {error}",
                                path.display()
                            ))
                        })?
                        .into_owned(),
                );
            }
            Ok(Event::End(event)) => {
                let tag = String::from_utf8_lossy(event.name().as_ref()).into_owned();
                if tag == "Name" {
                    seen_name = false;
                } else if tag == "Properties" {
                    seen_properties = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(AppError::Validation(format!(
                    "failed to parse external dump xml '{}': {error}",
                    path.display()
                )));
            }
            _ => {}
        }
        buf.clear();
    }

    let root_tag = artifact_root_tag.or(root_tag).ok_or_else(|| {
        AppError::Validation(format!("missing root XML element in '{}'", path.display()))
    })?;
    let logical_name = logical_name
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::Validation(format!(
                "external dump '{}' must contain Properties/Name",
                path.display()
            ))
        })?;
    Ok((root_tag, logical_name))
}

fn cleanup_staging_on_platform_failure(staging_dir: &Path, error: AppError) -> AppError {
    let sidecar = metadata_sidecar_path(staging_dir);
    let _ = remove_path_if_exists(staging_dir);
    let _ = remove_path_if_exists(&sidecar);
    error
}

fn cleanup_staging_on_interruption(staging_dir: &Path, error: AppError) -> AppError {
    cleanup_staging_on_platform_failure(staging_dir, error)
}

fn resolve_target(config: &AppConfig, args: &DumpArgs) -> Result<ResolvedDumpTarget, AppError> {
    let service = SourceSetsService::new(config);
    let designer_contexts_by_name: HashMap<String, SourceSetContext> = service
        .designer_contexts()
        .into_iter()
        .map(|context| (context.name().to_owned(), context))
        .collect();
    let config_by_name: HashMap<String, &SourceSetConfig> = config
        .source_sets
        .iter()
        .map(|source_set| (source_set.name.clone(), source_set))
        .collect();

    let (source_set, extension) = match (args.source_set.as_deref(), args.extension.as_deref()) {
        (Some(source_set_name), None) => {
            let source_set = config_by_name
                .get(source_set_name)
                .copied()
                .ok_or_else(|| {
                    AppError::Validation(format!("unknown source-set '{source_set_name}'"))
                })?;
            if source_set.purpose != SourceSetPurpose::Configuration {
                return Err(AppError::Validation(format!(
                    "source-set '{source_set_name}' is an extension and requires --extension"
                )));
            }
            (source_set, None)
        }
        (None, Some(extension_name)) => {
            let source_set = config_by_name.get(extension_name).copied().ok_or_else(|| {
                AppError::Validation(format!("unknown extension '{extension_name}'"))
            })?;
            if source_set.purpose != SourceSetPurpose::Extension {
                return Err(AppError::Validation(format!(
                    "source-set '{extension_name}' is not an extension source-set"
                )));
            }
            (source_set, Some(extension_name.to_owned()))
        }
        (Some(source_set_name), Some(extension_name)) => {
            if source_set_name != extension_name {
                return Err(AppError::Validation(format!(
                    "--source-set '{source_set_name}' does not match --extension '{extension_name}'"
                )));
            }
            let source_set = config_by_name
                .get(source_set_name)
                .copied()
                .ok_or_else(|| {
                    AppError::Validation(format!("unknown extension '{extension_name}'"))
                })?;
            if source_set.purpose != SourceSetPurpose::Extension {
                return Err(AppError::Validation(format!(
                    "source-set '{source_set_name}' is not an extension source-set"
                )));
            }
            (source_set, Some(extension_name.to_owned()))
        }
        (None, None) => {
            let configuration_source_sets = config
                .source_sets
                .iter()
                .filter(|source_set| source_set.purpose == SourceSetPurpose::Configuration)
                .collect::<Vec<_>>();
            if configuration_source_sets.len() != 1 {
                let candidates = configuration_source_sets
                    .iter()
                    .map(|source_set| source_set.name.as_str())
                    .collect::<Vec<_>>();
                return Err(AppError::Validation(format!(
                    "dump requires exactly one configuration source-set when --source-set is omitted; found [{}]",
                    candidates.join(", ")
                )));
            }
            (configuration_source_sets[0], None)
        }
    };

    let target_path = resolve_source_set_path(config, source_set);
    let platform_target_path = if config.format == SourceFormat::Edt {
        designer_contexts_by_name
            .get(&source_set.name)
            .cloned()
            .ok_or_else(|| {
                AppError::Runtime(format!(
                    "missing designer runtime context for source-set '{}'",
                    source_set.name
                ))
            })?
            .path()
            .to_path_buf()
    } else {
        target_path.clone()
    };
    let canonical_target_path = nearest_existing_canonical_path(&target_path).map_err(|error| {
        AppError::Runtime(format!("failed to canonicalize target path: {error}"))
    })?;
    let canonical_platform_target_path = nearest_existing_canonical_path(&platform_target_path)
        .map_err(|error| {
            AppError::Runtime(format!(
                "failed to canonicalize platform target path: {error}"
            ))
        })?;
    let canonical_base_path = nearest_existing_canonical_path(&config.base_path)
        .map_err(|error| AppError::Runtime(format!("failed to canonicalize basePath: {error}")))?;
    let canonical_work_path = nearest_existing_canonical_path(&config.work_path)
        .map_err(|error| AppError::Runtime(format!("failed to canonicalize workPath: {error}")))?;
    let target_identity = stable_path_identity(&canonical_target_path);
    let platform_target_identity = stable_path_identity(&canonical_platform_target_path);
    if target_path.parent().is_none() {
        return Err(AppError::Runtime(format!(
            "target path has no parent: {}",
            target_path.display()
        )));
    }
    let edt_base_project_name = if config.format == SourceFormat::Edt
        && source_set.purpose == SourceSetPurpose::Extension
    {
        Some(resolve_dump_edt_base_project_name(config)?)
    } else {
        None
    };
    let lock_path = hashed_lock_path(&canonical_target_path, "dump")
        .map_err(|error| AppError::Runtime(format!("failed to resolve dump lock path: {error}")))?;

    Ok(ResolvedDumpTarget {
        source_set_name: source_set.name.clone(),
        extension,
        target_path,
        canonical_target_path,
        platform_target_path,
        canonical_platform_target_path,
        canonical_base_path,
        canonical_work_path,
        target_identity,
        platform_target_identity,
        lock_path,
        edt_base_project_name,
    })
}

fn validate_supported_matrix(config: &AppConfig) -> Option<AppError> {
    if matches!(
        config.builder,
        BuilderBackend::Designer | BuilderBackend::Ibcmd
    ) {
        None
    } else {
        Some(AppError::Validation(SUPPORTED_DUMP_ERROR.to_owned()))
    }
}

fn validate_publish_target(resolved: &ResolvedDumpTarget) -> Result<(), AppError> {
    validate_publish_target_path(
        &resolved.target_path,
        &resolved.canonical_target_path,
        &resolved.canonical_base_path,
        &resolved.canonical_work_path,
    )
}

fn cleanup_orphan_dirs(resolved: &ResolvedDumpTarget) -> Result<(), AppError> {
    cleanup_orphan_dirs_for(&resolved.target_path, &resolved.target_identity)
}

fn validate_platform_target(resolved: &ResolvedDumpTarget) -> Result<(), AppError> {
    validate_publish_target_path(
        &resolved.platform_target_path,
        &resolved.canonical_platform_target_path,
        &resolved.canonical_base_path,
        &resolved.canonical_work_path,
    )
}

fn validate_publish_target_path(
    target_path: &Path,
    canonical_target_path: &Path,
    canonical_base_path: &Path,
    canonical_work_path: &Path,
) -> Result<(), AppError> {
    if canonical_target_path
        != nearest_existing_canonical_path(target_path).map_err(|error| {
            AppError::Runtime(format!("failed to re-canonicalize target path: {error}"))
        })?
    {
        return Err(AppError::Validation(format!(
            "target path changed during dump resolution: {}",
            target_path.display()
        )));
    }

    if canonical_target_path == canonical_base_path {
        return Err(AppError::Validation(
            "dump target must not equal basePath".to_owned(),
        ));
    }
    if canonical_target_path == canonical_work_path {
        return Err(AppError::Validation(
            "dump target must not equal workPath".to_owned(),
        ));
    }
    if is_filesystem_root(canonical_target_path) {
        return Err(AppError::Validation(
            "dump target must not equal filesystem root".to_owned(),
        ));
    }
    Ok(())
}

fn cleanup_platform_orphan_dirs(resolved: &ResolvedDumpTarget) -> Result<(), AppError> {
    cleanup_orphan_dirs_for(
        &resolved.platform_target_path,
        &resolved.platform_target_identity,
    )
}

fn cleanup_orphan_dirs_for(target_path: &Path, target_identity: &str) -> Result<(), AppError> {
    let target_parent = target_path.parent().ok_or_else(|| {
        AppError::Runtime(format!(
            "target path has no parent: {}",
            target_path.display()
        ))
    })?;
    if !target_parent.exists() {
        return Ok(());
    }

    for entry in std::fs::read_dir(target_parent)
        .map_err(|error| AppError::Runtime(format!("failed to read target parent: {error}")))?
    {
        let entry = entry
            .map_err(|error| AppError::Runtime(format!("failed to read dir entry: {error}")))?;
        let path = entry.path();
        if !entry
            .file_type()
            .map_err(|error| AppError::Runtime(format!("failed to stat dir entry: {error}")))?
            .is_dir()
        {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !file_name.starts_with(".dump-stage-") && !file_name.starts_with(".dump-backup-") {
            continue;
        }

        let Ok(metadata) = read_temp_dir_metadata(&path) else {
            continue;
        };
        if !is_known_tool_name(&metadata.tool) || metadata.target_identity != target_identity {
            continue;
        }
        if (chrono::Utc::now() - metadata.created_at)
            .to_std()
            .unwrap_or_default()
            < ORPHAN_TTL
        {
            continue;
        }

        remove_path_if_exists(&path).map_err(|error| {
            AppError::Runtime(format!(
                "failed to remove stale temp dir '{}': {error}",
                path.display()
            ))
        })?;
        remove_path_if_exists(&metadata_sidecar_path(&path)).map_err(|error| {
            AppError::Runtime(format!(
                "failed to remove stale temp metadata '{}': {error}",
                metadata_sidecar_path(&path).display()
            ))
        })?;
    }

    Ok(())
}

fn resolve_source_set_path(config: &AppConfig, source_set: &SourceSetConfig) -> PathBuf {
    if source_set.path.is_absolute() {
        source_set.path.clone()
    } else {
        config.base_path.join(&source_set.path)
    }
}

fn resolve_dump_edt_base_project_name(config: &AppConfig) -> Result<String, AppError> {
    let configuration_source_sets = config
        .source_sets
        .iter()
        .filter(|source_set| source_set.purpose == SourceSetPurpose::Configuration)
        .collect::<Vec<_>>();
    if configuration_source_sets.len() != 1 {
        let candidates = configuration_source_sets
            .iter()
            .map(|source_set| source_set.name.as_str())
            .collect::<Vec<_>>();
        return Err(AppError::Validation(format!(
            "dump EDT extension reverse sync requires exactly one configuration source-set to infer EDT base project name; found [{}]",
            candidates.join(", ")
        )));
    }

    read_edt_project_name(
        &resolve_source_set_path(config, configuration_source_sets[0]),
        &format!(
            "configuration source-set '{}'",
            configuration_source_sets[0].name
        ),
    )
}

fn read_edt_project_name(path: &Path, label: &str) -> Result<String, AppError> {
    let project_file = path.join(".project");
    let contents = std::fs::read_to_string(&project_file).map_err(|error| {
        AppError::Runtime(format!(
            "failed to read {label} project file '{}': {error}",
            project_file.display()
        ))
    })?;
    extract_xml_tag_text(&contents, "name")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::Validation(format!(
                "{label} must contain a non-empty EDT project name: {}",
                project_file.display()
            ))
        })
}

fn extract_xml_tag_text(contents: &str, tag_name: &str) -> Option<String> {
    let open_tag = format!("<{tag_name}>");
    let close_tag = format!("</{tag_name}>");
    let start = contents.find(&open_tag)? + open_tag.len();
    let rest = &contents[start..];
    let end = rest.find(&close_tag)?;
    Some(rest[..end].trim().to_owned())
}

fn build_designer_dsl<'a>(
    context: &ExecutionContext,
    config: &AppConfig,
    binary: &Path,
    runner: &'a dyn ProcessRunner,
    source_set_name: &str,
    action: &str,
) -> Result<DesignerDsl<'a>, AppError> {
    let log_dir = platform_logs_dir(&config.work_path).map_err(|error| {
        AppError::Runtime(format!("failed to create platform logs dir: {error}"))
    })?;
    let log_file = log_dir.join(format!("dump-{source_set_name}-{action}.log"));

    Ok(DesignerDsl::new(
        binary.to_path_buf(),
        config.v8_connection(),
        runner,
        Some(log_file),
    )
    .with_execution_policy(context.process_policy(InterruptionSafetyClass::GracefulThenKill, None)))
}

fn build_ibcmd_dsl<'a>(
    context: &ExecutionContext,
    config: &AppConfig,
    binary: &Path,
    runner: &'a dyn ProcessRunner,
) -> Result<IbcmdDsl<'a>, AppError> {
    let connection = IbcmdConnection::from_infobase(&config.infobase).map_err(map_ibcmd_error)?;

    Ok(
        IbcmdDsl::new(binary.to_path_buf(), connection, runner).with_execution_policy(
            context.process_policy(InterruptionSafetyClass::GracefulThenKill, None),
        ),
    )
}

fn map_ibcmd_error(error: IbcmdError) -> AppError {
    match error {
        IbcmdError::MissingServerDbmsField(_) => AppError::Validation(error.to_string()),
        IbcmdError::Spawn(_) => AppError::Platform(error.to_string()),
    }
}

fn merge_optional_messages(left: Option<String>, right: Option<String>) -> Option<String> {
    match (left, right) {
        (Some(left), Some(right)) => Some(format!("{left}; {right}")),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

fn dump_publication_warning(
    command: crate::use_cases::context::CommandName,
    deferred_interruption: Option<crate::use_cases::context::ExecutionInterruption>,
) -> Option<String> {
    deferred_interruption.map(|interruption| {
        format!(
            "dump publication completed after {} for command '{}' during critical phase; unsafe interruption was not performed",
            match interruption {
                crate::use_cases::context::ExecutionInterruption::Cancelled => "cancellation request",
                crate::use_cases::context::ExecutionInterruption::TimedOut => "timeout",
            },
            command.as_str()
        )
    })
}

fn validate_dump_objects(
    mode: &DumpMode,
    objects: &[String],
) -> Result<Option<Vec<String>>, AppError> {
    match mode {
        DumpMode::Partial => normalize_partial_objects(objects).map(Some),
        _ if !objects.is_empty() => Err(AppError::Validation(NON_PARTIAL_OBJECTS_ERROR.to_owned())),
        _ => Ok(None),
    }
}

fn normalize_partial_objects(objects: &[String]) -> Result<Vec<String>, AppError> {
    if objects.is_empty() {
        return Err(AppError::Validation(
            PARTIAL_OBJECTS_REQUIRED_ERROR.to_owned(),
        ));
    }

    let mut normalized = Vec::with_capacity(objects.len());
    for object in objects {
        if object.chars().any(char::is_control) {
            return Err(AppError::Validation(
                PARTIAL_OBJECT_CONTROL_ERROR.to_owned(),
            ));
        }
        let object = object.trim();
        if object.is_empty() {
            return Err(AppError::Validation(PARTIAL_OBJECT_BLANK_ERROR.to_owned()));
        }
        normalized.push(object.to_owned());
    }

    if normalized.is_empty() {
        return Err(AppError::Validation(
            PARTIAL_OBJECTS_REQUIRED_ERROR.to_owned(),
        ));
    }

    Ok(normalized)
}

fn create_dump_object_list_file(
    work_path: &Path,
    objects: &[String],
) -> Result<NamedTempFile, AppError> {
    create_dump_object_list_file_with(work_path, objects, write_partial_object_list)
}

fn create_dump_object_list_file_with<F>(
    work_path: &Path,
    objects: &[String],
    write_list: F,
) -> Result<NamedTempFile, AppError>
where
    F: FnOnce(&mut NamedTempFile, &[String]) -> std::io::Result<()>,
{
    let mut list_file = dump_object_list_file(work_path).map_err(|error| {
        AppError::Runtime(format!("failed to create partial dump list: {error}"))
    })?;
    write_list(&mut list_file, objects).map_err(|error| {
        AppError::Runtime(format!("failed to write partial dump list: {error}"))
    })?;
    Ok(list_file)
}

fn write_partial_object_list(
    list_file: &mut NamedTempFile,
    objects: &[String],
) -> std::io::Result<()> {
    let writer = list_file.as_file_mut();
    writer.set_len(0)?;
    for object in objects {
        writeln!(writer, "{object}")?;
    }
    writer.flush()
}

fn ibcmd_partial_warning(resolved: &ResolvedDumpTarget) -> String {
    match resolved.extension.as_deref() {
        Some(extension) => format!(
            "IBCMD does not support object-scoped partial dump; ran incremental export for extension '{extension}' instead"
        ),
        None => format!(
            "IBCMD does not support object-scoped partial dump; ran incremental export for source-set '{}' instead",
            resolved.source_set_name
        ),
    }
}

fn decorate_ibcmd_partial_error(error: AppError, warning: &str) -> AppError {
    match error {
        AppError::Validation(message) => AppError::Validation(format!("{warning}; {message}")),
        AppError::Runtime(message) => AppError::Runtime(format!("{warning}; {message}")),
        AppError::Platform(message) => AppError::Platform(format!("{warning}; {message}")),
        AppError::Config(error) => AppError::Runtime(format!("{warning}; {error}")),
    }
}

fn ensure_platform_success(
    action: &str,
    resolved: &ResolvedDumpTarget,
    result: &PlatformCommandResult,
) -> Result<(), AppError> {
    if result.process.exit_code == 0 {
        return Ok(());
    }

    Err(AppError::Platform(format_ibcmd_failure_details(
        action,
        "source-set",
        &resolved.source_set_name,
        result.process.exit_code,
        &result.process.stdout,
        &result.process.stderr,
        result.platform_log.as_deref(),
        result.platform_log_path.as_deref(),
    )))
}

fn empty_result(
    mode: DumpMode,
    started: Instant,
    source_set: Option<String>,
    extension: Option<String>,
    target_path: Option<PathBuf>,
    message: Option<String>,
) -> DumpResult {
    DumpResult {
        ok: false,
        source_set,
        extension,
        mode,
        target_path: target_path.unwrap_or_default(),
        platform_log_path: None,
        duration_ms: started.elapsed().as_millis() as u64,
        message,
    }
}

fn make_run_id() -> String {
    let timestamp = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
    format!("{}-{timestamp:x}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::{
        build_designer_dsl, cleanup_orphan_dirs, cleanup_staging_on_interruption,
        create_dump_object_list_file_with, finalize_edt_dump, metadata_sidecar_path,
        parse_external_dump_descriptor, resolve_target, run_dump, run_external_dump_designer,
        validate_publish_target, validate_supported_matrix, DUMP_COMMAND,
        NON_PARTIAL_OBJECTS_ERROR, ORPHAN_TTL, PARTIAL_OBJECTS_REQUIRED_ERROR,
        PARTIAL_OBJECT_BLANK_ERROR, PARTIAL_OBJECT_CONTROL_ERROR,
    };
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, PlatformToolConfig, SourceFormat, SourceSetConfig,
        SourceSetPurpose, TestsConfig, ToolsConfig,
    };
    use crate::domain::dump::DumpMode;
    use crate::output::json::Envelope;
    use crate::platform::process::{
        ProcessError, ProcessExecutionPolicy, ProcessRequest, ProcessResult, ProcessRunner,
        SpawnResult,
    };
    use crate::platform::result::PlatformCommandResult;
    use crate::support::error::AppError;
    use crate::support::fs::{
        acquire_advisory_lock, read_temp_dir_metadata, write_temp_dir_metadata, TempDirKind,
    };
    use crate::support::path::{nearest_existing_canonical_path, stable_path_identity};
    use crate::use_cases::context::ExecutionContext;
    use crate::use_cases::external_artifacts::ExternalArtifactKind;
    use crate::use_cases::request::{DumpModeRequest, DumpRequest as DumpArgs};
    use crate::use_cases::result::UseCaseErrorKind;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread;
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    #[cfg(unix)]
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) {}

    fn write_script(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent");
        }
        fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write");
        make_executable(path);
    }

    fn write_dump_script(path: &Path, calls_log: &Path, fail_pattern: Option<&str>, sleep_ms: u64) {
        let pattern_branch = fail_pattern
            .map(|pattern| {
                format!(
                    "if printf '%s' \"$args\" | grep -F -q -- '{}'; then exit 17; fi",
                    pattern
                )
            })
            .unwrap_or_default();
        let sleep_branch = if sleep_ms == 0 {
            String::new()
        } else {
            format!("sleep {}", sleep_ms as f64 / 1000.0)
        };
        let body = format!(
            "args=\"$*\"\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf 'designer log for %s\\n' \"$args\" > \"$out\"; fi\nprintf '%s\\n' \"$args\" >> \"{}\"\n{}\n{}\nmkdir -p \"$(printf '%s' \"$args\" | awk '{{print $NF}}')\"\nexit 0",
            calls_log.display(),
            sleep_branch,
            pattern_branch
        );
        write_script(path, &body);
    }

    fn write_ibcmd_dump_script(
        path: &Path,
        calls_log: &Path,
        fail_pattern: Option<&str>,
        sleep_ms: u64,
    ) {
        let pattern_branch = fail_pattern
            .map(|pattern| {
                format!(
                    "if printf '%s' \"$args\" | grep -F -q -- '{}'; then exit 17; fi",
                    pattern
                )
            })
            .unwrap_or_default();
        let sleep_branch = if sleep_ms == 0 {
            String::new()
        } else {
            format!("sleep {}", sleep_ms as f64 / 1000.0)
        };
        let body = format!(
            "args=\"$*\"\nprintf '%s\\n' \"$args\" >> \"{}\"\n{}\n{}\nmkdir -p \"$(printf '%s' \"$args\" | awk '{{print $NF}}')\"\nexit 0",
            calls_log.display(),
            sleep_branch,
            pattern_branch
        );
        write_script(path, &body);
    }

    fn write_designer_dump_script_for_edt(
        path: &Path,
        calls_log: &Path,
        fail_pattern: Option<&str>,
    ) {
        let pattern_branch = fail_pattern
            .map(|pattern| {
                format!(
                    "if printf '%s' \"$args\" | grep -F -q -- '{}'; then exit 17; fi",
                    pattern
                )
            })
            .unwrap_or_default();
        let body = format!(
            "args=\"$*\"\nout=\"\"\ntarget=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  if [ \"$prev\" = \"/DumpConfigToFiles\" ]; then target=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf 'designer log for %s\\n' \"$args\" > \"$out\"; fi\nprintf '%s\\n' \"$args\" >> \"{}\"\n{}\nmkdir -p \"$target\"\nif printf '%s' \"$args\" | grep -F -q -- '-partial'; then\n  printf '<Partial />\\n' > \"$target/PartialOnly.xml\"\nelse\n  printf '<Configuration />\\n' > \"$target/Configuration.xml\"\nfi\nexit 0",
            calls_log.display(),
            pattern_branch
        );
        write_script(path, &body);
    }

    fn write_ibcmd_dump_script_for_edt(path: &Path, calls_log: &Path, fail_pattern: Option<&str>) {
        let pattern_branch = fail_pattern
            .map(|pattern| {
                format!(
                    "if printf '%s' \"$args\" | grep -F -q -- '{}'; then exit 17; fi",
                    pattern
                )
            })
            .unwrap_or_default();
        let body = format!(
            "args=\"$*\"\ntarget=\"$(printf '%s' \"$args\" | awk '{{print $NF}}')\"\nprintf '%s\\n' \"$args\" >> \"{}\"\n{}\nmkdir -p \"$target\"\nprintf '<Configuration />\\n' > \"$target/Configuration.xml\"\nexit 0",
            calls_log.display(),
            pattern_branch
        );
        write_script(path, &body);
    }

    fn write_edt_import_script(path: &Path, calls_log: &Path) {
        let body = format!(
            "args=\"$*\"\nprintf '%s\\n' \"$args\" >> \"{}\"\nproject=\"\"\nconfig_files=\"\"\nbase_project_name=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"--project\" ]; then project=\"$arg\"; fi\n  if [ \"$prev\" = \"--configuration-files\" ]; then config_files=\"$arg\"; fi\n  if [ \"$prev\" = \"--base-project-name\" ]; then base_project_name=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nsource_name=$(basename \"$config_files\")\nmkdir -p \"$project\"\ncase \"$source_name\" in\n  main)\n    imported_name=\"BaseProject\"\n    ;;\n  ext)\n    if [ \"$base_project_name\" != \"BaseProject\" ]; then\n      printf 'unexpected base project: %s\\n' \"$base_project_name\" >&2\n      exit 23\n    fi\n    imported_name=\"ExtensionProject\"\n    ;;\n  *)\n    imported_name=\"ImportedProject\"\n    ;;\nesac\nprintf '<projectDescription><name>%s</name></projectDescription>\\n' \"$imported_name\" > \"$project/.project\"\nexit 0",
            calls_log.display()
        );
        write_script(path, &body);
    }

    #[derive(Clone, Default)]
    struct TestProcessRunner {
        calls: Arc<Mutex<Vec<Vec<String>>>>,
        cancel_after_call: Option<(usize, CancellationToken)>,
        on_run: Option<Arc<dyn Fn(&ProcessRequest) + Send + Sync>>,
    }

    impl TestProcessRunner {
        fn with_cancellation_on_call(call_index: usize, cancellation: CancellationToken) -> Self {
            Self {
                cancel_after_call: Some((call_index, cancellation)),
                ..Self::default()
            }
        }

        fn with_on_run(on_run: impl Fn(&ProcessRequest) + Send + Sync + 'static) -> Self {
            Self {
                on_run: Some(Arc::new(on_run)),
                ..Self::default()
            }
        }

        fn call_count(&self) -> usize {
            self.calls.lock().expect("calls").len()
        }

        fn run_request(&self, request: &ProcessRequest) -> Result<ProcessResult, ProcessError> {
            let call_index = {
                let mut calls = self.calls.lock().expect("calls");
                calls.push(request.args.clone());
                calls.len()
            };
            if let Some(on_run) = &self.on_run {
                on_run(request);
            }
            if let Some((cancel_on_call, cancellation)) = &self.cancel_after_call {
                if call_index == *cancel_on_call {
                    cancellation.cancel();
                }
            }
            Ok(ProcessResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
                interruption: None,
            })
        }
    }

    impl ProcessRunner for TestProcessRunner {
        fn run(&self, request: &ProcessRequest) -> Result<ProcessResult, ProcessError> {
            self.run_request(request)
        }

        fn run_with_timeout(
            &self,
            request: &ProcessRequest,
            _timeout: Duration,
        ) -> Result<ProcessResult, ProcessError> {
            self.run_request(request)
        }

        fn run_with_policy(
            &self,
            request: &ProcessRequest,
            _policy: &ProcessExecutionPolicy,
        ) -> Result<ProcessResult, ProcessError> {
            self.run_request(request)
        }

        fn spawn(&self, _request: &ProcessRequest) -> Result<SpawnResult, ProcessError> {
            panic!("spawn must not be used in dump_config tests")
        }
    }

    fn build_config(base_path: &Path, work_path: &Path, platform_path: &Path) -> AppConfig {
        build_config_with_builder(
            base_path,
            work_path,
            platform_path,
            BuilderBackend::Designer,
        )
    }

    fn build_config_with_builder(
        base_path: &Path,
        work_path: &Path,
        platform_path: &Path,
        builder: BuilderBackend,
    ) -> AppConfig {
        AppConfig {
            base_path: base_path.to_path_buf(),
            work_path: work_path.to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
            build: BuildConfig::default(),
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
        edt_path: &Path,
        builder: BuilderBackend,
    ) -> AppConfig {
        let mut config = build_config_with_builder(base_path, work_path, platform_path, builder);
        config.format = SourceFormat::Edt;
        config.tools.edt_cli.path = Some(edt_path.to_path_buf());
        config
    }

    fn create_source_tree(base_path: &Path) {
        fs::create_dir_all(base_path.join("main").join("Catalogs.Items")).expect("main");
        fs::create_dir_all(base_path.join("ext").join("CommonModules")).expect("ext");
        fs::write(
            base_path
                .join("main")
                .join("Catalogs.Items")
                .join("ObjectModule.bsl"),
            "module",
        )
        .expect("main bsl");
        fs::write(
            base_path
                .join("ext")
                .join("CommonModules")
                .join("Module.bsl"),
            "module",
        )
        .expect("ext bsl");
    }

    fn create_edt_source_tree(base_path: &Path) {
        fs::create_dir_all(base_path.join("main")).expect("main");
        fs::create_dir_all(base_path.join("ext")).expect("ext");
        fs::write(
            base_path.join("main").join(".project"),
            "<projectDescription><name>BaseProject</name></projectDescription>\n",
        )
        .expect("main project");
        fs::write(
            base_path.join("ext").join(".project"),
            "<projectDescription><name>ExtensionProject</name></projectDescription>\n",
        )
        .expect("ext project");
    }

    fn partial_list_paths(work_path: &Path) -> Vec<PathBuf> {
        let partial_dir = work_path.join("temp").join("partial-lists");
        if !partial_dir.is_dir() {
            return Vec::new();
        }

        let mut paths = fs::read_dir(partial_dir)
            .expect("partial lists dir")
            .map(|entry| entry.expect("entry").path())
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }

    #[test]
    fn edt_dump_support_matrix_accepts_designer_backend() {
        let dir = tempdir().expect("tempdir");
        let config = AppConfig {
            format: SourceFormat::Edt,
            builder: BuilderBackend::Designer,
            ..build_config(dir.path(), dir.path(), dir.path())
        };

        let error = validate_supported_matrix(&config);

        assert!(error.is_none());
    }

    #[test]
    fn ibcmd_dump_support_matrix_accepts_designer_format_with_ibcmd_builder() {
        let dir = tempdir().expect("tempdir");
        let config =
            build_config_with_builder(dir.path(), dir.path(), dir.path(), BuilderBackend::Ibcmd);

        let error = validate_supported_matrix(&config);

        assert!(error.is_none());
    }

    #[test]
    fn partial_requires_at_least_one_object() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        create_source_tree(dir.path());
        write_script(&script, "exit 0");
        let config = build_config(dir.path(), &dir.path().join("work"), &script);

        let failure = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Partial,
                source_set: None,
                extension: None,
                objects: vec![],
            },
        )
        .expect_err("failure");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Validation);
        assert_eq!(failure.error.message(), PARTIAL_OBJECTS_REQUIRED_ERROR);
    }

    #[test]
    fn partial_rejects_blank_objects() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        create_source_tree(dir.path());
        write_script(&script, "exit 0");
        let config = build_config(dir.path(), &dir.path().join("work"), &script);

        let failure = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Partial,
                source_set: None,
                extension: None,
                objects: vec!["   ".to_owned()],
            },
        )
        .expect_err("failure");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Validation);
        assert_eq!(failure.error.message(), PARTIAL_OBJECT_BLANK_ERROR);
    }

    #[test]
    fn partial_rejects_control_characters_in_objects() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        create_source_tree(dir.path());
        write_script(&script, "exit 0");
        let config = build_config(dir.path(), &dir.path().join("work"), &script);

        let failure = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Partial,
                source_set: None,
                extension: None,
                objects: vec!["Catalog.Items\nLine".to_owned()],
            },
        )
        .expect_err("failure");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Validation);
        assert_eq!(failure.error.message(), PARTIAL_OBJECT_CONTROL_ERROR);
    }

    #[test]
    fn partial_rejects_leading_or_trailing_control_characters_after_trim() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        create_source_tree(dir.path());
        write_script(&script, "exit 0");
        let config = build_config(dir.path(), &dir.path().join("work"), &script);

        for object in ["\nCatalog.Items".to_owned(), "Catalog.Items\t".to_owned()] {
            let failure = run_dump(
                &config,
                &DumpArgs {
                    mode: DumpModeRequest::Partial,
                    source_set: None,
                    extension: None,
                    objects: vec![object],
                },
            )
            .expect_err("failure");

            assert_eq!(failure.error.kind(), UseCaseErrorKind::Validation);
            assert_eq!(failure.error.message(), PARTIAL_OBJECT_CONTROL_ERROR);
        }
    }

    #[test]
    fn rejects_objects_for_incremental() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        create_source_tree(dir.path());
        write_script(&script, "exit 0");
        let config = build_config(dir.path(), &dir.path().join("work"), &script);

        let failure = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Incremental,
                source_set: None,
                extension: None,
                objects: vec!["Catalog:Items".to_owned()],
            },
        )
        .expect_err("failure");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Validation);
        assert_eq!(failure.error.message(), NON_PARTIAL_OBJECTS_ERROR);
    }

    #[test]
    fn rejects_objects_for_full() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        create_source_tree(dir.path());
        write_script(&script, "exit 0");
        let config = build_config(dir.path(), &dir.path().join("work"), &script);

        let failure = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Full,
                source_set: None,
                extension: None,
                objects: vec!["Catalog:Items".to_owned()],
            },
        )
        .expect_err("failure");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Validation);
        assert_eq!(failure.error.message(), NON_PARTIAL_OBJECTS_ERROR);
    }

    #[test]
    fn resolve_target_requires_explicit_source_set_when_multiple_configurations_exist() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        create_source_tree(dir.path());
        write_script(&script, "exit 0");
        let mut config = build_config(dir.path(), &dir.path().join("work"), &script);
        config.source_sets.push(SourceSetConfig {
            name: "main2".to_owned(),
            purpose: SourceSetPurpose::Configuration,
            path: PathBuf::from("main"),
        });

        let error = resolve_target(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Full,
                source_set: None,
                extension: None,
                objects: vec![],
            },
        )
        .expect_err("expected ambiguity");

        assert!(matches!(error, AppError::Validation(_)));
    }

    #[test]
    fn resolve_target_requires_extension_source_set_to_match_extension_name() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        create_source_tree(dir.path());
        write_script(&script, "exit 0");
        let config = build_config(dir.path(), &dir.path().join("work"), &script);

        let error = resolve_target(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Full,
                source_set: Some("main".to_owned()),
                extension: Some("ext".to_owned()),
                objects: vec![],
            },
        )
        .expect_err("expected mismatch");

        assert!(matches!(error, AppError::Validation(_)));
    }

    #[test]
    fn validate_publish_target_allows_absolute_source_set_outside_base_path() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let external = dir.path().join("external-main");
        let work = dir.path().join("work");
        let script = dir.path().join("1cv8");
        fs::create_dir_all(&base).expect("base");
        fs::create_dir_all(&external).expect("external");
        fs::create_dir_all(&work).expect("work");
        write_script(&script, "exit 0");

        let mut config = build_config(&base, &work, &script);
        config.source_sets[0].path = external.clone();

        let resolved = resolve_target(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Full,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec![],
            },
        )
        .expect("resolved");

        validate_publish_target(&resolved).expect("absolute source-set should be allowed");
    }

    #[test]
    fn validate_publish_target_rejects_base_path() {
        let dir = tempdir().expect("tempdir");
        let resolved = super::ResolvedDumpTarget {
            source_set_name: "main".to_owned(),
            extension: None,
            target_path: dir.path().to_path_buf(),
            canonical_target_path: std::fs::canonicalize(dir.path()).expect("canonical"),
            platform_target_path: dir.path().to_path_buf(),
            canonical_platform_target_path: std::fs::canonicalize(dir.path()).expect("canonical"),
            canonical_base_path: std::fs::canonicalize(dir.path()).expect("canonical"),
            canonical_work_path: std::fs::canonicalize(dir.path().join("work").as_path())
                .unwrap_or_else(|_| dir.path().join("work")),
            target_identity: "id".to_owned(),
            platform_target_identity: "id".to_owned(),
            lock_path: dir.path().join(".lock"),
            edt_base_project_name: None,
        };

        let error = validate_publish_target(&resolved).expect_err("expected invalid");
        assert!(matches!(error, AppError::Validation(_)));
    }

    #[test]
    fn nearest_existing_canonical_path_uses_existing_ancestor() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("root");
        fs::create_dir_all(&root).expect("root");

        let resolved =
            nearest_existing_canonical_path(&root.join("nested").join("target")).expect("resolved");

        assert_eq!(
            resolved,
            std::fs::canonicalize(&root)
                .expect("canonical")
                .join("nested/target")
        );
    }

    #[test]
    fn cleanup_orphan_dirs_ignores_malformed_metadata() {
        let dir = tempdir().expect("tempdir");
        let target = dir.path().join("main");
        fs::create_dir_all(&target).expect("target");
        let canonical = std::fs::canonicalize(&target).expect("canonical");
        let identity = stable_path_identity(&canonical);
        let stage_dir = target
            .parent()
            .expect("parent")
            .join(".dump-stage-malformed");
        fs::create_dir_all(&stage_dir).expect("stage");
        fs::write(metadata_sidecar_path(&stage_dir), b"not json").expect("metadata");

        let resolved = super::ResolvedDumpTarget {
            source_set_name: "main".to_owned(),
            extension: None,
            target_path: target.clone(),
            canonical_target_path: canonical.clone(),
            platform_target_path: target.clone(),
            canonical_platform_target_path: canonical.clone(),
            canonical_base_path: std::fs::canonicalize(dir.path()).expect("canonical base"),
            canonical_work_path: std::fs::canonicalize(dir.path()).expect("canonical work"),
            target_identity: identity.clone(),
            platform_target_identity: identity.clone(),
            lock_path: target.parent().expect("parent").join(".lock"),
            edt_base_project_name: None,
        };

        cleanup_orphan_dirs(&resolved).expect("cleanup");
        assert!(stage_dir.exists());
    }

    #[test]
    fn cleanup_orphan_dirs_removes_old_valid_metadata() {
        let dir = tempdir().expect("tempdir");
        let target = dir.path().join("main");
        fs::create_dir_all(&target).expect("target");
        let canonical = std::fs::canonicalize(&target).expect("canonical");
        let identity = stable_path_identity(&canonical);
        let stage_dir = target.parent().expect("parent").join(".dump-stage-old");
        fs::create_dir_all(&stage_dir).expect("stage");
        write_temp_dir_metadata(&stage_dir, TempDirKind::Stage, "run", &target, &identity)
            .expect("metadata");
        let meta_path = metadata_sidecar_path(&stage_dir);
        let mut metadata = read_temp_dir_metadata(&stage_dir).expect("metadata");
        metadata.created_at = chrono::Utc::now()
            - chrono::Duration::from_std(ORPHAN_TTL + Duration::from_secs(1)).expect("duration");
        fs::write(&meta_path, serde_json::to_vec(&metadata).expect("json"))
            .expect("write metadata");

        let resolved = super::ResolvedDumpTarget {
            source_set_name: "main".to_owned(),
            extension: None,
            target_path: target.clone(),
            canonical_target_path: canonical.clone(),
            platform_target_path: target.clone(),
            canonical_platform_target_path: canonical.clone(),
            canonical_base_path: std::fs::canonicalize(dir.path()).expect("canonical base"),
            canonical_work_path: std::fs::canonicalize(dir.path()).expect("canonical work"),
            target_identity: identity.clone(),
            platform_target_identity: identity.clone(),
            lock_path: target.parent().expect("parent").join(".lock"),
            edt_base_project_name: None,
        };

        cleanup_orphan_dirs(&resolved).expect("cleanup");
        assert!(!stage_dir.exists());
        assert!(!meta_path.exists());
    }

    #[test]
    fn cleanup_orphan_dirs_ignores_recent_metadata() {
        let dir = tempdir().expect("tempdir");
        let target = dir.path().join("main");
        fs::create_dir_all(&target).expect("target");
        let canonical = std::fs::canonicalize(&target).expect("canonical");
        let identity = stable_path_identity(&canonical);
        let backup_dir = target.parent().expect("parent").join(".dump-backup-recent");
        fs::create_dir_all(&backup_dir).expect("backup");
        write_temp_dir_metadata(&backup_dir, TempDirKind::Backup, "run", &target, &identity)
            .expect("metadata");

        let resolved = super::ResolvedDumpTarget {
            source_set_name: "main".to_owned(),
            extension: None,
            target_path: target.clone(),
            canonical_target_path: canonical.clone(),
            platform_target_path: target.clone(),
            canonical_platform_target_path: canonical.clone(),
            canonical_base_path: std::fs::canonicalize(dir.path()).expect("canonical base"),
            canonical_work_path: std::fs::canonicalize(dir.path()).expect("canonical work"),
            target_identity: identity.clone(),
            platform_target_identity: identity.clone(),
            lock_path: target.parent().expect("parent").join(".lock"),
            edt_base_project_name: None,
        };

        cleanup_orphan_dirs(&resolved).expect("cleanup");

        assert!(backup_dir.exists());
        assert!(metadata_sidecar_path(&backup_dir).exists());
    }

    #[test]
    fn cleanup_orphan_dirs_ignores_foreign_metadata() {
        let dir = tempdir().expect("tempdir");
        let target = dir.path().join("main");
        fs::create_dir_all(&target).expect("target");
        let canonical = std::fs::canonicalize(&target).expect("canonical");
        let identity = stable_path_identity(&canonical);
        let stage_dir = target.parent().expect("parent").join(".dump-stage-foreign");
        fs::create_dir_all(&stage_dir).expect("stage");
        write_temp_dir_metadata(&stage_dir, TempDirKind::Stage, "run", &target, &identity)
            .expect("metadata");
        let meta_path = metadata_sidecar_path(&stage_dir);
        let mut metadata = read_temp_dir_metadata(&stage_dir).expect("metadata");
        metadata.tool = "foreign-tool".to_owned();
        metadata.created_at = chrono::Utc::now()
            - chrono::Duration::from_std(ORPHAN_TTL + Duration::from_secs(1)).expect("duration");
        fs::write(&meta_path, serde_json::to_vec(&metadata).expect("json"))
            .expect("write metadata");

        let resolved = super::ResolvedDumpTarget {
            source_set_name: "main".to_owned(),
            extension: None,
            target_path: target.clone(),
            canonical_target_path: canonical.clone(),
            platform_target_path: target.clone(),
            canonical_platform_target_path: canonical.clone(),
            canonical_base_path: std::fs::canonicalize(dir.path()).expect("canonical base"),
            canonical_work_path: std::fs::canonicalize(dir.path()).expect("canonical work"),
            target_identity: identity.clone(),
            platform_target_identity: identity.clone(),
            lock_path: target.parent().expect("parent").join(".lock"),
            edt_base_project_name: None,
        };

        cleanup_orphan_dirs(&resolved).expect("cleanup");

        assert!(stage_dir.exists());
        assert!(meta_path.exists());
    }

    #[test]
    fn cleanup_staging_on_interruption_removes_stage_dir_and_sidecar() {
        let dir = tempdir().expect("tempdir");
        let target = dir.path().join("main");
        fs::create_dir_all(&target).expect("target");
        let canonical = std::fs::canonicalize(&target).expect("canonical");
        let identity = stable_path_identity(&canonical);
        let stage_dir = target.parent().expect("parent").join(".dump-stage-run");
        fs::create_dir_all(&stage_dir).expect("stage");
        write_temp_dir_metadata(&stage_dir, TempDirKind::Stage, "run", &target, &identity)
            .expect("metadata");
        let meta_path = metadata_sidecar_path(&stage_dir);

        let error = cleanup_staging_on_interruption(
            &stage_dir,
            AppError::Runtime("interrupted before publish".to_owned()),
        );

        assert_eq!(
            error.to_string(),
            "runtime error: interrupted before publish"
        );
        assert!(!stage_dir.exists());
        assert!(!meta_path.exists());
    }

    #[test]
    fn dump_incremental_creates_missing_target_dir() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("1cv8");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_dump_script(&script, &calls, None, 0);
        let config = build_config(&base, &work, &script);
        fs::remove_dir_all(base.join("main")).expect("remove target");

        let result = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Incremental,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec![],
            },
        )
        .expect("dump");

        assert!(result.ok);
        assert!(base.join("main").exists());
        let calls = fs::read_to_string(calls).expect("calls");
        assert!(calls.contains("/DumpConfigToFiles"));
    }

    #[test]
    fn partial_validation_is_shared_with_ibcmd() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("ibcmd");
        create_source_tree(dir.path());
        write_script(&script, "exit 0");
        let config = build_config_with_builder(
            dir.path(),
            &dir.path().join("work"),
            &script,
            BuilderBackend::Ibcmd,
        );

        let failure = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Partial,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec!["Catalog\nItem".to_owned()],
            },
        )
        .expect_err("failure");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Validation);
        assert_eq!(failure.error.message(), PARTIAL_OBJECT_CONTROL_ERROR);
    }

    #[test]
    fn partial_dump_write_failure_cleans_up_temp_file() {
        let dir = tempdir().expect("tempdir");
        let work = dir.path().join("work");
        let partial_root = work.join("temp");
        fs::create_dir_all(&partial_root).expect("temp");
        fs::write(partial_root.join("partial-lists"), "not a dir").expect("sentinel");

        let error = create_dump_object_list_file_with(
            &work,
            &["Catalog.Items".to_owned()],
            |_file, _objects| Ok(()),
        )
        .expect_err("expected failure");

        assert!(matches!(error, AppError::Runtime(_)));
        assert!(partial_list_paths(&work).is_empty());
    }

    #[test]
    fn partial_dump_writer_failure_does_not_leave_temp_file() {
        let dir = tempdir().expect("tempdir");
        let work = dir.path().join("work");

        let error = create_dump_object_list_file_with(
            &work,
            &["Catalog.Items".to_owned()],
            |_file, _objects| Err(std::io::Error::other("boom")),
        )
        .expect_err("expected failure");

        assert!(matches!(error, AppError::Runtime(_)));
        assert!(partial_list_paths(&work).is_empty());
    }

    #[test]
    fn dump_partial_designer_creates_missing_target_dir_and_writes_normalized_list() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("1cv8");
        let calls = dir.path().join("calls.log");
        let captured_list = dir.path().join("captured-list.txt");
        create_source_tree(&base);
        write_script(
            &script,
            &format!(
                "args=\"$*\"\nout=\"\"\nlist=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  if [ \"$prev\" = \"-listFile\" ]; then list=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf 'designer log for %s\\n' \"$args\" > \"$out\"; fi\nif [ -n \"$list\" ]; then cp \"$list\" \"{}\"; fi\nprintf '%s\\n' \"$args\" >> \"{}\"\nexit 0",
                captured_list.display(),
                calls.display(),
            ),
        );
        let config = build_config(&base, &work, &script);
        fs::remove_dir_all(base.join("main")).expect("remove target");

        let result = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Partial,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec!["  Catalog.Items  ".to_owned(), "Document.Order".to_owned()],
            },
        )
        .expect("dump");

        assert!(result.ok);
        assert_eq!(result.mode, DumpMode::Partial);
        assert!(base.join("main").exists());
        let calls = fs::read_to_string(calls).expect("calls");
        assert!(calls.contains("/DumpConfigToFiles"));
        assert!(calls.contains("-partial"));
        assert!(calls.contains("-listFile"));
        assert_eq!(
            fs::read_to_string(captured_list).expect("captured list"),
            "Catalog.Items\nDocument.Order\n"
        );
        assert!(partial_list_paths(&work).is_empty());
    }

    #[test]
    fn dump_partial_designer_extension_uses_extension_flag() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("1cv8");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_dump_script(&script, &calls, None, 0);
        let config = build_config(&base, &work, &script);

        let result = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Partial,
                source_set: Some("ext".to_owned()),
                extension: Some("ext".to_owned()),
                objects: vec!["CommonModule.Module".to_owned()],
            },
        )
        .expect("dump");

        assert!(result.ok);
        let calls = fs::read_to_string(calls).expect("calls");
        assert!(calls.contains("-Extension"));
        assert!(calls.contains("ext"));
        assert!(partial_list_paths(&work).is_empty());
    }

    #[test]
    fn dump_partial_designer_failure_cleans_up_temp_file_and_keeps_partial_mode() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("1cv8");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_dump_script(&script, &calls, Some("-partial"), 0);
        let config = build_config(&base, &work, &script);

        let failure = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Partial,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec!["Catalog.Items".to_owned()],
            },
        )
        .expect_err("failure");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Platform);
        assert_eq!(failure.payload.expect("payload").mode, DumpMode::Partial);
        assert!(partial_list_paths(&work).is_empty());
    }

    #[test]
    fn dump_partial_ibcmd_uses_sync_and_returns_warning() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("ibcmd");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_ibcmd_dump_script(&script, &calls, None, 0);
        let config = build_config_with_builder(&base, &work, &script, BuilderBackend::Ibcmd);

        let result = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Partial,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec!["Catalog.Items".to_owned()],
            },
        )
        .expect("dump");

        assert!(result.ok);
        assert_eq!(result.mode, DumpMode::Partial);
        assert!(result
            .message
            .as_deref()
            .expect("warning")
            .contains("IBCMD does not support object-scoped partial dump"));
        let calls = fs::read_to_string(calls).expect("calls");
        assert!(calls.contains("--sync"));
    }

    #[test]
    fn dump_partial_ibcmd_extension_uses_extension_flag() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("ibcmd");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_ibcmd_dump_script(&script, &calls, None, 0);
        let config = build_config_with_builder(&base, &work, &script, BuilderBackend::Ibcmd);

        let result = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Partial,
                source_set: Some("ext".to_owned()),
                extension: Some("ext".to_owned()),
                objects: vec!["CommonModule.Module".to_owned()],
            },
        )
        .expect("dump");

        assert!(result.ok);
        assert_eq!(result.mode, DumpMode::Partial);
        let calls = fs::read_to_string(calls).expect("calls");
        assert!(calls.contains("--sync"));
        assert!(calls.contains("--extension ext"));
        assert!(result
            .message
            .as_deref()
            .expect("warning")
            .contains("extension 'ext'"));
    }

    #[test]
    fn dump_partial_ibcmd_failure_keeps_partial_mode_and_warning() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("ibcmd");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_ibcmd_dump_script(&script, &calls, Some("--sync"), 0);
        let config = build_config_with_builder(&base, &work, &script, BuilderBackend::Ibcmd);

        let failure = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Partial,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec!["Catalog.Items".to_owned()],
            },
        )
        .expect_err("failure");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Platform);
        assert!(failure
            .error
            .message()
            .contains("IBCMD does not support object-scoped partial dump"));
        let payload = failure.payload.expect("payload");
        assert_eq!(payload.mode, DumpMode::Partial);
        assert!(payload
            .message
            .as_deref()
            .expect("message")
            .contains("IBCMD does not support object-scoped partial dump"));
    }

    #[test]
    fn dump_full_preserves_old_dump_on_platform_failure() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("1cv8");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_dump_script(&script, &calls, Some("/DumpConfigToFiles"), 0);
        let config = build_config(&base, &work, &script);
        fs::write(base.join("main").join("old.txt"), "keep me").expect("old");

        let failure = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Full,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec![],
            },
        )
        .expect_err("failure");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Platform);
        assert_eq!(
            fs::read_to_string(base.join("main").join("old.txt")).expect("old"),
            "keep me"
        );
    }

    #[test]
    fn dump_full_success_replaces_old_target() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("1cv8");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_dump_script(&script, &calls, None, 0);
        let config = build_config(&base, &work, &script);
        fs::write(base.join("main").join("old.txt"), "old").expect("old");

        let result = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Full,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec![],
            },
        )
        .expect("dump");

        assert!(result.ok);
        assert!(!base.join("main").join("old.txt").exists());
    }

    #[test]
    fn ibcmd_dump_full_uses_staging_dir_and_atomic_publish() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("ibcmd");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_ibcmd_dump_script(&script, &calls, None, 0);
        let config = build_config_with_builder(&base, &work, &script, BuilderBackend::Ibcmd);
        fs::write(base.join("main").join("old.txt"), "old").expect("old");

        let result = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Full,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec![],
            },
        )
        .expect("dump");

        let calls = fs::read_to_string(calls).expect("calls");
        assert!(result.ok);
        assert!(calls.contains("--force"));
        assert!(calls.contains(".dump-stage-"));
        assert!(!base.join("main").join("old.txt").exists());
    }

    #[test]
    fn ibcmd_dump_with_server_infobase_passes_dbms_and_infobase_credentials() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("ibcmd");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_ibcmd_dump_script(&script, &calls, None, 0);
        let mut config = build_config_with_builder(&base, &work, &script, BuilderBackend::Ibcmd);
        config.infobase = crate::config::model::InfobaseConfig::server(
            "Srvr=cluster:1541;Ref=demo",
            crate::config::model::InfobaseDbmsConfig::new("PostgreSQL", "localhost", "demo")
                .with_credentials(Some("postgres".to_owned()), Some("pg-secret".to_owned())),
        )
        .with_credentials(Some("Admin".to_owned()), Some("secret".to_owned()));

        let result = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Full,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec![],
            },
        )
        .expect("dump");

        assert!(result.ok);
        let calls = fs::read_to_string(calls).expect("calls");
        assert!(calls.contains("--dbms PostgreSQL"));
        assert!(calls.contains("--database-server localhost"));
        assert!(calls.contains("--database-name demo"));
        assert!(calls.contains("--database-user postgres"));
        assert!(calls.contains("--database-password pg-secret"));
        assert!(calls.contains("--user Admin"));
        assert!(calls.contains("--password secret"));
    }

    #[test]
    fn ibcmd_dump_full_preserves_old_target_on_platform_failure() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("ibcmd");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_ibcmd_dump_script(&script, &calls, Some("--force"), 0);
        let config = build_config_with_builder(&base, &work, &script, BuilderBackend::Ibcmd);
        fs::write(base.join("main").join("old.txt"), "keep me").expect("old");

        let failure = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Full,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec![],
            },
        )
        .expect_err("failure");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Platform);
        assert_eq!(
            fs::read_to_string(base.join("main").join("old.txt")).expect("old"),
            "keep me"
        );
    }

    #[test]
    fn ibcmd_dump_incremental_uses_sync_against_resolved_target() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("ibcmd");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_ibcmd_dump_script(&script, &calls, None, 0);
        let config = build_config_with_builder(&base, &work, &script, BuilderBackend::Ibcmd);
        fs::remove_dir_all(base.join("main")).expect("remove target");

        let result = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Incremental,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec![],
            },
        )
        .expect("dump");

        assert!(result.ok);
        let calls = fs::read_to_string(calls).expect("calls");
        assert!(calls.contains("--sync"));
        assert!(calls.contains(base.join("main").display().to_string().as_str()));
    }

    #[test]
    fn dump_full_edt_designer_updates_designer_mirror_and_publishes_edt_target() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let designer = dir.path().join("1cv8");
        let edt = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer-calls.log");
        let edt_calls = dir.path().join("edt-calls.log");
        create_edt_source_tree(&base);
        write_designer_dump_script_for_edt(&designer, &designer_calls, None);
        write_edt_import_script(&edt, &edt_calls);
        let config = build_edt_config(&base, &work, &designer, &edt, BuilderBackend::Designer);
        fs::write(base.join("main").join("stale.txt"), "stale").expect("stale");

        let result = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Full,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec![],
            },
        )
        .expect("dump");

        assert!(result.ok);
        assert_eq!(result.target_path, base.join("main"));
        assert!(base.join("main").join(".project").exists());
        assert!(!base.join("main").join("stale.txt").exists());
        assert!(work
            .join("designer")
            .join("main")
            .join("Configuration.xml")
            .exists());

        let designer_calls = fs::read_to_string(designer_calls).expect("designer calls");
        let edt_calls = fs::read_to_string(edt_calls).expect("edt calls");
        assert!(designer_calls.contains(work.join("designer").display().to_string().as_str()));
        assert!(edt_calls.contains(work.join("designer/main").display().to_string().as_str()));
        assert!(edt_calls.contains(work.join("edt-workspace").display().to_string().as_str()));
    }

    #[test]
    fn dump_partial_edt_designer_bootstraps_missing_or_invalid_designer_snapshot() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let designer = dir.path().join("1cv8");
        let edt = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer-calls.log");
        let edt_calls = dir.path().join("edt-calls.log");
        create_edt_source_tree(&base);
        write_designer_dump_script_for_edt(&designer, &designer_calls, None);
        write_edt_import_script(&edt, &edt_calls);
        let config = build_edt_config(&base, &work, &designer, &edt, BuilderBackend::Designer);
        fs::create_dir_all(work.join("designer").join("main")).expect("empty designer snapshot");
        fs::write(
            work.join("designer").join("main").join("BrokenMirror.xml"),
            "<Broken />\n",
        )
        .expect("broken snapshot marker");

        let result = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Partial,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec!["Catalog.Items".to_owned()],
            },
        )
        .expect("dump");

        assert!(result.ok);
        assert!(base.join("main").join(".project").exists());
        assert!(work
            .join("designer")
            .join("main")
            .join("Configuration.xml")
            .exists());
        assert!(work
            .join("designer")
            .join("main")
            .join("PartialOnly.xml")
            .exists());

        let designer_calls = fs::read_to_string(designer_calls).expect("designer calls");
        let edt_calls = fs::read_to_string(edt_calls).expect("edt calls");
        assert_eq!(designer_calls.matches("/DumpConfigToFiles").count(), 2);
        assert!(designer_calls.contains("-partial"));
        assert_eq!(edt_calls.matches("-command import").count(), 1);
    }

    #[test]
    fn dump_full_edt_extension_infers_base_project_name_from_configuration_source_set() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let designer = dir.path().join("1cv8");
        let edt = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer-calls.log");
        let edt_calls = dir.path().join("edt-calls.log");
        create_edt_source_tree(&base);
        write_designer_dump_script_for_edt(&designer, &designer_calls, None);
        write_edt_import_script(&edt, &edt_calls);
        let config = build_edt_config(&base, &work, &designer, &edt, BuilderBackend::Designer);

        let result = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Full,
                source_set: Some("ext".to_owned()),
                extension: Some("ext".to_owned()),
                objects: vec![],
            },
        )
        .expect("dump");

        assert!(result.ok);
        assert!(base.join("ext").join(".project").exists());
        let designer_calls = fs::read_to_string(designer_calls).expect("designer calls");
        let edt_calls = fs::read_to_string(edt_calls).expect("edt calls");
        assert!(designer_calls.contains("-Extension"));
        assert!(designer_calls.contains("ext"));
        assert!(edt_calls.contains("--base-project-name BaseProject"));
    }

    #[test]
    fn dump_full_edt_ibcmd_exports_to_designer_mirror_before_edt_import() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let ibcmd = dir.path().join("ibcmd");
        let edt = dir.path().join("edt").join("1cedtcli");
        let ibcmd_calls = dir.path().join("ibcmd-calls.log");
        let edt_calls = dir.path().join("edt-calls.log");
        create_edt_source_tree(&base);
        write_ibcmd_dump_script_for_edt(&ibcmd, &ibcmd_calls, None);
        write_edt_import_script(&edt, &edt_calls);
        let config = build_edt_config(&base, &work, &ibcmd, &edt, BuilderBackend::Ibcmd);

        let result = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Full,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec![],
            },
        )
        .expect("dump");

        assert!(result.ok);
        assert!(base.join("main").join(".project").exists());
        assert!(work
            .join("designer")
            .join("main")
            .join("Configuration.xml")
            .exists());

        let ibcmd_calls = fs::read_to_string(ibcmd_calls).expect("ibcmd calls");
        let edt_calls = fs::read_to_string(edt_calls).expect("edt calls");
        assert!(ibcmd_calls.contains("--force"));
        assert!(ibcmd_calls.contains(work.join("designer").display().to_string().as_str()));
        assert!(edt_calls.contains(work.join("designer/main").display().to_string().as_str()));
    }

    #[test]
    fn dump_incremental_edt_designer_stops_after_bootstrap_when_interruption_becomes_pending() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let designer = dir.path().join("1cv8");
        let edt = dir.path().join("1cedtcli");
        create_edt_source_tree(&base);
        let config = build_edt_config(&base, &work, &designer, &edt, BuilderBackend::Designer);
        let resolved = resolve_target(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Incremental,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec![],
            },
        )
        .expect("resolved");
        let cancellation = CancellationToken::new();
        let dump_runner = TestProcessRunner::with_cancellation_on_call(1, cancellation.clone());
        let edt_runner = TestProcessRunner::default();
        let context = ExecutionContext::cli(crate::use_cases::context::CommandName::Dump)
            .with_cancellation(cancellation);

        let error = super::run_incremental_dump_edt_designer(
            &context,
            &config,
            &resolved,
            &designer,
            &edt,
            &dump_runner,
            &edt_runner,
        )
        .expect_err("interrupted after bootstrap");

        assert!(matches!(error, AppError::Runtime(_)));
        assert_eq!(dump_runner.call_count(), 1);
        assert_eq!(edt_runner.call_count(), 0);
    }

    #[test]
    fn dump_partial_edt_designer_stops_after_bootstrap_when_interruption_becomes_pending() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let designer = dir.path().join("1cv8");
        let edt = dir.path().join("1cedtcli");
        create_edt_source_tree(&base);
        let config = build_edt_config(&base, &work, &designer, &edt, BuilderBackend::Designer);
        let resolved = resolve_target(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Partial,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec!["Catalogs.Items".to_owned()],
            },
        )
        .expect("resolved");
        let cancellation = CancellationToken::new();
        let dump_runner = TestProcessRunner::with_cancellation_on_call(1, cancellation.clone());
        let edt_runner = TestProcessRunner::default();
        let context = ExecutionContext::cli(crate::use_cases::context::CommandName::Dump)
            .with_cancellation(cancellation);

        let error = super::run_partial_dump_edt_designer(
            &context,
            &config,
            &resolved,
            &designer,
            &edt,
            &dump_runner,
            &edt_runner,
            &["Catalogs.Items".to_owned()],
        )
        .expect_err("interrupted after bootstrap");

        assert!(matches!(error, AppError::Runtime(_)));
        assert_eq!(dump_runner.call_count(), 1);
        assert_eq!(edt_runner.call_count(), 0);
    }

    #[test]
    fn dump_incremental_edt_ibcmd_stops_after_bootstrap_when_interruption_becomes_pending() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let ibcmd = dir.path().join("ibcmd");
        let edt = dir.path().join("1cedtcli");
        create_edt_source_tree(&base);
        let config = build_edt_config(&base, &work, &ibcmd, &edt, BuilderBackend::Ibcmd);
        let resolved = resolve_target(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Incremental,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec![],
            },
        )
        .expect("resolved");
        let cancellation = CancellationToken::new();
        let dump_runner = TestProcessRunner::with_cancellation_on_call(1, cancellation.clone());
        let edt_runner = TestProcessRunner::default();
        let context = ExecutionContext::cli(crate::use_cases::context::CommandName::Dump)
            .with_cancellation(cancellation);

        let error = super::run_incremental_dump_edt_ibcmd(
            &context,
            &config,
            &resolved,
            &ibcmd,
            &edt,
            &dump_runner,
            &edt_runner,
        )
        .expect_err("interrupted after bootstrap");

        assert!(matches!(error, AppError::Runtime(_)));
        assert_eq!(dump_runner.call_count(), 1);
        assert_eq!(edt_runner.call_count(), 0);
    }

    #[test]
    fn dump_partial_edt_ibcmd_stops_after_bootstrap_when_interruption_becomes_pending() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let ibcmd = dir.path().join("ibcmd");
        let edt = dir.path().join("1cedtcli");
        create_edt_source_tree(&base);
        let config = build_edt_config(&base, &work, &ibcmd, &edt, BuilderBackend::Ibcmd);
        let resolved = resolve_target(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Partial,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec!["Catalogs.Items".to_owned()],
            },
        )
        .expect("resolved");
        let cancellation = CancellationToken::new();
        let dump_runner = TestProcessRunner::with_cancellation_on_call(1, cancellation.clone());
        let edt_runner = TestProcessRunner::default();
        let context = ExecutionContext::cli(crate::use_cases::context::CommandName::Dump)
            .with_cancellation(cancellation);

        let error = super::run_partial_dump_edt_ibcmd(
            &context,
            &config,
            &resolved,
            &ibcmd,
            &edt,
            &dump_runner,
            &edt_runner,
            &["Catalogs.Items".to_owned()],
        )
        .expect_err("interrupted after bootstrap");

        assert!(matches!(error, AppError::Runtime(_)));
        assert_eq!(dump_runner.call_count(), 1);
        assert_eq!(edt_runner.call_count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn finalize_edt_dump_revalidates_publish_target_after_import() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let designer = dir.path().join("1cv8");
        let edt = dir.path().join("1cedtcli");
        let drift = dir.path().join("drift-target");
        create_edt_source_tree(&base);
        fs::create_dir_all(&drift).expect("drift target");
        let config = build_edt_config(&base, &work, &designer, &edt, BuilderBackend::Designer);
        let resolved = resolve_target(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Full,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec![],
            },
        )
        .expect("resolved");
        let target_path = resolved.target_path.clone();
        let drift_path = drift.clone();
        let edt_runner = TestProcessRunner::with_on_run(move |request| {
            let project = request
                .args
                .windows(2)
                .find_map(|window| (window[0] == "--project").then(|| PathBuf::from(&window[1])))
                .expect("project arg");
            fs::create_dir_all(&project).expect("project dir");
            fs::write(
                project.join(".project"),
                "<projectDescription><name>ImportedProject</name></projectDescription>\n",
            )
            .expect("project file");
            fs::remove_dir_all(&target_path).expect("remove target");
            symlink(&drift_path, &target_path).expect("retarget");
        });
        let context = ExecutionContext::cli(crate::use_cases::context::CommandName::Dump);

        let error = finalize_edt_dump(
            &context,
            &config,
            &resolved,
            &edt,
            &edt_runner,
            PlatformCommandResult {
                process: ProcessResult {
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                    interruption: None,
                },
                platform_log_path: None,
                platform_log: None,
                platform_log_read_error: None,
            },
            None,
        )
        .expect_err("expected publish target re-validation failure");

        match error {
            AppError::Validation(message) => {
                assert!(message.contains("target path changed during dump resolution"));
            }
            other => panic!("expected validation error, got {other:?}"),
        }
        assert_eq!(edt_runner.call_count(), 1);
        assert!(fs::symlink_metadata(resolved.target_path.as_path())
            .expect("target metadata")
            .file_type()
            .is_symlink());
        assert!(!drift.join(".project").exists());
        let leftover_stage_dirs = fs::read_dir(&base)
            .expect("base dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".dump-stage-"))
            .count();
        assert_eq!(leftover_stage_dirs, 0);
    }

    #[test]
    fn finalize_edt_dump_cleans_staging_dir_when_edt_dsl_initialization_fails() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let designer = dir.path().join("1cv8");
        let edt = dir.path().join("edt").join("1cedtcli");
        create_edt_source_tree(&base);
        fs::create_dir_all(&work).expect("work");
        fs::write(work.join("edt-workspace"), "not a directory").expect("workspace file");
        let mut config = build_edt_config(&base, &work, &designer, &edt, BuilderBackend::Designer);
        config.tools.edt_cli.interactive_mode = true;
        let resolved = resolve_target(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Full,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec![],
            },
        )
        .expect("resolved");
        let context = ExecutionContext::cli(crate::use_cases::context::CommandName::Dump);

        let error = finalize_edt_dump(
            &context,
            &config,
            &resolved,
            &edt,
            &TestProcessRunner::default(),
            PlatformCommandResult {
                process: ProcessResult {
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                    interruption: None,
                },
                platform_log_path: None,
                platform_log: None,
                platform_log_read_error: None,
            },
            None,
        )
        .expect_err("expected EDT DSL initialization failure");

        assert!(matches!(error, AppError::Platform(_)));
        let leftover_stage_dirs = fs::read_dir(&base)
            .expect("base dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".dump-stage-"))
            .count();
        assert_eq!(leftover_stage_dirs, 0);
    }

    #[test]
    fn finalize_edt_dump_stops_before_import_when_interruption_is_already_pending() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let designer = dir.path().join("1cv8");
        let edt = dir.path().join("edt").join("1cedtcli");
        let edt_calls = dir.path().join("edt-calls.log");
        create_edt_source_tree(&base);
        write_edt_import_script(&edt, &edt_calls);
        write_script(&designer, "exit 0");
        let config = build_edt_config(&base, &work, &designer, &edt, BuilderBackend::Designer);
        fs::create_dir_all(work.join("designer").join("main")).expect("designer snapshot");
        fs::write(
            work.join("designer").join("main").join("Configuration.xml"),
            "<Configuration />\n",
        )
        .expect("configuration xml");
        let resolved = resolve_target(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Full,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec![],
            },
        )
        .expect("resolved");
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let context = ExecutionContext::cli(crate::use_cases::context::CommandName::Dump)
            .with_cancellation(cancellation);

        let error = finalize_edt_dump(
            &context,
            &config,
            &resolved,
            &edt,
            &crate::platform::process::ProcessExecutor,
            PlatformCommandResult {
                process: ProcessResult {
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                    interruption: None,
                },
                platform_log_path: None,
                platform_log: None,
                platform_log_read_error: None,
            },
            None,
        )
        .expect_err("interrupted");

        assert!(matches!(error, AppError::Runtime(_)));
        assert!(
            !edt_calls.exists()
                || fs::read_to_string(edt_calls)
                    .expect("edt calls")
                    .trim()
                    .is_empty()
        );
        assert!(!base.join("main").join("stale.txt").exists());
    }

    #[test]
    fn advisory_lock_serializes_access() {
        let dir = tempdir().expect("tempdir");
        let lock_path = dir.path().join("test.lock");
        let guard = acquire_advisory_lock(&lock_path).expect("lock");
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let lock_path_clone = lock_path.clone();

        let handle = thread::spawn(move || {
            started_tx.send(()).expect("send started");
            let _guard = acquire_advisory_lock(&lock_path_clone).expect("second lock");
            done_tx.send(()).expect("send done");
        });

        started_rx.recv().expect("started");
        assert!(done_rx.recv_timeout(Duration::from_millis(100)).is_err());
        drop(guard);
        done_rx.recv_timeout(Duration::from_secs(1)).expect("done");
        handle.join().expect("join");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_target_uses_same_lock_path_for_canonical_and_symlinked_base_path() {
        let dir = tempdir().expect("tempdir");
        let real_base = dir.path().join("real-base");
        let base_link = dir.path().join("base-link");
        let work = dir.path().join("work");
        let script = dir.path().join("1cv8");
        create_source_tree(&real_base);
        fs::create_dir_all(&work).expect("work");
        write_script(&script, "exit 0");
        symlink(&real_base, &base_link).expect("symlink");

        let config_real = build_config(&real_base, &work, &script);
        let config_link = build_config(&base_link, &work, &script);

        let resolved_real = resolve_target(
            &config_real,
            &DumpArgs {
                mode: DumpModeRequest::Full,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec![],
            },
        )
        .expect("resolved real");
        let resolved_link = resolve_target(
            &config_link,
            &DumpArgs {
                mode: DumpModeRequest::Full,
                source_set: Some("main".to_owned()),
                extension: None,
                objects: vec![],
            },
        )
        .expect("resolved link");

        assert_eq!(resolved_real.lock_path, resolved_link.lock_path);
    }

    #[cfg(unix)]
    #[test]
    fn lock_identity_is_based_on_canonical_target() {
        let dir = tempdir().expect("tempdir");
        let real = dir.path().join("real");
        let link = dir.path().join("link");
        fs::create_dir_all(&real).expect("real");
        symlink(&real, &link).expect("symlink");

        let hash_real = stable_path_identity(&std::fs::canonicalize(&real).expect("canonical"));
        let hash_link = stable_path_identity(&std::fs::canonicalize(&link).expect("canonical"));

        assert_eq!(hash_real, hash_link);
    }

    #[test]
    fn dump_result_json_contains_new_fields() {
        let result = crate::domain::dump::DumpResult {
            ok: true,
            source_set: Some("main".to_owned()),
            extension: Some("ext".to_owned()),
            mode: DumpMode::Incremental,
            target_path: PathBuf::from("/tmp/main"),
            platform_log_path: Some(PathBuf::from("/tmp/platform.log")),
            duration_ms: 5,
            message: Some("ok".to_owned()),
        };

        let envelope = Envelope::ok(DUMP_COMMAND, result.duration_ms, result);
        let json = serde_json::to_value(envelope).expect("json");

        assert_eq!(json["data"]["source_set"], "main");
        assert_eq!(json["data"]["extension"], "ext");
        assert_eq!(json["data"]["platform_log_path"], "/tmp/platform.log");
    }

    #[test]
    fn build_designer_dsl_requests_platform_log() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        let calls = dir.path().join("calls.log");
        create_source_tree(dir.path());
        write_dump_script(&script, &calls, None, 0);
        let config = build_config(dir.path(), &dir.path().join("work"), &script);
        let runner = crate::platform::process::ProcessExecutor;
        let context = crate::use_cases::context::ExecutionContext::cli(
            crate::use_cases::context::CommandName::Dump,
        );
        let dsl = build_designer_dsl(&context, &config, &script, &runner, "main", "incremental")
            .expect("dsl");

        let result = dsl
            .dump_config_to_files(dir.path().join("out").as_path(), None)
            .expect("dump");

        assert!(result.platform_log_path.is_some());
        assert!(result
            .platform_log
            .as_deref()
            .unwrap_or_default()
            .contains("designer log"));
    }

    #[test]
    fn parse_external_dump_descriptor_decodes_escaped_name() {
        let xml = "<ExternalDataProcessor><Properties><Name>Foo &amp; Bar</Name></Properties></ExternalDataProcessor>";
        let (root, logical_name) =
            parse_external_dump_descriptor(xml, Path::new("/tmp/dump.xml")).expect("parse");

        assert_eq!(root, "ExternalDataProcessor");
        assert_eq!(logical_name, "Foo & Bar");
    }

    #[test]
    fn parse_external_dump_descriptor_accepts_metadataobject_wrapper() {
        let xml = "<MetaDataObject><ExternalDataProcessor><Properties><Name>Foo</Name></Properties></ExternalDataProcessor></MetaDataObject>";
        let (root, logical_name) =
            parse_external_dump_descriptor(xml, Path::new("/tmp/dump.xml")).expect("parse");

        assert_eq!(root, "ExternalDataProcessor");
        assert_eq!(logical_name, "Foo");
    }

    #[test]
    fn run_external_dump_designer_rejects_missing_descriptor() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        let calls = dir.path().join("calls.log");
        let work = dir.path().join("work");
        let root_xml = dir.path().join("out").join("root.xml");
        create_source_tree(dir.path());
        write_dump_script(&script, &calls, None, 0);
        let config = build_config(dir.path(), &work, &script);
        let runner = crate::platform::process::ProcessExecutor;
        let context = crate::use_cases::context::ExecutionContext::cli(
            crate::use_cases::context::CommandName::Dump,
        );
        let dsl = build_designer_dsl(&context, &config, &script, &runner, "main", "incremental")
            .expect("dsl");

        let error = run_external_dump_designer(
            &dsl,
            &script,
            &root_xml,
            ExternalArtifactKind::DataProcessor,
            "Foo",
        )
        .expect_err("missing descriptor");

        assert!(matches!(error.0, AppError::Validation(_)));
    }
}
