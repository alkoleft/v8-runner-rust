use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::Utc;
use sha2::{Digest, Sha256};
use tracing::info;

use crate::change_detection::source_sets::SourceSetsService;
use crate::config::model::{
    AppConfig, BuilderBackend, SourceFormat, SourceSetConfig, SourceSetPurpose,
};
use crate::domain::artifact::{
    ArtifactKind, ArtifactRef, ArtifactSet, ARTIFACT_ROLE_PACKAGE_FILE, ARTIFACT_ROLE_PLATFORM_LOG,
    ARTIFACT_ROLE_STAGE_FILE,
};
use crate::domain::artifacts::{ArtifactBuildMetadata, ArtifactBuildMode, ArtifactsResult};
use crate::domain::execution::{ExecutionError, ExecutionOutcome, ExecutionStatus};
use crate::domain::runner::RunnerKind;
use crate::platform::designer::DesignerDsl;
use crate::platform::locator::UtilityType;
use crate::platform::process::ProcessRunner;
use crate::platform::result::PlatformCommandResult;
use crate::platform::utilities::PlatformUtilities;
use crate::support::error::AppError;
use crate::support::fs::{
    acquire_advisory_lock, ensure_dir, metadata_sidecar_path, read_temp_dir_metadata,
    remove_path_if_exists, replace_dir_atomically, replace_file_atomically,
    write_temp_dir_metadata, TempDirKind,
};
use crate::support::temp::platform_logs_dir;
use crate::use_cases::context::ExecutionContext;
use crate::use_cases::dump_config::run_external_dump_designer;
use crate::use_cases::external_artifacts::{
    discover_designer_external_artifacts, prepare_edt_external_artifacts, resolve_source_set_path,
    sanitize_file_stem, source_set_external_kind, ExternalArtifactDescriptor,
};
use crate::use_cases::request::{ArtifactsModeRequest, ArtifactsRequest};
use crate::use_cases::result::{UseCaseFailure, UseCaseResult};

const SUPPORTED_ARTIFACTS_ERROR: &str =
    "artifacts currently supports only builder=DESIGNER with designer backend profile";
const ORPHAN_TTL: Duration = Duration::from_secs(24 * 60 * 60);

pub fn execute(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &ArtifactsRequest,
) -> UseCaseResult<ArtifactsResult> {
    info!(
        command = context.command().as_str(),
        transport = ?context.transport(),
        mode = ?args.mode,
        source_set = args.source_set.as_deref().unwrap_or("<auto>"),
        extension = args.extension.as_deref().unwrap_or("<none>"),
        "executing artifacts use case"
    );
    run_artifacts(config, args)
}

type ArtifactsExecutionFailure = UseCaseFailure<ArtifactsResult>;

#[derive(Debug, Clone)]
struct ResolvedArtifactsTarget {
    mode: ArtifactBuildMode,
    source_set_name: String,
    extension: Option<String>,
    output_path: PathBuf,
    source_path: PathBuf,
    is_directory_output: bool,
    canonical_output_path: PathBuf,
    canonical_base_path: PathBuf,
    canonical_work_path: PathBuf,
    target_identity: String,
    lock_path: PathBuf,
}

fn run_artifacts(config: &AppConfig, args: &ArtifactsRequest) -> UseCaseResult<ArtifactsResult> {
    let started = Instant::now();
    let mode = map_mode(args.mode);

    if let Some(error) = validate_supported_matrix(config, args) {
        return Err(ArtifactsExecutionFailure::with_payload(
            error,
            empty_result(
                mode,
                started,
                None,
                args.extension.clone(),
                PathBuf::from(&args.output_path),
                Some(SUPPORTED_ARTIFACTS_ERROR.to_owned()),
            ),
        ));
    }

    let resolved = match resolve_target(config, args) {
        Ok(resolved) => resolved,
        Err(error) => {
            let message = error.to_string();
            return Err(ArtifactsExecutionFailure::with_payload(
                error,
                empty_result(
                    mode,
                    started,
                    args.source_set.clone(),
                    args.extension.clone(),
                    PathBuf::from(&args.output_path),
                    Some(message),
                ),
            ));
        }
    };

    if let Err(error) = validate_publish_target(&resolved) {
        let message = error.to_string();
        return Err(ArtifactsExecutionFailure::with_payload(
            error,
            empty_result(
                resolved.mode,
                started,
                Some(resolved.source_set_name.clone()),
                resolved.extension.clone(),
                resolved.output_path.clone(),
                Some(message),
            ),
        ));
    }

    let mut utilities = PlatformUtilities::from_config(config);
    let location = match utilities.locate(UtilityType::V8) {
        Ok(location) => location,
        Err(error) => {
            let message = error.to_string();
            return Err(ArtifactsExecutionFailure::with_payload(
                AppError::Platform(message.clone()),
                empty_result(
                    resolved.mode,
                    started,
                    Some(resolved.source_set_name.clone()),
                    resolved.extension.clone(),
                    resolved.output_path.clone(),
                    Some(message),
                ),
            ));
        }
    };

    let lock_guard = match acquire_advisory_lock(&resolved.lock_path) {
        Ok(lock_guard) => lock_guard,
        Err(error) => {
            let message = format!(
                "failed to acquire artifacts lock '{}': {error}",
                resolved.lock_path.display()
            );
            return Err(ArtifactsExecutionFailure::with_payload(
                AppError::Runtime(message.clone()),
                empty_result(
                    resolved.mode,
                    started,
                    Some(resolved.source_set_name.clone()),
                    resolved.extension.clone(),
                    resolved.output_path.clone(),
                    Some(message),
                ),
            ));
        }
    };

    if let Err(error) = cleanup_orphan_files(&resolved) {
        let message = error.to_string();
        return Err(ArtifactsExecutionFailure::with_payload(
            error,
            empty_result(
                resolved.mode,
                started,
                Some(resolved.source_set_name.clone()),
                resolved.extension.clone(),
                resolved.output_path.clone(),
                Some(message),
            ),
        ));
    }

    let execution_result = run_designer_export(
        config,
        &resolved,
        location.path.as_path(),
        utilities.runner_for(UtilityType::V8),
    );
    drop(lock_guard);

    match execution_result {
        Ok((platform_result, mut artifacts, message)) => {
            let platform_log_path = platform_result.platform_log_path.clone();
            if let Some(path) = platform_log_path.as_ref() {
                artifacts.push(
                    ArtifactRef::new(ArtifactKind::PlatformLog, path)
                        .with_role(ARTIFACT_ROLE_PLATFORM_LOG),
                );
            }
            let metadata = ArtifactBuildMetadata {
                artifact_type: resolved.mode,
                output_path: resolved.output_path.clone(),
                file_names: published_file_names(&artifacts),
                published: true,
            };
            Ok(ArtifactsResult {
                ok: true,
                mode: resolved.mode,
                source_set: Some(resolved.source_set_name),
                extension: resolved.extension,
                output_path: resolved.output_path,
                platform_log_path,
                artifacts: artifacts.clone(),
                duration_ms: started.elapsed().as_millis() as u64,
                message,
                execution: ExecutionOutcome::new(ExecutionStatus::Succeeded)
                    .with_artifacts(artifacts)
                    .with_payload(metadata),
            })
        }
        Err((error, artifacts, platform_log_path)) => {
            let message = error.to_string();
            let metadata = ArtifactBuildMetadata {
                artifact_type: resolved.mode,
                output_path: resolved.output_path.clone(),
                file_names: published_file_names(&artifacts),
                published: false,
            };
            let artifact_for_error = artifacts
                .get_by_role(ARTIFACT_ROLE_PLATFORM_LOG)
                .or_else(|| artifacts.get_by_role(ARTIFACT_ROLE_STAGE_FILE))
                .map(|path| ArtifactRef::new(ArtifactKind::Other("diagnostic".to_owned()), path));
            let payload = ArtifactsResult {
                ok: false,
                mode: resolved.mode,
                source_set: Some(resolved.source_set_name),
                extension: resolved.extension,
                output_path: resolved.output_path,
                platform_log_path,
                artifacts: artifacts.clone(),
                duration_ms: started.elapsed().as_millis() as u64,
                message: Some(message.clone()),
                execution: ExecutionOutcome::new(ExecutionStatus::Failed)
                    .with_errors(vec![ExecutionError {
                        code: "designer_export_failed".to_owned(),
                        message,
                        details: Vec::new(),
                        artifact: artifact_for_error,
                        retryable: false,
                    }])
                    .with_artifacts(artifacts)
                    .with_payload(metadata),
            };
            Err(ArtifactsExecutionFailure::with_payload(error, payload))
        }
    }
}

fn run_designer_export(
    config: &AppConfig,
    resolved: &ResolvedArtifactsTarget,
    binary: &Path,
    runner: &dyn ProcessRunner,
) -> Result<
    (PlatformCommandResult, ArtifactSet, Option<String>),
    (AppError, ArtifactSet, Option<PathBuf>),
> {
    if matches!(
        resolved.mode,
        ArtifactBuildMode::ExternalDataProcessorEpf | ArtifactBuildMode::ExternalReportErf
    ) {
        return run_external_designer_export(config, resolved, binary, runner);
    }

    let target_parent = resolved.output_path.parent().ok_or_else(|| {
        (
            AppError::Runtime(format!(
                "target path has no parent: {}",
                resolved.output_path.display()
            )),
            ArtifactSet::default(),
            None,
        )
    })?;
    ensure_dir(target_parent).map_err(|error| {
        (
            AppError::Runtime(format!("failed to create target parent dir: {error}")),
            ArtifactSet::default(),
            None,
        )
    })?;

    let run_id = make_run_id();
    let staging_file = target_parent.join(format!(
        ".artifacts-stage-{run_id}.{}",
        resolved.mode.file_extension()
    ));
    write_temp_dir_metadata(
        &staging_file,
        TempDirKind::Stage,
        &run_id,
        &resolved.output_path,
        &resolved.target_identity,
    )
    .map_err(|error| {
        (
            AppError::Runtime(format!("failed to write staging metadata: {error}")),
            ArtifactSet::default(),
            None,
        )
    })?;

    let dsl = build_designer_dsl(
        config,
        binary,
        runner,
        &resolved.source_set_name,
        resolved.mode,
    )
    .map_err(|error| (error, ArtifactSet::default(), None))?;
    let dump_result = dsl
        .dump_cfg(&staging_file, resolved.extension.as_deref())
        .map_err(|error| {
            (
                AppError::Platform(error.to_string()),
                ArtifactSet::default(),
                None,
            )
        })?;

    let mut artifacts = ArtifactSet::default();
    if staging_file.exists() {
        artifacts.push(
            ArtifactRef::new(
                ArtifactKind::Other("staged_artifact".to_owned()),
                &staging_file,
            )
            .with_role(ARTIFACT_ROLE_STAGE_FILE),
        );
    }
    if let Some(path) = dump_result.platform_log_path.as_ref() {
        artifacts.push(
            ArtifactRef::new(ArtifactKind::PlatformLog, path).with_role(ARTIFACT_ROLE_PLATFORM_LOG),
        );
    }

    if let Err(error) = ensure_platform_success(&resolved.source_set_name, &dump_result) {
        return Err((error, artifacts, dump_result.platform_log_path.clone()));
    }
    if !staging_file.is_file() {
        return Err((
            AppError::Platform(format!(
                "designer did not produce artifact file '{}'",
                staging_file.display()
            )),
            artifacts,
            dump_result.platform_log_path.clone(),
        ));
    }

    let replace_outcome = replace_file_atomically(
        &staging_file,
        &resolved.output_path,
        &run_id,
        &resolved.target_identity,
    )
    .map_err(|error| {
        (
            AppError::Runtime(format!("failed to publish staged artifact: {error}")),
            artifacts.clone(),
            dump_result.platform_log_path.clone(),
        )
    })?;

    let mut published_artifacts = ArtifactSet::default();
    published_artifacts.push(
        ArtifactRef::new(ArtifactKind::Package, &resolved.output_path)
            .with_role(ARTIFACT_ROLE_PACKAGE_FILE),
    );

    Ok((
        dump_result,
        published_artifacts,
        replace_outcome.cleanup_warning,
    ))
}

fn run_external_designer_export(
    config: &AppConfig,
    resolved: &ResolvedArtifactsTarget,
    binary: &Path,
    runner: &dyn ProcessRunner,
) -> Result<
    (PlatformCommandResult, ArtifactSet, Option<String>),
    (AppError, ArtifactSet, Option<PathBuf>),
> {
    let target_parent = resolved.output_path.parent().ok_or_else(|| {
        (
            AppError::Runtime(format!(
                "external artifacts target has no parent: {}",
                resolved.output_path.display()
            )),
            ArtifactSet::default(),
            None,
        )
    })?;
    ensure_dir(target_parent).map_err(|error| {
        (
            AppError::Runtime(format!("failed to create external publish parent: {error}")),
            ArtifactSet::default(),
            None,
        )
    })?;
    let run_id = make_run_id();
    let staging_dir = target_parent.join(format!(".artifacts-stage-{run_id}"));
    ensure_dir(&staging_dir).map_err(|error| {
        (
            AppError::Runtime(format!("failed to create external staging dir: {error}")),
            ArtifactSet::default(),
            None,
        )
    })?;

    let dsl = build_designer_dsl(
        config,
        binary,
        runner,
        &resolved.source_set_name,
        resolved.mode,
    )
    .map_err(|error| (error, ArtifactSet::default(), None))?;
    let descriptors = external_descriptors(config, resolved, runner, binary)
        .map_err(|error| (error, ArtifactSet::default(), None))?;
    let mut artifacts = ArtifactSet::default();
    let mut last_result = PlatformCommandResult {
        process: crate::platform::process::ProcessResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        },
        platform_log_path: None,
        platform_log: None,
        platform_log_read_error: None,
    };

    for descriptor in &descriptors {
        let publish_name = format!(
            "{}.{}",
            sanitize_file_stem(&descriptor.logical_name),
            resolved.mode.file_extension()
        );
        let staging_file = staging_dir.join(&publish_name);
        let published_file = resolved.output_path.join(&publish_name);
        write_temp_dir_metadata(
            &staging_file,
            TempDirKind::Stage,
            &run_id,
            &published_file,
            &resolved.target_identity,
        )
        .map_err(|error| {
            (
                AppError::Runtime(format!("failed to write staging metadata: {error}")),
                artifacts.clone(),
                None,
            )
        })?;

        let result = dsl
            .load_external_data_processor_or_report_from_files(
                &descriptor.descriptor_xml_path,
                &staging_file,
            )
            .map_err(|error| {
                (
                    AppError::Platform(error.to_string()),
                    artifacts.clone(),
                    None,
                )
            })?;
        last_result = result.clone();
        if staging_file.exists() {
            artifacts.push(
                ArtifactRef::new(ArtifactKind::Package, &staging_file)
                    .with_role(ARTIFACT_ROLE_STAGE_FILE),
            );
        }
        if let Some(path) = result.platform_log_path.as_ref() {
            artifacts.push(
                ArtifactRef::new(ArtifactKind::PlatformLog, path)
                    .with_role(ARTIFACT_ROLE_PLATFORM_LOG),
            );
        }
        if let Err(error) = ensure_platform_success(&resolved.source_set_name, &result) {
            return Err((error, artifacts, result.platform_log_path.clone()));
        }
        if !staging_file.is_file() {
            return Err((
                AppError::Platform(format!(
                    "designer did not produce external artifact file '{}'",
                    staging_file.display()
                )),
                artifacts,
                result.platform_log_path.clone(),
            ));
        }

        run_external_dump_designer(
            &dsl,
            &staging_file,
            &config
                .work_path
                .join("external-dump")
                .join(&resolved.source_set_name)
                .join(&descriptor.stable_id)
                .join(format!("{}.xml", descriptor.logical_name)),
            descriptor.artifact_type,
            &descriptor.logical_name,
        )
        .map_err(|(error, platform_log_path)| {
            (
                error,
                artifacts.clone(),
                platform_log_path.or_else(|| result.platform_log_path.clone()),
            )
        })?;
    }

    replace_dir_atomically(
        &staging_dir,
        &resolved.output_path,
        &run_id,
        &resolved.target_identity,
    )
    .map_err(|error| {
        (
            AppError::Runtime(format!(
                "failed to publish staged external directory: {error}"
            )),
            artifacts.clone(),
            last_result.platform_log_path.clone(),
        )
    })?;

    for descriptor in &descriptors {
        let publish_name = format!(
            "{}.{}",
            sanitize_file_stem(&descriptor.logical_name),
            resolved.mode.file_extension()
        );
        let published_file = resolved.output_path.join(&publish_name);
        artifacts.push(
            ArtifactRef::new(ArtifactKind::Package, &published_file)
                .with_role(ARTIFACT_ROLE_PACKAGE_FILE),
        );
    }

    Ok((last_result, artifacts, None))
}

fn resolve_target(
    config: &AppConfig,
    args: &ArtifactsRequest,
) -> Result<ResolvedArtifactsTarget, AppError> {
    let output_path = validate_output_path(args)?;
    let service = SourceSetsService::new(config);
    let contexts_by_name: HashMap<String, PathBuf> = service
        .designer_contexts()
        .into_iter()
        .map(|context| (context.name().to_owned(), context.path().to_path_buf()))
        .collect();
    let config_by_name: HashMap<String, &SourceSetConfig> = config
        .source_sets
        .iter()
        .map(|source_set| (source_set.name.clone(), source_set))
        .collect();

    let (source_set, extension) = match args.mode {
        ArtifactsModeRequest::ConfigurationCf => {
            let source_set = match args.source_set.as_deref() {
                Some(name) => {
                    let source_set = config_by_name.get(name).copied().ok_or_else(|| {
                        AppError::Validation(format!("unknown source-set '{name}'"))
                    })?;
                    if source_set.purpose != SourceSetPurpose::Configuration {
                        return Err(AppError::Validation(format!(
                            "source-set '{name}' is not a configuration source-set"
                        )));
                    }
                    source_set
                }
                None => resolve_single_configuration_source_set(config)?,
            };
            (source_set, None)
        }
        ArtifactsModeRequest::ExtensionCfe => {
            let requested_extension = args
                .extension
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    AppError::Validation(
                        "artifacts cfe export requires non-empty --extension".to_owned(),
                    )
                })?;

            if let Some(source_set_name) = args.source_set.as_deref() {
                let source_set = config_by_name
                    .get(source_set_name)
                    .copied()
                    .ok_or_else(|| {
                        AppError::Validation(format!("unknown source-set '{source_set_name}'"))
                    })?;
                if source_set.purpose != SourceSetPurpose::Extension {
                    return Err(AppError::Validation(format!(
                        "source-set '{source_set_name}' is not an extension source-set"
                    )));
                }
                let resolved_extension_name = resolve_extension_name(config, source_set);
                if resolved_extension_name != requested_extension {
                    return Err(AppError::Validation(format!(
                        "source-set '{source_set_name}' resolves to extension '{resolved_extension_name}', expected '{requested_extension}'"
                    )));
                }
                (source_set, Some(requested_extension.to_owned()))
            } else {
                let candidates = config
                    .source_sets
                    .iter()
                    .filter(|source_set| source_set.purpose == SourceSetPurpose::Extension)
                    .filter_map(|source_set| {
                        let resolved_name = resolve_extension_name(config, source_set);
                        (resolved_name == requested_extension).then_some(source_set)
                    })
                    .collect::<Vec<_>>();
                if candidates.is_empty() {
                    let available = config
                        .source_sets
                        .iter()
                        .filter(|source_set| source_set.purpose == SourceSetPurpose::Extension)
                        .map(|source_set| {
                            format!(
                                "{}=>{}",
                                source_set.name,
                                resolve_extension_name(config, source_set)
                            )
                        })
                        .collect::<Vec<_>>();
                    return Err(AppError::Validation(format!(
                        "no extension source-set resolves to '{requested_extension}'; candidates [{}]",
                        available.join(", ")
                    )));
                }
                if candidates.len() != 1 {
                    let names = candidates
                        .iter()
                        .map(|source_set| source_set.name.as_str())
                        .collect::<Vec<_>>();
                    return Err(AppError::Validation(format!(
                        "extension '{requested_extension}' is ambiguous; matching source-sets [{}]",
                        names.join(", ")
                    )));
                }
                (candidates[0], Some(requested_extension.to_owned()))
            }
        }
        ArtifactsModeRequest::ExternalDataProcessorEpf
        | ArtifactsModeRequest::ExternalReportErf => {
            if args.extension.is_some() {
                return Err(AppError::Validation(
                    "external artifacts export does not support --extension".to_owned(),
                ));
            }
            let source_set_name = args.source_set.as_deref().ok_or_else(|| {
                AppError::Validation("external artifacts export requires --source-set".to_owned())
            })?;
            let source_set = config_by_name
                .get(source_set_name)
                .copied()
                .ok_or_else(|| {
                    AppError::Validation(format!("unknown source-set '{source_set_name}'"))
                })?;
            let expected_purpose = match args.mode {
                ArtifactsModeRequest::ExternalDataProcessorEpf => {
                    SourceSetPurpose::ExternalDataProcessors
                }
                ArtifactsModeRequest::ExternalReportErf => SourceSetPurpose::ExternalReports,
                _ => unreachable!(),
            };
            if source_set.purpose != expected_purpose {
                return Err(AppError::Validation(format!(
                    "source-set '{source_set_name}' has incompatible purpose for requested external export"
                )));
            }
            (source_set, None)
        }
    };

    let _runtime_context = contexts_by_name.get(&source_set.name).ok_or_else(|| {
        AppError::Runtime(format!(
            "missing runtime context for source-set '{}'",
            source_set.name
        ))
    })?;

    let canonical_output_path = nearest_existing_canonical_path(&output_path).map_err(|error| {
        AppError::Runtime(format!("failed to canonicalize output path: {error}"))
    })?;
    let canonical_base_path = nearest_existing_canonical_path(&config.base_path)
        .map_err(|error| AppError::Runtime(format!("failed to canonicalize basePath: {error}")))?;
    let canonical_work_path = nearest_existing_canonical_path(&config.work_path)
        .map_err(|error| AppError::Runtime(format!("failed to canonicalize workPath: {error}")))?;
    let target_identity = hash_path(&canonical_output_path);
    let canonical_parent = canonical_output_path.parent().ok_or_else(|| {
        AppError::Runtime(format!(
            "canonical output path has no parent: {}",
            canonical_output_path.display()
        ))
    })?;
    let lock_path = canonical_parent.join(format!(".artifacts-{target_identity}.lock"));

    Ok(ResolvedArtifactsTarget {
        mode: map_mode(args.mode),
        source_set_name: source_set.name.clone(),
        extension,
        output_path,
        source_path: resolve_source_set_path(config, source_set),
        is_directory_output: matches!(
            args.mode,
            ArtifactsModeRequest::ExternalDataProcessorEpf
                | ArtifactsModeRequest::ExternalReportErf
        ),
        canonical_output_path,
        canonical_base_path,
        canonical_work_path,
        target_identity,
        lock_path,
    })
}

fn validate_supported_matrix(config: &AppConfig, args: &ArtifactsRequest) -> Option<AppError> {
    if config.builder != BuilderBackend::Designer {
        return Some(AppError::Validation(SUPPORTED_ARTIFACTS_ERROR.to_owned()));
    }
    if args.execution.profile.backend_hint.as_deref() != Some("designer") {
        return Some(AppError::Validation(SUPPORTED_ARTIFACTS_ERROR.to_owned()));
    }
    let expected_kind = match args.mode {
        ArtifactsModeRequest::ConfigurationCf => RunnerKind::Cf,
        ArtifactsModeRequest::ExtensionCfe => RunnerKind::Cfe,
        ArtifactsModeRequest::ExternalDataProcessorEpf => RunnerKind::Epf,
        ArtifactsModeRequest::ExternalReportErf => RunnerKind::Erf,
    };
    if args.execution.profile.kind != expected_kind {
        return Some(AppError::Validation(SUPPORTED_ARTIFACTS_ERROR.to_owned()));
    }
    None
}

fn validate_output_path(args: &ArtifactsRequest) -> Result<PathBuf, AppError> {
    let output = args.output_path.trim();
    if output.is_empty() {
        return Err(AppError::Validation(
            "artifacts requires non-empty --output".to_owned(),
        ));
    }
    let output_path = PathBuf::from(output);
    match args.mode {
        ArtifactsModeRequest::ConfigurationCf | ArtifactsModeRequest::ExtensionCfe => {
            let expected_extension = match args.mode {
                ArtifactsModeRequest::ConfigurationCf => "cf",
                ArtifactsModeRequest::ExtensionCfe => "cfe",
                _ => unreachable!(),
            };
            if output_path.extension().and_then(|value| value.to_str()) != Some(expected_extension)
            {
                return Err(AppError::Validation(format!(
                    "artifacts output must end with .{expected_extension}"
                )));
            }
            if output_path.is_dir() {
                return Err(AppError::Validation(format!(
                    "artifacts output must be a file, got directory '{}'",
                    output_path.display()
                )));
            }
        }
        ArtifactsModeRequest::ExternalDataProcessorEpf
        | ArtifactsModeRequest::ExternalReportErf => {
            if output_path.extension().is_some() && !output_path.is_dir() {
                return Err(AppError::Validation(
                    "external artifacts output must be a directory".to_owned(),
                ));
            }
        }
    }
    Ok(output_path)
}

fn resolve_single_configuration_source_set(
    config: &AppConfig,
) -> Result<&SourceSetConfig, AppError> {
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
            "artifacts cf export requires exactly one configuration source-set when --source-set is omitted; found [{}]",
            candidates.join(", ")
        )));
    }
    Ok(configuration_source_sets[0])
}

fn resolve_extension_name(config: &AppConfig, source_set: &SourceSetConfig) -> String {
    if config.format != SourceFormat::Edt {
        return source_set.name.clone();
    }

    let project_file = config.base_path.join(&source_set.path).join(".project");
    std::fs::read_to_string(project_file)
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

fn validate_publish_target(resolved: &ResolvedArtifactsTarget) -> Result<(), AppError> {
    if resolved.canonical_output_path
        != nearest_existing_canonical_path(&resolved.output_path).map_err(|error| {
            AppError::Runtime(format!("failed to re-canonicalize output path: {error}"))
        })?
    {
        return Err(AppError::Validation(format!(
            "output path changed during artifacts resolution: {}",
            resolved.output_path.display()
        )));
    }
    if resolved.canonical_output_path == resolved.canonical_base_path {
        return Err(AppError::Validation(
            "artifacts output must not equal basePath".to_owned(),
        ));
    }
    if resolved.canonical_output_path == resolved.canonical_work_path {
        return Err(AppError::Validation(
            "artifacts output must not equal workPath".to_owned(),
        ));
    }
    if resolved.canonical_output_path == Path::new("/") {
        return Err(AppError::Validation(
            "artifacts output must not equal filesystem root".to_owned(),
        ));
    }
    if !resolved.is_directory_output
        && resolved.output_path.exists()
        && resolved.output_path.is_dir()
    {
        return Err(AppError::Validation(format!(
            "artifacts output conflicts with existing directory '{}'",
            resolved.output_path.display()
        )));
    }
    Ok(())
}

fn cleanup_orphan_files(resolved: &ResolvedArtifactsTarget) -> Result<(), AppError> {
    let mut scan_roots = Vec::new();
    if let Some(parent) = resolved.output_path.parent() {
        scan_roots.push(parent.to_path_buf());
    }
    if resolved.is_directory_output {
        scan_roots.push(resolved.output_path.clone());
    }
    scan_roots.sort();
    scan_roots.dedup();

    for root in scan_roots {
        if !root.exists() {
            continue;
        }
        for entry in std::fs::read_dir(&root)
            .map_err(|error| AppError::Runtime(format!("failed to read output dir: {error}")))?
        {
            let entry = entry
                .map_err(|error| AppError::Runtime(format!("failed to read dir entry: {error}")))?;
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !file_name.starts_with(".artifacts-stage-") && !file_name.contains(".backup-") {
                continue;
            }
            let Ok(metadata) = read_temp_dir_metadata(&path) else {
                continue;
            };
            if metadata.tool != "v8-test-runner"
                || metadata.target_identity != resolved.target_identity
            {
                continue;
            }
            if (Utc::now() - metadata.created_at)
                .to_std()
                .unwrap_or_default()
                < ORPHAN_TTL
            {
                continue;
            }

            remove_path_if_exists(&path).map_err(|error| {
                AppError::Runtime(format!(
                    "failed to remove stale artifact temp '{}': {error}",
                    path.display()
                ))
            })?;
            remove_path_if_exists(&metadata_sidecar_path(&path)).map_err(|error| {
                AppError::Runtime(format!(
                    "failed to remove stale artifact metadata '{}': {error}",
                    metadata_sidecar_path(&path).display()
                ))
            })?;
        }
    }
    Ok(())
}

fn build_designer_dsl<'a>(
    config: &AppConfig,
    binary: &Path,
    runner: &'a dyn ProcessRunner,
    source_set_name: &str,
    mode: ArtifactBuildMode,
) -> Result<DesignerDsl<'a>, AppError> {
    let log_dir = platform_logs_dir(&config.work_path).map_err(|error| {
        AppError::Runtime(format!("failed to create platform logs dir: {error}"))
    })?;
    let suffix = mode.file_extension();
    let log_file = log_dir.join(format!("artifacts-{source_set_name}-{suffix}.log"));
    Ok(DesignerDsl::new(
        binary.to_path_buf(),
        config.v8_connection(),
        runner,
        Some(log_file),
    ))
}

fn ensure_platform_success(
    source_set_name: &str,
    result: &PlatformCommandResult,
) -> Result<(), AppError> {
    if result.process.exit_code == 0 {
        return Ok(());
    }

    let mut details = vec![format!(
        "designer artifact export failed for source-set '{source_set_name}' with exit code {}",
        result.process.exit_code
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
        .map(str::trim)
        .filter(|log| !log.is_empty())
    {
        details.push(format!("platform log: {log}"));
    } else if let Some(path) = result.platform_log_path.as_ref() {
        details.push(format!("platform log path: {}", path.display()));
    }
    if let Some(error) = result.platform_log_read_error.as_deref() {
        details.push(error.to_owned());
    }

    Err(AppError::Platform(details.join("; ")))
}

fn empty_result(
    mode: ArtifactBuildMode,
    started: Instant,
    source_set: Option<String>,
    extension: Option<String>,
    output_path: PathBuf,
    message: Option<String>,
) -> ArtifactsResult {
    let metadata = ArtifactBuildMetadata {
        artifact_type: mode,
        output_path: output_path.clone(),
        file_names: output_path
            .file_name()
            .map(|value| vec![value.to_string_lossy().into_owned()])
            .unwrap_or_default(),
        published: false,
    };
    ArtifactsResult {
        ok: false,
        mode,
        source_set,
        extension,
        output_path,
        platform_log_path: None,
        artifacts: ArtifactSet::default(),
        duration_ms: started.elapsed().as_millis() as u64,
        message: message.clone(),
        execution: ExecutionOutcome::new(ExecutionStatus::Failed).with_payload(metadata),
    }
}

fn map_mode(mode: ArtifactsModeRequest) -> ArtifactBuildMode {
    match mode {
        ArtifactsModeRequest::ConfigurationCf => ArtifactBuildMode::ConfigurationCf,
        ArtifactsModeRequest::ExtensionCfe => ArtifactBuildMode::ExtensionCfe,
        ArtifactsModeRequest::ExternalDataProcessorEpf => {
            ArtifactBuildMode::ExternalDataProcessorEpf
        }
        ArtifactsModeRequest::ExternalReportErf => ArtifactBuildMode::ExternalReportErf,
    }
}

fn external_descriptors(
    config: &AppConfig,
    resolved: &ResolvedArtifactsTarget,
    _runner: &dyn ProcessRunner,
    _binary: &Path,
) -> Result<Vec<ExternalArtifactDescriptor>, AppError> {
    let source_set = config
        .source_sets
        .iter()
        .find(|source_set| source_set.name == resolved.source_set_name)
        .ok_or_else(|| {
            AppError::Runtime(format!(
                "failed to resolve source-set '{}'",
                resolved.source_set_name
            ))
        })?;
    let expected_kind = source_set_external_kind(source_set).ok_or_else(|| {
        AppError::Validation(format!("source-set '{}' is not external", source_set.name))
    })?;
    match config.format {
        SourceFormat::Designer => discover_designer_external_artifacts(
            &resolved.source_set_name,
            &resolved.source_path,
            expected_kind,
        ),
        SourceFormat::Edt => {
            let mut utilities = PlatformUtilities::from_config(config);
            let location = utilities
                .locate(UtilityType::EdtCli)
                .map_err(|error| AppError::Platform(error.to_string()))?;
            let edt = crate::platform::edt::EdtDsl::new(
                location.path,
                config.work_path.join("edt-workspace"),
                utilities.runner_for(UtilityType::EdtCli),
            );
            prepare_edt_external_artifacts(config, source_set, &edt)
        }
    }
}

fn published_file_names(artifacts: &ArtifactSet) -> Vec<String> {
    artifacts
        .items
        .iter()
        .filter(|artifact| artifact.role.as_deref() == Some(ARTIFACT_ROLE_PACKAGE_FILE))
        .filter_map(|artifact| artifact.path.file_name())
        .map(|value| value.to_string_lossy().into_owned())
        .collect()
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

fn make_run_id() -> String {
    let timestamp = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    format!("{}-{timestamp:x}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::{
        cleanup_orphan_files, resolve_target, run_artifacts, validate_supported_matrix,
        ResolvedArtifactsTarget,
    };
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, PlatformToolConfig, SourceFormat, SourceSetConfig,
        SourceSetPurpose, TestsConfig, ToolsConfig,
    };
    use crate::domain::artifact::{ARTIFACT_ROLE_PACKAGE_FILE, ARTIFACT_ROLE_PLATFORM_LOG};
    use crate::domain::artifacts::ArtifactBuildMode;
    use crate::support::fs::{
        metadata_sidecar_path, read_temp_dir_metadata, write_temp_dir_metadata, TempDirKind,
    };
    use crate::use_cases::request::{ArtifactsModeRequest, ArtifactsRequest};
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
    fn write_script(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create dirs");
        }
        fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        make_executable(path);
    }

    fn sample_config(
        base: &Path,
        work: &Path,
        platform_path: &Path,
        format: SourceFormat,
    ) -> AppConfig {
        AppConfig {
            base_path: base.to_path_buf(),
            work_path: work.to_path_buf(),
            format,
            builder: BuilderBackend::Designer,
            connection: "File=/tmp/ib".to_owned(),
            credentials: Default::default(),
            source_sets: vec![
                SourceSetConfig {
                    name: "configuration".to_owned(),
                    purpose: SourceSetPurpose::Configuration,
                    path: PathBuf::from("configuration"),
                },
                SourceSetConfig {
                    name: "ext-sales".to_owned(),
                    purpose: SourceSetPurpose::Extension,
                    path: PathBuf::from("extensions/ext-sales"),
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

    fn cf_request(output: &str) -> ArtifactsRequest {
        ArtifactsRequest {
            execution: ArtifactsRequest::default_execution(ArtifactsModeRequest::ConfigurationCf),
            mode: ArtifactsModeRequest::ConfigurationCf,
            output_path: output.to_owned(),
            source_set: None,
            extension: None,
        }
    }

    fn external_request(
        mode: ArtifactsModeRequest,
        output: &str,
        source_set: &str,
    ) -> ArtifactsRequest {
        ArtifactsRequest {
            execution: ArtifactsRequest::default_execution(mode),
            mode,
            output_path: output.to_owned(),
            source_set: Some(source_set.to_owned()),
            extension: None,
        }
    }

    fn add_external_source_set(config: &mut AppConfig, name: &str, purpose: SourceSetPurpose) {
        config.source_sets.push(SourceSetConfig {
            name: name.to_owned(),
            purpose,
            path: PathBuf::from(name),
        });
    }

    #[test]
    fn validate_supported_matrix_rejects_non_designer_profile() {
        let dir = tempdir().expect("tempdir");
        let mut request = cf_request("release.cf");
        request.execution.profile.backend_hint = Some("ibcmd".to_owned());
        let config = sample_config(
            dir.path(),
            dir.path(),
            Path::new("/tmp/1cv8"),
            SourceFormat::Designer,
        );

        let error = validate_supported_matrix(&config, &request).expect("error");

        assert!(error.to_string().contains("builder=DESIGNER"));
    }

    #[test]
    fn resolve_target_uses_edt_project_name_for_extension_matching() {
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("extensions/ext-sales")).expect("extension dir");
        fs::write(
            dir.path().join("extensions/ext-sales/.project"),
            "<projectDescription><name>SalesAddon</name></projectDescription>",
        )
        .expect("project");
        let config = sample_config(
            dir.path(),
            dir.path(),
            Path::new("/tmp/1cv8"),
            SourceFormat::Edt,
        );
        let request = ArtifactsRequest {
            execution: ArtifactsRequest::default_execution(ArtifactsModeRequest::ExtensionCfe),
            mode: ArtifactsModeRequest::ExtensionCfe,
            output_path: "dist/sales.cfe".to_owned(),
            source_set: None,
            extension: Some("SalesAddon".to_owned()),
        };

        let resolved = resolve_target(&config, &request).expect("resolved");

        assert_eq!(resolved.source_set_name, "ext-sales");
        assert_eq!(resolved.extension.as_deref(), Some("SalesAddon"));
        assert_eq!(resolved.mode, ArtifactBuildMode::ExtensionCfe);
    }

    #[test]
    fn resolve_target_rejects_blank_extension_for_cfe_mode() {
        let dir = tempdir().expect("tempdir");
        let config = sample_config(
            dir.path(),
            dir.path(),
            Path::new("/tmp/1cv8"),
            SourceFormat::Designer,
        );
        let request = ArtifactsRequest {
            execution: ArtifactsRequest::default_execution(ArtifactsModeRequest::ExtensionCfe),
            mode: ArtifactsModeRequest::ExtensionCfe,
            output_path: "dist/sales.cfe".to_owned(),
            source_set: Some("ext-sales".to_owned()),
            extension: Some("   ".to_owned()),
        };

        let error = resolve_target(&config, &request).expect_err("blank extension should fail");

        assert!(error.to_string().contains("non-empty --extension"));
    }

    #[cfg(unix)]
    #[test]
    fn run_artifacts_exports_cf_and_records_artifacts() {
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("configuration")).expect("config dir");
        let script = dir.path().join("1cv8");
        write_script(
            &script,
            "out=''\nprev=''\nfor arg in \"$@\"; do\n  if [ \"$prev\" = '/DumpCfg' ]; then printf 'cf' > \"$arg\"; fi\n  if [ \"$prev\" = '/Out' ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf 'designer log' > \"$out\"; fi\nexit 0",
        );
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        fs::create_dir_all(base.join("configuration")).expect("base config");
        fs::create_dir_all(&work).expect("work");
        let config = sample_config(&base, &work, &script, SourceFormat::Designer);
        let request = cf_request(&dir.path().join("dist/release.cf").display().to_string());

        let result = run_artifacts(&config, &request).expect("result");

        assert!(result.ok);
        assert!(result.output_path.is_file());
        assert_eq!(
            result.artifacts.get_by_role(ARTIFACT_ROLE_PACKAGE_FILE),
            Some(result.output_path.as_path())
        );
        assert!(result
            .artifacts
            .get_by_role(ARTIFACT_ROLE_PLATFORM_LOG)
            .is_some());
    }

    #[cfg(unix)]
    #[test]
    fn run_artifacts_exports_external_processors_and_records_all_published_files() {
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("base/external-processors")).expect("external dir");
        fs::write(
            dir.path().join("base/external-processors/alpha.xml"),
            "<ExternalDataProcessor><Properties><Name>Alpha</Name></Properties></ExternalDataProcessor>",
        )
        .expect("alpha descriptor");
        fs::write(
            dir.path().join("base/external-processors/beta.xml"),
            "<ExternalDataProcessor><Properties><Name>Beta</Name></Properties></ExternalDataProcessor>",
        )
        .expect("beta descriptor");
        let script = dir.path().join("1cv8");
        write_script(
            &script,
            "out=''\nprev=''\nload_state=0\ndump_state=0\nname=''\nfor arg in \"$@\"; do\n  if [ \"$prev\" = '/LoadExternalDataProcessorOrReportFromFiles' ]; then load_state=1; prev=\"$arg\"; continue; fi\n  if [ \"$load_state\" = 1 ]; then printf 'external' > \"$arg\"; load_state=0; fi\n  if [ \"$prev\" = '/DumpExternalDataProcessorOrReportToFiles' ]; then dump_state=1; prev=\"$arg\"; continue; fi\n  if [ \"$dump_state\" = 1 ]; then case \"$arg\" in *Alpha.xml) name='Alpha' ;; *Beta.xml) name='Beta' ;; *) name='Unknown' ;; esac; printf '<ExternalDataProcessor><Properties><Name>%s</Name></Properties></ExternalDataProcessor>' \"$name\" > \"$arg\"; dump_state=0; fi\n  if [ \"$prev\" = '/Out' ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf 'designer log' > \"$out\"; fi\nexit 0",
        );
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        fs::create_dir_all(&base).expect("base dir");
        fs::create_dir_all(&work).expect("work");
        let mut config = sample_config(&base, &work, &script, SourceFormat::Designer);
        add_external_source_set(
            &mut config,
            "external-processors",
            SourceSetPurpose::ExternalDataProcessors,
        );
        let request = external_request(
            ArtifactsModeRequest::ExternalDataProcessorEpf,
            &dir.path().join("dist/external").display().to_string(),
            "external-processors",
        );

        let result = run_artifacts(&config, &request).expect("result");
        let mut file_names = result
            .execution
            .payload
            .as_ref()
            .expect("payload")
            .file_names
            .clone();
        file_names.sort();

        assert!(result.ok);
        assert!(result.output_path.is_dir());
        assert_eq!(
            file_names,
            vec!["Alpha.epf".to_owned(), "Beta.epf".to_owned()]
        );
        assert!(result
            .artifacts
            .get_by_role(ARTIFACT_ROLE_PLATFORM_LOG)
            .is_some());
        assert!(result.output_path.join("Alpha.epf").is_file());
        assert!(result.output_path.join("Beta.epf").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn run_artifacts_replaces_stale_external_packages_atomically() {
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("base/external-processors")).expect("external dir");
        fs::write(
            dir.path().join("base/external-processors/alpha.xml"),
            "<ExternalDataProcessor><Properties><Name>Alpha</Name></Properties></ExternalDataProcessor>",
        )
        .expect("alpha descriptor");
        let script = dir.path().join("1cv8");
        write_script(
            &script,
            "out=''\nprev=''\nload_state=0\ndump_state=0\nname=''\nfor arg in \"$@\"; do\n  if [ \"$prev\" = '/LoadExternalDataProcessorOrReportFromFiles' ]; then load_state=1; prev=\"$arg\"; continue; fi\n  if [ \"$load_state\" = 1 ]; then printf 'external' > \"$arg\"; load_state=0; fi\n  if [ \"$prev\" = '/DumpExternalDataProcessorOrReportToFiles' ]; then dump_state=1; prev=\"$arg\"; continue; fi\n  if [ \"$dump_state\" = 1 ]; then case \"$arg\" in *Alpha.xml) name='Alpha' ;; *) name='Unknown' ;; esac; printf '<ExternalDataProcessor><Properties><Name>%s</Name></Properties></ExternalDataProcessor>' \"$name\" > \"$arg\"; dump_state=0; fi\n  if [ \"$prev\" = '/Out' ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf 'designer log' > \"$out\"; fi\nexit 0",
        );
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        fs::create_dir_all(&base).expect("base dir");
        fs::create_dir_all(&work).expect("work");
        let output = dir.path().join("dist/external");
        fs::create_dir_all(&output).expect("output");
        fs::write(output.join("stale.epf"), "stale").expect("stale file");
        let mut config = sample_config(&base, &work, &script, SourceFormat::Designer);
        add_external_source_set(
            &mut config,
            "external-processors",
            SourceSetPurpose::ExternalDataProcessors,
        );
        let request = external_request(
            ArtifactsModeRequest::ExternalDataProcessorEpf,
            &output.display().to_string(),
            "external-processors",
        );

        let result = run_artifacts(&config, &request).expect("result");
        let mut file_names = result
            .execution
            .payload
            .as_ref()
            .expect("payload")
            .file_names
            .clone();
        file_names.sort();

        assert!(result.ok);
        assert_eq!(file_names, vec!["Alpha.epf".to_owned()]);
        assert!(!output.join("stale.epf").exists());
        assert!(output.join("Alpha.epf").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn run_artifacts_keeps_existing_target_when_mid_batch_publish_fails() {
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("base/external-processors")).expect("external dir");
        fs::write(
            dir.path().join("base/external-processors/alpha.xml"),
            "<ExternalDataProcessor><Properties><Name>Alpha</Name></Properties></ExternalDataProcessor>",
        )
        .expect("alpha descriptor");
        fs::write(
            dir.path().join("base/external-processors/beta.xml"),
            "<ExternalDataProcessor><Properties><Name>Beta</Name></Properties></ExternalDataProcessor>",
        )
        .expect("beta descriptor");
        let script = dir.path().join("1cv8");
        write_script(
            &script,
            "out=''\nload_state=0\nseen=0\nfor arg in \"$@\"; do\n  if [ \"$prev\" = '/LoadExternalDataProcessorOrReportFromFiles' ]; then load_state=1; prev=\"$arg\"; continue; fi\n  if [ \"$load_state\" = 1 ]; then seen=$((seen + 1)); if [ \"$seen\" = 1 ]; then printf 'external' > \"$arg\"; else printf 'boom' > \"$arg\"; exit 12; fi; load_state=0; fi\n  if [ \"$prev\" = '/Out' ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf 'platform fail' > \"$out\"; fi\nexit 0",
        );
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        fs::create_dir_all(&base).expect("base dir");
        fs::create_dir_all(&work).expect("work");
        let output = dir.path().join("dist/external");
        fs::create_dir_all(&output).expect("output");
        fs::write(output.join("stale.epf"), "stale").expect("stale file");
        let mut config = sample_config(&base, &work, &script, SourceFormat::Designer);
        add_external_source_set(
            &mut config,
            "external-processors",
            SourceSetPurpose::ExternalDataProcessors,
        );
        let request = external_request(
            ArtifactsModeRequest::ExternalDataProcessorEpf,
            &output.display().to_string(),
            "external-processors",
        );

        let failure = run_artifacts(&config, &request).expect_err("failure");
        let payload = failure.payload.expect("payload");

        assert_eq!(
            fs::read_to_string(output.join("stale.epf")).expect("stale"),
            "stale"
        );
        assert!(!output.join("Alpha.epf").exists());
        assert!(!output.join("Beta.epf").exists());
        assert!(!payload.ok);
        assert!(!payload.execution.errors[0].message.is_empty());
    }

    #[test]
    fn cleanup_orphan_files_scans_directory_output_root() {
        let dir = tempdir().expect("tempdir");
        let output = dir.path().join("dist/external");
        fs::create_dir_all(&output).expect("output");
        let stale = output.join(".artifacts-stage-old.epf");
        fs::create_dir_all(&stale).expect("stale");
        write_temp_dir_metadata(
            &stale,
            TempDirKind::Stage,
            "run-1",
            &output.join("published.epf"),
            "identity",
        )
        .expect("metadata");
        let mut metadata = read_temp_dir_metadata(&stale).expect("read metadata");
        metadata.created_at -= chrono::Duration::days(2);
        fs::write(
            metadata_sidecar_path(&stale),
            serde_json::to_vec_pretty(&metadata).expect("json"),
        )
        .expect("rewrite metadata");
        let resolved = ResolvedArtifactsTarget {
            mode: ArtifactBuildMode::ExternalDataProcessorEpf,
            source_set_name: "external".to_owned(),
            extension: None,
            output_path: output.clone(),
            source_path: dir.path().join("external"),
            is_directory_output: true,
            canonical_output_path: output.clone(),
            canonical_base_path: dir.path().to_path_buf(),
            canonical_work_path: dir.path().to_path_buf(),
            target_identity: "identity".to_owned(),
            lock_path: dir.path().join("lock"),
        };

        cleanup_orphan_files(&resolved).expect("cleanup");

        assert!(!stale.exists());
    }
}
