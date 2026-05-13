use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use crate::change_detection::analyzer::{self, AnalysisOutcome};
use crate::change_detection::partial_load;
use crate::config::model::{AppConfig, BuilderBackend, SourceFormat, SourceSetConfig};
use crate::domain::build::{BuildMode, BuildResult};
use crate::domain::source_set::SourceSetContext;
use crate::platform::edt::EdtDsl;
use crate::platform::edt_session::{EdtSessionHostOptions, EdtSessionManager};
use crate::platform::ibcmd::DynamicUpdateMode;
use crate::platform::locator::UtilityType;
use crate::platform::process::ProcessRunner;
use crate::platform::utilities::PlatformUtilities;
use crate::support::edt_project;
use crate::support::error::AppError;
use crate::support::temp::{partial_list_file, platform_logs_dir, reserved_source_set_dir};
use crate::use_cases::build_progress::{
    build_mode_label, log_build_step_timeline, log_timeline_stage, TimelineStageStatus,
};
use crate::use_cases::context::{ExecutionContext, InterruptionSafetyClass};
use crate::use_cases::external_artifacts::{
    discover_designer_external_artifacts, prepare_edt_external_artifacts, source_set_external_kind,
};
use crate::use_cases::request::BuildRequest as BuildArgs;
use crate::use_cases::result::{UseCaseFailure, UseCaseResult};
use crate::use_cases::source_inventory::SourceSetInventory;
use crate::use_cases::tool_extension;
use tracing::debug;

mod coordinator;
mod helpers;

pub(crate) use self::helpers::ensure_platform_success;
use self::helpers::{
    build_designer_dsl, build_ibcmd_dsl, commit_step_state, deferred_interruption_warning,
    extension_name, fail_from_source_set_index, interruption_before_safe_point, map_ibcmd_error,
    merge_step_message, plan_configurator_load_step, plan_edt_export_step,
    plan_generated_designer_load_step, push_build_step, remove_storage_path, StepCommit, StepPlan,
};

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
    run_build_unlocked(context, config, args)
}

pub(crate) type BuildExecutionFailure = UseCaseFailure<BuildResult>;

#[cfg(test)]
pub(crate) fn run_build(config: &AppConfig, args: &BuildArgs) -> UseCaseResult<BuildResult> {
    run_build_unlocked(
        &ExecutionContext::cli(crate::use_cases::context::CommandName::Build),
        config,
        args,
    )
}

/// Caller must ensure exclusive ownership of `config.work_path`.
pub(crate) fn run_build_unlocked(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &BuildArgs,
) -> UseCaseResult<BuildResult> {
    if config.format == SourceFormat::Edt {
        return run_build_edt(context, config, args);
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
        BuilderBackend::Designer => run_build_designer(context, config, args),
        BuilderBackend::Ibcmd => run_build_ibcmd(context, config, args),
    }
}

fn run_build_designer(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &BuildArgs,
) -> Result<BuildResult, BuildExecutionFailure> {
    let started = Instant::now();
    let mut result = coordinator::run_build_designer(context, config, args)?;
    append_client_mcp_extension_step(context, config, args, started, &mut result)?;
    Ok(result)
}

fn run_build_ibcmd(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &BuildArgs,
) -> Result<BuildResult, BuildExecutionFailure> {
    let started = Instant::now();
    let mut result = coordinator::run_build_ibcmd(context, config, args)?;
    append_client_mcp_extension_step(context, config, args, started, &mut result)?;
    Ok(result)
}

/// Resolves the effective `/UpdateDBCfg -Dynamic+` flag for a build invocation.
///
/// `args.dynamic_update` (CLI/MCP one-shot override) takes priority over
/// `config.build.dynamic_update` (project-wide default). Default is `false`.
pub(crate) fn resolve_dynamic_update(config: &AppConfig, args: &BuildArgs) -> bool {
    args.dynamic_update.unwrap_or(config.build.dynamic_update)
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
    context: &ExecutionContext,
    config: &AppConfig,
    args: &BuildArgs,
) -> Result<BuildResult, BuildExecutionFailure> {
    let started = Instant::now();
    let mut result = coordinator::run_build_edt(context, config, args)?;
    append_client_mcp_extension_step(context, config, args, started, &mut result)?;
    Ok(result)
}

fn append_client_mcp_extension_step(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &BuildArgs,
    started: Instant,
    result: &mut BuildResult,
) -> Result<(), BuildExecutionFailure> {
    match tool_extension::prepare_client_mcp_extension(context, config, args.full_rebuild) {
        Ok(Some(step)) => {
            log_build_step_timeline(&step);
            result.steps.push(step);
            result.duration_ms = started.elapsed().as_millis() as u64;
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(failure) => {
            result.ok = false;
            log_build_step_timeline(&failure.step);
            result.steps.push(failure.step);
            result.duration_ms = started.elapsed().as_millis() as u64;
            Err(BuildExecutionFailure::with_payload(
                failure.error,
                result.clone(),
            ))
        }
    }
}

fn selected_ordered_source_sets<'a>(
    inventory: &'a SourceSetInventory<'a>,
    source_set_name: Option<&str>,
) -> Result<Vec<&'a SourceSetConfig>, AppError> {
    match source_set_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        Some(name) => inventory
            .ordered_source_sets()
            .into_iter()
            .find(|source_set| source_set.name == name)
            .map(|source_set| vec![source_set])
            .ok_or_else(|| AppError::Validation(format!("unknown source-set '{name}'"))),
        None => {
            if source_set_name.is_some() {
                return Err(AppError::Validation(
                    "build source-set requires a non-empty name".to_owned(),
                ));
            }
            Ok(inventory.ordered_source_sets())
        }
    }
}

fn designer_contexts_for_source_sets(
    inventory: &SourceSetInventory<'_>,
    source_sets: &[&SourceSetConfig],
) -> Vec<SourceSetContext> {
    inventory
        .designer_contexts()
        .iter()
        .filter(|context| {
            source_sets
                .iter()
                .any(|source_set| source_set.name == context.name())
        })
        .cloned()
        .collect()
}

fn edt_contexts_for_source_sets(
    inventory: &SourceSetInventory<'_>,
    source_sets: &[&SourceSetConfig],
) -> Vec<SourceSetContext> {
    inventory
        .edt_contexts()
        .iter()
        .filter(|context| {
            source_sets
                .iter()
                .any(|source_set| source_set.name == context.name())
        })
        .cloned()
        .collect()
}

fn analyze_contexts_by_name(
    inventory: &SourceSetInventory<'_>,
    contexts: &[SourceSetContext],
) -> HashMap<String, Result<AnalysisOutcome, analyzer::ChangeDetectionError>> {
    inventory
        .analyze_contexts(contexts)
        .into_iter()
        .map(|analysis| (analysis.context.name().to_owned(), analysis.outcome))
        .collect()
}

fn execute_edt_export_step(
    context: &ExecutionContext,
    config: &AppConfig,
    dsl: &EdtDsl<'_>,
    source_set: &SourceSetConfig,
    edt_context: &SourceSetContext,
    designer_context: &SourceSetContext,
    step_index: usize,
) -> Result<Vec<String>, AppError> {
    if let Some(error) = interruption_before_safe_point(
        context,
        format!("EDT export for source-set '{}'", source_set.name),
    ) {
        return Err(error);
    }
    let export_target = reserved_source_set_dir(&config.work_path, &source_set.name);
    let project_name = resolve_edt_project_name(source_set, edt_context)?;
    recreate_directory(&export_target).map_err(|error| {
        AppError::Runtime(format!(
            "failed to prepare EDT export directory '{}': {error}",
            export_target.display()
        ))
    })?;
    let export_result = dsl
        .export_project(&project_name, designer_context.path())
        .map_err(AppError::from)?;
    let export_log_path = write_edt_export_log(
        config,
        source_set,
        step_index,
        &project_name,
        designer_context.path(),
        &export_result,
    )?;
    ensure_edt_export_success(source_set, &export_result, &export_log_path)?;
    ensure_edt_export_output(
        source_set,
        &project_name,
        designer_context.path(),
        &export_result,
        &export_log_path,
    )?;
    Ok(deferred_interruption_warning("edt_export", &export_result)
        .into_iter()
        .collect())
}

fn write_edt_export_log(
    config: &AppConfig,
    source_set: &SourceSetConfig,
    step_index: usize,
    project_name: &str,
    export_target: &Path,
    result: &crate::platform::result::PlatformCommandResult,
) -> Result<PathBuf, AppError> {
    let log_dir = platform_logs_dir(&config.work_path).map_err(|error| {
        AppError::Runtime(format!("failed to create platform logs dir: {error}"))
    })?;
    let log_path = log_dir.join(format!(
        "build-{step_index:02}-{}-edt-export.log",
        source_set.name
    ));
    let contents = format!(
        "action: edt_export\nsource-set: {}\nproject-name: {project_name}\nexport-target: {}\nexit-code: {}\nstdout:\n{}\nstderr:\n{}\n",
        source_set.name,
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

fn ensure_edt_export_success(
    source_set: &SourceSetConfig,
    result: &crate::platform::result::PlatformCommandResult,
    export_log_path: &Path,
) -> Result<(), AppError> {
    ensure_platform_success("edt_export", source_set, result).map_err(|error| match error {
        AppError::Platform(message) => AppError::Platform(format!(
            "{message}; edt export log path: {}",
            export_log_path.display()
        )),
        other => other,
    })
}

fn ensure_edt_export_output(
    source_set: &SourceSetConfig,
    project_name: &str,
    export_target: &Path,
    result: &crate::platform::result::PlatformCommandResult,
    export_log_path: &Path,
) -> Result<(), AppError> {
    if !edt_export_requires_configuration_xml(source_set) {
        return Ok(());
    }

    let expected = export_target.join("Configuration.xml");
    if expected.is_file() {
        return Ok(());
    }

    let mut details = vec![
        format!(
            "EDT export for source-set '{}' completed with exit code 0 but did not produce required Designer file '{}'",
            source_set.name,
            expected.display()
        ),
        format!("EDT project: '{project_name}'"),
        format!("export target: {}", export_target.display()),
        format!("edt export log path: {}", export_log_path.display()),
    ];
    if !result.process.stdout.trim().is_empty() {
        details.push(format!("stdout: {}", result.process.stdout.trim()));
    }
    if !result.process.stderr.trim().is_empty() {
        details.push(format!("stderr: {}", result.process.stderr.trim()));
    }

    Err(AppError::Platform(details.join("; ")))
}

fn edt_export_requires_configuration_xml(source_set: &SourceSetConfig) -> bool {
    match source_set.purpose {
        crate::config::model::SourceSetPurpose::Configuration
        | crate::config::model::SourceSetPurpose::Extension => true,
        crate::config::model::SourceSetPurpose::ExternalDataProcessors
        | crate::config::model::SourceSetPurpose::ExternalReports => false,
    }
}

fn resolve_edt_project_name(
    source_set: &SourceSetConfig,
    edt_context: &SourceSetContext,
) -> Result<String, AppError> {
    edt_project::read_project_descriptor_from_dir(edt_context.path())
        .map_err(AppError::Validation)?
        .map(|project| project.name)
        .ok_or_else(|| {
            AppError::Validation(format!(
                "EDT source-set '{}' must contain a valid '.project' with projectDescription/name: {}",
                source_set.name,
                edt_context.path().display()
            ))
        })
}

fn recreate_directory(path: &Path) -> std::io::Result<()> {
    remove_storage_path(path)?;
    std::fs::create_dir_all(path)
}

// Designer build step is intentionally wide: it threads the execution context, config, the
// source set and its commit/load directories, the partial-load file list and the dynamic
// flag. Refactoring into a builder is out of scope for TASK-124; the IBCMD sibling already
// breaches the same limit (9/7) without an allow.
#[allow(clippy::too_many_arguments)]
fn execute_source_set_step(
    context: &ExecutionContext,
    config: &AppConfig,
    binary: &Path,
    runner: &dyn ProcessRunner,
    source_set: &SourceSetConfig,
    load_context: &SourceSetContext,
    commit_context: &SourceSetContext,
    step_index: usize,
    partial_paths: Option<&[PathBuf]>,
    commit: &StepCommit,
    dynamic_update_db_cfg: bool,
) -> Result<Vec<String>, AppError> {
    if let Some(error) = interruption_before_safe_point(
        context,
        format!("build load for source-set '{}'", source_set.name),
    ) {
        return Err(error);
    }
    if let Some(paths) = partial_paths {
        log_timeline_stage(
            &source_set.name,
            &build_mode_label(&BuildMode::Partial {
                file_count: paths.len(),
            }),
            "[Конфигуратор] Загрузка изменений в базу",
            TimelineStageStatus::Running,
        );
    } else {
        log_timeline_stage(
            &source_set.name,
            "full",
            "[Конфигуратор] Загрузка в базу",
            TimelineStageStatus::Running,
        );
    }
    let load_result = if let Some(paths) = partial_paths {
        let list_file = partial_list_file(&config.work_path).map_err(|error| {
            AppError::Runtime(format!("failed to create partial list file: {error}"))
        })?;
        partial_load::write_list_file(paths, load_context.path(), list_file.path()).map_err(
            |error| AppError::Runtime(format!("failed to write partial load list: {error}")),
        )?;
        build_designer_dsl(
            context,
            config,
            binary,
            runner,
            &source_set.name,
            step_index,
            "load",
            InterruptionSafetyClass::CriticalNonAbortable,
        )?
        .load_config_from_files_partial(
            load_context.path(),
            list_file.path(),
            extension_name(source_set),
        )
        .map_err(AppError::from)?
    } else {
        build_designer_dsl(
            context,
            config,
            binary,
            runner,
            &source_set.name,
            step_index,
            "load",
            InterruptionSafetyClass::CriticalNonAbortable,
        )?
        .load_config_from_files_full(load_context.path(), extension_name(source_set))
        .map_err(AppError::from)?
    };
    ensure_platform_success("load", source_set, &load_result)?;

    if let Some(error) = interruption_before_safe_point(
        context,
        format!("update_db_cfg for source-set '{}'", source_set.name),
    ) {
        return Err(error);
    }

    debug!(
        source_set = source_set.name.as_str(),
        "updating database configuration after load"
    );
    log_timeline_stage(
        &source_set.name,
        "update_db_cfg",
        "[Конфигуратор] Применение изменений",
        TimelineStageStatus::Running,
    );
    let update_result = build_designer_dsl(
        context,
        config,
        binary,
        runner,
        &source_set.name,
        step_index,
        "update",
        InterruptionSafetyClass::CriticalNonAbortable,
    )?
    .update_db_cfg(extension_name(source_set), dynamic_update_db_cfg)
    .map_err(AppError::from)?;
    ensure_platform_success("update_db_cfg", source_set, &update_result)?;

    commit_step_state(source_set, commit_context, &config.work_path, commit)?;

    Ok([
        deferred_interruption_warning("load", &load_result),
        deferred_interruption_warning("update_db_cfg", &update_result),
    ]
    .into_iter()
    .flatten()
    .collect())
}

fn execute_source_set_step_ibcmd(
    context: &ExecutionContext,
    config: &AppConfig,
    binary: &Path,
    runner: &dyn ProcessRunner,
    source_set: &SourceSetConfig,
    load_context: &SourceSetContext,
    commit_context: &SourceSetContext,
    partial_paths: Option<&[PathBuf]>,
    commit: &StepCommit,
) -> Result<Vec<String>, AppError> {
    if let Some(error) = interruption_before_safe_point(
        context,
        format!("ibcmd import for source-set '{}'", source_set.name),
    ) {
        return Err(error);
    }
    if partial_paths.is_some() {
        debug!(
            source_set = source_set.name.as_str(),
            "loading partial changes into infobase with ibcmd"
        );
    } else {
        debug!(
            source_set = source_set.name.as_str(),
            "loading source set into infobase with ibcmd"
        );
    }

    let import_dsl = build_ibcmd_dsl(
        context,
        config,
        binary,
        runner,
        InterruptionSafetyClass::CriticalNonAbortable,
    )?;
    let extension = extension_name(source_set);
    let load_result = if let Some(paths) = partial_paths {
        let rel_paths =
            partial_load::relative_paths(paths, load_context.path()).map_err(|error| {
                AppError::Runtime(format!("failed to convert partial paths: {error}"))
            })?;
        log_timeline_stage(
            &source_set.name,
            "ibcmd_import",
            "[ibcmd] Загрузка изменений в базу",
            TimelineStageStatus::Running,
        );
        import_dsl
            .config_import_partial(load_context.path(), &rel_paths, extension)
            .map_err(map_ibcmd_error)?
    } else {
        log_timeline_stage(
            &source_set.name,
            "ibcmd_import",
            "[ibcmd] Загрузка в базу",
            TimelineStageStatus::Running,
        );
        import_dsl
            .config_import_full(load_context.path(), extension)
            .map_err(map_ibcmd_error)?
    };
    ensure_platform_success("load", source_set, &load_result)?;

    if let Some(error) = interruption_before_safe_point(
        context,
        format!("ibcmd apply for source-set '{}'", source_set.name),
    ) {
        return Err(error);
    }

    debug!(
        source_set = source_set.name.as_str(),
        "applying database configuration after ibcmd load"
    );
    let apply_dsl = build_ibcmd_dsl(
        context,
        config,
        binary,
        runner,
        InterruptionSafetyClass::CriticalNonAbortable,
    )?;
    log_timeline_stage(
        &source_set.name,
        "ibcmd_apply",
        "[ibcmd] Применение изменений",
        TimelineStageStatus::Running,
    );
    let apply_result = apply_dsl
        .config_apply(extension, DynamicUpdateMode::Auto)
        .map_err(map_ibcmd_error)?;
    ensure_platform_success("apply", source_set, &apply_result)?;

    commit_step_state(source_set, commit_context, &config.work_path, commit)?;

    Ok([
        deferred_interruption_warning("ibcmd_import", &load_result),
        deferred_interruption_warning("apply", &apply_result),
    ]
    .into_iter()
    .flatten()
    .collect())
}

#[cfg(test)]
mod tests {
    use super::{run_build, BUILD_COMMAND};
    use crate::change_detection::hash_storage::{HashStorage, FILES_MTIME};
    use crate::change_detection::source_sets::SourceSetsService;
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, PlatformToolConfig, SourceFormat, SourceSetConfig,
        SourceSetPurpose, TestsConfig, ToolExtensionArtifactConfig, ToolExtensionConfig,
        ToolExtensionInput, ToolExtensionSourceConfig, ToolsConfig,
    };
    use crate::domain::build::BuildMode;
    use crate::domain::source_set::SourceSetContext;
    use crate::use_cases::context::{CommandName, ExecutionContext};
    use crate::use_cases::request::BuildRequest as BuildArgs;
    use crate::use_cases::result::UseCaseErrorKind;
    use std::fs;
    use std::io::ErrorKind;
    use std::path::{Path, PathBuf};
    use std::thread;
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

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
            "args=\"$*\"\nproject=\"\"\ntarget=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"--project-name\" ]; then project=\"$arg\"; fi\n  if [ \"$prev\" = \"--configuration-files\" ]; then target=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$target\" ]; then mkdir -p \"$target\"; printf 'exported from %s\\n' \"$project\" > \"$target/exported.txt\"; printf '<Configuration />\\n' > \"$target/Configuration.xml\"; fi\nprintf '%s\\n' \"$args\" >> \"{}\"\n{}\nexit 0",
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
    fn write_edt_script_without_configuration(path: &Path, calls_log: &Path) {
        let body = format!(
            "args=\"$*\"\ntarget=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"--configuration-files\" ]; then target=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$target\" ]; then mkdir -p \"$target\"; printf 'diagnostic marker\\n' > \"$target/exported.txt\"; fi\nprintf 'edt stdout detail\\n'\nprintf 'edt stderr detail\\n' >&2\nprintf '%s\\n' \"$args\" >> \"{}\"\nexit 0",
            calls_log.display()
        );

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create dirs");
        }
        fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        make_executable(path);
    }

    #[cfg(unix)]
    fn write_edt_external_processor_script(path: &Path, calls_log: &Path) {
        let body = format!(
            "args=\"$*\"\ntarget=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"--configuration-files\" ]; then target=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$target\" ]; then mkdir -p \"$target\"; printf '<ExternalDataProcessor><Properties><Name>Processor One</Name></Properties></ExternalDataProcessor>\\n' > \"$target/Processor One.xml\"; fi\nprintf '%s\\n' \"$args\" >> \"{}\"\nexit 0",
            calls_log.display()
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
             current_dir=\"\"\n\
             prev=\"\"\n\
             for arg in \"$@\"; do\n\
               if [ \"$prev\" = \"-data\" ]; then current_dir=\"$arg\"; fi\n\
               prev=\"$arg\"\n\
             done\n\
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
                   if [ \"$#\" -eq 0 ]; then\n\
                     printf '%s\\n' \"$current_dir\"\n\
                   else\n\
                     current_dir=\"$1\"\n\
                   fi\n\
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
                   if [ -n \"$target\" ]; then mkdir -p \"$target\"; printf 'exported from %s\\n' \"$project\" > \"$target/exported.txt\"; printf '<Configuration />\\n' > \"$target/Configuration.xml\"; fi\n\
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
            execution_timeout: 300_000,
            format,
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
            build: BuildConfig {
                partial_load_threshold: threshold,
                dynamic_update: false,
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
            execution_timeout: 300_000,
            format: SourceFormat::Edt,
            builder: BuilderBackend::Designer,
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
            build: BuildConfig {
                partial_load_threshold: 20,
                dynamic_update: false,
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
                ..Default::default()
            },
            mcp: Default::default(),
            tests: TestsConfig::default(),
        }
    }

    fn build_args(full_rebuild: bool) -> BuildArgs {
        BuildArgs {
            full_rebuild,
            source_set: None,
            dynamic_update: None,
        }
    }

    fn build_args_dynamic(full_rebuild: bool, dynamic: bool) -> BuildArgs {
        BuildArgs {
            full_rebuild,
            source_set: None,
            dynamic_update: Some(dynamic),
        }
    }

    #[cfg(unix)]
    #[test]
    fn execute_build_honors_interruption_before_load_safe_point() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform = dir.path().join("1cv8");
        let calls_log = dir.path().join("designer.calls.log");
        fs::create_dir_all(base.join("main")).expect("main");
        fs::create_dir_all(base.join("ext")).expect("ext");
        fs::create_dir_all(&work).expect("work");
        fs::write(base.join("main").join("Catalogs.xml"), "<Catalogs />").expect("catalog");
        write_designer_script(&platform, &calls_log, None);
        let config = build_config(
            &base,
            &work,
            &platform,
            20,
            SourceFormat::Designer,
            BuilderBackend::Designer,
        );
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let failure = super::execute(
            &ExecutionContext::cli(CommandName::Build).with_cancellation(cancellation),
            &config,
            &build_args(true),
        )
        .expect_err("build must stop before load");

        assert!(failure
            .error
            .message()
            .contains("before entering build load for source-set 'main' safe point"));
        assert!(
            !calls_log.exists()
                || fs::read_to_string(&calls_log)
                    .expect("calls")
                    .trim()
                    .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn execute_ibcmd_build_honors_interruption_before_apply_safe_point() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let ibcmd = dir.path().join("ibcmd");
        let calls_log = dir.path().join("ibcmd.calls.log");
        fs::create_dir_all(base.join("main")).expect("main");
        fs::create_dir_all(base.join("ext")).expect("ext");
        fs::create_dir_all(&work).expect("work");
        fs::write(base.join("main").join("Catalogs.xml"), "<Catalogs />").expect("catalog");
        if let Some(parent) = ibcmd.parent() {
            fs::create_dir_all(parent).expect("create ibcmd dir");
        }
        fs::write(
            &ibcmd,
            format!(
                "#!/bin/sh\nargs=\"$*\"\nprintf '%s\\n' \"$args\" >> \"{}\"\n\
                 if printf '%s' \"$args\" | grep -F -q -- 'config import'; then sleep 0.1; fi\n\
                 if printf '%s' \"$args\" | grep -F -q -- 'config apply'; then sleep 0.07; fi\n\
                 exit 0\n",
                calls_log.display()
            ),
        )
        .expect("write ibcmd script");
        make_executable(&ibcmd);
        let config = build_config(
            &base,
            &work,
            &ibcmd,
            20,
            SourceFormat::Designer,
            BuilderBackend::Ibcmd,
        );
        let cancellation = CancellationToken::new();
        let delayed_cancel = cancellation.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            delayed_cancel.cancel();
        });

        let failure = super::execute(
            &ExecutionContext::cli(CommandName::Build).with_cancellation(cancellation),
            &config,
            &build_args(true),
        )
        .expect_err("build must stop before ibcmd apply");

        assert!(failure
            .error
            .message()
            .contains("before entering ibcmd apply for source-set 'main' safe point"));
        let calls = fs::read_to_string(&calls_log).expect("calls");
        assert!(calls.contains("config import"));
        assert!(!calls.contains("config apply"));
    }

    fn create_source_tree(base_path: &Path) {
        fs::create_dir_all(base_path.join("main").join("DT-INF")).expect("main dt-inf");
        fs::create_dir_all(base_path.join("main").join("src").join("Configuration"))
            .expect("main edt marker dir");
        fs::create_dir_all(base_path.join("main").join("Catalogs.Items")).expect("main dir");
        fs::write(
            base_path.join("main").join(".project"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<projectDescription>\n  <name>main</name>\n  <natures>\n    <nature>com._1c.g5.v8.dt.core.V8ConfigurationNature</nature>\n  </natures>\n</projectDescription>\n",
        )
        .expect("main project");
        fs::write(
            base_path.join("main").join("DT-INF").join("PROJECT.PMF"),
            "Manifest-Version: 1.0\nRuntime-Version: 8.3.27\n",
        )
        .expect("main manifest");
        fs::write(
            base_path
                .join("main")
                .join("src")
                .join("Configuration")
                .join("Configuration.mdo"),
            "<Configuration />\n",
        )
        .expect("main edt marker");

        fs::create_dir_all(base_path.join("ext").join("DT-INF")).expect("ext dt-inf");
        fs::create_dir_all(base_path.join("ext").join("src").join("Configuration"))
            .expect("ext edt marker dir");
        fs::create_dir_all(base_path.join("ext").join("CommonModules")).expect("ext dir");
        fs::write(
            base_path.join("ext").join(".project"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<projectDescription>\n  <name>ext</name>\n  <natures>\n    <nature>com._1c.g5.v8.dt.core.V8ExtensionNature</nature>\n  </natures>\n</projectDescription>\n",
        )
        .expect("ext project");
        fs::write(
            base_path.join("ext").join("DT-INF").join("PROJECT.PMF"),
            "Base-Project: main\nManifest-Version: 1.0\nRuntime-Version: 8.3.27\n",
        )
        .expect("ext manifest");
        fs::write(
            base_path
                .join("ext")
                .join("src")
                .join("Configuration")
                .join("Configuration.mdo"),
            "<Configuration />\n",
        )
        .expect("ext edt marker");
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

    fn create_edt_tool_extension_source(path: &Path) {
        fs::create_dir_all(path.join("DT-INF")).expect("tool dt-inf");
        fs::create_dir_all(path.join("src").join("Configuration")).expect("tool src");
        fs::write(
            path.join(".project"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<projectDescription>\n  <name>client-mcp-project</name>\n  <natures>\n    <nature>com._1c.g5.v8.dt.core.V8ExtensionNature</nature>\n  </natures>\n</projectDescription>\n",
        )
        .expect("tool project");
        fs::write(
            path.join("DT-INF").join("PROJECT.PMF"),
            "Base-Project: main\nManifest-Version: 1.0\nRuntime-Version: 8.3.27\n",
        )
        .expect("tool manifest");
        fs::write(
            path.join("src")
                .join("Configuration")
                .join("Configuration.mdo"),
            "<Configuration />\n",
        )
        .expect("tool mdo");
        fs::write(
            path.join("src").join("Configuration").join("Module.bsl"),
            "Procedure Test()\nEndProcedure\n",
        )
        .expect("tool module");
    }

    fn remove_file_if_exists(path: &Path) {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => panic!("failed to remove '{}': {error}", path.display()),
        }
    }

    #[test]
    fn edt_export_configuration_xml_check_applies_only_to_configuration_and_extension() {
        let source_set = |purpose| SourceSetConfig {
            name: "set".to_owned(),
            purpose,
            path: PathBuf::from("set"),
        };

        assert!(super::edt_export_requires_configuration_xml(&source_set(
            SourceSetPurpose::Configuration
        )));
        assert!(super::edt_export_requires_configuration_xml(&source_set(
            SourceSetPurpose::Extension
        )));
        assert!(!super::edt_export_requires_configuration_xml(&source_set(
            SourceSetPurpose::ExternalDataProcessors
        )));
        assert!(!super::edt_export_requires_configuration_xml(&source_set(
            SourceSetPurpose::ExternalReports
        )));
    }

    #[cfg(unix)]
    #[test]
    fn edt_external_export_uses_external_descriptor_without_configuration_xml() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform_script = dir.path().join("platform").join("bin").join("1cv8");
        let edt_script = dir.path().join("edt").join("1cedtcli");
        let edt_calls = dir.path().join("edt-calls.log");
        let source = base.join("processors").join("ProcessorOne");
        fs::create_dir_all(source.join("DT-INF")).expect("dt-inf");
        fs::create_dir_all(source.join("src")).expect("src");
        fs::write(
            source.join(".project"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<projectDescription>\n  <name>Processor One</name>\n  <natures>\n    <nature>{}</nature>\n  </natures>\n</projectDescription>\n",
                crate::support::edt_project::V8_EXTERNAL_OBJECTS_NATURE
            ),
        )
        .expect("project");
        fs::write(
            source.join("DT-INF").join("PROJECT.PMF"),
            "Base-Project: main\nManifest-Version: 1.0\nRuntime-Version: 8.3.27\n",
        )
        .expect("manifest");
        fs::write(
            source.join("src").join("root.xml"),
            "<ExternalDataProcessor><Properties><Name>Processor One</Name></Properties></ExternalDataProcessor>\n",
        )
        .expect("root xml");
        fs::create_dir_all(&work).expect("work");
        let designer_calls = dir.path().join("designer-calls.log");
        write_designer_script(&platform_script, &designer_calls, None);
        write_edt_external_processor_script(&edt_script, &edt_calls);

        let mut config = build_edt_config(&base, &work, &dir.path().join("platform"), &edt_script);
        config.source_sets = vec![SourceSetConfig {
            name: "processors".to_owned(),
            purpose: SourceSetPurpose::ExternalDataProcessors,
            path: PathBuf::from("processors"),
        }];

        let result = run_build(&config, &build_args(true)).expect("build");
        let mut export_roots = fs::read_dir(work.join("designer").join("processors"))
            .expect("external export dir")
            .map(|entry| entry.expect("entry").path())
            .collect::<Vec<_>>();
        export_roots.sort();
        assert_eq!(export_roots.len(), 1);
        let export_root = &export_roots[0];

        assert!(result.ok);
        assert!(result.steps.iter().any(|step| {
            step.source_set == "processors" && matches!(step.mode, BuildMode::EdtExport) && step.ok
        }));
        assert!(export_root.join("Processor One.xml").is_file());
        assert!(!export_root.join("Configuration.xml").exists());
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

    fn tool_extension_storage_generation(
        config: &AppConfig,
        source_path: &Path,
        extension_name: &str,
    ) -> u64 {
        let storage_path = tool_extension_storage_path(config, source_path, extension_name);
        HashStorage::new(storage_path)
            .load_snapshot()
            .expect("tool extension snapshot")
            .generation
    }

    fn tool_extension_storage_path(
        config: &AppConfig,
        source_path: &Path,
        extension_name: &str,
    ) -> PathBuf {
        let context = SourceSetContext::new(
            format!("tool:{extension_name}"),
            source_path.to_path_buf(),
            format!("tool-{extension_name}-source"),
        );
        context.storage_path(&config.work_path)
    }

    fn write_recoverable_tool_extension_storage(path: &Path) {
        remove_file_if_exists(path);
        let db = redb::Database::create(path).expect("create recoverable storage");
        let tx = db.begin_write().expect("begin recoverable storage write");
        {
            let mut table = tx.open_table(FILES_MTIME).expect("open mtime table");
            table.insert("orphan.bsl", 1).expect("insert orphan mtime");
        }
        tx.commit().expect("commit recoverable storage");
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
        let result = run_build(&config, &build_args(true)).expect("build");

        assert!(result.ok);
        let calls_text = fs::read_to_string(&calls).expect("calls");
        assert!(calls_text.contains("config import"));
        assert!(calls_text.contains("config apply"));
    }

    #[cfg(unix)]
    #[test]
    fn build_prepares_client_mcp_cfe_tool_extension_after_project_sources() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform = dir.path().join("platform").join("bin").join("1cv8");
        let calls = dir.path().join("designer.calls.log");
        let artifact = dir.path().join("client_mcp.cfe");
        create_source_tree(&base);
        fs::write(&artifact, "cfe").expect("artifact");
        write_designer_script(&platform, &calls, None);
        let mut config = build_config(
            &base,
            &work,
            &dir.path().join("platform"),
            20,
            SourceFormat::Designer,
            BuilderBackend::Designer,
        );
        config.source_sets = vec![SourceSetConfig {
            name: "main".to_owned(),
            purpose: SourceSetPurpose::Configuration,
            path: PathBuf::from("main"),
        }];
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Artifact(ToolExtensionArtifactConfig {
                path: artifact.clone(),
            }),
        });

        let result = run_build(&config, &build_args(true)).expect("build");

        assert!(result.ok);
        assert!(result.steps.iter().any(|step| {
            step.source_set == "tool:client_mcp" && matches!(step.mode, BuildMode::Full) && step.ok
        }));
        let calls_text = fs::read_to_string(&calls).expect("calls");
        let project_load = calls_text
            .find("/LoadConfigFromFiles")
            .expect("project source load");
        let tool_load = calls_text.find("/LoadCfg").expect("tool artifact load");
        assert!(project_load < tool_load);
        assert!(calls_text.contains(&artifact.display().to_string()));
        assert!(calls_text.contains("-Extension client_mcp"));
    }

    #[cfg(unix)]
    #[test]
    fn build_exports_edt_client_mcp_source_before_loading_tool_extension() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform = dir.path().join("platform").join("bin").join("1cv8");
        let edt = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer.calls.log");
        let edt_calls = dir.path().join("edt.calls.log");
        let tool_source = base.join("tool-client-mcp");
        create_source_tree(&base);
        fs::create_dir_all(tool_source.join("DT-INF")).expect("tool dt-inf");
        fs::create_dir_all(tool_source.join("src").join("Configuration")).expect("tool src");
        fs::write(
            tool_source.join(".project"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<projectDescription>\n  <name>client-mcp-project</name>\n  <natures>\n    <nature>com._1c.g5.v8.dt.core.V8ExtensionNature</nature>\n  </natures>\n</projectDescription>\n",
        )
        .expect("tool project");
        fs::write(
            tool_source.join("DT-INF").join("PROJECT.PMF"),
            "Base-Project: main\nManifest-Version: 1.0\nRuntime-Version: 8.3.27\n",
        )
        .expect("tool manifest");
        fs::write(
            tool_source
                .join("src")
                .join("Configuration")
                .join("Configuration.mdo"),
            "<Configuration />\n",
        )
        .expect("tool mdo");
        write_designer_script(&platform, &designer_calls, None);
        write_edt_script(&edt, &edt_calls, None);
        let mut config = build_edt_config(&base, &work, &dir.path().join("platform"), &edt);
        config.source_sets = vec![SourceSetConfig {
            name: "main".to_owned(),
            purpose: SourceSetPurpose::Configuration,
            path: PathBuf::from("main"),
        }];
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Source(ToolExtensionSourceConfig {
                path: tool_source,
                format: Some(SourceFormat::Edt),
            }),
        });

        let result = run_build(&config, &build_args(true)).expect("build");

        assert!(result.ok);
        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");
        let designer_calls_text = fs::read_to_string(&designer_calls).expect("designer calls");
        assert!(edt_calls_text.contains("--project-name client-mcp-project"));
        assert!(edt_calls_text.contains("tool-extensions/client_mcp"));
        assert!(designer_calls_text.contains("tool-extensions/client_mcp"));
        assert!(designer_calls_text.contains("-Extension client_mcp"));
    }

    #[cfg(unix)]
    #[test]
    fn build_skips_unchanged_edt_client_mcp_source_tool_extension() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform = dir.path().join("platform").join("bin").join("1cv8");
        let edt = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer.calls.log");
        let edt_calls = dir.path().join("edt.calls.log");
        let tool_source = base.join("tool-client-mcp");
        create_source_tree(&base);
        create_edt_tool_extension_source(&tool_source);
        write_designer_script(&platform, &designer_calls, None);
        write_edt_script(&edt, &edt_calls, None);
        let mut config = build_edt_config(&base, &work, &dir.path().join("platform"), &edt);
        config.source_sets = vec![SourceSetConfig {
            name: "main".to_owned(),
            purpose: SourceSetPurpose::Configuration,
            path: PathBuf::from("main"),
        }];
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Source(ToolExtensionSourceConfig {
                path: tool_source.clone(),
                format: Some(SourceFormat::Edt),
            }),
        });

        let first = run_build(&config, &build_args(false)).expect("first build");
        assert!(first.ok);
        assert_eq!(
            tool_extension_storage_generation(&config, &tool_source, "client_mcp"),
            1
        );
        remove_file_if_exists(&designer_calls);
        remove_file_if_exists(&edt_calls);

        let second = run_build(&config, &build_args(false)).expect("second build");

        assert!(second.ok);
        assert!(second.steps.iter().any(|step| {
            step.source_set == "tool:client_mcp"
                && matches!(step.mode, BuildMode::Skipped)
                && step.ok
                && step.message.as_deref() == Some("no changes")
        }));
        assert!(
            !edt_calls.exists(),
            "unchanged source must not invoke EDT export"
        );
        assert!(
            !designer_calls.exists(),
            "unchanged tool extension must not invoke load/apply"
        );
        assert_eq!(
            tool_extension_storage_generation(&config, &tool_source, "client_mcp"),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn build_refreshes_changed_edt_client_mcp_source_tool_extension_and_commits_state() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform = dir.path().join("platform").join("bin").join("1cv8");
        let edt = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer.calls.log");
        let edt_calls = dir.path().join("edt.calls.log");
        let tool_source = base.join("tool-client-mcp");
        create_source_tree(&base);
        create_edt_tool_extension_source(&tool_source);
        write_designer_script(&platform, &designer_calls, None);
        write_edt_script(&edt, &edt_calls, None);
        let mut config = build_edt_config(&base, &work, &dir.path().join("platform"), &edt);
        config.source_sets = vec![SourceSetConfig {
            name: "main".to_owned(),
            purpose: SourceSetPurpose::Configuration,
            path: PathBuf::from("main"),
        }];
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Source(ToolExtensionSourceConfig {
                path: tool_source.clone(),
                format: Some(SourceFormat::Edt),
            }),
        });
        run_build(&config, &build_args(false)).expect("first build");
        remove_file_if_exists(&designer_calls);
        remove_file_if_exists(&edt_calls);

        fs::write(
            tool_source
                .join("src")
                .join("Configuration")
                .join("Module.bsl"),
            "Procedure Test()\n    // changed\nEndProcedure\n",
        )
        .expect("change tool module");

        let second = run_build(&config, &build_args(false)).expect("second build");
        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");
        let designer_calls_text = fs::read_to_string(&designer_calls).expect("designer calls");

        assert!(second.ok);
        assert!(second.steps.iter().any(|step| {
            step.source_set == "tool:client_mcp" && matches!(step.mode, BuildMode::Full) && step.ok
        }));
        assert!(edt_calls_text.contains("--project-name client-mcp-project"));
        assert!(edt_calls_text.contains("tool-extensions/client_mcp"));
        assert!(designer_calls_text.contains("tool-extensions/client_mcp"));
        assert!(designer_calls_text.contains("-Extension client_mcp"));
        assert_eq!(
            tool_extension_storage_generation(&config, &tool_source, "client_mcp"),
            2
        );
    }

    #[cfg(unix)]
    #[test]
    fn full_rebuild_refreshes_edt_client_mcp_source_tool_extension() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform = dir.path().join("platform").join("bin").join("1cv8");
        let edt = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer.calls.log");
        let edt_calls = dir.path().join("edt.calls.log");
        let tool_source = base.join("tool-client-mcp");
        create_source_tree(&base);
        create_edt_tool_extension_source(&tool_source);
        write_designer_script(&platform, &designer_calls, None);
        write_edt_script(&edt, &edt_calls, None);
        let mut config = build_edt_config(&base, &work, &dir.path().join("platform"), &edt);
        config.source_sets = vec![SourceSetConfig {
            name: "main".to_owned(),
            purpose: SourceSetPurpose::Configuration,
            path: PathBuf::from("main"),
        }];
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Source(ToolExtensionSourceConfig {
                path: tool_source.clone(),
                format: Some(SourceFormat::Edt),
            }),
        });
        run_build(&config, &build_args(false)).expect("first build");
        remove_file_if_exists(&designer_calls);
        remove_file_if_exists(&edt_calls);

        let rebuild = run_build(&config, &build_args(true)).expect("full rebuild");
        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");
        let designer_calls_text = fs::read_to_string(&designer_calls).expect("designer calls");

        assert!(rebuild.ok);
        assert!(edt_calls_text.contains("--project-name client-mcp-project"));
        assert!(edt_calls_text.contains("tool-extensions/client_mcp"));
        assert!(designer_calls_text.contains("tool-extensions/client_mcp"));
        assert!(designer_calls_text.contains("-Extension client_mcp"));
        assert_eq!(
            tool_extension_storage_generation(&config, &tool_source, "client_mcp"),
            2
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_edt_client_mcp_source_export_does_not_commit_tool_extension_state() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform = dir.path().join("platform").join("bin").join("1cv8");
        let edt = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer.calls.log");
        let edt_calls = dir.path().join("edt.calls.log");
        let tool_source = base.join("tool-client-mcp");
        create_source_tree(&base);
        create_edt_tool_extension_source(&tool_source);
        write_designer_script(&platform, &designer_calls, None);
        write_edt_script(&edt, &edt_calls, None);
        let mut config = build_edt_config(&base, &work, &dir.path().join("platform"), &edt);
        config.source_sets = vec![SourceSetConfig {
            name: "main".to_owned(),
            purpose: SourceSetPurpose::Configuration,
            path: PathBuf::from("main"),
        }];
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Source(ToolExtensionSourceConfig {
                path: tool_source.clone(),
                format: Some(SourceFormat::Edt),
            }),
        });
        run_build(&config, &build_args(false)).expect("first build");
        assert_eq!(
            tool_extension_storage_generation(&config, &tool_source, "client_mcp"),
            1
        );
        write_edt_script(
            &edt,
            &edt_calls,
            Some("export --project-name client-mcp-project"),
        );

        fs::write(
            tool_source
                .join("src")
                .join("Configuration")
                .join("Module.bsl"),
            "Procedure Test()\n    // changed\nEndProcedure\n",
        )
        .expect("change tool module");

        let failure = run_build(&config, &build_args(false)).expect_err("failed export");

        assert!(failure.error.message().contains("tool extension"));
        assert_eq!(
            tool_extension_storage_generation(&config, &tool_source, "client_mcp"),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn recoverable_tool_extension_storage_fallback_refreshes_and_commits_after_success() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform = dir.path().join("platform").join("bin").join("1cv8");
        let edt = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer.calls.log");
        let edt_calls = dir.path().join("edt.calls.log");
        let tool_source = base.join("tool-client-mcp");
        create_source_tree(&base);
        create_edt_tool_extension_source(&tool_source);
        write_designer_script(&platform, &designer_calls, None);
        write_edt_script(&edt, &edt_calls, None);
        let mut config = build_edt_config(&base, &work, &dir.path().join("platform"), &edt);
        config.source_sets = vec![SourceSetConfig {
            name: "main".to_owned(),
            purpose: SourceSetPurpose::Configuration,
            path: PathBuf::from("main"),
        }];
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Source(ToolExtensionSourceConfig {
                path: tool_source.clone(),
                format: Some(SourceFormat::Edt),
            }),
        });
        run_build(&config, &build_args(false)).expect("first build");
        let storage_path = tool_extension_storage_path(&config, &tool_source, "client_mcp");
        write_recoverable_tool_extension_storage(&storage_path);
        remove_file_if_exists(&designer_calls);
        remove_file_if_exists(&edt_calls);

        let fallback = run_build(&config, &build_args(false)).expect("fallback build");
        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");
        let designer_calls_text = fs::read_to_string(&designer_calls).expect("designer calls");

        assert!(fallback.ok);
        assert!(fallback.steps.iter().any(|step| {
            step.source_set == "tool:client_mcp" && matches!(step.mode, BuildMode::Full) && step.ok
        }));
        assert!(edt_calls_text.contains("--project-name client-mcp-project"));
        assert!(designer_calls_text.contains("-Extension client_mcp"));
        assert_eq!(
            tool_extension_storage_generation(&config, &tool_source, "client_mcp"),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_edt_client_mcp_source_load_does_not_commit_tool_extension_state() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform = dir.path().join("platform").join("bin").join("1cv8");
        let edt = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer.calls.log");
        let edt_calls = dir.path().join("edt.calls.log");
        let tool_source = base.join("tool-client-mcp");
        create_source_tree(&base);
        create_edt_tool_extension_source(&tool_source);
        write_designer_script(&platform, &designer_calls, None);
        write_edt_script(&edt, &edt_calls, None);
        let mut config = build_edt_config(&base, &work, &dir.path().join("platform"), &edt);
        config.source_sets = vec![SourceSetConfig {
            name: "main".to_owned(),
            purpose: SourceSetPurpose::Configuration,
            path: PathBuf::from("main"),
        }];
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Source(ToolExtensionSourceConfig {
                path: tool_source.clone(),
                format: Some(SourceFormat::Edt),
            }),
        });
        run_build(&config, &build_args(false)).expect("first build");
        write_designer_script(&platform, &designer_calls, Some("/LoadConfigFromFiles"));
        fs::write(
            tool_source
                .join("src")
                .join("Configuration")
                .join("Module.bsl"),
            "Procedure Test()\n    // changed\nEndProcedure\n",
        )
        .expect("change tool module");

        let failure = run_build(&config, &build_args(false)).expect_err("failed load");

        assert!(failure.error.message().contains("tool extension"));
        assert_eq!(
            tool_extension_storage_generation(&config, &tool_source, "client_mcp"),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_edt_client_mcp_source_update_does_not_commit_tool_extension_state() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform = dir.path().join("platform").join("bin").join("1cv8");
        let edt = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer.calls.log");
        let edt_calls = dir.path().join("edt.calls.log");
        let tool_source = base.join("tool-client-mcp");
        create_source_tree(&base);
        create_edt_tool_extension_source(&tool_source);
        write_designer_script(&platform, &designer_calls, None);
        write_edt_script(&edt, &edt_calls, None);
        let mut config = build_edt_config(&base, &work, &dir.path().join("platform"), &edt);
        config.source_sets = vec![SourceSetConfig {
            name: "main".to_owned(),
            purpose: SourceSetPurpose::Configuration,
            path: PathBuf::from("main"),
        }];
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Source(ToolExtensionSourceConfig {
                path: tool_source.clone(),
                format: Some(SourceFormat::Edt),
            }),
        });
        run_build(&config, &build_args(false)).expect("first build");
        write_designer_script(&platform, &designer_calls, Some("/UpdateDBCfg"));
        fs::write(
            tool_source
                .join("src")
                .join("Configuration")
                .join("Module.bsl"),
            "Procedure Test()\n    // changed\nEndProcedure\n",
        )
        .expect("change tool module");

        let failure = run_build(&config, &build_args(false)).expect_err("failed update");

        assert!(failure.error.message().contains("tool extension"));
        assert_eq!(
            tool_extension_storage_generation(&config, &tool_source, "client_mcp"),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn ibcmd_build_with_server_infobase_passes_dbms_and_infobase_credentials() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("ibcmd");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_ibcmd_script(&script, &calls, None);
        let mut config = build_config(
            &base,
            &work,
            &script,
            20,
            SourceFormat::Designer,
            BuilderBackend::Ibcmd,
        );
        config.infobase = crate::config::model::InfobaseConfig::server(
            "Srvr=cluster:1541;Ref=demo",
            crate::config::model::InfobaseDbmsConfig::new("PostgreSQL", "localhost", "demo")
                .with_credentials(Some("postgres".to_owned()), Some("pg-secret".to_owned())),
        )
        .with_credentials(Some("Admin".to_owned()), Some("secret".to_owned()));

        let result = run_build(&config, &build_args(true)).expect("build");

        assert!(result.ok);
        let calls_text = fs::read_to_string(&calls).expect("calls");
        assert!(calls_text.contains("--dbms PostgreSQL"));
        assert!(calls_text.contains("--database-server localhost"));
        assert!(calls_text.contains("--database-name demo"));
        assert!(calls_text.contains("--database-user postgres"));
        assert!(calls_text.contains("--database-password pg-secret"));
        assert!(calls_text.contains("--user Admin"));
        assert!(calls_text.contains("--password secret"));
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

        let failure = run_build(&config, &build_args(false)).expect_err("expected failure");
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

        let result = run_build(&config, &build_args(false)).expect("build");
        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");
        let designer_calls_text = fs::read_to_string(&designer_calls).expect("designer calls");

        assert!(result.ok);
        assert!(result
            .steps
            .iter()
            .any(|step| matches!(step.mode, BuildMode::EdtExport) && step.ok));
        assert!(result.steps.iter().any(|step| {
            step.source_set == "main" && matches!(step.mode, BuildMode::Full) && step.ok
        }));
        assert!(edt_calls_text.contains("export --project-name main"));
        assert!(designer_calls_text.contains("/LoadConfigFromFiles"));
        assert!(!designer_calls_text.contains("-partial"));
        assert!(designer_calls_text.contains(
            work.join("designer")
                .join("main")
                .display()
                .to_string()
                .as_str()
        ));
        assert_eq!(edt_storage_generation(&config, "main"), 2);
        assert_eq!(storage_generation(&config, "main"), 1);

        let rerun = run_build(&config, &build_args(false)).expect("rerun");
        let rerun_edt_calls = fs::read_to_string(&edt_calls).expect("edt calls after rerun");

        assert!(rerun
            .steps
            .iter()
            .filter(|step| step.source_set == "main")
            .all(|step| matches!(step.mode, BuildMode::Skipped) && step.ok));
        assert_eq!(
            rerun_edt_calls
                .matches("export --project-name main")
                .count(),
            1
        );
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

        let result = run_build(&config, &build_args(false)).expect("build");
        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");
        let ibcmd_calls_text = fs::read_to_string(&ibcmd_calls).expect("ibcmd calls");

        assert!(result.ok);
        assert!(result
            .steps
            .iter()
            .any(|step| matches!(step.mode, BuildMode::EdtExport) && step.ok));
        assert!(result.steps.iter().any(|step| {
            step.source_set == "main" && matches!(step.mode, BuildMode::Full) && step.ok
        }));
        assert!(edt_calls_text.contains("export --project-name main"));
        assert!(ibcmd_calls_text.contains("infobase --db-path /tmp/ib config import"));
        assert!(!ibcmd_calls_text.contains("config import files"));
        assert!(!ibcmd_calls_text.contains("--partial"));
        assert!(ibcmd_calls_text.contains("infobase --db-path /tmp/ib config apply"));
        assert!(ibcmd_calls_text.contains(
            work.join("designer")
                .join("main")
                .display()
                .to_string()
                .as_str()
        ));
        assert_eq!(edt_storage_generation(&config, "main"), 2);
        assert_eq!(storage_generation(&config, "main"), 1);
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

        let result = run_build(&config, &build_args(false)).expect("build");
        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");

        assert!(result.ok);
        assert!(edt_calls_text.contains("export --project-name client_mcp"));
        assert!(!edt_calls_text.contains("export --project-name client-mcp"));
    }

    #[cfg(unix)]
    #[test]
    fn edt_build_requires_valid_dot_project_for_project_name() {
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

        let error = run_build(&config, &build_args(false))
            .expect_err("build should fail without valid .project");

        assert_eq!(error.error.kind(), UseCaseErrorKind::Validation);
        assert!(error
            .error
            .message()
            .contains("must contain a valid '.project'"));
    }

    #[cfg(unix)]
    #[test]
    fn edt_extension_build_forces_full_load_after_incremental_export() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform_script = dir.path().join("platform").join("bin").join("1cv8");
        let edt_script = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer-calls.log");
        let edt_calls = dir.path().join("edt-calls.log");
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

        let result = run_build(&config, &build_args(false)).expect("build");
        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");
        let designer_calls_text = fs::read_to_string(&designer_calls).expect("designer calls");

        assert!(result.ok);
        assert!(result.steps.iter().any(|step| {
            step.source_set == "client_mcp" && matches!(step.mode, BuildMode::EdtExport) && step.ok
        }));
        assert!(result.steps.iter().any(|step| {
            step.source_set == "client_mcp" && matches!(step.mode, BuildMode::Full) && step.ok
        }));
        assert!(!result.steps.iter().any(|step| {
            step.source_set == "client_mcp" && matches!(step.mode, BuildMode::Partial { .. })
        }));
        assert!(edt_calls_text.contains("export --project-name client_mcp"));
        assert!(designer_calls_text.contains("/LoadConfigFromFiles"));
        assert!(designer_calls_text.contains("-Extension client_mcp"));
        assert!(!designer_calls_text.contains("-partial"));
        assert_eq!(edt_storage_generation(&config, "client_mcp"), 2);
        assert_eq!(storage_generation(&config, "client_mcp"), 1);

        let rerun = run_build(&config, &build_args(false)).expect("rerun");
        let rerun_designer_calls =
            fs::read_to_string(&designer_calls).expect("designer calls after rerun");

        assert!(rerun
            .steps
            .iter()
            .filter(|step| step.source_set == "client_mcp")
            .all(|step| matches!(step.mode, BuildMode::Skipped) && step.ok));
        assert_eq!(
            rerun_designer_calls.matches("/LoadConfigFromFiles").count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn edt_extension_build_with_ibcmd_forces_full_import_after_incremental_export() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let ibcmd_script = dir.path().join("ibcmd");
        let edt_script = dir.path().join("edt").join("1cedtcli");
        let ibcmd_calls = dir.path().join("ibcmd-calls.log");
        let edt_calls = dir.path().join("edt-calls.log");
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
        write_ibcmd_script(&ibcmd_script, &ibcmd_calls, None);
        write_edt_script(&edt_script, &edt_calls, None);

        let mut config = build_edt_config(&base, &work, &ibcmd_script, &edt_script);
        config.builder = BuilderBackend::Ibcmd;
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

        let result = run_build(&config, &build_args(false)).expect("build");
        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");
        let ibcmd_calls_text = fs::read_to_string(&ibcmd_calls).expect("ibcmd calls");

        assert!(result.ok);
        assert!(result.steps.iter().any(|step| {
            step.source_set == "client_mcp" && matches!(step.mode, BuildMode::EdtExport) && step.ok
        }));
        assert!(result.steps.iter().any(|step| {
            step.source_set == "client_mcp" && matches!(step.mode, BuildMode::Full) && step.ok
        }));
        assert!(!result.steps.iter().any(|step| {
            step.source_set == "client_mcp" && matches!(step.mode, BuildMode::Partial { .. })
        }));
        assert!(edt_calls_text.contains("export --project-name client_mcp"));
        assert!(ibcmd_calls_text.contains("config import"));
        assert!(ibcmd_calls_text.contains("--extension client_mcp"));
        assert!(!ibcmd_calls_text.contains("config import files"));
        assert!(!ibcmd_calls_text.contains("--partial"));
        assert_eq!(edt_storage_generation(&config, "client_mcp"), 2);
        assert_eq!(storage_generation(&config, "client_mcp"), 1);
    }

    #[cfg(unix)]
    #[test]
    fn edt_extension_load_failure_does_not_commit_generated_designer_snapshot() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform_script = dir.path().join("platform").join("bin").join("1cv8");
        let edt_script = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer-calls.log");
        let edt_calls = dir.path().join("edt-calls.log");
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
        write_designer_script(
            &platform_script,
            &designer_calls,
            Some("/UpdateDBCfg -Extension client_mcp"),
        );
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

        let failure = run_build(&config, &build_args(false)).expect_err("expected failure");
        let result = failure
            .payload
            .expect("build failures should preserve a structured payload");
        let designer_storage_path = SourceSetsService::new(&config)
            .designer_contexts()
            .into_iter()
            .find(|context| context.name() == "client_mcp")
            .expect("designer context")
            .storage_path(&config.work_path);
        let designer_calls_text = fs::read_to_string(&designer_calls).expect("designer calls");

        assert!(!result.ok);
        assert!(matches!(result.steps[0].mode, BuildMode::EdtExport));
        assert!(result.steps[0].ok);
        assert!(result.steps.iter().any(|step| {
            step.source_set == "client_mcp" && matches!(step.mode, BuildMode::Full) && !step.ok
        }));
        assert!(!result.steps.iter().any(|step| {
            step.source_set == "client_mcp" && matches!(step.mode, BuildMode::Partial { .. })
        }));
        assert!(designer_calls_text.contains("/UpdateDBCfg -Extension client_mcp"));
        assert_eq!(edt_storage_generation(&config, "client_mcp"), 2);
        assert!(!designer_storage_path.exists());
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

        let result = run_build(&config, &build_args(false)).expect("build");
        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");

        assert!(result.ok);
        assert_eq!(edt_calls_text.matches("START").count(), 1);
        assert_eq!(edt_calls_text.matches("EXIT").count(), 1);
        assert_eq!(edt_calls_text.matches("export --project-name").count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn edt_build_loads_generated_designer_diff_even_when_edt_sources_are_unchanged() {
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

        run_build(&config, &build_args(false)).expect("initial build");

        fs::write(
            work.join("designer").join("main").join("exported.txt"),
            "generated designer drift\n",
        )
        .expect("modify generated designer source");

        let edt_generation_before = edt_storage_generation(&config, "main");
        let designer_generation_before = storage_generation(&config, "main");

        let result = run_build(&config, &build_args(false)).expect("second build");
        let designer_calls_text = fs::read_to_string(&designer_calls).expect("designer calls");
        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");

        assert!(result.steps.iter().any(|step| {
            step.source_set == "main" && matches!(step.mode, BuildMode::Skipped) && step.ok
        }));
        assert!(result.steps.iter().any(|step| {
            step.source_set == "main" && matches!(step.mode, BuildMode::Partial { .. }) && step.ok
        }));
        assert_eq!(
            edt_calls_text.matches("export --project-name main").count(),
            1
        );
        assert_eq!(
            designer_calls_text.matches("/LoadConfigFromFiles").count(),
            2
        );
        assert_eq!(
            edt_storage_generation(&config, "main"),
            edt_generation_before
        );
        assert_eq!(
            storage_generation(&config, "main"),
            designer_generation_before + 1
        );
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

        let failure = run_build(&config, &build_args(false)).expect_err("expected failure");
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
    fn edt_export_success_without_configuration_xml_reports_export_diagnostics() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform_script = dir.path().join("platform").join("bin").join("1cv8");
        let edt_script = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer-calls.log");
        let edt_calls = dir.path().join("edt-calls.log");
        create_source_tree(&base);
        write_designer_script(&platform_script, &designer_calls, None);
        write_edt_script_without_configuration(&edt_script, &edt_calls);
        let config = build_edt_config(&base, &work, &dir.path().join("platform"), &edt_script);
        prime_edt_snapshots(&config);

        fs::write(
            base.join("main")
                .join("Catalogs.Items")
                .join("ObjectModule.bsl"),
            "procedure Test()\n  // changed in edt\nendprocedure",
        )
        .expect("modify edt main");

        let failure = run_build(&config, &build_args(false)).expect_err("expected failure");
        let result = failure
            .payload
            .expect("build failures should preserve a structured payload");
        let log_path = work
            .join("logs")
            .join("platform")
            .join("build-00-main-edt-export.log");
        let log_contents = fs::read_to_string(&log_path).expect("edt export log");

        assert!(!result.ok);
        assert!(matches!(result.steps[0].mode, BuildMode::EdtExport));
        assert!(!result.steps[0].ok);
        assert!(!designer_calls.exists());
        assert_eq!(edt_storage_generation(&config, "main"), 1);
        assert!(failure
            .error
            .message()
            .contains("did not produce required Designer file"));
        assert!(failure.error.message().contains("Configuration.xml"));
        assert!(failure.error.message().contains("edt export log path"));
        assert!(failure.error.message().contains("edt stdout detail"));
        assert!(failure.error.message().contains("edt stderr detail"));
        assert!(log_contents.contains("action: edt_export"));
        assert!(log_contents.contains("project-name: main"));
        assert!(log_contents.contains("edt stdout detail"));
        assert!(log_contents.contains("edt stderr detail"));
    }

    #[cfg(unix)]
    #[test]
    fn edt_build_failure_does_not_commit_generated_designer_snapshot() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let platform_script = dir.path().join("platform").join("bin").join("1cv8");
        let edt_script = dir.path().join("edt").join("1cedtcli");
        let designer_calls = dir.path().join("designer-calls.log");
        let edt_calls = dir.path().join("edt-calls.log");
        create_source_tree(&base);
        write_designer_script(&platform_script, &designer_calls, Some("/UpdateDBCfg"));
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

        let failure = run_build(&config, &build_args(false)).expect_err("expected failure");
        let result = failure
            .payload
            .expect("build failures should preserve a structured payload");
        let designer_storage_path = SourceSetsService::new(&config)
            .designer_contexts()
            .into_iter()
            .find(|context| context.name() == "main")
            .expect("designer context")
            .storage_path(&config.work_path);

        assert!(!result.ok);
        assert!(matches!(result.steps[0].mode, BuildMode::EdtExport));
        assert!(result.steps[0].ok);
        assert!(result.steps.iter().any(|step| {
            step.source_set == "main" && matches!(step.mode, BuildMode::Full) && !step.ok
        }));
        assert_eq!(edt_storage_generation(&config, "main"), 2);
        assert!(!designer_storage_path.exists());
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

        let result = run_build(&config, &build_args(false)).expect("build");

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

        let result = run_build(&config, &build_args(false)).expect("build");
        let calls_text = fs::read_to_string(&calls).expect("calls");

        assert!(matches!(result.steps[0].mode, BuildMode::Partial { .. }));
        assert!(result.steps[0].ok);
        assert!(calls_text.contains("/LoadConfigFromFiles"));
        assert!(calls_text.contains("-partial"));
        assert!(calls_text.contains("/UpdateDBCfg"));
        assert!(calls_text.contains("-listFile"));
        assert_eq!(storage_generation(&config, "main"), 2);

        let rerun = run_build(&config, &build_args(false)).expect("rerun");
        assert!(matches!(rerun.steps[0].mode, BuildMode::Skipped));
    }

    #[cfg(unix)]
    #[test]
    fn dynamic_cli_flag_emits_dynamic_marker_in_update_db_cfg() {
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

        // CLI `--dynamic` should reach DESIGNER as `/UpdateDBCfg -Dynamic+`.
        let result = run_build(&config, &build_args_dynamic(false, true)).expect("dynamic build");
        let calls_text = fs::read_to_string(&calls).expect("calls");

        assert!(result.ok);
        assert!(calls_text.contains("/UpdateDBCfg"));
        assert!(calls_text.contains("-Dynamic+"));
    }

    #[cfg(unix)]
    #[test]
    fn build_dynamic_update_config_default_emits_dynamic_marker() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("1cv8");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_designer_script(&script, &calls, None);
        let mut config = build_config(
            &base,
            &work,
            &script,
            20,
            SourceFormat::Designer,
            BuilderBackend::Designer,
        );
        config.build.dynamic_update = true;
        prime_snapshots(&config);

        fs::write(
            base.join("main")
                .join("Catalogs.Items")
                .join("ObjectModule.bsl"),
            "procedure Test()\n  // changed\nendprocedure",
        )
        .expect("modify main");

        // `build.dynamicUpdate: true` in v8project.yaml is enough — no CLI flag needed.
        let result = run_build(&config, &build_args(false)).expect("dynamic build");
        let calls_text = fs::read_to_string(&calls).expect("calls");

        assert!(result.ok);
        assert!(calls_text.contains("-Dynamic+"));
    }

    #[cfg(unix)]
    #[test]
    fn build_without_dynamic_flag_emits_static_update_db_cfg() {
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

        // Regression guard: default behavior MUST NOT add `-Dynamic+`.
        let _ = run_build(&config, &build_args(false)).expect("static build");
        let calls_text = fs::read_to_string(&calls).expect("calls");

        assert!(calls_text.contains("/UpdateDBCfg"));
        assert!(!calls_text.contains("-Dynamic"));
    }

    #[cfg(unix)]
    #[test]
    fn edt_build_designer_dynamic_update_modes_reach_update_db_cfg() {
        for (case, config_dynamic_update, arg_dynamic_update, expected_dynamic) in [
            ("cli_dynamic", false, Some(true), true),
            ("config_default_dynamic", true, None, true),
            ("cli_static_overrides_config", true, Some(false), false),
            ("static_default", false, None, false),
        ] {
            let dir = tempdir().expect("tempdir");
            let base = dir.path().join("base");
            let work = dir.path().join("work");
            let platform_script = dir.path().join("platform").join("bin").join("1cv8");
            let edt_script = dir.path().join("edt").join("1cedtcli");
            let designer_calls = dir.path().join(format!("{case}-designer-calls.log"));
            let edt_calls = dir.path().join(format!("{case}-edt-calls.log"));
            create_source_tree(&base);
            write_designer_script(&platform_script, &designer_calls, None);
            write_edt_script(&edt_script, &edt_calls, None);
            let mut config =
                build_edt_config(&base, &work, &dir.path().join("platform"), &edt_script);
            config.build.dynamic_update = config_dynamic_update;
            prime_edt_snapshots(&config);

            fs::write(
                base.join("main")
                    .join("Catalogs.Items")
                    .join("ObjectModule.bsl"),
                format!("procedure Test()\n  // changed in {case}\nendprocedure"),
            )
            .expect("modify edt main");

            let result = run_build(
                &config,
                &BuildArgs {
                    full_rebuild: false,
                    source_set: None,
                    dynamic_update: arg_dynamic_update,
                },
            )
            .expect(case);
            let calls_text = fs::read_to_string(&designer_calls).expect("designer calls");

            assert!(result.ok, "{case}");
            assert!(calls_text.contains("/UpdateDBCfg"), "{case}");
            assert_eq!(
                calls_text.contains("-Dynamic+"),
                expected_dynamic,
                "{case}: {calls_text}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn infobase_unlock_code_propagates_to_designer_args() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("1cv8");
        let calls = dir.path().join("calls.log");
        create_source_tree(&base);
        write_designer_script(&script, &calls, None);
        let mut config = build_config(
            &base,
            &work,
            &script,
            20,
            SourceFormat::Designer,
            BuilderBackend::Designer,
        );
        config.infobase.unlock_code = Some("seal-42".to_owned());
        prime_snapshots(&config);

        fs::write(
            base.join("main")
                .join("Catalogs.Items")
                .join("ObjectModule.bsl"),
            "procedure Test()\n  // changed\nendprocedure",
        )
        .expect("modify main");

        let _ = run_build(&config, &build_args(false)).expect("build with /UC");
        let calls_text = fs::read_to_string(&calls).expect("calls");

        // `/UC` reaches DESIGNER as a separate token + value pair.
        assert!(calls_text.contains("/UC"));
        assert!(calls_text.contains("seal-42"));
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

        let result = run_build(&config, &build_args(false)).expect("build");
        let calls_text = fs::read_to_string(&calls).expect("calls");

        assert!(matches!(result.steps[0].mode, BuildMode::Skipped));
        assert!(matches!(result.steps[1].mode, BuildMode::Partial { .. }));
        assert!(calls_text.contains("-Extension ext"));
        assert_eq!(storage_generation(&config, "main"), 1);
        assert_eq!(storage_generation(&config, "ext"), 2);
    }

    #[cfg(unix)]
    #[test]
    fn source_set_build_analyzes_and_loads_only_requested_source_set() {
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
        let main_context = service
            .designer_contexts()
            .into_iter()
            .find(|context| context.name() == "main")
            .expect("main context");
        fs::write(main_context.storage_path(&config.work_path), "corrupt").expect("corrupt main");
        fs::write(
            base.join("ext").join("CommonModules").join("Module.bsl"),
            "procedure Test()\n  // ext changed\nendprocedure",
        )
        .expect("modify ext");

        let result = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
                source_set: Some("ext".to_owned()),
                dynamic_update: None,
            },
        )
        .expect("build");
        let calls_text = fs::read_to_string(&calls).expect("calls");

        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].source_set, "ext");
        assert!(matches!(result.steps[0].mode, BuildMode::Partial { .. }));
        assert!(calls_text.contains("-Extension ext"));
        assert_eq!(storage_generation(&config, "ext"), 2);
    }

    #[cfg(unix)]
    #[test]
    fn source_set_build_still_runs_configured_tool_extension_post_build_step() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let script = dir.path().join("1cv8");
        let calls = dir.path().join("calls.log");
        let artifact = dir.path().join("client_mcp.cfe");
        create_source_tree(&base);
        fs::write(&artifact, "cfe").expect("artifact");
        write_designer_script(&script, &calls, None);
        let mut config = build_config(
            &base,
            &work,
            &script,
            20,
            SourceFormat::Designer,
            BuilderBackend::Designer,
        );
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Artifact(ToolExtensionArtifactConfig {
                path: artifact.clone(),
            }),
        });
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
                source_set: Some("ext".to_owned()),
                dynamic_update: None,
            },
        )
        .expect("build");
        let calls_text = fs::read_to_string(&calls).expect("calls");

        assert_eq!(result.steps.len(), 2);
        assert_eq!(result.steps[0].source_set, "ext");
        assert_eq!(result.steps[1].source_set, "tool:client_mcp");
        assert!(matches!(result.steps[0].mode, BuildMode::Partial { .. }));
        assert!(matches!(result.steps[1].mode, BuildMode::Full));
        assert!(calls_text.contains("-Extension ext"));
        assert!(calls_text.contains(&artifact.display().to_string()));
        assert!(calls_text.contains("-Extension client_mcp"));
    }

    #[cfg(unix)]
    #[test]
    fn source_set_build_rejects_unknown_source_set() {
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

        let failure = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
                source_set: Some("missing".to_owned()),
                dynamic_update: None,
            },
        )
        .expect_err("unknown source-set must fail");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Validation);
        assert_eq!(failure.error.message(), "unknown source-set 'missing'");
        assert!(failure.payload.expect("payload").steps.is_empty());
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

        let result = run_build(&config, &build_args(true)).expect("build");
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

        let failure = run_build(&config, &build_args(true)).expect_err("failure");

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

        let failure = run_build(&config, &build_args(false)).expect_err("failure");
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

        let json = serde_json::to_value(result).expect("json");
        assert_eq!(BUILD_COMMAND, "build");
        assert_eq!(json["steps"][0]["mode"], "full");
    }
}
