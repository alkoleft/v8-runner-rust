use std::collections::HashMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::change_detection::source_sets::SourceSetsService;
use crate::config::model::{
    AppConfig, BuilderBackend, SourceFormat, SourceSetConfig, SourceSetPurpose,
};
use crate::domain::dump::{DumpMode, DumpResult};
use crate::domain::source_set::SourceSetContext;
use crate::platform::designer::DesignerDsl;
use crate::platform::ibcmd::{IbcmdConnection, IbcmdDsl, IbcmdError};
use crate::platform::locator::UtilityType;
use crate::platform::process::ProcessRunner;
use crate::platform::result::PlatformCommandResult;
use crate::platform::utilities::PlatformUtilities;
use crate::support::error::AppError;
use crate::support::fs::{
    acquire_advisory_lock, ensure_dir, metadata_sidecar_path, read_temp_dir_metadata,
    remove_path_if_exists, replace_dir_atomically, write_temp_dir_metadata, TempDirKind,
};
use crate::support::temp::{dump_object_list_file, platform_logs_dir};
use crate::use_cases::context::ExecutionContext;
use crate::use_cases::ibcmd_diagnostics::format_ibcmd_failure_details;
use crate::use_cases::request::{DumpModeRequest, DumpRequest as DumpArgs};
use crate::use_cases::result::{UseCaseFailure, UseCaseResult};
use tracing::info;

#[cfg(test)]
const DUMP_COMMAND: &str = crate::use_cases::context::CommandName::Dump.as_str();
const SUPPORTED_DUMP_ERROR: &str =
    "dump currently supports only builder=DESIGNER or IBCMD with format=DESIGNER";
const PARTIAL_OBJECTS_REQUIRED_ERROR: &str = "partial dump requires at least one object";
const PARTIAL_OBJECT_BLANK_ERROR: &str = "partial dump objects must not be blank";
const PARTIAL_OBJECT_CONTROL_ERROR: &str =
    "partial dump objects must not contain control characters";
const NON_PARTIAL_OBJECTS_ERROR: &str = "dump objects are supported only for mode 'partial'";
const ORPHAN_TTL: Duration = Duration::from_secs(24 * 60 * 60);

pub fn execute(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &DumpArgs,
) -> UseCaseResult<DumpResult> {
    info!(
        command = context.command().as_str(),
        transport = ?context.transport(),
        "executing dump use case"
    );
    run_dump(config, args)
}

type DumpExecutionFailure = UseCaseFailure<DumpResult>;

#[derive(Debug, Clone)]
struct ResolvedDumpTarget {
    source_set_name: String,
    extension: Option<String>,
    target_path: PathBuf,
    canonical_target_path: PathBuf,
    canonical_base_path: PathBuf,
    canonical_work_path: PathBuf,
    target_identity: String,
    lock_path: PathBuf,
}

fn run_dump(config: &AppConfig, args: &DumpArgs) -> UseCaseResult<DumpResult> {
    let started = Instant::now();
    let mode = match args.mode {
        DumpModeRequest::Full => DumpMode::Full,
        DumpModeRequest::Incremental => DumpMode::Incremental,
        DumpModeRequest::Partial => DumpMode::Partial,
    };
    info!(
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

    let partial_objects = partial_objects.as_deref();
    let result = match (&mode, &config.builder, partial_objects) {
        (DumpMode::Incremental, BuilderBackend::Designer, _) => run_incremental_dump_designer(
            config,
            &resolved,
            location.path.as_path(),
            utilities.runner_for(UtilityType::V8),
        ),
        (DumpMode::Incremental, BuilderBackend::Ibcmd, _) => run_incremental_dump_ibcmd(
            config,
            &resolved,
            location.path.as_path(),
            utilities.runner_for(UtilityType::Ibcmd),
        ),
        (DumpMode::Full, BuilderBackend::Designer, _) => run_full_dump_designer(
            config,
            &resolved,
            location.path.as_path(),
            utilities.runner_for(UtilityType::V8),
        ),
        (DumpMode::Full, BuilderBackend::Ibcmd, _) => run_full_dump_ibcmd(
            config,
            &resolved,
            location.path.as_path(),
            utilities.runner_for(UtilityType::Ibcmd),
        ),
        (DumpMode::Partial, BuilderBackend::Designer, Some(objects)) => run_partial_dump_designer(
            config,
            &resolved,
            location.path.as_path(),
            utilities.runner_for(UtilityType::V8),
            objects,
        ),
        (DumpMode::Partial, BuilderBackend::Ibcmd, Some(objects)) => run_partial_dump_ibcmd(
            config,
            &resolved,
            location.path.as_path(),
            utilities.runner_for(UtilityType::Ibcmd),
            objects,
        ),
        (DumpMode::Partial, _, None) => Err(AppError::Runtime(
            "partial dump objects were not validated before execution".to_owned(),
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
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    runner: &dyn ProcessRunner,
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    info!(
        source_set = resolved.source_set_name.as_str(),
        target = %resolved.target_path.display(),
        "running incremental dump"
    );
    ensure_dir(&resolved.target_path)
        .map_err(|error| AppError::Runtime(format!("failed to create target dir: {error}")))?;

    let dump_result = build_designer_dsl(
        config,
        binary,
        runner,
        &resolved.source_set_name,
        "incremental",
    )?
    .dump_config_to_files(&resolved.target_path, resolved.extension.as_deref())
    .map_err(|error| AppError::Platform(error.to_string()))?;
    ensure_platform_success("dump", resolved, &dump_result)?;
    Ok((dump_result, None))
}

fn run_full_dump_designer(
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    runner: &dyn ProcessRunner,
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    info!(
        source_set = resolved.source_set_name.as_str(),
        target = %resolved.target_path.display(),
        "running full dump via staging directory"
    );
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
    info!(path = %staging_dir.display(), "created dump staging directory");
    write_temp_dir_metadata(
        &staging_dir,
        TempDirKind::Stage,
        &run_id,
        &resolved.target_path,
        &resolved.target_identity,
    )
    .map_err(|error| AppError::Runtime(format!("failed to write stage metadata: {error}")))?;

    let dump_result =
        match build_designer_dsl(config, binary, runner, &resolved.source_set_name, "full")?
            .dump_config_to_files(&staging_dir, resolved.extension.as_deref())
            .map_err(|error| AppError::Platform(error.to_string()))
        {
            Ok(result) => result,
            Err(error) => return Err(cleanup_staging_on_platform_failure(&staging_dir, error)),
        };
    ensure_platform_success("dump", resolved, &dump_result)
        .map_err(|error| cleanup_staging_on_platform_failure(&staging_dir, error))?;

    validate_publish_target(resolved)?;

    let publish_outcome = replace_dir_atomically(
        &staging_dir,
        &resolved.target_path,
        &run_id,
        &resolved.target_identity,
    )
    .map_err(|error| AppError::Runtime(format!("failed to publish staged dump: {error}")))?;
    info!(target = %resolved.target_path.display(), "published staged dump");

    Ok((dump_result, publish_outcome.cleanup_warning))
}

fn run_incremental_dump_ibcmd(
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    runner: &dyn ProcessRunner,
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    info!(
        source_set = resolved.source_set_name.as_str(),
        target = %resolved.target_path.display(),
        "running incremental ibcmd dump"
    );
    ensure_dir(&resolved.target_path)
        .map_err(|error| AppError::Runtime(format!("failed to create target dir: {error}")))?;

    let dump_result = build_ibcmd_dsl(config, binary, runner)?
        .config_export_incremental(&resolved.target_path, resolved.extension.as_deref())
        .map_err(map_ibcmd_error)?;
    ensure_platform_success("dump", resolved, &dump_result)?;
    Ok((dump_result, None))
}

fn run_full_dump_ibcmd(
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    runner: &dyn ProcessRunner,
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    info!(
        source_set = resolved.source_set_name.as_str(),
        target = %resolved.target_path.display(),
        "running full ibcmd dump via staging directory"
    );
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
    info!(path = %staging_dir.display(), "created dump staging directory");
    write_temp_dir_metadata(
        &staging_dir,
        TempDirKind::Stage,
        &run_id,
        &resolved.target_path,
        &resolved.target_identity,
    )
    .map_err(|error| AppError::Runtime(format!("failed to write stage metadata: {error}")))?;

    let dump_result = match build_ibcmd_dsl(config, binary, runner)?
        .config_export_full(&staging_dir, resolved.extension.as_deref())
        .map_err(map_ibcmd_error)
    {
        Ok(result) => result,
        Err(error) => return Err(cleanup_staging_on_platform_failure(&staging_dir, error)),
    };
    ensure_platform_success("dump", resolved, &dump_result)
        .map_err(|error| cleanup_staging_on_platform_failure(&staging_dir, error))?;

    validate_publish_target(resolved)?;

    let publish_outcome = replace_dir_atomically(
        &staging_dir,
        &resolved.target_path,
        &run_id,
        &resolved.target_identity,
    )
    .map_err(|error| AppError::Runtime(format!("failed to publish staged dump: {error}")))?;
    info!(target = %resolved.target_path.display(), "published staged dump");

    Ok((dump_result, publish_outcome.cleanup_warning))
}

fn run_partial_dump_designer(
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    runner: &dyn ProcessRunner,
    objects: &[String],
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    info!(
        source_set = resolved.source_set_name.as_str(),
        target = %resolved.target_path.display(),
        object_count = objects.len(),
        "running partial designer dump"
    );
    ensure_dir(&resolved.target_path)
        .map_err(|error| AppError::Runtime(format!("failed to create target dir: {error}")))?;

    let list_file = create_dump_object_list_file(&config.work_path, objects)?;
    let dump_result =
        build_designer_dsl(config, binary, runner, &resolved.source_set_name, "partial")?
            .dump_config_to_files_partial(
                &resolved.target_path,
                list_file.path(),
                resolved.extension.as_deref(),
            )
            .map_err(|error| AppError::Platform(error.to_string()))?;
    ensure_platform_success("dump", resolved, &dump_result)?;
    Ok((dump_result, None))
}

fn run_partial_dump_ibcmd(
    config: &AppConfig,
    resolved: &ResolvedDumpTarget,
    binary: &Path,
    runner: &dyn ProcessRunner,
    _objects: &[String],
) -> Result<(PlatformCommandResult, Option<String>), AppError> {
    let warning = ibcmd_partial_warning(resolved);
    match run_incremental_dump_ibcmd(config, resolved, binary, runner) {
        Ok((dump_result, _)) => Ok((dump_result, Some(warning))),
        Err(error) => Err(decorate_ibcmd_partial_error(error, &warning)),
    }
}

fn cleanup_staging_on_platform_failure(staging_dir: &Path, error: AppError) -> AppError {
    let sidecar = metadata_sidecar_path(staging_dir);
    let _ = remove_path_if_exists(staging_dir);
    let _ = remove_path_if_exists(&sidecar);
    error
}

fn resolve_target(config: &AppConfig, args: &DumpArgs) -> Result<ResolvedDumpTarget, AppError> {
    let service = SourceSetsService::new(config);
    let contexts_by_name: HashMap<String, SourceSetContext> = service
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

    let context = contexts_by_name
        .get(&source_set.name)
        .cloned()
        .ok_or_else(|| {
            AppError::Runtime(format!(
                "missing runtime context for source-set '{}'",
                source_set.name
            ))
        })?;
    let target_path = context.path().to_path_buf();
    let canonical_target_path = nearest_existing_canonical_path(&target_path).map_err(|error| {
        AppError::Runtime(format!("failed to canonicalize target path: {error}"))
    })?;
    let canonical_base_path = nearest_existing_canonical_path(&config.base_path)
        .map_err(|error| AppError::Runtime(format!("failed to canonicalize basePath: {error}")))?;
    let canonical_work_path = nearest_existing_canonical_path(&config.work_path)
        .map_err(|error| AppError::Runtime(format!("failed to canonicalize workPath: {error}")))?;
    let target_identity = hash_path(&canonical_target_path);
    if target_path.parent().is_none() {
        return Err(AppError::Runtime(format!(
            "target path has no parent: {}",
            target_path.display()
        )));
    }
    let canonical_target_parent = canonical_target_path.parent().ok_or_else(|| {
        AppError::Runtime(format!(
            "canonical target path has no parent: {}",
            canonical_target_path.display()
        ))
    })?;
    let lock_path = canonical_target_parent.join(format!(".dump-{target_identity}.lock"));

    Ok(ResolvedDumpTarget {
        source_set_name: source_set.name.clone(),
        extension,
        target_path,
        canonical_target_path,
        canonical_base_path,
        canonical_work_path,
        target_identity,
        lock_path,
    })
}

fn validate_supported_matrix(config: &AppConfig) -> Option<AppError> {
    if config.format == SourceFormat::Designer
        && matches!(
            config.builder,
            BuilderBackend::Designer | BuilderBackend::Ibcmd
        )
    {
        None
    } else {
        Some(AppError::Validation(SUPPORTED_DUMP_ERROR.to_owned()))
    }
}

fn validate_publish_target(resolved: &ResolvedDumpTarget) -> Result<(), AppError> {
    if resolved.canonical_target_path
        != nearest_existing_canonical_path(&resolved.target_path).map_err(|error| {
            AppError::Runtime(format!("failed to re-canonicalize target path: {error}"))
        })?
    {
        return Err(AppError::Validation(format!(
            "target path changed during dump resolution: {}",
            resolved.target_path.display()
        )));
    }

    if resolved.canonical_target_path == resolved.canonical_base_path {
        return Err(AppError::Validation(
            "dump target must not equal basePath".to_owned(),
        ));
    }
    if resolved.canonical_target_path == resolved.canonical_work_path {
        return Err(AppError::Validation(
            "dump target must not equal workPath".to_owned(),
        ));
    }
    if resolved.canonical_target_path == Path::new("/") {
        return Err(AppError::Validation(
            "dump target must not equal filesystem root".to_owned(),
        ));
    }
    Ok(())
}

fn nearest_existing_canonical_path(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    let mut existing = absolute.as_path();
    while !existing.exists() {
        existing = existing.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no existing ancestor for path '{}'", path.display()),
            )
        })?;
    }

    let existing_canonical = std::fs::canonicalize(existing)?;
    if existing == absolute {
        return Ok(existing_canonical);
    }

    let suffix = absolute
        .strip_prefix(existing)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let suffix =
        suffix
            .components()
            .try_fold(PathBuf::new(), |mut acc, component| match component {
                Component::Normal(part) => {
                    acc.push(part);
                    Ok(acc)
                }
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "path '{}' contains unsupported component '{}'",
                        path.display(),
                        component.as_os_str().to_string_lossy()
                    ),
                )),
            })?;

    Ok(existing_canonical.join(suffix))
}

fn hash_path(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn cleanup_orphan_dirs(resolved: &ResolvedDumpTarget) -> Result<(), AppError> {
    let target_parent = resolved.target_path.parent().ok_or_else(|| {
        AppError::Runtime(format!(
            "target path has no parent: {}",
            resolved.target_path.display()
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
        if metadata.tool != "v8-test-runner" || metadata.target_identity != resolved.target_identity
        {
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

fn build_designer_dsl<'a>(
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
        build_designer_dsl, cleanup_orphan_dirs, create_dump_object_list_file_with, hash_path,
        metadata_sidecar_path, nearest_existing_canonical_path, resolve_target, run_dump,
        validate_publish_target, validate_supported_matrix, DUMP_COMMAND,
        NON_PARTIAL_OBJECTS_ERROR, ORPHAN_TTL, PARTIAL_OBJECTS_REQUIRED_ERROR,
        PARTIAL_OBJECT_BLANK_ERROR, PARTIAL_OBJECT_CONTROL_ERROR, SUPPORTED_DUMP_ERROR,
    };
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, PlatformToolConfig, SourceFormat, SourceSetConfig,
        SourceSetPurpose, TestsConfig, ToolsConfig,
    };
    use crate::domain::dump::DumpMode;
    use crate::output::json::Envelope;
    use crate::support::error::AppError;
    use crate::support::fs::{
        acquire_advisory_lock, read_temp_dir_metadata, write_temp_dir_metadata, TempDirKind,
    };
    use crate::use_cases::request::{DumpModeRequest, DumpRequest as DumpArgs};
    use crate::use_cases::result::UseCaseErrorKind;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use tempfile::tempdir;

    fn make_executable(path: &Path) {
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }

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
            format: SourceFormat::Designer,
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
    fn rejects_unsupported_matrix() {
        let dir = tempdir().expect("tempdir");
        let config = AppConfig {
            format: SourceFormat::Edt,
            builder: BuilderBackend::Designer,
            ..build_config(dir.path(), dir.path(), dir.path())
        };

        let failure = run_dump(
            &config,
            &DumpArgs {
                mode: DumpModeRequest::Full,
                source_set: None,
                extension: None,
                objects: vec![],
            },
        )
        .expect_err("failure");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Validation);
        assert_eq!(failure.error.message(), SUPPORTED_DUMP_ERROR);
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
            canonical_base_path: std::fs::canonicalize(dir.path()).expect("canonical"),
            canonical_work_path: std::fs::canonicalize(dir.path().join("work").as_path())
                .unwrap_or_else(|_| dir.path().join("work")),
            target_identity: "id".to_owned(),
            lock_path: dir.path().join(".lock"),
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
        let identity = hash_path(&canonical);
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
            canonical_base_path: std::fs::canonicalize(dir.path()).expect("canonical base"),
            canonical_work_path: std::fs::canonicalize(dir.path()).expect("canonical work"),
            target_identity: identity,
            lock_path: target.parent().expect("parent").join(".lock"),
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
        let identity = hash_path(&canonical);
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
            canonical_base_path: std::fs::canonicalize(dir.path()).expect("canonical base"),
            canonical_work_path: std::fs::canonicalize(dir.path()).expect("canonical work"),
            target_identity: identity,
            lock_path: target.parent().expect("parent").join(".lock"),
        };

        cleanup_orphan_dirs(&resolved).expect("cleanup");
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
        assert!(calls.contains("--extension=ext"));
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

    #[test]
    fn lock_identity_is_based_on_canonical_target() {
        let dir = tempdir().expect("tempdir");
        let real = dir.path().join("real");
        let link = dir.path().join("link");
        fs::create_dir_all(&real).expect("real");
        symlink(&real, &link).expect("symlink");

        let hash_real = hash_path(&std::fs::canonicalize(&real).expect("canonical"));
        let hash_link = hash_path(&std::fs::canonicalize(&link).expect("canonical"));

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
        let dsl =
            build_designer_dsl(&config, &script, &runner, "main", "incremental").expect("dsl");

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
}
