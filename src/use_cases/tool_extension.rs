use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::change_detection::analyzer::{self, AnalysisOutcome};
use crate::change_detection::hash_storage::{HashStorage, StorageError};
use crate::config::model::{
    AppConfig, BuilderBackend, SourceFormat, ToolExtensionConfig, ToolExtensionInput,
    ToolExtensionSourceConfig,
};
use crate::domain::build::{BuildMode, BuildStep};
use crate::domain::source_set::SourceSetContext;
use crate::platform::designer::DesignerDsl;
use crate::platform::edt::EdtDsl;
use crate::platform::edt_session::{EdtSessionHostOptions, EdtSessionManager};
use crate::platform::ibcmd::{DynamicUpdateMode, IbcmdConnection, IbcmdDsl};
use crate::platform::locator::UtilityType;
use crate::platform::process::ProcessRunner;
use crate::platform::result::PlatformCommandResult;
use crate::platform::utilities::PlatformUtilities;
use crate::support::edt_project;
use crate::support::error::AppError;
use crate::support::temp::platform_logs_dir;
use crate::use_cases::build_progress::{log_timeline_stage, TimelineStageStatus};
use crate::use_cases::context::{ExecutionContext, InterruptionSafetyClass};
use crate::use_cases::ibcmd_diagnostics::format_ibcmd_failure_details;
use crate::use_cases::interruption;

#[derive(Debug)]
pub(crate) struct ToolExtensionFailure {
    pub(crate) step: BuildStep,
    pub(crate) error: AppError,
}

pub(crate) fn client_mcp_extension(config: &AppConfig) -> Option<&ToolExtensionConfig> {
    config.tools.client_mcp.extension.as_ref()
}

pub(crate) fn client_mcp_build_hint(config: &AppConfig) -> Option<&'static str> {
    client_mcp_extension(config).map(|_| {
        "client MCP extension is configured; run `v8-runner build` before launch when the extension is missing or stale"
    })
}

pub(crate) fn client_mcp_edt_source_path(config: &AppConfig) -> Option<PathBuf> {
    let extension = client_mcp_extension(config)?;
    let source = extension.source()?;
    (source.format.unwrap_or(config.format) == SourceFormat::Edt).then(|| source.path.clone())
}

pub(crate) fn prepare_client_mcp_extension(
    context: &ExecutionContext,
    config: &AppConfig,
    full_rebuild: bool,
) -> Result<Option<BuildStep>, ToolExtensionFailure> {
    let Some(extension) = client_mcp_extension(config) else {
        return Ok(None);
    };
    prepare_extension(context, config, extension, full_rebuild)
        .map(Some)
        .map_err(|error| {
            let message = error.to_string();
            ToolExtensionFailure {
                step: failed_build_step(extension, message),
                error,
            }
        })
}

fn prepare_extension(
    context: &ExecutionContext,
    config: &AppConfig,
    extension: &ToolExtensionConfig,
    full_rebuild: bool,
) -> Result<BuildStep, AppError> {
    let started = Instant::now();

    if let Some(error) = interruption_before_safe_point(context, extension, "tool extension load") {
        return Err(error);
    }

    let mut utilities = PlatformUtilities::from_config(config);
    match &extension.input {
        ToolExtensionInput::Source(source) => prepare_source_extension(
            context,
            config,
            extension,
            source,
            &mut utilities,
            started,
            full_rebuild,
        ),
        ToolExtensionInput::Artifact(artifact) => {
            prepare_artifact_extension(context, config, extension, &artifact.path, &mut utilities)?;
            Ok(successful_build_step(
                extension,
                format!("prepared extension '{}' from .cfe artifact", extension.name),
                started.elapsed().as_millis() as u64,
            ))
        }
    }
}

fn prepare_source_extension(
    context: &ExecutionContext,
    config: &AppConfig,
    extension: &ToolExtensionConfig,
    source: &ToolExtensionSourceConfig,
    utilities: &mut PlatformUtilities,
    started: Instant,
    full_rebuild: bool,
) -> Result<BuildStep, AppError> {
    let source_context = tool_extension_source_context(config, extension, source)?;
    if full_rebuild {
        prepare_source_extension_full(context, config, extension, source, utilities)?;
        commit_tool_extension_full_rescan(&source_context, &config.work_path, true)?;
        return Ok(successful_build_step(
            extension,
            format!("prepared extension '{}' from sources", extension.name),
            started.elapsed().as_millis() as u64,
        ));
    }

    let outcome = match analyzer::analyze_context(&source_context, &config.work_path).outcome {
        Ok(outcome) => outcome,
        Err(error) if storage_needs_recovery(&source_context, &config.work_path) => {
            prepare_source_extension_full(context, config, extension, source, utilities)?;
            commit_tool_extension_full_rescan(&source_context, &config.work_path, true)?;
            return Ok(successful_build_step(
                extension,
                format!("prepared extension '{}' from sources", extension.name),
                started.elapsed().as_millis() as u64,
            ));
        }
        Err(error) => return Err(AppError::Runtime(error.to_string())),
    };

    match outcome {
        AnalysisOutcome::NoChanges => Ok(skipped_build_step(
            extension,
            "no changes".to_owned(),
            started.elapsed().as_millis() as u64,
        )),
        AnalysisOutcome::Fallback => {
            prepare_source_extension_full(context, config, extension, source, utilities)?;
            commit_tool_extension_full_rescan(&source_context, &config.work_path, false)?;
            Ok(successful_build_step(
                extension,
                format!("prepared extension '{}' from sources", extension.name),
                started.elapsed().as_millis() as u64,
            ))
        }
        AnalysisOutcome::Changes { prepared, .. } => {
            prepare_source_extension_full(context, config, extension, source, utilities)?;
            analyzer::commit_success(&source_context, &config.work_path, &prepared)
                .map_err(|error| AppError::Runtime(error.to_string()))?;
            Ok(successful_build_step(
                extension,
                format!("prepared extension '{}' from sources", extension.name),
                started.elapsed().as_millis() as u64,
            ))
        }
    }
}

fn prepare_source_extension_full(
    context: &ExecutionContext,
    config: &AppConfig,
    extension: &ToolExtensionConfig,
    source: &ToolExtensionSourceConfig,
    utilities: &mut PlatformUtilities,
) -> Result<(), AppError> {
    match source.format.unwrap_or(config.format) {
        SourceFormat::Designer => {
            prepare_designer_source_extension(context, config, extension, &source.path, utilities)
        }
        SourceFormat::Edt => {
            let exported =
                export_edt_source_extension(context, config, extension, &source.path, utilities)?;
            prepare_designer_source_extension(context, config, extension, &exported, utilities)
        }
    }
}

fn tool_extension_source_context(
    config: &AppConfig,
    extension: &ToolExtensionConfig,
    source: &ToolExtensionSourceConfig,
) -> Result<SourceSetContext, AppError> {
    let base_path = absolutize_path(&config.base_path)?;
    let source_path = if source.path.is_absolute() {
        source.path.clone()
    } else {
        base_path.join(&source.path)
    };
    Ok(SourceSetContext::new(
        format!("tool:{}", extension.name),
        source_path,
        format!("tool-{}-source", extension.name),
    ))
}

fn absolutize_path(path: &Path) -> Result<PathBuf, AppError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    std::env::current_dir()
        .map(|current_dir| current_dir.join(path))
        .map_err(|error| AppError::Runtime(format!("failed to resolve current directory: {error}")))
}

fn log_tool_extension_stage(extension: &ToolExtensionConfig, stage: &str, detail: &str) {
    log_timeline_stage(
        &format!("tool:{}", extension.name),
        stage,
        detail,
        TimelineStageStatus::Running,
    );
}

fn extension_stage_detail(executor: &str, action: &str, extension: &ToolExtensionConfig) -> String {
    format!("[{executor}] {action} расширения {}", extension.name)
}

fn commit_tool_extension_full_rescan(
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

fn prepare_designer_source_extension(
    context: &ExecutionContext,
    config: &AppConfig,
    extension: &ToolExtensionConfig,
    source_path: &Path,
    utilities: &mut PlatformUtilities,
) -> Result<(), AppError> {
    match config.builder {
        BuilderBackend::Designer => {
            let binary = utilities
                .locate(UtilityType::V8)
                .map_err(AppError::from)?
                .path;
            let dsl = build_designer_dsl(
                context,
                config,
                &binary,
                utilities.runner_for(UtilityType::V8),
                extension,
                "load",
            )?;
            log_tool_extension_stage(
                extension,
                "load",
                &extension_stage_detail("Конфигуратор", "Загрузка", extension),
            );
            let load = dsl
                .load_config_from_files_full(source_path, Some(&extension.name))
                .map_err(AppError::from)?;
            ensure_tool_extension_success("load", extension, &load)?;

            if let Some(error) =
                interruption_before_safe_point(context, extension, "tool extension update_db_cfg")
            {
                return Err(error);
            }
            let dsl = build_designer_dsl(
                context,
                config,
                &binary,
                utilities.runner_for(UtilityType::V8),
                extension,
                "update",
            )?;
            log_tool_extension_stage(
                extension,
                "update",
                &extension_stage_detail("Конфигуратор", "Применение", extension),
            );
            // Tool-extension flow keeps the historical static update; dynamic mode is opt-in
            // through `build --dynamic` / `build.dynamicUpdate` and not propagated here.
            let update = dsl
                .update_db_cfg(Some(&extension.name), false)
                .map_err(AppError::from)?;
            ensure_tool_extension_success("update_db_cfg", extension, &update)
        }
        BuilderBackend::Ibcmd => {
            let binary = utilities
                .locate(UtilityType::Ibcmd)
                .map_err(AppError::from)?
                .path;
            let dsl = build_ibcmd_dsl(
                context,
                config,
                &binary,
                utilities.runner_for(UtilityType::Ibcmd),
            )?;
            log_tool_extension_stage(
                extension,
                "ibcmd_import",
                &extension_stage_detail("ibcmd", "Загрузка", extension),
            );
            let import = dsl
                .config_import_full(source_path, Some(&extension.name))
                .map_err(AppError::from)?;
            ensure_tool_extension_success("ibcmd_import", extension, &import)?;

            if let Some(error) =
                interruption_before_safe_point(context, extension, "tool extension ibcmd apply")
            {
                return Err(error);
            }
            let dsl = build_ibcmd_dsl(
                context,
                config,
                &binary,
                utilities.runner_for(UtilityType::Ibcmd),
            )?;
            log_tool_extension_stage(
                extension,
                "ibcmd_apply",
                &extension_stage_detail("ibcmd", "Применение", extension),
            );
            let apply = dsl
                .config_apply(Some(&extension.name), DynamicUpdateMode::Auto)
                .map_err(AppError::from)?;
            ensure_tool_extension_success("ibcmd_apply", extension, &apply)
        }
    }
}

fn prepare_artifact_extension(
    context: &ExecutionContext,
    config: &AppConfig,
    extension: &ToolExtensionConfig,
    artifact_path: &Path,
    utilities: &mut PlatformUtilities,
) -> Result<(), AppError> {
    let binary = utilities
        .locate(UtilityType::V8)
        .map_err(AppError::from)?
        .path;
    let dsl = build_designer_dsl(
        context,
        config,
        &binary,
        utilities.runner_for(UtilityType::V8),
        extension,
        "load-artifact",
    )?;
    log_tool_extension_stage(
        extension,
        "load_artifact",
        &extension_stage_detail("Конфигуратор", "Загрузка .cfe", extension),
    );
    let load = dsl
        .load_cfg(artifact_path, Some(&extension.name))
        .map_err(AppError::from)?;
    ensure_tool_extension_success("load_cfg", extension, &load)?;

    if let Some(error) =
        interruption_before_safe_point(context, extension, "tool extension update_db_cfg")
    {
        return Err(error);
    }
    let dsl = build_designer_dsl(
        context,
        config,
        &binary,
        utilities.runner_for(UtilityType::V8),
        extension,
        "update-artifact",
    )?;
    // Tool-extension artifact apply path uses static update; see tool_extension::execute().
    let update = dsl
        .update_db_cfg(Some(&extension.name), false)
        .map_err(AppError::from)?;
    ensure_tool_extension_success("update_db_cfg", extension, &update)
}

fn export_edt_source_extension(
    context: &ExecutionContext,
    config: &AppConfig,
    extension: &ToolExtensionConfig,
    source_path: &Path,
    utilities: &mut PlatformUtilities,
) -> Result<PathBuf, AppError> {
    if let Some(error) =
        interruption_before_safe_point(context, extension, "tool extension EDT export")
    {
        return Err(error);
    }
    let target = config
        .work_path
        .join("designer")
        .join("tool-extensions")
        .join(&extension.name);
    recreate_directory(&target).map_err(|error| {
        AppError::Runtime(format!(
            "failed to prepare tool extension export directory '{}': {error}",
            target.display()
        ))
    })?;

    let binary = utilities
        .locate(UtilityType::EdtCli)
        .map_err(AppError::from)?
        .path;
    let dsl = build_edt_dsl(context, config, &binary, utilities)?;
    let project_name = resolve_edt_project_name(extension, source_path)?;
    log_tool_extension_stage(
        extension,
        "edt_export",
        &extension_stage_detail("EDT", "Экспорт", extension),
    );
    let result = dsl
        .export_project(&project_name, &target)
        .map_err(AppError::from)?;
    let log_path = write_edt_export_log(config, extension, &project_name, &target, &result)?;
    ensure_tool_extension_success("edt_export", extension, &result).map_err(
        |error| match error {
            AppError::Platform(message) => AppError::Platform(format!(
                "{message}; edt export log path: {}",
                log_path.display()
            )),
            other => other,
        },
    )?;
    let expected = target.join("Configuration.xml");
    if !expected.is_file() {
        return Err(AppError::Platform(format!(
            "EDT export for tool extension '{}' completed but did not produce '{}'; edt export log path: {}",
            extension.name,
            expected.display(),
            log_path.display()
        )));
    }
    Ok(target)
}

fn build_designer_dsl<'a>(
    context: &ExecutionContext,
    config: &AppConfig,
    binary: &Path,
    runner: &'a dyn ProcessRunner,
    extension: &ToolExtensionConfig,
    action: &str,
) -> Result<DesignerDsl<'a>, AppError> {
    let log_dir = platform_logs_dir(&config.work_path).map_err(|error| {
        AppError::Runtime(format!("failed to create platform logs dir: {error}"))
    })?;
    let log_file = log_dir.join(format!("build-tool-{}-{action}.log", extension.name));

    Ok(DesignerDsl::new(
        binary.to_path_buf(),
        config.v8_connection(),
        runner,
        Some(log_file),
    )
    .with_execution_policy(
        context.process_policy(InterruptionSafetyClass::CriticalNonAbortable, None),
    ))
}

fn build_ibcmd_dsl<'a>(
    context: &ExecutionContext,
    config: &AppConfig,
    binary: &Path,
    runner: &'a dyn ProcessRunner,
) -> Result<IbcmdDsl<'a>, AppError> {
    let connection = IbcmdConnection::from_infobase(&config.infobase).map_err(AppError::from)?;

    Ok(
        IbcmdDsl::new(binary.to_path_buf(), connection, runner).with_execution_policy(
            context.process_policy(InterruptionSafetyClass::CriticalNonAbortable, None),
        ),
    )
}

fn build_edt_dsl<'a>(
    context: &ExecutionContext,
    config: &AppConfig,
    binary: &Path,
    utilities: &'a mut PlatformUtilities,
) -> Result<EdtDsl<'a>, AppError> {
    let workspace = config.work_path.join("edt-workspace");
    let dsl = if config.tools.edt_cli.interactive_mode {
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
        .map_err(AppError::from)?
    } else {
        EdtDsl::new(
            binary.to_path_buf(),
            workspace,
            utilities.runner_for(UtilityType::EdtCli),
        )
    };
    Ok(dsl.with_execution_policy(
        context.process_policy(InterruptionSafetyClass::GracefulThenKill, None),
    ))
}

fn resolve_edt_project_name(
    extension: &ToolExtensionConfig,
    source_path: &Path,
) -> Result<String, AppError> {
    edt_project::read_project_descriptor_from_dir(source_path)
        .map_err(AppError::Validation)?
        .map(|project| project.name)
        .ok_or_else(|| {
            AppError::Validation(format!(
                "tool extension '{}' source must contain a valid '.project' with projectDescription/name: {}",
                extension.name,
                source_path.display()
            ))
        })
}

fn write_edt_export_log(
    config: &AppConfig,
    extension: &ToolExtensionConfig,
    project_name: &str,
    export_target: &Path,
    result: &PlatformCommandResult,
) -> Result<PathBuf, AppError> {
    let log_dir = platform_logs_dir(&config.work_path).map_err(|error| {
        AppError::Runtime(format!("failed to create platform logs dir: {error}"))
    })?;
    let log_path = log_dir.join(format!("build-tool-{}-edt-export.log", extension.name));
    let contents = format!(
        "action: tool_extension_edt_export\nextension: {}\nproject-name: {project_name}\nexport-target: {}\nexit-code: {}\nstdout:\n{}\nstderr:\n{}\n",
        extension.name,
        export_target.display(),
        result.process.exit_code,
        result.process.stdout,
        result.process.stderr
    );
    std::fs::write(&log_path, contents).map_err(|error| {
        AppError::Runtime(format!(
            "failed to write EDT export log '{}': {error}",
            log_path.display()
        ))
    })?;
    Ok(log_path)
}

fn ensure_tool_extension_success(
    action: &str,
    extension: &ToolExtensionConfig,
    result: &PlatformCommandResult,
) -> Result<(), AppError> {
    if result.process.exit_code == 0 {
        return Ok(());
    }

    Err(AppError::Platform(format_ibcmd_failure_details(
        action,
        "tool extension",
        &extension.name,
        result.process.exit_code,
        &result.process.stdout,
        &result.process.stderr,
        result.platform_log.as_deref(),
        result.platform_log_path.as_deref(),
    )))
}

fn interruption_before_safe_point(
    context: &ExecutionContext,
    extension: &ToolExtensionConfig,
    safe_point: &str,
) -> Option<AppError> {
    interruption::interruption_before_safe_point(
        context,
        format!("{safe_point} for tool extension '{}'", extension.name),
    )
}

fn successful_build_step(
    extension: &ToolExtensionConfig,
    message: String,
    duration_ms: u64,
) -> BuildStep {
    BuildStep {
        source_set: format!("tool:{}", extension.name),
        mode: BuildMode::Full,
        ok: true,
        message: Some(message),
        duration_ms,
    }
}

fn skipped_build_step(
    extension: &ToolExtensionConfig,
    message: String,
    duration_ms: u64,
) -> BuildStep {
    BuildStep {
        source_set: format!("tool:{}", extension.name),
        mode: BuildMode::Skipped,
        ok: true,
        message: Some(message),
        duration_ms,
    }
}

fn failed_build_step(extension: &ToolExtensionConfig, message: String) -> BuildStep {
    BuildStep {
        source_set: format!("tool:{}", extension.name),
        mode: BuildMode::Full,
        ok: false,
        message: Some(message),
        duration_ms: 0,
    }
}

fn recreate_directory(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        std::fs::remove_dir_all(path)?;
    }
    std::fs::create_dir_all(path)
}
