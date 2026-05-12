use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use tempfile::NamedTempFile;

use crate::config::model::{AppConfig, BuilderBackend, SourceSetPurpose};
use crate::domain::dump::{DumpMode, DumpResult};
use crate::platform::designer::DesignerDsl;
use crate::platform::ibcmd::{IbcmdConnection, IbcmdDsl, IbcmdError};
use crate::platform::process::ProcessRunner;
use crate::platform::result::PlatformCommandResult;
use crate::support::edt_project;
use crate::support::error::AppError;
use crate::support::fs::{
    is_known_tool_name, metadata_sidecar_path, read_temp_dir_metadata, remove_path_if_exists,
};
use crate::support::path::{is_filesystem_root, nearest_existing_canonical_path};
use crate::support::temp::{dump_object_list_file, platform_logs_dir};
use crate::use_cases::context::{ExecutionContext, InterruptionSafetyClass};
use crate::use_cases::ibcmd_diagnostics::format_ibcmd_failure_details;
use crate::use_cases::interruption;
use crate::use_cases::source_inventory::SourceSetInventory;

use super::ResolvedDumpTarget;

pub(super) fn validate_supported_matrix(config: &AppConfig) -> Option<AppError> {
    if matches!(
        config.builder,
        BuilderBackend::Designer | BuilderBackend::Ibcmd
    ) {
        None
    } else {
        Some(AppError::Validation(super::SUPPORTED_DUMP_ERROR.to_owned()))
    }
}

pub(super) fn validate_publish_target(resolved: &ResolvedDumpTarget) -> Result<(), AppError> {
    validate_publish_target_path(
        &resolved.target_path,
        &resolved.canonical_target_path,
        &resolved.canonical_base_path,
        &resolved.canonical_work_path,
    )
}

pub(super) fn cleanup_orphan_dirs(resolved: &ResolvedDumpTarget) -> Result<(), AppError> {
    cleanup_orphan_dirs_for(&resolved.target_path, &resolved.target_identity)
}

pub(super) fn validate_platform_target(resolved: &ResolvedDumpTarget) -> Result<(), AppError> {
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
            "dump target must not equal project base path".to_owned(),
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

pub(super) fn cleanup_platform_orphan_dirs(resolved: &ResolvedDumpTarget) -> Result<(), AppError> {
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
            < super::ORPHAN_TTL
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

pub(super) fn resolve_dump_edt_base_project_name(
    inventory: &SourceSetInventory<'_>,
) -> Result<String, AppError> {
    let configuration_source_sets =
        inventory.source_sets_with_purpose(SourceSetPurpose::Configuration);
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
        &inventory.source_path(configuration_source_sets[0]),
        &format!(
            "configuration source-set '{}'",
            configuration_source_sets[0].name
        ),
    )
}

fn read_edt_project_name(path: &Path, label: &str) -> Result<String, AppError> {
    edt_project::read_project_name_from_dir(path).map_err(|error| {
        AppError::Validation(format!(
            "{label} must contain a valid EDT project name in '.project': {error}"
        ))
    })
}

pub(super) fn build_designer_dsl<'a>(
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

pub(super) fn build_ibcmd_dsl<'a>(
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

pub(super) fn map_ibcmd_error(error: IbcmdError) -> AppError {
    AppError::from(error)
}

pub(super) fn merge_optional_messages(
    left: Option<String>,
    right: Option<String>,
) -> Option<String> {
    match (left, right) {
        (Some(left), Some(right)) => Some(format!("{left}; {right}")),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

pub(super) fn dump_publication_warning(
    command: crate::use_cases::context::CommandName,
    deferred_interruption: Option<crate::use_cases::context::ExecutionInterruption>,
) -> Option<String> {
    deferred_interruption.map(|interruption| {
        interruption::deferred_interruption_warning_for_command(
            "dump publication completed",
            command,
            interruption,
        )
    })
}

pub(super) fn validate_dump_objects(
    mode: &DumpMode,
    objects: &[String],
) -> Result<Option<Vec<String>>, AppError> {
    match mode {
        DumpMode::Partial => normalize_partial_objects(objects).map(Some),
        _ if !objects.is_empty() => Err(AppError::Validation(
            super::NON_PARTIAL_OBJECTS_ERROR.to_owned(),
        )),
        _ => Ok(None),
    }
}

fn normalize_partial_objects(objects: &[String]) -> Result<Vec<String>, AppError> {
    if objects.is_empty() {
        return Err(AppError::Validation(
            super::PARTIAL_OBJECTS_REQUIRED_ERROR.to_owned(),
        ));
    }

    let mut normalized = Vec::with_capacity(objects.len());
    for object in objects {
        if object.chars().any(char::is_control) {
            return Err(AppError::Validation(
                super::PARTIAL_OBJECT_CONTROL_ERROR.to_owned(),
            ));
        }
        let object = object.trim();
        if object.is_empty() {
            return Err(AppError::Validation(
                super::PARTIAL_OBJECT_BLANK_ERROR.to_owned(),
            ));
        }
        normalized.push(object.to_owned());
    }

    if normalized.is_empty() {
        return Err(AppError::Validation(
            super::PARTIAL_OBJECTS_REQUIRED_ERROR.to_owned(),
        ));
    }

    Ok(normalized)
}

pub(super) fn create_dump_object_list_file(
    work_path: &Path,
    objects: &[String],
) -> Result<NamedTempFile, AppError> {
    create_dump_object_list_file_with(work_path, objects, write_partial_object_list)
}

pub(super) fn create_dump_object_list_file_with<F>(
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

pub(super) fn ibcmd_partial_warning(resolved: &ResolvedDumpTarget) -> String {
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

pub(super) fn decorate_ibcmd_partial_error(error: AppError, warning: &str) -> AppError {
    error.with_context(warning)
}

pub(super) fn ensure_platform_success(
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

pub(super) fn empty_result(
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

#[cfg(test)]
mod tests {
    use super::decorate_ibcmd_partial_error;
    use crate::platform::process::ProcessError;
    use crate::support::error::AppError;

    #[test]
    fn ibcmd_partial_warning_preserves_typed_process_error_source() {
        let warning = "IBCMD does not support object-scoped partial dump";
        let error = decorate_ibcmd_partial_error(
            AppError::PlatformProcess(ProcessError::SpawnFailed {
                cmd: "ibcmd config export".to_owned(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "missing ibcmd"),
            }),
            warning,
        );

        assert!(error.to_string().contains(warning));
        assert!(matches!(
            error,
            AppError::PlatformProcessContext {
                source: ProcessError::SpawnFailed { .. },
                ..
            }
        ));
    }
}
