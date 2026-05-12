use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;

use crate::config::model::{AppConfig, SourceFormat, SourceSetConfig, SourceSetPurpose};
use crate::domain::convert::{ConvertDirection, ConvertOutput, ConvertResult, ConvertScope};
use crate::platform::edt::EdtDsl;
use crate::platform::edt_session::{EdtSessionHostOptions, EdtSessionManager};
use crate::platform::locator::UtilityType;
use crate::platform::result::PlatformCommandResult;
use crate::platform::utilities::PlatformUtilities;
use crate::support::edt_project::{self, EdtProjectKind};
use crate::support::error::AppError;
use crate::support::fs::{
    ensure_dir, remove_path_if_exists, replace_dir_atomically, write_temp_dir_metadata, TempDirKind,
};
use crate::support::path::{
    is_filesystem_root, nearest_existing_canonical_path, stable_path_identity,
};
use crate::use_cases::context::{ExecutionContext, InterruptionSafetyClass};
use crate::use_cases::external_artifacts::{
    discover_designer_external_artifacts, parse_external_descriptor, ExternalArtifactKind,
};
use crate::use_cases::interruption;
use crate::use_cases::progress::log_live_stage;
use crate::use_cases::request::{ConvertRequest, ConvertScopeRequest};
use crate::use_cases::result::{UseCaseFailure, UseCaseResult};

const CONVERT_BACKUP_PREFIX: &str = ".convert-backup";

type ConvertExecutionFailure = UseCaseFailure<ConvertResult>;

#[derive(Debug, Clone, Default)]
struct ConvertImportOptions {
    version: Option<String>,
    base_project_name: Option<String>,
    base_project_source: Option<ConvertBaseProjectSource>,
    build: bool,
}

#[derive(Debug, Clone)]
struct ConvertBaseProjectSource {
    source_set_name: String,
    source_path: PathBuf,
    target_path: PathBuf,
    stable_project_dir_name: String,
}

#[derive(Debug, Clone)]
struct ResolvedConvertItem {
    source_set_name: String,
    purpose: SourceSetPurpose,
    source_path: PathBuf,
    target_path: PathBuf,
    canonical_target_path: PathBuf,
    target_identity: String,
    stable_project_dir_name: String,
    import_options: ConvertImportOptions,
}

#[derive(Debug, Clone)]
struct ResolvedConvertRequest {
    direction: ConvertDirection,
    scope: ConvertScope,
    source_set: Option<String>,
    workspace_path: PathBuf,
    items: Vec<ResolvedConvertItem>,
}

pub fn execute(
    context: &ExecutionContext,
    config: &AppConfig,
    request: &ConvertRequest,
) -> UseCaseResult<ConvertResult> {
    run_convert_with_context(context, config, request)
}

pub fn preflight_validate(config: &AppConfig, request: &ConvertRequest) -> Result<(), AppError> {
    resolve_request(config, request).map(|_| ())
}

fn run_convert_with_context(
    context: &ExecutionContext,
    config: &AppConfig,
    request: &ConvertRequest,
) -> UseCaseResult<ConvertResult> {
    let started = Instant::now();
    let direction = direction_from_format(config.format);
    let scope = scope_from_request(request);
    let workspace_path = convert_workspace_path(config);

    if let Some(interruption) = context.interruption() {
        let error = AppError::Runtime(interruption::command_interruption_message(
            context,
            interruption,
        ));
        let message = error.to_string();
        return Err(ConvertExecutionFailure::with_payload(
            error,
            result_snapshot(
                false,
                direction,
                scope,
                source_set_from_request(request),
                workspace_path,
                Vec::new(),
                started,
                Some(message),
            ),
        ));
    }

    let resolved = match resolve_request(config, request) {
        Ok(resolved) => resolved,
        Err(error) => {
            let message = error.to_string();
            return Err(ConvertExecutionFailure::with_payload(
                error,
                result_snapshot(
                    false,
                    direction,
                    scope,
                    source_set_from_request(request),
                    workspace_path,
                    Vec::new(),
                    started,
                    Some(message),
                ),
            ));
        }
    };

    let mut utilities = PlatformUtilities::from_config(config);
    let location = match utilities.locate(UtilityType::EdtCli) {
        Ok(location) => location,
        Err(error) => {
            let app_error = AppError::from(error);
            let message = app_error.to_string();
            return Err(ConvertExecutionFailure::with_payload(
                app_error,
                result_snapshot(
                    false,
                    resolved.direction,
                    resolved.scope,
                    resolved.source_set.clone(),
                    resolved.workspace_path.clone(),
                    Vec::new(),
                    started,
                    Some(message),
                ),
            ));
        }
    };

    let policy = context.process_policy(InterruptionSafetyClass::GracefulThenKill, None);
    if config.tools.edt_cli.interactive_mode {
        let manager = EdtSessionManager::for_config(
            config,
            convert_session_host_options(config, &resolved.workspace_path),
        )
        .map_err(|error| {
            let app_error = AppError::from(error);
            let message = app_error.to_string();
            ConvertExecutionFailure::with_payload(
                app_error,
                result_snapshot(
                    false,
                    resolved.direction,
                    resolved.scope,
                    resolved.source_set.clone(),
                    resolved.workspace_path.clone(),
                    Vec::new(),
                    started,
                    Some(message),
                ),
            )
        })?;
        let dsl = EdtDsl::new_shared_session(
            location.path.clone(),
            resolved.workspace_path.clone(),
            Arc::new(manager),
            Duration::from_millis(config.tools.edt_cli.startup_timeout_ms),
            Duration::from_millis(config.tools.edt_cli.command_timeout_ms),
        )
        .map_err(|error| {
            let app_error = AppError::from(error);
            let message = app_error.to_string();
            ConvertExecutionFailure::with_payload(
                app_error,
                result_snapshot(
                    false,
                    resolved.direction,
                    resolved.scope,
                    resolved.source_set.clone(),
                    resolved.workspace_path.clone(),
                    Vec::new(),
                    started,
                    Some(message),
                ),
            )
        })?
        .with_timeout(context.edt_timeout())
        .with_execution_policy(policy);
        execute_with_dsl(context, &dsl, &resolved, started)
    } else {
        let dsl = EdtDsl::new(
            location.path.clone(),
            resolved.workspace_path.clone(),
            utilities.runner_for(UtilityType::EdtCli),
        )
        .with_timeout(context.edt_timeout())
        .with_execution_policy(policy);
        execute_with_dsl(context, &dsl, &resolved, started)
    }
}

fn execute_with_dsl(
    context: &ExecutionContext,
    dsl: &EdtDsl<'_>,
    resolved: &ResolvedConvertRequest,
    started: Instant,
) -> UseCaseResult<ConvertResult> {
    let mut outputs = Vec::new();
    let mut messages = Vec::new();
    let mut processed_source_sets = HashSet::new();
    let mut base_project_names = HashMap::new();

    for item in &resolved.items {
        let import_options = resolve_runtime_import_options(
            dsl,
            item,
            &processed_source_sets,
            &mut base_project_names,
        )
        .map_err(|error| {
            let message = error.to_string();
            ConvertExecutionFailure::with_payload(
                error,
                result_snapshot(
                    false,
                    resolved.direction,
                    resolved.scope,
                    resolved.source_set.clone(),
                    resolved.workspace_path.clone(),
                    outputs.clone(),
                    started,
                    Some(message),
                ),
            )
        })?;
        let target_parent = item.target_path.parent().ok_or_else(|| {
            ConvertExecutionFailure::with_payload(
                AppError::Validation(format!(
                    "convert output path has no parent: {}",
                    item.target_path.display()
                )),
                result_snapshot(
                    false,
                    resolved.direction,
                    resolved.scope,
                    resolved.source_set.clone(),
                    resolved.workspace_path.clone(),
                    outputs.clone(),
                    started,
                    Some(format!(
                        "convert output path has no parent: {}",
                        item.target_path.display()
                    )),
                ),
            )
        })?;
        ensure_dir(target_parent).map_err(|error| {
            ConvertExecutionFailure::with_payload(
                AppError::Runtime(format!(
                    "failed to create convert output parent '{}': {error}",
                    target_parent.display()
                )),
                result_snapshot(
                    false,
                    resolved.direction,
                    resolved.scope,
                    resolved.source_set.clone(),
                    resolved.workspace_path.clone(),
                    outputs.clone(),
                    started,
                    Some(format!(
                        "failed to create convert output parent '{}': {error}",
                        target_parent.display()
                    )),
                ),
            )
        })?;

        let run_id = make_run_id();
        let staging_root = target_parent.join(format!(".convert-stage-{run_id}"));
        ensure_dir(&staging_root).map_err(|error| {
            ConvertExecutionFailure::with_payload(
                AppError::Runtime(format!(
                    "failed to create convert staging directory '{}': {error}",
                    staging_root.display()
                )),
                result_snapshot(
                    false,
                    resolved.direction,
                    resolved.scope,
                    resolved.source_set.clone(),
                    resolved.workspace_path.clone(),
                    outputs.clone(),
                    started,
                    Some(format!(
                        "failed to create convert staging directory '{}': {error}",
                        staging_root.display()
                    )),
                ),
            )
        })?;
        let staging_publish_dir = staging_publication_dir(resolved.direction, item, &staging_root);
        ensure_dir(&staging_publish_dir).map_err(|error| {
            let _ = remove_path_if_exists(&staging_root);
            ConvertExecutionFailure::with_payload(
                AppError::Runtime(format!(
                    "failed to create convert staging directory '{}': {error}",
                    staging_publish_dir.display()
                )),
                result_snapshot(
                    false,
                    resolved.direction,
                    resolved.scope,
                    resolved.source_set.clone(),
                    resolved.workspace_path.clone(),
                    outputs.clone(),
                    started,
                    Some(format!(
                        "failed to create convert staging directory '{}': {error}",
                        staging_publish_dir.display()
                    )),
                ),
            )
        })?;
        write_temp_dir_metadata(
            &staging_publish_dir,
            TempDirKind::Stage,
            &run_id,
            &item.target_path,
            &item.target_identity,
        )
        .map_err(|error| {
            let _ = remove_path_if_exists(&staging_root);
            ConvertExecutionFailure::with_payload(
                AppError::Runtime(format!(
                    "failed to write convert staging metadata '{}': {error}",
                    staging_publish_dir.display()
                )),
                result_snapshot(
                    false,
                    resolved.direction,
                    resolved.scope,
                    resolved.source_set.clone(),
                    resolved.workspace_path.clone(),
                    outputs.clone(),
                    started,
                    Some(format!(
                        "failed to write convert staging metadata '{}': {error}",
                        staging_publish_dir.display()
                    )),
                ),
            )
        })?;

        run_platform_conversion(
            dsl,
            resolved.direction,
            item,
            &import_options,
            &staging_publish_dir,
        )
        .map_err(|error| {
            let _ = remove_path_if_exists(&staging_root);
            let message = error.to_string();
            ConvertExecutionFailure::with_payload(
                error,
                result_snapshot(
                    false,
                    resolved.direction,
                    resolved.scope,
                    resolved.source_set.clone(),
                    resolved.workspace_path.clone(),
                    outputs.clone(),
                    started,
                    Some(message),
                ),
            )
        })?;

        validate_staging_output(
            resolved.direction,
            item,
            &import_options,
            &staging_publish_dir,
        )
        .map_err(|error| {
            let _ = remove_path_if_exists(&staging_root);
            let message = error.to_string();
            ConvertExecutionFailure::with_payload(
                error,
                result_snapshot(
                    false,
                    resolved.direction,
                    resolved.scope,
                    resolved.source_set.clone(),
                    resolved.workspace_path.clone(),
                    outputs.clone(),
                    started,
                    Some(message),
                ),
            )
        })?;

        if let Some(interruption) = context.interruption() {
            let _ = remove_path_if_exists(&staging_root);
            let error = AppError::Runtime(interruption::interruption_before_safe_point_message(
                context,
                interruption,
                "convert publication",
            ));
            let message = error.to_string();
            return Err(ConvertExecutionFailure::with_payload(
                error,
                result_snapshot(
                    false,
                    resolved.direction,
                    resolved.scope,
                    resolved.source_set.clone(),
                    resolved.workspace_path.clone(),
                    outputs,
                    started,
                    Some(message),
                ),
            ));
        }

        let publish_phase = context
            .run_no_process_critical_phase(|| {
                replace_dir_atomically(
                    &staging_publish_dir,
                    &item.target_path,
                    &run_id,
                    &item.target_identity,
                    CONVERT_BACKUP_PREFIX,
                )
                .map_err(|error| {
                    AppError::Runtime(format!("failed to publish convert output: {error}"))
                })
            })
            .map_err(|error| {
                let message = error.to_string();
                ConvertExecutionFailure::with_payload(
                    error,
                    result_snapshot(
                        false,
                        resolved.direction,
                        resolved.scope,
                        resolved.source_set.clone(),
                        resolved.workspace_path.clone(),
                        outputs.clone(),
                        started,
                        Some(message),
                    ),
                )
            })?;
        if staging_root.exists() {
            if let Err(error) = remove_path_if_exists(&staging_root) {
                messages.push(format!(
                    "failed to remove convert staging wrapper '{}': {error}",
                    staging_root.display()
                ));
            }
        }

        if let Some(message) = publish_phase.value.cleanup_warning {
            messages.push(message);
        }
        if let Some(message) = deferred_interruption_warning(publish_phase.deferred_interruption) {
            messages.push(message);
        }
        outputs.push(ConvertOutput {
            source_set: item.source_set_name.clone(),
            source_path: item.source_path.clone(),
            target_path: item.target_path.clone(),
        });
        processed_source_sets.insert(item.source_set_name.clone());
    }

    Ok(result_snapshot(
        true,
        resolved.direction,
        resolved.scope,
        resolved.source_set.clone(),
        resolved.workspace_path.clone(),
        outputs,
        started,
        merge_messages(messages),
    ))
}

fn resolve_request(
    config: &AppConfig,
    request: &ConvertRequest,
) -> Result<ResolvedConvertRequest, AppError> {
    if config.source_sets.is_empty() {
        return Err(AppError::Validation(
            "convert requires at least one source-set in v8project.yaml".to_owned(),
        ));
    }

    let direction = direction_from_format(config.format);
    let scope = scope_from_request(request);
    let source_set = source_set_from_request(request);
    let explicit_output_root = explicit_output_root(request)?;
    let selected = select_source_sets(config, request)?;
    let base_project_source = resolve_base_project_source(
        config,
        direction,
        &selected,
        source_set.as_deref(),
        explicit_output_root.as_deref(),
    )?;

    let mut items = Vec::new();
    for selected_source_set in selected {
        let source_path = resolve_source_set_path(config, selected_source_set);
        validate_selected_source(selected_source_set, direction, &source_path)?;

        let target_path = convert_output_path(
            config,
            selected_source_set,
            direction,
            explicit_output_root.as_deref(),
        )?;
        validate_convert_target(
            config,
            &target_path,
            &selected_source_set.name,
            explicit_output_root.is_some(),
        )?;
        let canonical_target_path =
            nearest_existing_canonical_path(&target_path).map_err(|error| {
                AppError::Runtime(format!(
                    "failed to canonicalize convert output '{}': {error}",
                    target_path.display()
                ))
            })?;
        if is_filesystem_root(&canonical_target_path) {
            return Err(AppError::Validation(
                "convert output target must not equal filesystem root".to_owned(),
            ));
        }
        let target_identity = stable_path_identity(&canonical_target_path);

        items.push(ResolvedConvertItem {
            source_set_name: selected_source_set.name.clone(),
            purpose: selected_source_set.purpose,
            source_path,
            target_path,
            canonical_target_path,
            target_identity,
            stable_project_dir_name: stable_project_dir_name(config, selected_source_set),
            import_options: infer_import_options(
                config,
                selected_source_set,
                direction,
                base_project_source.as_ref(),
            )?,
        });
    }
    validate_convert_targets_do_not_overlap(&items)?;

    Ok(ResolvedConvertRequest {
        direction,
        scope,
        source_set,
        workspace_path: convert_workspace_path(config),
        items,
    })
}

fn select_source_sets<'a>(
    config: &'a AppConfig,
    request: &ConvertRequest,
) -> Result<Vec<&'a SourceSetConfig>, AppError> {
    match &request.scope {
        ConvertScopeRequest::All => Ok(config.source_sets.iter().collect()),
        ConvertScopeRequest::SourceSet { name } => config
            .source_sets
            .iter()
            .find(|source_set| source_set.name == *name)
            .map(|source_set| vec![source_set])
            .ok_or_else(|| AppError::Validation(format!("unknown source-set '{name}'"))),
    }
}

fn resolve_base_project_source(
    config: &AppConfig,
    direction: ConvertDirection,
    selected: &[&SourceSetConfig],
    requested_source_set: Option<&str>,
    explicit_output_root: Option<&Path>,
) -> Result<Option<ConvertBaseProjectSource>, AppError> {
    if direction != ConvertDirection::DesignerToEdt
        || !selected
            .iter()
            .any(|source_set| source_set.purpose == SourceSetPurpose::Extension)
    {
        return Ok(None);
    }

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
        let selector = requested_source_set.unwrap_or("<all>");
        return Err(AppError::Validation(format!(
            "convert source-set '{selector}' requires exactly one configuration source-set to infer EDT base project name; found [{}]",
            candidates.join(", ")
        )));
    }

    let configuration_source_set = configuration_source_sets[0];
    let source_path = resolve_source_set_path(config, configuration_source_set);
    validate_selected_source(configuration_source_set, direction, &source_path)?;

    Ok(Some(ConvertBaseProjectSource {
        source_set_name: configuration_source_set.name.clone(),
        source_path,
        target_path: convert_output_path(
            config,
            configuration_source_set,
            direction,
            explicit_output_root,
        )?,
        stable_project_dir_name: stable_project_dir_name(config, configuration_source_set),
    }))
}

fn infer_import_options(
    config: &AppConfig,
    source_set: &SourceSetConfig,
    direction: ConvertDirection,
    base_project_source: Option<&ConvertBaseProjectSource>,
) -> Result<ConvertImportOptions, AppError> {
    if direction != ConvertDirection::DesignerToEdt {
        return Ok(ConvertImportOptions::default());
    }

    Ok(ConvertImportOptions {
        version: normalize_config_hint(config.tools.platform.version.as_deref()),
        base_project_name: match source_set.purpose {
            SourceSetPurpose::Configuration => None,
            SourceSetPurpose::Extension => None,
            _ => None,
        },
        base_project_source: match source_set.purpose {
            SourceSetPurpose::Extension => Some(
                base_project_source
                    .ok_or_else(|| {
                        AppError::Validation(format!(
                            "source-set '{}' requires inferred EDT base project name",
                            source_set.name
                        ))
                    })?
                    .clone(),
            ),
            _ => None,
        },
        build: false,
    })
}

fn normalize_config_hint(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn validate_selected_source(
    source_set: &SourceSetConfig,
    direction: ConvertDirection,
    path: &Path,
) -> Result<(), AppError> {
    validate_directory_path(path, "source-set path")?;

    match direction {
        ConvertDirection::EdtToDesigner => {
            if source_set.purpose.is_external() {
                discover_edt_external_projects(&source_set.name, source_set.purpose, path)
                    .map(|_| ())
            } else {
                validate_native_ordinary_edt_project(
                    &source_set.name,
                    source_set.purpose,
                    path,
                    "EDT source-set path",
                    None,
                )
            }
        }
        ConvertDirection::DesignerToEdt => {
            if source_set.purpose.is_external() {
                validate_designer_external_source(&source_set.name, source_set.purpose, path)
            } else {
                validate_designer_layout(path, "Designer source-set path")
            }
        }
    }
}

fn validate_convert_target(
    config: &AppConfig,
    target_path: &Path,
    source_set_name: &str,
    is_explicit_output: bool,
) -> Result<(), AppError> {
    let target = nearest_existing_canonical_path(target_path).map_err(|error| {
        AppError::Runtime(format!(
            "failed to canonicalize convert output '{}': {error}",
            target_path.display()
        ))
    })?;

    for source_set in &config.source_sets {
        let source_path = resolve_source_set_path(config, source_set);
        let source = nearest_existing_canonical_path(&source_path).map_err(|error| {
            AppError::Runtime(format!(
                "failed to canonicalize convert source-set '{}' path '{}': {error}",
                source_set.name,
                source_path.display()
            ))
        })?;
        if paths_overlap(&source, &target) {
            return Err(AppError::Validation(format!(
                "convert output for source-set '{source_set_name}' overlaps source-set '{}' path: source={}, target={}",
                source_set.name,
                source_path.display(),
                target_path.display()
            )));
        }
    }

    if is_explicit_output {
        validate_explicit_convert_target_roots(config, target_path, &target)?;
    }

    Ok(())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn validate_explicit_convert_target_roots(
    config: &AppConfig,
    target_path: &Path,
    canonical_target_path: &Path,
) -> Result<(), AppError> {
    let canonical_base_path =
        nearest_existing_canonical_path(&config.base_path).map_err(|error| {
            AppError::Runtime(format!("failed to canonicalize project base path: {error}"))
        })?;
    if canonical_target_path == canonical_base_path
        || canonical_target_path.starts_with(&canonical_base_path)
    {
        return Err(AppError::Validation(format!(
            "convert --output target must not be inside project base path: project_base_path={}, target={}",
            config.base_path.display(),
            target_path.display()
        )));
    }

    let canonical_work_path = nearest_existing_canonical_path(&config.work_path)
        .map_err(|error| AppError::Runtime(format!("failed to canonicalize workPath: {error}")))?;
    if canonical_target_path == canonical_work_path
        || canonical_target_path.starts_with(&canonical_work_path)
    {
        return Err(AppError::Validation(format!(
            "convert --output target must not be inside workPath: workPath={}, target={}",
            config.work_path.display(),
            target_path.display()
        )));
    }

    Ok(())
}

fn validate_directory_path(path: &Path, label: &str) -> Result<(), AppError> {
    if !path.exists() {
        return Err(AppError::Validation(format!(
            "convert {label} does not exist: {}",
            path.display()
        )));
    }
    if !path.is_dir() {
        return Err(AppError::Validation(format!(
            "convert {label} is not a directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_native_ordinary_edt_project(
    source_set_name: &str,
    purpose: SourceSetPurpose,
    path: &Path,
    label: &str,
    expected_base_project: Option<&str>,
) -> Result<(), AppError> {
    let expected_kind = match purpose {
        SourceSetPurpose::Configuration => EdtProjectKind::Configuration,
        SourceSetPurpose::Extension => EdtProjectKind::Extension,
        _ => {
            return Err(AppError::Validation(format!(
                "{label} for source-set '{source_set_name}' must resolve to an ordinary EDT project: {}",
                path.display()
            )));
        }
    };
    edt_project::validate_native_ordinary_project(path, expected_kind, expected_base_project)
        .map(|_| ())
        .map_err(|error| {
            AppError::Validation(format!(
                "{label} for source-set '{source_set_name}' is not a valid native EDT project: {error}"
            ))
        })
}

fn validate_designer_layout(path: &Path, label: &str) -> Result<(), AppError> {
    if path.join("Configuration.xml").exists() {
        return Ok(());
    }

    let has_top_level_xml = std::fs::read_dir(path)
        .map_err(|error| {
            AppError::Runtime(format!(
                "failed to inspect Designer files '{}': {error}",
                path.display()
            ))
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .any(|entry_path| {
            entry_path.is_file()
                && entry_path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("xml"))
        });

    if has_top_level_xml {
        Ok(())
    } else {
        Err(AppError::Validation(format!(
            "{label} must contain 'Configuration.xml' or a top-level XML descriptor: {}",
            path.display()
        )))
    }
}

fn validate_designer_external_source(
    source_set_name: &str,
    purpose: SourceSetPurpose,
    path: &Path,
) -> Result<(), AppError> {
    discover_designer_external_artifacts(
        source_set_name,
        path,
        external_artifact_kind(purpose).ok_or_else(|| {
            AppError::Validation(format!("source-set '{source_set_name}' is not external"))
        })?,
    )
    .map(|_| ())
}

fn discover_edt_external_projects(
    source_set_name: &str,
    purpose: SourceSetPurpose,
    path: &Path,
) -> Result<Vec<PathBuf>, AppError> {
    let expected_kind = external_artifact_kind(purpose).ok_or_else(|| {
        AppError::Validation(format!("source-set '{source_set_name}' is not external"))
    })?;
    let mut projects = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|error| {
        AppError::Runtime(format!(
            "failed to read EDT external source-set '{source_set_name}': {error}"
        ))
    })? {
        let entry = entry.map_err(|error| {
            AppError::Runtime(format!(
                "failed to read EDT external source-set entry for '{source_set_name}': {error}"
            ))
        })?;
        let file_type = entry.file_type().map_err(|error| {
            AppError::Runtime(format!(
                "failed to inspect EDT external source-set entry for '{source_set_name}': {error}"
            ))
        })?;
        if file_type.is_symlink() || !file_type.is_dir() {
            continue;
        }
        let child = entry.path();
        if !edt_project::project_descriptor_path(&child).is_file() {
            continue;
        }
        edt_project::validate_native_external_project(&child).map_err(|error| {
            AppError::Validation(format!(
                "external EDT child project '{}' in source-set '{}' is invalid: {error}",
                child.display(),
                source_set_name
            ))
        })?;
        let descriptor =
            parse_external_descriptor(&edt_project::external_root_descriptor_path(&child))
                .map_err(|error| match error {
                    AppError::Validation(message) => AppError::Validation(format!(
                        "external EDT child project '{}' in source-set '{}' is invalid: {message}",
                        child.display(),
                        source_set_name
                    )),
                    AppError::ValidationIbcmd(error) => AppError::ValidationIbcmd(error),
                    AppError::ValidationIbcmdContext { context, source } => {
                        AppError::ValidationIbcmdContext { context, source }
                    }
                    AppError::Runtime(message) => AppError::Runtime(message),
                    AppError::Platform(message) => AppError::Platform(message),
                    AppError::PlatformDesigner(error) => AppError::PlatformDesigner(error),
                    AppError::PlatformDesignerContext { context, source } => {
                        AppError::PlatformDesignerContext { context, source }
                    }
                    AppError::PlatformLocator(error) => AppError::PlatformLocator(error),
                    AppError::PlatformProcess(error) => AppError::PlatformProcess(error),
                    AppError::PlatformLocatorContext { context, source } => {
                        AppError::PlatformLocatorContext { context, source }
                    }
                    AppError::PlatformProcessContext { context, source } => {
                        AppError::PlatformProcessContext { context, source }
                    }
                    AppError::PlatformEdt(error) => AppError::PlatformEdt(error),
                    AppError::PlatformEdtContext { context, source } => {
                        AppError::PlatformEdtContext { context, source }
                    }
                    AppError::PlatformEdtSession(error) => AppError::PlatformEdtSession(error),
                    AppError::PlatformEdtSessionContext { context, source } => {
                        AppError::PlatformEdtSessionContext { context, source }
                    }
                    AppError::Config(error) => AppError::Config(error),
                    AppError::ConfigContext { context, source } => {
                        AppError::ConfigContext { context, source }
                    }
                })?;
        if descriptor.artifact_type != expected_kind {
            return Err(AppError::Validation(format!(
                "external EDT child project '{}' in source-set '{}' resolves to {}, expected {}",
                child.display(),
                source_set_name,
                descriptor.artifact_type.root_tag(),
                expected_kind.root_tag()
            )));
        }
        projects.push(child);
    }

    if projects.is_empty() {
        return Err(AppError::Validation(format!(
            "external EDT source-set '{source_set_name}' must contain at least one valid native EDT child project"
        )));
    }

    Ok(projects)
}

fn merge_directory_contents(source: &Path, target: &Path) -> Result<(), AppError> {
    for entry in std::fs::read_dir(source).map_err(|error| {
        AppError::Runtime(format!(
            "failed to read convert export directory '{}': {error}",
            source.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            AppError::Runtime(format!(
                "failed to read convert export entry in '{}': {error}",
                source.display()
            ))
        })?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if target_path.exists() {
            return Err(AppError::Validation(format!(
                "convert external export produced duplicate path '{}'",
                target_path.display()
            )));
        }
        if source_path.is_dir() {
            copy_directory_recursively(&source_path, &target_path)?;
        } else {
            std::fs::copy(&source_path, &target_path).map_err(|error| {
                AppError::Runtime(format!(
                    "failed to copy convert external export '{}' to '{}': {error}",
                    source_path.display(),
                    target_path.display()
                ))
            })?;
        }
    }

    Ok(())
}

fn copy_directory_recursively(source: &Path, target: &Path) -> Result<(), AppError> {
    ensure_dir(target).map_err(|error| {
        AppError::Runtime(format!(
            "failed to create convert external target directory '{}': {error}",
            target.display()
        ))
    })?;
    for entry in std::fs::read_dir(source).map_err(|error| {
        AppError::Runtime(format!(
            "failed to read convert external source directory '{}': {error}",
            source.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            AppError::Runtime(format!(
                "failed to read convert external source entry in '{}': {error}",
                source.display()
            ))
        })?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if source_path.is_dir() {
            copy_directory_recursively(&source_path, &target_path)?;
        } else {
            std::fs::copy(&source_path, &target_path).map_err(|error| {
                AppError::Runtime(format!(
                    "failed to copy convert external file '{}' to '{}': {error}",
                    source_path.display(),
                    target_path.display()
                ))
            })?;
        }
    }

    Ok(())
}

fn resolve_runtime_import_options(
    dsl: &EdtDsl<'_>,
    item: &ResolvedConvertItem,
    processed_source_sets: &HashSet<String>,
    base_project_names: &mut HashMap<String, String>,
) -> Result<ConvertImportOptions, AppError> {
    let mut options = item.import_options.clone();
    let Some(base_project_source) = options.base_project_source.clone() else {
        return Ok(options);
    };

    if let Some(name) = base_project_names.get(&base_project_source.source_set_name) {
        options.base_project_name = Some(name.clone());
        return Ok(options);
    }

    let project_name = if processed_source_sets.contains(&base_project_source.source_set_name) {
        read_edt_project_name(
            &base_project_source.target_path,
            &format!(
                "convert output for source-set '{}'",
                base_project_source.source_set_name
            ),
        )?
    } else {
        infer_runtime_base_project_name(dsl, &base_project_source, options.version.as_deref())?
    };
    base_project_names.insert(
        base_project_source.source_set_name.clone(),
        project_name.clone(),
    );
    options.base_project_name = Some(project_name);
    Ok(options)
}

fn infer_runtime_base_project_name(
    dsl: &EdtDsl<'_>,
    base_project_source: &ConvertBaseProjectSource,
    version: Option<&str>,
) -> Result<String, AppError> {
    let temp_parent = base_project_source.target_path.parent().ok_or_else(|| {
        AppError::Validation(format!(
            "convert output path has no parent: {}",
            base_project_source.target_path.display()
        ))
    })?;
    ensure_dir(temp_parent).map_err(|error| {
        AppError::Runtime(format!(
            "failed to create temporary base project parent '{}': {error}",
            temp_parent.display()
        ))
    })?;

    let temp_root = temp_parent.join(format!(".convert-base-project-{}", make_run_id()));
    remove_path_if_exists(&temp_root).map_err(|error| {
        AppError::Runtime(format!(
            "failed to clean temporary base project directory '{}': {error}",
            temp_root.display()
        ))
    })?;
    ensure_dir(&temp_root).map_err(|error| {
        AppError::Runtime(format!(
            "failed to create temporary base project directory '{}': {error}",
            temp_root.display()
        ))
    })?;
    let temp_project_dir = temp_root.join(&base_project_source.stable_project_dir_name);
    ensure_dir(&temp_project_dir).map_err(|error| {
        AppError::Runtime(format!(
            "failed to create temporary base project directory '{}': {error}",
            temp_project_dir.display()
        ))
    })?;

    log_live_stage(
        "convert: base project import",
        "[EDT] importing Designer files for base project name",
    );
    let import_result = dsl
        .import_configuration_files(
            &temp_project_dir,
            &base_project_source.source_path,
            version,
            None,
            false,
        )
        .map_err(AppError::from);
    let outcome = (|| {
        let result = import_result?;
        ensure_platform_success(
            &base_project_source.source_set_name,
            "designer-to-edt-base-project",
            &result,
        )?;
        read_edt_project_name(
            &temp_project_dir,
            &format!(
                "temporary EDT import for source-set '{}'",
                base_project_source.source_set_name
            ),
        )
    })();
    let cleanup_result = remove_path_if_exists(&temp_root).map_err(|error| {
        AppError::Runtime(format!(
            "failed to cleanup temporary base project directory '{}': {error}",
            temp_root.display()
        ))
    });

    match (outcome, cleanup_result) {
        (Ok(project_name), Ok(())) => Ok(project_name),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

fn read_edt_project_name(path: &Path, label: &str) -> Result<String, AppError> {
    edt_project::read_project_name_from_dir(path).map_err(|error| {
        AppError::Validation(format!(
            "{label} must contain a valid EDT project name in '.project': {error}"
        ))
    })
}

fn run_platform_conversion(
    dsl: &EdtDsl<'_>,
    direction: ConvertDirection,
    item: &ResolvedConvertItem,
    import_options: &ConvertImportOptions,
    staging_dir: &Path,
) -> Result<(), AppError> {
    match direction {
        ConvertDirection::EdtToDesigner => {
            if item.purpose.is_external() {
                let project_paths = discover_edt_external_projects(
                    &item.source_set_name,
                    item.purpose,
                    &item.source_path,
                )?;
                let expected_kind = external_artifact_kind(item.purpose).ok_or_else(|| {
                    AppError::Validation(format!(
                        "source-set '{}' is not external",
                        item.source_set_name
                    ))
                })?;
                for (index, project_path) in project_paths.iter().enumerate() {
                    let export_target = staging_dir.join(format!(".external-export-{index}"));
                    remove_path_if_exists(&export_target).map_err(|error| {
                        AppError::Runtime(format!(
                            "failed to clean convert external export target '{}': {error}",
                            export_target.display()
                        ))
                    })?;
                    ensure_dir(&export_target).map_err(|error| {
                        AppError::Runtime(format!(
                            "failed to create convert external export target '{}': {error}",
                            export_target.display()
                        ))
                    })?;
                    log_live_stage(
                        "convert: edt export",
                        "[EDT] exporting external project to Designer files",
                    );
                    let result = dsl
                        .export_project_path(project_path, &export_target)
                        .map_err(AppError::from)?;
                    ensure_platform_success(&item.source_set_name, "edt-to-designer", &result)?;
                    let discovered = discover_designer_external_artifacts(
                        &item.source_set_name,
                        &export_target,
                        expected_kind,
                    )?;
                    if discovered.len() != 1 {
                        return Err(AppError::Validation(format!(
                            "EDT export for source-set '{}' must produce exactly one external descriptor per project",
                            item.source_set_name
                        )));
                    }
                    merge_directory_contents(&export_target, staging_dir)?;
                    remove_path_if_exists(&export_target).map_err(|error| {
                        AppError::Runtime(format!(
                            "failed to cleanup convert external export target '{}': {error}",
                            export_target.display()
                        ))
                    })?;
                }
                let exported = discover_designer_external_artifacts(
                    &item.source_set_name,
                    staging_dir,
                    expected_kind,
                )?;
                if exported.len() != project_paths.len() {
                    return Err(AppError::Validation(format!(
                        "convert source-set '{}' exported {} external descriptors for {} EDT projects",
                        item.source_set_name,
                        exported.len(),
                        project_paths.len()
                    )));
                }
                Ok(())
            } else {
                log_live_stage(
                    "convert: edt export",
                    "[EDT] exporting project to Designer files",
                );
                let result = dsl
                    .export_project_path(&item.source_path, staging_dir)
                    .map_err(AppError::from)?;
                ensure_platform_success(&item.source_set_name, "edt-to-designer", &result)
            }
        }
        ConvertDirection::DesignerToEdt => {
            log_live_stage("convert: designer import", "[EDT] importing Designer files");
            let result = dsl
                .import_configuration_files(
                    staging_dir,
                    &item.source_path,
                    import_options.version.as_deref(),
                    import_options.base_project_name.as_deref(),
                    import_options.build,
                )
                .map_err(AppError::from)?;
            ensure_platform_success(&item.source_set_name, "designer-to-edt", &result)
        }
    }
}

fn ensure_platform_success(
    source_set_name: &str,
    direction_label: &str,
    result: &PlatformCommandResult,
) -> Result<(), AppError> {
    if result.process.exit_code == 0 {
        return Ok(());
    }

    let mut details = vec![format!(
        "convert source-set '{source_set_name}' ({direction_label}) failed with exit code {}",
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
        .filter(|value| !value.trim().is_empty())
    {
        details.push(format!("platform log: {}", log.trim()));
    }
    if let Some(path) = result.platform_log_path.as_ref() {
        details.push(format!("platform log path: {}", path.display()));
    }

    Err(AppError::Platform(details.join("; ")))
}

fn validate_staging_output(
    direction: ConvertDirection,
    item: &ResolvedConvertItem,
    import_options: &ConvertImportOptions,
    staging_dir: &Path,
) -> Result<(), AppError> {
    match direction {
        ConvertDirection::EdtToDesigner => {
            if item.purpose.is_external() {
                validate_designer_external_source(&item.source_set_name, item.purpose, staging_dir)
            } else {
                validate_designer_layout(staging_dir, "Designer convert output")
            }
        }
        ConvertDirection::DesignerToEdt => {
            if item.purpose.is_external() {
                discover_edt_external_projects(&item.source_set_name, item.purpose, staging_dir)
                    .map(|_| ())
            } else {
                validate_native_ordinary_edt_project(
                    &item.source_set_name,
                    item.purpose,
                    staging_dir,
                    "EDT convert output",
                    import_options.base_project_name.as_deref(),
                )
            }
        }
    }
}

fn result_snapshot(
    ok: bool,
    direction: ConvertDirection,
    scope: ConvertScope,
    source_set: Option<String>,
    workspace_path: PathBuf,
    outputs: Vec<ConvertOutput>,
    started: Instant,
    message: Option<String>,
) -> ConvertResult {
    ConvertResult {
        ok,
        direction,
        scope,
        source_set,
        workspace_path,
        outputs,
        duration_ms: started.elapsed().as_millis() as u64,
        message,
    }
}

fn convert_workspace_path(config: &AppConfig) -> PathBuf {
    config.work_path.join("convert").join("edt-workspace")
}

fn convert_session_host_options(
    config: &AppConfig,
    workspace_path: &Path,
) -> EdtSessionHostOptions {
    let mut options = EdtSessionHostOptions::for_cli_command(config);
    options.workspace = workspace_path.to_path_buf();
    options
}

fn explicit_output_root(request: &ConvertRequest) -> Result<Option<PathBuf>, AppError> {
    let Some(output_root) = request.output_root.as_deref() else {
        return Ok(None);
    };
    let output_root = output_root.trim();
    if output_root.is_empty() {
        return Err(AppError::Validation(
            "convert requires non-empty --output".to_owned(),
        ));
    }
    let path = PathBuf::from(output_root);
    if is_filesystem_root(&path) {
        return Err(AppError::Validation(
            "convert --output must not equal filesystem root".to_owned(),
        ));
    }
    Ok(Some(path))
}

fn convert_output_path(
    config: &AppConfig,
    source_set: &SourceSetConfig,
    direction: ConvertDirection,
    explicit_output_root: Option<&Path>,
) -> Result<PathBuf, AppError> {
    if let Some(output_root) = explicit_output_root {
        return Ok(output_root.join(source_set_output_relative_path(config, source_set)?));
    }

    Ok(config
        .work_path
        .join("convert")
        .join("out")
        .join(&source_set.name)
        .join(match direction {
            ConvertDirection::EdtToDesigner => "designer",
            ConvertDirection::DesignerToEdt => "edt",
        }))
}

fn resolve_source_set_path(config: &AppConfig, source_set: &SourceSetConfig) -> PathBuf {
    if source_set.path.is_absolute() {
        source_set.path.clone()
    } else {
        config.base_path.join(&source_set.path)
    }
}

fn source_set_output_relative_path(
    config: &AppConfig,
    source_set: &SourceSetConfig,
) -> Result<PathBuf, AppError> {
    let raw_path = source_set.path.as_path();
    let relative = if raw_path.is_absolute() {
        raw_path.strip_prefix(&config.base_path).map_err(|_| {
            AppError::Validation(format!(
                "convert --output requires source-set '{}' path to be relative to project base path",
                source_set.name
            ))
        })?
    } else {
        raw_path
    };

    normalize_relative_source_set_path(&source_set.name, relative)
}

fn normalize_relative_source_set_path(
    source_set_name: &str,
    relative: &Path,
) -> Result<PathBuf, AppError> {
    let mut normalized = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AppError::Validation(format!(
                    "convert --output cannot mirror unsafe source-set '{}' path '{}'",
                    source_set_name,
                    relative.display()
                )));
            }
        }
    }
    Ok(normalized)
}

fn stable_project_dir_name(config: &AppConfig, source_set: &SourceSetConfig) -> String {
    let logical_path = source_set_output_relative_path(config, source_set).unwrap_or_else(|_| {
        source_set
            .path
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_default()
    });
    logical_path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| source_set.name.clone())
}

fn validate_convert_targets_do_not_overlap(items: &[ResolvedConvertItem]) -> Result<(), AppError> {
    for (index, left) in items.iter().enumerate() {
        for right in items.iter().skip(index + 1) {
            if paths_overlap(&left.canonical_target_path, &right.canonical_target_path) {
                return Err(AppError::Validation(format!(
                    "convert output targets overlap: source-set '{}' -> {}, source-set '{}' -> {}",
                    left.source_set_name,
                    left.target_path.display(),
                    right.source_set_name,
                    right.target_path.display()
                )));
            }
        }
    }
    Ok(())
}

fn staging_publication_dir(
    direction: ConvertDirection,
    item: &ResolvedConvertItem,
    staging_root: &Path,
) -> PathBuf {
    match direction {
        ConvertDirection::EdtToDesigner => staging_root.to_path_buf(),
        ConvertDirection::DesignerToEdt => staging_root.join(&item.stable_project_dir_name),
    }
}

fn direction_from_format(format: SourceFormat) -> ConvertDirection {
    match format {
        SourceFormat::Edt => ConvertDirection::EdtToDesigner,
        SourceFormat::Designer => ConvertDirection::DesignerToEdt,
    }
}

fn scope_from_request(request: &ConvertRequest) -> ConvertScope {
    match request.scope {
        ConvertScopeRequest::All => ConvertScope::All,
        ConvertScopeRequest::SourceSet { .. } => ConvertScope::Single,
    }
}

fn source_set_from_request(request: &ConvertRequest) -> Option<String> {
    match &request.scope {
        ConvertScopeRequest::All => None,
        ConvertScopeRequest::SourceSet { name } => Some(name.clone()),
    }
}

fn external_artifact_kind(purpose: SourceSetPurpose) -> Option<ExternalArtifactKind> {
    match purpose {
        SourceSetPurpose::ExternalDataProcessors => Some(ExternalArtifactKind::DataProcessor),
        SourceSetPurpose::ExternalReports => Some(ExternalArtifactKind::Report),
        _ => None,
    }
}

fn deferred_interruption_warning(
    interruption: Option<crate::use_cases::context::ExecutionInterruption>,
) -> Option<String> {
    interruption.map(|interruption| {
        interruption::deferred_interruption_warning(
            "convert publication completed successfully",
            interruption,
        )
    })
}

fn merge_messages(messages: Vec<String>) -> Option<String> {
    if messages.is_empty() {
        None
    } else {
        Some(messages.join("; "))
    }
}

fn make_run_id() -> String {
    let timestamp = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    format!("{}-{timestamp:x}", std::process::id())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::config::model::{
        AppConfig, BuilderBackend, InfobaseConfig, McpConfig, SourceFormat, TestsConfig,
        ToolsConfig,
    };

    use super::{convert_session_host_options, convert_workspace_path};

    fn sample_config() -> AppConfig {
        AppConfig {
            base_path: PathBuf::from("/tmp/project"),
            work_path: PathBuf::from("/tmp/work"),
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: InfobaseConfig::file("File=/tmp/ib"),
            source_sets: vec![],
            build: Default::default(),
            tools: ToolsConfig::default(),
            mcp: McpConfig::default(),
            tests: TestsConfig::default(),
        }
    }

    #[test]
    fn interactive_convert_uses_dedicated_workspace_host_options() {
        let config = sample_config();
        let workspace = convert_workspace_path(&config);

        let options = convert_session_host_options(&config, &workspace);

        assert_eq!(
            options.workspace,
            PathBuf::from("/tmp/work/convert/edt-workspace")
        );
    }
}
