use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::change_detection::analyzer::{self, AnalysisOutcome, PreparedStateUpdate};
use crate::change_detection::hash_storage::{HashStorage, StorageError};
use crate::change_detection::partial_load::{self, LoadDecision};
use crate::change_detection::source_sets::SourceSetsService;
use crate::cli::args::BuildArgs;
use crate::config::model::{
    AppConfig, BuilderBackend, SourceFormat, SourceSetConfig, SourceSetPurpose,
};
use crate::domain::build::{BuildMode, BuildResult, BuildStep};
use crate::domain::source_set::SourceSetContext;
use crate::output::json::Envelope;
use crate::output::presenter::Presenter;
use crate::platform::connection::V8Connection;
use crate::platform::designer::DesignerDsl;
use crate::platform::locator::UtilityType;
use crate::platform::process::ProcessRunner;
use crate::platform::result::PlatformCommandResult;
use crate::platform::utilities::PlatformUtilities;
use crate::support::error::AppError;
use crate::support::temp::{partial_list_file, platform_logs_dir};
use tracing::info;

const BUILD_COMMAND: &str = "build";
const SUPPORTED_BUILD_ERROR: &str =
    "build currently supports only builder=DESIGNER and format=DESIGNER";

pub fn execute(
    config: &AppConfig,
    args: &BuildArgs,
    presenter: &Presenter,
) -> Result<(), AppError> {
    let result = match run_build(config, args) {
        Ok(result) => result,
        Err(failure) => {
            if presenter.is_json() {
                presenter.print_envelope(&Envelope::err(
                    BUILD_COMMAND,
                    failure.result.duration_ms,
                    failure.result.clone(),
                ));
            } else {
                render_text_result(&failure.result, presenter, false);
                presenter.print_error(&failure.error.to_string());
            }
            return Err(failure.error);
        }
    };

    if presenter.is_json() {
        presenter.print_envelope(&Envelope::ok(BUILD_COMMAND, result.duration_ms, result));
    } else {
        render_text_result(&result, presenter, true);
    }

    Ok(())
}

#[derive(Debug)]
pub(crate) struct BuildExecutionFailure {
    pub(crate) error: AppError,
    pub(crate) result: BuildResult,
}

enum StepCommit {
    Prepared(PreparedStateUpdate),
    RescanFull { recover_storage: bool },
}

enum StepPlan {
    Skip {
        message: String,
        ok: bool,
    },
    Execute {
        mode: BuildMode,
        message: String,
        partial_paths: Option<Vec<PathBuf>>,
        commit: StepCommit,
    },
}

pub(crate) fn run_build(
    config: &AppConfig,
    args: &BuildArgs,
) -> Result<BuildResult, BuildExecutionFailure> {
    info!(full_rebuild = args.full_rebuild, "preparing build plan");
    if let Some(error) = validate_supported_matrix(config) {
        return Err(BuildExecutionFailure {
            error,
            result: BuildResult {
                ok: false,
                steps: vec![],
                duration_ms: 0,
            },
        });
    }

    let started = Instant::now();
    let service = SourceSetsService::new(config);
    let contexts = service.designer_contexts();
    let contexts_by_name: HashMap<String, SourceSetContext> = contexts
        .into_iter()
        .map(|context| (context.name().to_owned(), context))
        .collect();
    let ordered_source_sets = ordered_source_sets(config);

    let analysis_by_name = if args.full_rebuild {
        None
    } else {
        Some(analyze_contexts_by_name(
            &service,
            &contexts_by_name.values().cloned().collect::<Vec<_>>(),
        ))
    };

    let mut utilities = PlatformUtilities::from_config(config);
    let mut designer_binary: Option<PathBuf> = None;
    let mut steps = Vec::new();

    for (index, source_set) in ordered_source_sets.iter().enumerate() {
        let Some(context) = contexts_by_name.get(&source_set.name).cloned() else {
            continue;
        };

        let plan = if args.full_rebuild {
            StepPlan::Execute {
                mode: BuildMode::Full,
                message: "forced full rebuild".to_owned(),
                partial_paths: None,
                commit: StepCommit::RescanFull {
                    recover_storage: true,
                },
            }
        } else {
            match analysis_by_name
                .as_ref()
                .and_then(|analysis| analysis.get(&source_set.name))
                .cloned()
                .expect("every source-set must have an analysis result")
            {
                Ok(AnalysisOutcome::NoChanges { .. }) => {
                    info!(
                        source_set = source_set.name.as_str(),
                        "change analysis result: no changes detected"
                    );
                    StepPlan::Skip {
                        message: "no changes".to_owned(),
                        ok: true,
                    }
                }
                Ok(AnalysisOutcome::Fallback) => {
                    info!(
                        source_set = source_set.name.as_str(),
                        "change analysis result: fallback to full load after recoverable issue"
                    );
                    StepPlan::Execute {
                        mode: BuildMode::Full,
                        message: "fallback to full load after recoverable change-detection issue"
                            .to_owned(),
                        partial_paths: None,
                        commit: StepCommit::RescanFull {
                            recover_storage: false,
                        },
                    }
                }
                Ok(AnalysisOutcome::Changes { changes, prepared }) => {
                    log_change_analysis(source_set.name.as_str(), &changes);
                    match partial_load::decide(
                        &changes,
                        context.path(),
                        config.build.partial_load_threshold,
                    ) {
                        LoadDecision::Partial(paths) => {
                            info!(
                                source_set = source_set.name.as_str(),
                                partial_file_count = paths.len(),
                                threshold = config.build.partial_load_threshold,
                                "change analysis decision: partial load"
                            );
                            StepPlan::Execute {
                                mode: BuildMode::Partial {
                                    file_count: paths.len(),
                                },
                                message: format!("partial load of {} files", paths.len()),
                                partial_paths: Some(paths),
                                commit: StepCommit::Prepared(prepared),
                            }
                        }
                        LoadDecision::Full => {
                            info!(
                                source_set = source_set.name.as_str(),
                                threshold = config.build.partial_load_threshold,
                                "change analysis decision: full load"
                            );
                            StepPlan::Execute {
                                mode: BuildMode::Full,
                                message: "full load selected by partial-load rules".to_owned(),
                                partial_paths: None,
                                commit: StepCommit::Prepared(prepared),
                            }
                        }
                    }
                }
                Err(error) => {
                    let result = fail_with_remaining_steps(
                        started,
                        steps,
                        ordered_source_sets
                            .iter()
                            .skip(index)
                            .copied()
                            .collect::<Vec<_>>(),
                        source_set,
                        BuildMode::Skipped,
                        error.to_string(),
                    );
                    return Err(BuildExecutionFailure {
                        error: AppError::Runtime(error.to_string()),
                        result,
                    });
                }
            }
        };

        match plan {
            StepPlan::Skip { message, ok } => {
                info!(
                    source_set = source_set.name.as_str(),
                    message = message.as_str(),
                    "skipping build step"
                );
                steps.push(BuildStep {
                    source_set: source_set.name.clone(),
                    mode: BuildMode::Skipped,
                    ok,
                    message: Some(message),
                    duration_ms: 0,
                })
            }
            StepPlan::Execute {
                mode,
                message,
                partial_paths,
                commit,
            } => {
                info!(
                    source_set = source_set.name.as_str(),
                    mode = ?mode,
                    message = message.as_str(),
                    "executing build step"
                );
                let binary = match designer_binary.clone() {
                    Some(path) => path,
                    None => {
                        let location = match utilities.locate(UtilityType::V8) {
                            Ok(location) => location,
                            Err(error) => {
                                let result = fail_with_remaining_steps(
                                    started,
                                    steps,
                                    ordered_source_sets
                                        .iter()
                                        .skip(index)
                                        .copied()
                                        .collect::<Vec<_>>(),
                                    source_set,
                                    mode.clone(),
                                    error.to_string(),
                                );
                                return Err(BuildExecutionFailure {
                                    error: AppError::Platform(error.to_string()),
                                    result,
                                });
                            }
                        };
                        designer_binary = Some(location.path.clone());
                        location.path
                    }
                };

                let step_started = Instant::now();
                match execute_source_set_step(
                    config,
                    &binary,
                    utilities.runner_for(UtilityType::V8),
                    source_set,
                    &context,
                    index,
                    partial_paths.as_deref(),
                    &commit,
                ) {
                    Ok(()) => steps.push(BuildStep {
                        source_set: source_set.name.clone(),
                        mode,
                        ok: true,
                        message: Some(message),
                        duration_ms: step_started.elapsed().as_millis() as u64,
                    }),
                    Err(error) => {
                        let result = fail_with_remaining_steps(
                            started,
                            steps,
                            ordered_source_sets
                                .iter()
                                .skip(index)
                                .copied()
                                .collect::<Vec<_>>(),
                            source_set,
                            mode,
                            error.to_string(),
                        );
                        return Err(BuildExecutionFailure { error, result });
                    }
                }
            }
        }
    }

    Ok(BuildResult {
        ok: true,
        steps,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

fn log_change_analysis(source_set_name: &str, changes: &[analyzer::FileChange]) {
    let mut added = 0usize;
    let mut modified = 0usize;
    let mut deleted = 0usize;

    for change in changes {
        match change.kind {
            analyzer::ChangeKind::Added => added += 1,
            analyzer::ChangeKind::Modified => modified += 1,
            analyzer::ChangeKind::Deleted => deleted += 1,
        }
    }

    info!(
        source_set = source_set_name,
        changed_files = changes.len(),
        added,
        modified,
        deleted,
        "change analysis result: changes detected"
    );
}

fn validate_supported_matrix(config: &AppConfig) -> Option<AppError> {
    if config.builder == BuilderBackend::Designer && config.format == SourceFormat::Designer {
        None
    } else {
        Some(AppError::Validation(SUPPORTED_BUILD_ERROR.to_owned()))
    }
}

fn analyze_contexts_by_name(
    service: &SourceSetsService<'_>,
    contexts: &[SourceSetContext],
) -> HashMap<String, Result<AnalysisOutcome, analyzer::ChangeDetectionError>> {
    service
        .analyze_contexts(contexts)
        .into_iter()
        .map(|analysis| (analysis.context.name().to_owned(), analysis.outcome))
        .collect()
}

fn ordered_source_sets(config: &AppConfig) -> Vec<&SourceSetConfig> {
    let mut configuration = Vec::new();
    let mut extensions = Vec::new();

    for source_set in &config.source_sets {
        match source_set.purpose {
            SourceSetPurpose::Configuration => configuration.push(source_set),
            SourceSetPurpose::Extension => extensions.push(source_set),
        }
    }

    configuration.extend(extensions);
    configuration
}

fn execute_source_set_step(
    config: &AppConfig,
    binary: &Path,
    runner: &dyn ProcessRunner,
    source_set: &SourceSetConfig,
    context: &SourceSetContext,
    step_index: usize,
    partial_paths: Option<&[PathBuf]>,
    commit: &StepCommit,
) -> Result<(), AppError> {
    info!(
        source_set = source_set.name.as_str(),
        partial = partial_paths.is_some(),
        "loading source-set into designer"
    );
    let load_result = if let Some(paths) = partial_paths {
        let list_file = partial_list_file(&config.work_path).map_err(|error| {
            AppError::Runtime(format!("failed to create partial list file: {error}"))
        })?;
        partial_load::write_list_file(paths, context.path(), list_file.path()).map_err(
            |error| AppError::Runtime(format!("failed to write partial load list: {error}")),
        )?;
        build_designer_dsl(config, binary, runner, &source_set.name, step_index, "load")?
            .load_config_from_files_partial(
                context.path(),
                list_file.path(),
                extension_name(source_set),
            )
            .map_err(|error| AppError::Platform(error.to_string()))?
    } else {
        build_designer_dsl(config, binary, runner, &source_set.name, step_index, "load")?
            .load_config_from_files_full(context.path(), extension_name(source_set))
            .map_err(|error| AppError::Platform(error.to_string()))?
    };
    ensure_platform_success("load", source_set, &load_result)?;

    info!(
        source_set = source_set.name.as_str(),
        "updating database configuration after load"
    );
    let update_result = build_designer_dsl(
        config,
        binary,
        runner,
        &source_set.name,
        step_index,
        "update",
    )?
    .update_db_cfg(extension_name(source_set))
    .map_err(|error| AppError::Platform(error.to_string()))?;
    ensure_platform_success("update_db_cfg", source_set, &update_result)?;

    match commit {
        StepCommit::Prepared(prepared) => {
            info!(
                source_set = source_set.name.as_str(),
                "committing prepared change-detection state"
            );
            analyzer::commit_success(context, &config.work_path, prepared)
                .map_err(|error| AppError::Runtime(error.to_string()))
        }
        StepCommit::RescanFull { recover_storage } => {
            info!(
                source_set = source_set.name.as_str(),
                recover_storage, "rescanning source-set state after full build"
            );
            commit_full_rescan(context, &config.work_path, *recover_storage)
        }
    }
}

fn commit_full_rescan(
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

fn build_designer_dsl<'a>(
    config: &AppConfig,
    binary: &Path,
    runner: &'a dyn ProcessRunner,
    source_set_name: &str,
    step_index: usize,
    action: &str,
) -> Result<DesignerDsl<'a>, AppError> {
    let log_dir = platform_logs_dir(&config.work_path).map_err(|error| {
        AppError::Runtime(format!("failed to create platform logs dir: {error}"))
    })?;
    let log_file = log_dir.join(format!(
        "build-{step_index:02}-{source_set_name}-{action}.log"
    ));

    Ok(DesignerDsl::new(
        binary.to_path_buf(),
        config.v8_connection(),
        runner,
        Some(log_file),
    ))
}

fn extension_name(source_set: &SourceSetConfig) -> Option<&str> {
    match source_set.purpose {
        SourceSetPurpose::Configuration => None,
        SourceSetPurpose::Extension => Some(source_set.name.as_str()),
    }
}

fn ensure_platform_success(
    action: &str,
    source_set: &SourceSetConfig,
    result: &PlatformCommandResult,
) -> Result<(), AppError> {
    if result.process.exit_code == 0 {
        return Ok(());
    }

    let mut details = vec![format!(
        "{action} failed for source-set '{}' with exit code {}",
        source_set.name, result.process.exit_code
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
        .filter(|log| !log.trim().is_empty())
    {
        details.push(format!("platform log: {}", log.trim()));
    }
    if let Some(path) = &result.platform_log_path {
        details.push(format!("platform log path: {}", path.display()));
    }

    Err(AppError::Platform(details.join("; ")))
}

fn fail_with_remaining_steps(
    started: Instant,
    mut completed_steps: Vec<BuildStep>,
    remaining: Vec<&SourceSetConfig>,
    failed_source_set: &SourceSetConfig,
    failed_mode: BuildMode,
    message: String,
) -> BuildResult {
    completed_steps.push(BuildStep {
        source_set: failed_source_set.name.clone(),
        mode: failed_mode,
        ok: false,
        message: Some(message),
        duration_ms: 0,
    });

    for source_set in remaining.into_iter().skip(1) {
        completed_steps.push(BuildStep {
            source_set: source_set.name.clone(),
            mode: BuildMode::Skipped,
            ok: false,
            message: Some("aborted after previous failure".to_owned()),
            duration_ms: 0,
        });
    }

    BuildResult {
        ok: false,
        steps: completed_steps,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

fn render_text_result(result: &BuildResult, presenter: &Presenter, succeeded: bool) {
    for step in &result.steps {
        let mode = match &step.mode {
            BuildMode::Full => "full",
            BuildMode::Partial { file_count } => {
                presenter.print_info(&format!(
                    "{}: partial ({file_count} files) - {}",
                    step.source_set,
                    step.message.as_deref().unwrap_or("ok")
                ));
                continue;
            }
            BuildMode::Skipped => "skipped",
        };

        presenter.print_info(&format!(
            "{}: {mode} - {}",
            step.source_set,
            step.message.as_deref().unwrap_or("ok")
        ));
    }

    if !succeeded {
        presenter.print_info("Build failed");
    } else if result
        .steps
        .iter()
        .all(|step| matches!(step.mode, BuildMode::Skipped) && step.ok)
    {
        presenter.print_ok("Build completed: no changes");
    } else {
        presenter.print_ok("Build completed successfully");
    }
}

#[cfg(test)]
mod tests {
    use super::{run_build, BUILD_COMMAND, SUPPORTED_BUILD_ERROR};
    use crate::change_detection::hash_storage::HashStorage;
    use crate::change_detection::source_sets::SourceSetsService;
    use crate::cli::args::BuildArgs;
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, PlatformToolConfig, SourceFormat, SourceSetConfig,
        SourceSetPurpose, TestsConfig, ToolsConfig,
    };
    use crate::domain::build::BuildMode;
    use crate::output::json::Envelope;
    use crate::support::error::AppError;
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
            format,
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
            tests: TestsConfig::default(),
        }
    }

    fn create_source_tree(base_path: &Path) {
        fs::create_dir_all(base_path.join("main").join("Catalogs.Items")).expect("main dir");
        fs::create_dir_all(base_path.join("ext").join("CommonModules")).expect("ext dir");
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

    #[cfg(unix)]
    #[test]
    fn rejects_unsupported_matrix_early() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        let calls = dir.path().join("calls.log");
        write_designer_script(&script, &calls, None);
        let config = build_config(
            dir.path(),
            dir.path().join("work").as_path(),
            &script,
            20,
            SourceFormat::Edt,
            BuilderBackend::Designer,
        );

        let failure = run_build(
            &config,
            &BuildArgs {
                full_rebuild: false,
            },
        )
        .expect_err("failure");
        assert!(
            matches!(failure.error, AppError::Validation(ref msg) if msg == SUPPORTED_BUILD_ERROR)
        );
        assert!(!calls.exists());
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

        assert!(matches!(failure.error, AppError::Runtime(_)));
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
        let calls_text = fs::read_to_string(&calls).expect("calls");

        assert!(failure.result.steps[0].ok);
        assert!(!failure.result.steps[1].ok);
        assert!(failure.result.steps[1]
            .message
            .as_deref()
            .expect("message")
            .contains("exit code 17"));
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

        let envelope = Envelope::ok(BUILD_COMMAND, result.duration_ms, result);
        let json = serde_json::to_string(&envelope).expect("json");
        assert!(json.contains("\"command\":\"build\""));
    }
}
