use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::model::{AppConfig, BuilderBackend, SourceFormat, SourceSetPurpose};
use crate::domain::dump::{DumpMode, DumpResult};
use crate::platform::designer::DesignerDsl;
use crate::platform::edt::EdtDsl;
use crate::platform::edt_session::{EdtSessionHostOptions, EdtSessionManager};
use crate::platform::locator::UtilityType;
use crate::platform::process::ProcessRunner;
use crate::platform::result::PlatformCommandResult;
use crate::platform::utilities::PlatformUtilities;
use crate::support::edt_project::{self, EdtProjectKind};
use crate::support::error::AppError;
use crate::support::fs::{acquire_advisory_lock, ensure_dir, remove_path_if_exists};
use crate::support::path::{
    hashed_lock_path, nearest_existing_canonical_path, stable_path_identity,
};
use crate::support::source_descriptor::{self, ExternalDescriptorParseError};
use crate::use_cases::context::{ExecutionContext, InterruptionSafetyClass};
use crate::use_cases::external_artifacts::ExternalArtifactKind;
use crate::use_cases::interruption;
use crate::use_cases::progress::log_live_stage;
use crate::use_cases::request::{DumpModeRequest, DumpRequest as DumpArgs};
use crate::use_cases::result::{UseCaseFailure, UseCaseResult};
use tracing::debug;

mod coordinator;
mod helpers;

#[cfg(test)]
use self::helpers::create_dump_object_list_file_with;
use self::helpers::{
    build_designer_dsl, build_ibcmd_dsl, cleanup_orphan_dirs, cleanup_platform_orphan_dirs,
    create_dump_object_list_file, decorate_ibcmd_partial_error, dump_publication_warning,
    empty_result, ensure_platform_success, ibcmd_partial_warning, map_ibcmd_error,
    merge_optional_messages, resolve_dump_edt_base_project_name, validate_dump_objects,
    validate_platform_target, validate_publish_target, validate_supported_matrix,
};
#[cfg(test)]
use super::staged_publication::cleanup_staging_path;
use super::staged_publication::{interruption_before_publish, StagedPublication};
#[cfg(test)]
use crate::support::fs::metadata_sidecar_path;
use crate::use_cases::source_inventory::SourceSetInventory;

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
    source_set_purpose: SourceSetPurpose,
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
    coordinator::run_dump_with_context(context, config, args)
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

    log_live_stage(
        "dump: incremental",
        "[Конфигуратор] exporting configuration files",
    );
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
    .map_err(AppError::from)?;
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
    let publication = StagedPublication::prepare_dir(
        &resolved.platform_target_path,
        &resolved.platform_target_identity,
        ".dump-stage",
    )?;
    let staging_dir = publication.staging_path().to_path_buf();
    debug!(path = %staging_dir.display(), "created dump staging directory");

    log_live_stage("dump: full", "[Конфигуратор] exporting configuration files");
    let dump_result = match build_designer_dsl(
        context,
        config,
        binary,
        runner,
        &resolved.source_set_name,
        "full",
    )?
    .dump_config_to_files(&staging_dir, resolved.extension.as_deref())
    .map_err(AppError::from)
    {
        Ok(result) => result,
        Err(error) => return Err(publication.cleanup_failure(error)),
    };
    ensure_platform_success("dump", resolved, &dump_result)
        .map_err(|error| publication.cleanup_failure(error))?;

    validate_platform_target(resolved).map_err(|error| publication.cleanup_failure(error))?;
    if let Some(error) = interruption_before_publish(context, "dump publication") {
        return Err(publication.cleanup_failure(error));
    }

    let publish_phase =
        publication.publish_dir(context, DUMP_BACKUP_PREFIX, "failed to publish staged dump")?;
    debug!(target = %resolved.platform_target_path.display(), "published staged dump");

    Ok((
        dump_result,
        merge_optional_messages(
            publish_phase.cleanup_warning,
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

    log_live_stage("dump: incremental", "[ibcmd] exporting configuration files");
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
    let publication = StagedPublication::prepare_dir(
        &resolved.platform_target_path,
        &resolved.platform_target_identity,
        ".dump-stage",
    )?;
    let staging_dir = publication.staging_path().to_path_buf();
    debug!(path = %staging_dir.display(), "created dump staging directory");

    log_live_stage("dump: full", "[ibcmd] exporting configuration files");
    let dump_result = match build_ibcmd_dsl(context, config, binary, runner)?
        .config_export_full(&staging_dir, resolved.extension.as_deref())
        .map_err(map_ibcmd_error)
    {
        Ok(result) => result,
        Err(error) => return Err(publication.cleanup_failure(error)),
    };
    ensure_platform_success("dump", resolved, &dump_result)
        .map_err(|error| publication.cleanup_failure(error))?;

    validate_platform_target(resolved).map_err(|error| publication.cleanup_failure(error))?;
    if let Some(error) = interruption_before_publish(context, "dump publication") {
        return Err(publication.cleanup_failure(error));
    }

    let publish_phase =
        publication.publish_dir(context, DUMP_BACKUP_PREFIX, "failed to publish staged dump")?;
    debug!(target = %resolved.platform_target_path.display(), "published staged dump");

    Ok((
        dump_result,
        merge_optional_messages(
            publish_phase.cleanup_warning,
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
    log_live_stage(
        "dump: partial",
        "[Конфигуратор] exporting selected configuration objects",
    );
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
    .map_err(AppError::from)?;
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
    interruption::pending_interruption_error(context, phase).map_or(Ok(()), Err)
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
    let publication = StagedPublication::prepare_dir(
        &resolved.target_path,
        &resolved.target_identity,
        ".dump-stage",
    )?;
    let staging_dir = publication.staging_path().to_path_buf();

    let edt_dsl = build_edt_dsl(context, config, edt_binary, edt_runner)
        .map_err(|error| publication.cleanup_failure(error))?;
    log_live_stage("dump: edt import", "[EDT] importing Designer snapshot");
    let import_result = match edt_dsl
        .import_configuration_files(
            &staging_dir,
            &resolved.platform_target_path,
            normalize_config_hint(config.tools.platform.version.as_deref()),
            resolved.edt_base_project_name.as_deref(),
            false,
        )
        .map_err(AppError::from)
    {
        Ok(result) => result,
        Err(error) => return Err(publication.cleanup_failure(error)),
    };
    ensure_import_success(resolved, &import_result)
        .map_err(|error| publication.cleanup_failure(error))?;
    validate_edt_dump_staging_output(
        &staging_dir,
        resolved.source_set_purpose,
        resolved.edt_base_project_name.as_deref(),
    )
    .map_err(|error| publication.cleanup_failure(error))?;
    validate_publish_target(resolved).map_err(|error| publication.cleanup_failure(error))?;

    if let Some(error) = interruption_before_publish(context, "dump publication") {
        return Err(publication.cleanup_failure(error));
    }

    let publish_phase =
        publication.publish_dir(context, DUMP_BACKUP_PREFIX, "failed to publish staged dump")?;

    Ok((
        platform_result,
        merge_optional_messages(
            inherited_message,
            merge_optional_messages(
                publish_phase.cleanup_warning,
                dump_publication_warning(context.command(), publish_phase.deferred_interruption),
            ),
        ),
    ))
}

fn validate_edt_dump_staging_output(
    staging_dir: &Path,
    expected_purpose: SourceSetPurpose,
    expected_base_project: Option<&str>,
) -> Result<(), AppError> {
    let expected_kind = match expected_purpose {
        SourceSetPurpose::Configuration => EdtProjectKind::Configuration,
        SourceSetPurpose::Extension => EdtProjectKind::Extension,
        _ => {
            return Err(AppError::Validation(format!(
                "EDT dump output validation supports only ordinary source-sets: {}",
                staging_dir.display()
            )));
        }
    };
    edt_project::validate_native_ordinary_project(staging_dir, expected_kind, expected_base_project)
        .map(|_| ())
        .map_err(|error| {
            AppError::Validation(format!(
                "EDT dump output is not a valid native EDT project: {error}"
            ))
        })
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
                .map_err(AppError::from)?;
        EdtDsl::new_shared_session(
            binary.to_path_buf(),
            workspace,
            Arc::new(manager),
            Duration::from_millis(config.tools.edt_cli.startup_timeout_ms),
            Duration::from_millis(config.tools.edt_cli.command_timeout_ms),
        )
        .map_err(AppError::from)
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
        .map_err(|error| (AppError::from(error), None))?;
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
    let parsed = parse_external_dump_descriptor(&contents, root_xml_path)
        .map_err(|error| (error, result.platform_log_path.clone()))?;
    if parsed.purpose.external_root_tag() != Some(expected_kind.root_tag()) {
        return Err((
            AppError::Validation(format!(
                "external dump '{}' has unexpected root element",
                root_xml_path.display()
            )),
            result.platform_log_path.clone(),
        ));
    }
    if parsed.logical_name != expected_logical_name {
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
) -> Result<source_descriptor::ParsedExternalDescriptor, AppError> {
    source_descriptor::parse_external_descriptor(contents).map_err(|error| match error {
        ExternalDescriptorParseError::Xml(error) => AppError::Validation(format!(
            "failed to parse external dump xml '{}': {error}",
            path.display()
        )),
        ExternalDescriptorParseError::DecodeLogicalName(error) => AppError::Validation(format!(
            "failed to decode external dump logical name in '{}': {error}",
            path.display()
        )),
        ExternalDescriptorParseError::MissingRootElement => {
            AppError::Validation(format!("missing root XML element in '{}'", path.display()))
        }
        ExternalDescriptorParseError::UnsupportedRootElement(root) => {
            AppError::Validation(format!(
                "unsupported root XML element '{root}' in '{}'",
                path.display()
            ))
        }
        ExternalDescriptorParseError::MissingLogicalName => AppError::Validation(format!(
            "external dump '{}' must contain Properties/Name",
            path.display()
        )),
    })
}

#[cfg(test)]
fn cleanup_staging_on_platform_failure(staging_dir: &Path, error: AppError) -> AppError {
    cleanup_staging_path(staging_dir, error)
}

#[cfg(test)]
fn cleanup_staging_on_interruption(staging_dir: &Path, error: AppError) -> AppError {
    cleanup_staging_on_platform_failure(staging_dir, error)
}

fn resolve_target(config: &AppConfig, args: &DumpArgs) -> Result<ResolvedDumpTarget, AppError> {
    let inventory = SourceSetInventory::new(config);

    let (source_set, extension) = match (args.source_set.as_deref(), args.extension.as_deref()) {
        (Some(source_set_name), None) => {
            let source_set = inventory.source_set(source_set_name).ok_or_else(|| {
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
            let source_set = inventory.source_set(extension_name).ok_or_else(|| {
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
            let source_set = inventory.source_set(source_set_name).ok_or_else(|| {
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
            let configuration_source_sets =
                inventory.source_sets_with_purpose(SourceSetPurpose::Configuration);
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

    let target_path = inventory.source_path(source_set);
    let platform_target_path = if config.format == SourceFormat::Edt {
        inventory
            .designer_context(&source_set.name)
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
    let canonical_base_path =
        nearest_existing_canonical_path(&config.base_path).map_err(|error| {
            AppError::Runtime(format!("failed to canonicalize project base path: {error}"))
        })?;
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
        Some(resolve_dump_edt_base_project_name(&inventory)?)
    } else {
        None
    };
    let lock_path = hashed_lock_path(&canonical_target_path, "dump")
        .map_err(|error| AppError::Runtime(format!("failed to resolve dump lock path: {error}")))?;

    Ok(ResolvedDumpTarget {
        source_set_name: source_set.name.clone(),
        source_set_purpose: source_set.purpose,
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
            "args=\"$*\"\nout=\"\"\ntarget=\"\"\nextension_name=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  if [ \"$prev\" = \"/DumpConfigToFiles\" ]; then target=\"$arg\"; fi\n  if [ \"$prev\" = \"-Extension\" ]; then extension_name=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf 'designer log for %s\\n' \"$args\" > \"$out\"; fi\nprintf '%s\\n' \"$args\" >> \"{}\"\n{}\nmkdir -p \"$target\"\nif [ -n \"$extension_name\" ]; then\n  config_xml='<Configuration><Properties><Name>ExtensionProject</Name></Properties><ConfigurationExtensionPurpose>Extension</ConfigurationExtensionPurpose></Configuration>'\nelse\n  config_xml='<Configuration><Properties><Name>BaseProject</Name></Properties></Configuration>'\nfi\nprintf '%s\\n' \"$config_xml\" > \"$target/Configuration.xml\"\nif printf '%s' \"$args\" | grep -F -q -- '-partial'; then\n  printf '<Partial />\\n' > \"$target/PartialOnly.xml\"\nfi\nexit 0",
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
            "args=\"$*\"\ntarget=\"$(printf '%s' \"$args\" | awk '{{print $NF}}')\"\nextension_name=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"-Extension\" ]; then extension_name=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nprintf '%s\\n' \"$args\" >> \"{}\"\n{}\nmkdir -p \"$target\"\nif [ -n \"$extension_name\" ]; then\n  printf '<Configuration><Properties><Name>ExtensionProject</Name></Properties><ConfigurationExtensionPurpose>Extension</ConfigurationExtensionPurpose></Configuration>\\n' > \"$target/Configuration.xml\"\nelse\n  printf '<Configuration><Properties><Name>BaseProject</Name></Properties></Configuration>\\n' > \"$target/Configuration.xml\"\nfi\nexit 0",
            calls_log.display(),
            pattern_branch
        );
        write_script(path, &body);
    }

    fn write_edt_import_script(path: &Path, calls_log: &Path) {
        let body = format!(
            r#"args="$*"
printf '%s\n' "$args" >> "{}"
project=""
config_files=""
base_project_name=""
prev=""
read_configuration_name() {{
  config_file="$1/Configuration.xml"
  if [ ! -f "$config_file" ]; then
    printf 'ImportedProject'
    return
  fi
  name=$(sed -n 's:.*<Name>\([^<][^<]*\)</Name>.*:\1:p' "$config_file" | head -n 1)
  if [ -n "$name" ]; then
    printf '%s' "$name"
  else
    printf 'ImportedProject'
  fi
}}
configuration_is_extension() {{
  config_file="$1/Configuration.xml"
  [ -f "$config_file" ] && grep -q 'ConfigurationExtensionPurpose\|ObjectBelonging' "$config_file"
}}
for arg in "$@"; do
  if [ "$prev" = "--project" ]; then project="$arg"; fi
  if [ "$prev" = "--configuration-files" ]; then config_files="$arg"; fi
  if [ "$prev" = "--base-project-name" ]; then base_project_name="$arg"; fi
  prev="$arg"
done
imported_name=$(read_configuration_name "$config_files")
mkdir -p "$project/DT-INF" "$project/src/Configuration"
if configuration_is_extension "$config_files"; then
  if [ "$base_project_name" != "BaseProject" ]; then
    printf 'unexpected base project: %s\n' "$base_project_name" >&2
    exit 23
  fi
  imported_nature="{}"
  imported_base="BaseProject"
else
  imported_nature="{}"
  imported_base=""
fi
cat > "$project/.project" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<projectDescription>
  <name>$imported_name</name>
  <natures>
    <nature>$imported_nature</nature>
  </natures>
</projectDescription>
EOF
{{
  if [ -n "$imported_base" ]; then printf 'Base-Project: %s\n' "$imported_base"; fi
  printf 'Manifest-Version: 1.0\nRuntime-Version: 8.3.27\n'
}} > "$project/DT-INF/PROJECT.PMF"
printf '<Configuration />\n' > "$project/src/Configuration/Configuration.mdo"
printf 'Procedure Test()\nEndProcedure\n' > "$project/src/Configuration/Module.bsl"
exit 0"#,
            calls_log.display(),
            crate::support::edt_project::V8_EXTENSION_NATURE,
            crate::support::edt_project::V8_CONFIGURATION_NATURE
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

    fn write_native_edt_project(path: &Path, project_name: &str, nature: &str, base: Option<&str>) {
        fs::create_dir_all(path.join("DT-INF")).expect("dt-inf");
        fs::create_dir_all(path.join("src").join("Configuration")).expect("src");
        let base_line = base
            .map(|value| format!("Base-Project: {value}\n"))
            .unwrap_or_default();
        fs::write(
            path.join(".project"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<projectDescription>\n  <name>{project_name}</name>\n  <natures>\n    <nature>{nature}</nature>\n  </natures>\n</projectDescription>\n"
            ),
        )
        .expect("project");
        fs::write(
            path.join("DT-INF").join("PROJECT.PMF"),
            format!("{base_line}Manifest-Version: 1.0\nRuntime-Version: 8.3.27\n"),
        )
        .expect("manifest");
        fs::write(
            path.join("src")
                .join("Configuration")
                .join("Configuration.mdo"),
            "<Configuration />\n",
        )
        .expect("configuration marker");
        fs::write(
            path.join("src").join("Configuration").join("Module.bsl"),
            "Procedure Test()\nEndProcedure\n",
        )
        .expect("module marker");
    }

    fn create_edt_source_tree(base_path: &Path) {
        write_native_edt_project(
            &base_path.join("main"),
            "BaseProject",
            crate::support::edt_project::V8_CONFIGURATION_NATURE,
            None,
        );
        write_native_edt_project(
            &base_path.join("ext"),
            "ExtensionProject",
            crate::support::edt_project::V8_EXTENSION_NATURE,
            Some("BaseProject"),
        );
    }

    fn assert_native_edt_project(path: &Path) {
        assert!(path.join(".project").exists());
        assert!(path.join("DT-INF").join("PROJECT.PMF").exists());
        assert!(path.join("src/Configuration/Configuration.mdo").exists());
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
            source_set_purpose: SourceSetPurpose::Configuration,
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
            source_set_purpose: SourceSetPurpose::Configuration,
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
            source_set_purpose: SourceSetPurpose::Configuration,
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
            source_set_purpose: SourceSetPurpose::Configuration,
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
            source_set_purpose: SourceSetPurpose::Configuration,
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
        assert_native_edt_project(&base.join("main"));
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
        assert_native_edt_project(&base.join("main"));
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
        assert_native_edt_project(&base.join("ext"));
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
            write_native_edt_project(
                &project,
                "ImportedProject",
                crate::support::edt_project::V8_CONFIGURATION_NATURE,
                None,
            );
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
    fn validate_edt_dump_staging_output_rejects_wrong_ordinary_kind() {
        let dir = tempdir().expect("tempdir");
        write_native_edt_project(
            dir.path(),
            "ImportedProject",
            crate::support::edt_project::V8_CONFIGURATION_NATURE,
            None,
        );

        let error =
            super::validate_edt_dump_staging_output(dir.path(), SourceSetPurpose::Extension, None)
                .expect_err("expected wrong ordinary kind");

        match error {
            AppError::Validation(message) => {
                assert!(message.contains("expected Extension"));
            }
            other => panic!("expected validation error, got {other:?}"),
        }
    }

    #[test]
    fn validate_edt_dump_staging_output_rejects_extension_without_base_project() {
        let dir = tempdir().expect("tempdir");
        write_native_edt_project(
            dir.path(),
            "ImportedExtension",
            crate::support::edt_project::V8_EXTENSION_NATURE,
            None,
        );

        let error = super::validate_edt_dump_staging_output(
            dir.path(),
            SourceSetPurpose::Extension,
            Some("BaseProject"),
        )
        .expect_err("expected missing Base-Project validation error");

        match error {
            AppError::Validation(message) => {
                assert!(message.contains("Base-Project"));
            }
            other => panic!("expected validation error, got {other:?}"),
        }
    }

    #[test]
    fn validate_edt_dump_staging_output_rejects_unexpected_extension_base_project() {
        let dir = tempdir().expect("tempdir");
        write_native_edt_project(
            dir.path(),
            "ImportedExtension",
            crate::support::edt_project::V8_EXTENSION_NATURE,
            Some("WrongBase"),
        );

        let error = super::validate_edt_dump_staging_output(
            dir.path(),
            SourceSetPurpose::Extension,
            Some("BaseProject"),
        )
        .expect_err("expected mismatched Base-Project validation error");

        match error {
            AppError::Validation(message) => {
                assert!(message.contains("BaseProject"));
                assert!(message.contains("WrongBase"));
            }
            other => panic!("expected validation error, got {other:?}"),
        }
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

        assert!(matches!(error, AppError::PlatformEdt(_)));
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

        let json = serde_json::to_value(result).expect("json");

        assert_eq!(DUMP_COMMAND, "dump");
        assert_eq!(json["source_set"], "main");
        assert_eq!(json["extension"], "ext");
        assert_eq!(json["platform_log_path"], "/tmp/platform.log");
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
        let parsed =
            parse_external_dump_descriptor(xml, Path::new("/tmp/dump.xml")).expect("parse");

        assert_eq!(
            parsed.purpose.external_root_tag(),
            Some("ExternalDataProcessor")
        );
        assert_eq!(parsed.logical_name, "Foo & Bar");
    }

    #[test]
    fn parse_external_dump_descriptor_accepts_metadataobject_wrapper() {
        let xml = "<MetaDataObject><ExternalDataProcessor><Properties><Name>Foo</Name></Properties></ExternalDataProcessor></MetaDataObject>";
        let parsed =
            parse_external_dump_descriptor(xml, Path::new("/tmp/dump.xml")).expect("parse");

        assert_eq!(
            parsed.purpose.external_root_tag(),
            Some("ExternalDataProcessor")
        );
        assert_eq!(parsed.logical_name, "Foo");
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
