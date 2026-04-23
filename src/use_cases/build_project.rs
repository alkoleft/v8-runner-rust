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
use crate::support::temp::{partial_list_file, reserved_source_set_dir};
use crate::use_cases::context::{ExecutionContext, InterruptionSafetyClass};
use crate::use_cases::external_artifacts::{
    discover_designer_external_artifacts, prepare_edt_external_artifacts, source_set_external_kind,
};
use crate::use_cases::request::BuildRequest as BuildArgs;
use crate::use_cases::result::{UseCaseFailure, UseCaseResult};
use crate::use_cases::source_inventory::SourceSetInventory;
use tracing::debug;

mod coordinator;
mod helpers;

pub(crate) use self::helpers::ensure_platform_success;
use self::helpers::{
    build_designer_dsl, build_ibcmd_dsl, build_mode_label, commit_step_state,
    deferred_interruption_warning, extension_name, fail_from_source_set_index,
    interruption_before_safe_point, log_timeline_stage, map_ibcmd_error, merge_step_message,
    plan_configurator_load_step, plan_edt_export_step, plan_generated_designer_load_step,
    push_build_step, remove_storage_path, StepCommit, StepPlan,
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

#[derive(Clone, Copy)]
enum TimelineStageStatus {
    Running,
    Succeeded,
    Failed,
    Skipped,
}

impl TimelineStageStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

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
    coordinator::run_build_designer(context, config, args)
}

fn run_build_ibcmd(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &BuildArgs,
) -> Result<BuildResult, BuildExecutionFailure> {
    coordinator::run_build_ibcmd(context, config, args)
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
    coordinator::run_build_edt(context, config, args)
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
    ensure_platform_success("edt_export", source_set, &export_result)?;
    Ok(deferred_interruption_warning("edt_export", &export_result)
        .into_iter()
        .collect())
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
    .update_db_cfg(extension_name(source_set))
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
    use crate::change_detection::hash_storage::HashStorage;
    use crate::change_detection::source_sets::SourceSetsService;
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, PlatformToolConfig, SourceFormat, SourceSetConfig,
        SourceSetPurpose, TestsConfig, ToolsConfig,
    };
    use crate::domain::build::BuildMode;
    use crate::use_cases::context::{CommandName, ExecutionContext};
    use crate::use_cases::request::BuildRequest as BuildArgs;
    use crate::use_cases::result::UseCaseErrorKind;
    use std::fs;
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
            &BuildArgs { full_rebuild: true },
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
            &BuildArgs { full_rebuild: true },
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

        let result = run_build(&config, &BuildArgs { full_rebuild: true }).expect("build");

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
        assert!(result.steps.iter().any(|step| {
            step.source_set == "main" && matches!(step.mode, BuildMode::Partial { .. }) && step.ok
        }));
        assert!(edt_calls_text.contains("export --project-name main"));
        assert!(designer_calls_text.contains("/LoadConfigFromFiles"));
        assert!(designer_calls_text.contains("-partial"));
        assert!(designer_calls_text.contains(
            work.join("designer")
                .join("main")
                .display()
                .to_string()
                .as_str()
        ));
        assert_eq!(edt_storage_generation(&config, "main"), 2);
        assert_eq!(storage_generation(&config, "main"), 1);

        let rerun = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
            },
        )
        .expect("rerun");
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
        assert!(result.steps.iter().any(|step| {
            step.source_set == "main" && matches!(step.mode, BuildMode::Partial { .. }) && step.ok
        }));
        assert!(edt_calls_text.contains("export --project-name main"));
        assert!(ibcmd_calls_text.contains("infobase --db-path /tmp/ib config import files"));
        assert!(ibcmd_calls_text.contains("--base-dir"));
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

        let error = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
            },
        )
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

        let rerun = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
            },
        )
        .expect("rerun");
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

        run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
            },
        )
        .expect("initial build");

        fs::write(
            work.join("designer").join("main").join("exported.txt"),
            "generated designer drift\n",
        )
        .expect("modify generated designer source");

        let edt_generation_before = edt_storage_generation(&config, "main");
        let designer_generation_before = storage_generation(&config, "main");

        let result = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
            },
        )
        .expect("second build");
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
            step.source_set == "main" && matches!(step.mode, BuildMode::Partial { .. }) && !step.ok
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

        let json = serde_json::to_value(result).expect("json");
        assert_eq!(BUILD_COMMAND, "build");
        assert_eq!(json["steps"][0]["mode"], "full");
    }
}
