use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use tracing::debug;

use crate::config::model::{
    AppConfig, BuilderBackend, SourceFormat, SourceSetConfig, SourceSetPurpose,
};
use crate::domain::init::{InitResult, InitStep, InitStepStatus};
use crate::platform::designer::DesignerDsl;
use crate::platform::edt::EdtDsl;
use crate::platform::edt_session::{EdtSessionHostOptions, EdtSessionManager};
use crate::platform::ibcmd::{
    IbcmdConnection, IbcmdDsl, IbcmdInfobaseCreateOutcome, IbcmdInfobaseCreateStatus,
};
use crate::platform::locator::UtilityType;
use crate::platform::result::PlatformCommandResult;
use crate::platform::utilities::PlatformUtilities;
use crate::support::error::AppError;
use crate::use_cases::context::{ExecutionContext, InterruptionSafetyClass};
use crate::use_cases::interruption;
use crate::use_cases::progress::{log_live_stage, log_live_stage_status, LiveStageStatus};
use crate::use_cases::request::InitRequest;
use crate::use_cases::result::{UseCaseError, UseCaseFailure, UseCaseResult};
use crate::use_cases::tool_extension;

pub fn execute(
    context: &ExecutionContext,
    config: &AppConfig,
    _args: &InitRequest,
) -> UseCaseResult<InitResult> {
    debug!(
        command = context.command().as_str(),
        transport = ?context.transport(),
        "executing init use case"
    );
    run_init(context, config)
}

pub(crate) type InitExecutionFailure = UseCaseFailure<InitResult>;
const EDT_WORKSPACE_MARKER: &str = ".v8tr-initialized";

fn run_init(context: &ExecutionContext, config: &AppConfig) -> UseCaseResult<InitResult> {
    let started = Instant::now();
    let mut utilities = PlatformUtilities::from_config(config);
    let mut steps = Vec::new();
    let mut first_error: Option<UseCaseError> = None;

    record_step(
        &mut steps,
        &mut first_error,
        ensure_infobase(context, config, &mut utilities),
    );
    record_step(
        &mut steps,
        &mut first_error,
        ensure_edt_workspace(context, config, &mut utilities),
    );

    let result = init_result(started, steps, first_error.is_none());

    match first_error {
        Some(error) => Err(InitExecutionFailure::with_payload(error, result)),
        None => Ok(result),
    }
}

fn init_result(started: Instant, steps: Vec<InitStep>, ok: bool) -> InitResult {
    InitResult {
        ok,
        steps,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

fn record_step(
    steps: &mut Vec<InitStep>,
    first_error: &mut Option<UseCaseError>,
    outcome: StepOutcome,
) {
    if first_error.is_none() {
        *first_error = outcome.error.clone();
    }
    log_step_status(&outcome.step);
    steps.push(outcome.step);
}

fn log_step_status(step: &InitStep) {
    let Some(label) = live_step_label(step) else {
        return;
    };
    let Some((status, marker)) = live_status_marker(&step.status) else {
        return;
    };
    log_live_stage_status(
        label,
        status,
        &format!("{marker} {}: {}", step.target, step.action),
    );
}

fn live_step_label(step: &InitStep) -> Option<&'static str> {
    match (step.target.as_str(), step.action.as_str()) {
        ("infobase", "create") => Some("init: infobase create"),
        ("edt_workspace", "import") if !matches!(step.status, InitStepStatus::Skipped) => {
            Some("init: edt import")
        }
        _ => None,
    }
}

fn live_status_marker(status: &InitStepStatus) -> Option<(LiveStageStatus, &'static str)> {
    match status {
        InitStepStatus::Ok => Some((LiveStageStatus::Succeeded, "✓")),
        InitStepStatus::Failed => Some((LiveStageStatus::Failed, "✗")),
        InitStepStatus::Skipped => None,
    }
}

#[derive(Debug, Clone)]
struct StepOutcome {
    step: InitStep,
    error: Option<UseCaseError>,
}

impl StepOutcome {
    fn ok(target: &str, action: &str, started: Instant, message: impl Into<String>) -> Self {
        Self {
            step: InitStep {
                target: target.to_owned(),
                action: action.to_owned(),
                status: InitStepStatus::Ok,
                message: Some(message.into()),
                duration_ms: started.elapsed().as_millis() as u64,
            },
            error: None,
        }
    }

    fn skipped(target: &str, action: &str, started: Instant, message: impl Into<String>) -> Self {
        Self {
            step: InitStep {
                target: target.to_owned(),
                action: action.to_owned(),
                status: InitStepStatus::Skipped,
                message: Some(message.into()),
                duration_ms: started.elapsed().as_millis() as u64,
            },
            error: None,
        }
    }

    fn failed(
        target: &str,
        action: &str,
        started: Instant,
        error: impl Into<UseCaseError>,
    ) -> Self {
        let error = error.into();
        Self {
            step: InitStep {
                target: target.to_owned(),
                action: action.to_owned(),
                status: InitStepStatus::Failed,
                message: Some(error.message().to_owned()),
                duration_ms: started.elapsed().as_millis() as u64,
            },
            error: Some(error),
        }
    }
}

fn ensure_infobase(
    context: &ExecutionContext,
    config: &AppConfig,
    utilities: &mut PlatformUtilities,
) -> StepOutcome {
    let Some(infobase_dir) = config.v8_connection().file_path().map(PathBuf::from) else {
        return match config.builder {
            BuilderBackend::Designer => StepOutcome::skipped(
                "infobase",
                "create",
                Instant::now(),
                "server infobase connection detected; automatic creation is not supported for builder=DESIGNER",
            ),
            BuilderBackend::Ibcmd => ensure_server_infobase(context, config, utilities),
        };
    };

    ensure_file_infobase(context, config, utilities, &infobase_dir)
}

fn ensure_file_infobase(
    context: &ExecutionContext,
    config: &AppConfig,
    utilities: &mut PlatformUtilities,
    infobase_dir: &Path,
) -> StepOutcome {
    let started = Instant::now();
    let marker = infobase_marker_path(infobase_dir);
    debug!("[Инфобаза] Подготовка: {}", infobase_dir.display());
    if marker.exists() {
        return StepOutcome::skipped(
            "infobase",
            "create",
            started,
            format!("infobase already exists: {}", marker.display()),
        );
    }

    if let Err(error) = prepare_infobase_parent(&infobase_dir) {
        return StepOutcome::failed("infobase", "create", started, error);
    }

    if let Some(outcome) =
        interruption_step_outcome(context, "infobase", "create", started, "infobase create")
    {
        return outcome;
    }

    log_live_stage("init: infobase create", "[Platform] creating infobase");
    let command_result = match create_infobase(context, config, utilities) {
        Ok(outcome) => outcome,
        Err(error) => return StepOutcome::failed("infobase", "create", started, error),
    };

    match command_result.status {
        IbcmdInfobaseCreateStatus::Failed => {
            if let Err(error) =
                ensure_platform_success("create infobase", "infobase", &command_result.result)
            {
                return StepOutcome::failed("infobase", "create", started, error);
            }
            unreachable!("failed status must return platform error");
        }
        IbcmdInfobaseCreateStatus::Created => {
            if !marker.exists() {
                return StepOutcome::failed(
                    "infobase",
                    "create",
                    started,
                    missing_infobase_marker_error(
                        "infobase creation did not produce marker file",
                        &marker,
                        &command_result.result,
                    ),
                );
            }
            StepOutcome::ok(
                "infobase",
                "create",
                started,
                with_deferred_warning(
                    format!("infobase created: {}", marker.display()),
                    &command_result.result,
                ),
            )
        }
        IbcmdInfobaseCreateStatus::AlreadyExists => {
            if !marker.exists() {
                return StepOutcome::failed(
                    "infobase",
                    "create",
                    started,
                    missing_infobase_marker_error(
                        "infobase create reported an existing file infobase but marker file is missing",
                        &marker,
                        &command_result.result,
                    ),
                );
            }
            StepOutcome::skipped(
                "infobase",
                "create",
                started,
                format!("infobase already exists: {}", marker.display()),
            )
        }
    }
}

fn ensure_server_infobase(
    context: &ExecutionContext,
    config: &AppConfig,
    utilities: &mut PlatformUtilities,
) -> StepOutcome {
    let started = Instant::now();
    if let Some(outcome) =
        interruption_step_outcome(context, "infobase", "create", started, "infobase create")
    {
        return outcome;
    }
    log_live_stage("init: infobase create", "[ibcmd] ensuring server infobase");
    match create_infobase(context, config, utilities) {
        Ok(outcome) => match outcome.status {
            IbcmdInfobaseCreateStatus::Created => StepOutcome::ok(
                "infobase",
                "create",
                started,
                with_deferred_warning(
                    format!(
                        "server infobase ensured via ibcmd: {}",
                        config.infobase.connection
                    ),
                    &outcome.result,
                ),
            ),
            IbcmdInfobaseCreateStatus::AlreadyExists => StepOutcome::skipped(
                "infobase",
                "create",
                started,
                format!(
                    "server infobase already exists: {}",
                    config.infobase.connection
                ),
            ),
            IbcmdInfobaseCreateStatus::Failed => {
                match ensure_platform_success("create infobase", "infobase", &outcome.result) {
                    Ok(()) => unreachable!("failed status must return platform error"),
                    Err(error) => StepOutcome::failed("infobase", "create", started, error),
                }
            }
        },
        Err(error) => StepOutcome::failed("infobase", "create", started, error),
    }
}

fn ensure_edt_workspace(
    context: &ExecutionContext,
    config: &AppConfig,
    utilities: &mut PlatformUtilities,
) -> StepOutcome {
    let started = Instant::now();
    let tool_extension_path = tool_extension::client_mcp_edt_source_path(config);
    if config.format != SourceFormat::Edt && tool_extension_path.is_none() {
        return StepOutcome::skipped(
            "edt_workspace",
            "import",
            started,
            "EDT workspace initialization is not applicable for format=DESIGNER",
        );
    }

    let workspace = config.work_path.join("edt-workspace");
    let marker = edt_workspace_marker_path(&workspace);
    let project_source_import = if config.format == SourceFormat::Edt && !marker.exists() {
        ProjectSourceImport::Include
    } else {
        ProjectSourceImport::Skip
    };
    let projects = edt_import_projects(config, project_source_import, tool_extension_path);
    if workspace.exists() && !workspace.is_dir() {
        return StepOutcome::failed(
            "edt_workspace",
            "import",
            started,
            AppError::Runtime(format!(
                "EDT workspace path exists but is not a directory: {}",
                workspace.display()
            )),
        );
    }
    if workspace.exists() && marker.exists() {
        if projects.is_empty() {
            return StepOutcome::skipped(
                "edt_workspace",
                "import",
                started,
                format!("workspace already initialized: {}", workspace.display()),
            );
        }
    }

    if let Err(error) = std::fs::create_dir_all(&workspace) {
        return StepOutcome::failed(
            "edt_workspace",
            "import",
            started,
            AppError::Runtime(format!(
                "failed to create EDT workspace '{}': {error}",
                workspace.display()
            )),
        );
    }

    if let Some(outcome) = interruption_step_outcome(
        context,
        "edt_workspace",
        "import",
        started,
        "EDT workspace import",
    ) {
        return outcome;
    }

    let binary = match utilities.locate(UtilityType::EdtCli) {
        Ok(location) => location.path,
        Err(error) => {
            return StepOutcome::failed("edt_workspace", "import", started, AppError::from(error))
        }
    };

    let dsl = if config.tools.edt_cli.interactive_mode {
        match EdtSessionManager::for_config(config, EdtSessionHostOptions::for_cli_command(config))
        {
            Ok(manager) => match EdtDsl::new_shared_session(
                binary,
                workspace.clone(),
                Arc::new(manager),
                Duration::from_millis(config.tools.edt_cli.startup_timeout_ms),
                Duration::from_millis(config.tools.edt_cli.command_timeout_ms),
            ) {
                Ok(dsl) => dsl.with_execution_policy(
                    context.process_policy(InterruptionSafetyClass::GracefulThenKill, None),
                ),
                Err(error) => {
                    return StepOutcome::failed(
                        "edt_workspace",
                        "import",
                        started,
                        AppError::from(error),
                    )
                }
            },
            Err(error) => {
                return StepOutcome::failed(
                    "edt_workspace",
                    "import",
                    started,
                    AppError::from(error),
                )
            }
        }
    } else {
        EdtDsl::new(
            binary,
            workspace.clone(),
            utilities.runner_for(UtilityType::EdtCli),
        )
        .with_execution_policy(
            context.process_policy(InterruptionSafetyClass::GracefulThenKill, None),
        )
    };
    debug!("[EDT] Инициализация workspace: {}", workspace.display());
    let mut imported_projects = Vec::new();
    for project in projects {
        if let Some(outcome) = interruption_step_outcome(
            context,
            "edt_workspace",
            "import",
            started,
            "EDT project import",
        ) {
            return outcome;
        }
        debug!("[EDT] Импорт проекта: {}", project.name);
        log_live_stage(
            "init: edt import",
            &format!("[EDT] importing source-set project '{}'", project.name),
        );
        match dsl.import_project(&project.path) {
            Ok(result) => {
                if let Err(error) =
                    ensure_platform_success("import EDT project", &project.name, &result)
                {
                    return StepOutcome::failed("edt_workspace", "import", started, error);
                }
            }
            Err(error) => {
                return StepOutcome::failed(
                    "edt_workspace",
                    "import",
                    started,
                    AppError::from(error),
                )
            }
        }
        imported_projects.push(project.name);
    }

    if let Err(error) = std::fs::write(&marker, b"initialized\n") {
        return StepOutcome::failed(
            "edt_workspace",
            "import",
            started,
            AppError::Runtime(format!(
                "failed to persist EDT workspace marker '{}': {error}",
                marker.display()
            )),
        );
    }

    StepOutcome::ok(
        "edt_workspace",
        "import",
        started,
        with_optional_warning(
            edt_workspace_initialized_message(&workspace, &imported_projects),
            context_deferred_warning(context),
        ),
    )
}

fn edt_workspace_initialized_message(workspace: &Path, imported_projects: &[String]) -> String {
    let mut message = format!("workspace initialized: {}", workspace.display());
    if !imported_projects.is_empty() {
        message.push_str("; imported EDT projects: ");
        message.push_str(&imported_projects.join(", "));
    }
    message
}

fn create_infobase_via_designer(
    context: &ExecutionContext,
    config: &AppConfig,
    utilities: &mut PlatformUtilities,
) -> Result<IbcmdInfobaseCreateOutcome, AppError> {
    let binary = utilities
        .locate(UtilityType::V8)
        .map_err(AppError::from)?
        .path;
    DesignerDsl::new(
        binary,
        config.v8_connection(),
        utilities.runner_for(UtilityType::V8),
        None,
    )
    .with_execution_policy(
        context.process_policy(InterruptionSafetyClass::CriticalNonAbortable, None),
    )
    .create_infobase()
    .map(|result| IbcmdInfobaseCreateOutcome {
        status: if result.process.exit_code == 0 {
            IbcmdInfobaseCreateStatus::Created
        } else {
            IbcmdInfobaseCreateStatus::Failed
        },
        result,
    })
    .map_err(AppError::from)
}

fn create_infobase_via_ibcmd(
    context: &ExecutionContext,
    config: &AppConfig,
    utilities: &mut PlatformUtilities,
) -> Result<IbcmdInfobaseCreateOutcome, AppError> {
    let binary = utilities
        .locate(UtilityType::Ibcmd)
        .map_err(AppError::from)?
        .path;
    let connection = IbcmdConnection::from_infobase(&config.infobase).map_err(AppError::from)?;
    IbcmdDsl::new(binary, connection, utilities.runner_for(UtilityType::Ibcmd))
        .with_execution_policy(
            context.process_policy(InterruptionSafetyClass::CriticalNonAbortable, None),
        )
        .ensure_infobase_create()
        .map_err(AppError::from)
}

fn create_infobase(
    context: &ExecutionContext,
    config: &AppConfig,
    utilities: &mut PlatformUtilities,
) -> Result<IbcmdInfobaseCreateOutcome, AppError> {
    match config.builder {
        BuilderBackend::Designer => create_infobase_via_designer(context, config, utilities),
        BuilderBackend::Ibcmd => create_infobase_via_ibcmd(context, config, utilities),
    }
}

fn interruption_step_outcome(
    context: &ExecutionContext,
    target: &str,
    action: &str,
    started: Instant,
    safe_point: &str,
) -> Option<StepOutcome> {
    context.interruption().map(|interruption| {
        let message =
            interruption::interruption_before_safe_point_message(context, interruption, safe_point);
        StepOutcome::failed(target, action, started, AppError::Runtime(message))
    })
}

fn with_deferred_warning(message: String, result: &PlatformCommandResult) -> String {
    with_optional_warning(message, deferred_interruption_warning(result))
}

fn with_optional_warning(message: String, warning: Option<String>) -> String {
    match warning {
        Some(warning) => format!("{message}; {warning}"),
        None => message,
    }
}

fn deferred_interruption_warning(result: &PlatformCommandResult) -> Option<String> {
    interruption::deferred_process_interruption_warning("operation completed successfully", result)
}

fn context_deferred_warning(context: &ExecutionContext) -> Option<String> {
    context.interruption().map(|interruption| {
        interruption::deferred_interruption_warning_for_command(
            "operation completed successfully",
            context.command(),
            interruption,
        )
    })
}

fn prepare_infobase_parent(path: &Path) -> Result<(), AppError> {
    let Some(parent) = path.parent() else {
        return Err(AppError::Runtime(format!(
            "infobase path '{}' has no parent directory",
            path.display()
        )));
    };
    std::fs::create_dir_all(parent).map_err(|error| {
        AppError::Runtime(format!(
            "failed to prepare infobase parent '{}': {error}",
            parent.display()
        ))
    })
}

fn infobase_marker_path(path: &Path) -> PathBuf {
    path.join("1Cv8.1CD")
}

fn edt_workspace_marker_path(path: &Path) -> PathBuf {
    path.join(EDT_WORKSPACE_MARKER)
}

fn resolve_source_set_path(config: &AppConfig, source_set: &SourceSetConfig) -> PathBuf {
    if source_set.path.is_absolute() {
        source_set.path.clone()
    } else {
        config.base_path.join(&source_set.path)
    }
}

fn ordered_source_sets(config: &AppConfig) -> Vec<&SourceSetConfig> {
    let mut configuration = Vec::new();
    let mut extensions = Vec::new();
    let mut external_processors = Vec::new();
    let mut external_reports = Vec::new();

    for source_set in &config.source_sets {
        match source_set.purpose {
            SourceSetPurpose::Configuration => configuration.push(source_set),
            SourceSetPurpose::Extension => extensions.push(source_set),
            SourceSetPurpose::ExternalDataProcessors => external_processors.push(source_set),
            SourceSetPurpose::ExternalReports => external_reports.push(source_set),
        }
    }

    configuration.extend(extensions);
    configuration.extend(external_processors);
    configuration.extend(external_reports);
    configuration
}

#[derive(Debug)]
struct EdtImportProject {
    name: String,
    path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectSourceImport {
    Include,
    Skip,
}

fn edt_import_projects(
    config: &AppConfig,
    project_source_import: ProjectSourceImport,
    tool_extension_path: Option<PathBuf>,
) -> Vec<EdtImportProject> {
    let mut projects = Vec::new();
    if project_source_import == ProjectSourceImport::Include {
        projects.extend(ordered_source_sets(config).into_iter().map(|source_set| {
            EdtImportProject {
                name: source_set.name.clone(),
                path: resolve_source_set_path(config, source_set),
            }
        }));
    }

    if let Some(path) = tool_extension_path {
        projects.push(EdtImportProject {
            name: "tool:client_mcp".to_owned(),
            path,
        });
    }

    projects
}

fn ensure_platform_success(
    action: &str,
    target: &str,
    result: &PlatformCommandResult,
) -> Result<(), AppError> {
    if result.process.exit_code == 0 {
        return Ok(());
    }

    let mut details = vec![format!(
        "{action} failed for '{target}' with exit code {}",
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
        .filter(|log| !log.trim().is_empty())
    {
        details.push(format!("platform log: {}", log.trim()));
    }
    if let Some(path) = &result.platform_log_path {
        details.push(format!("platform log path: {}", path.display()));
    }

    Err(AppError::Platform(details.join("; ")))
}

fn missing_infobase_marker_error(
    reason: &str,
    marker: &Path,
    result: &PlatformCommandResult,
) -> AppError {
    let mut details = vec![format!("{reason} '{}'", marker.display())];
    if !result.process.stdout.trim().is_empty() {
        details.push(format!("stdout: {}", result.process.stdout.trim()));
    }
    if !result.process.stderr.trim().is_empty() {
        details.push(format!("stderr: {}", result.process.stderr.trim()));
    }
    AppError::Runtime(details.join("; "))
}

#[cfg(test)]
mod tests {
    use super::{
        edt_workspace_marker_path, infobase_marker_path, ordered_source_sets, InitStepStatus,
    };
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, SourceFormat, SourceSetConfig, SourceSetPurpose,
        TestsConfig, ToolExtensionConfig, ToolExtensionInput, ToolExtensionSourceConfig,
        ToolsConfig,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    fn sample_config() -> AppConfig {
        AppConfig {
            base_path: PathBuf::from("/tmp/base"),
            work_path: PathBuf::from("/tmp/work"),
            execution_timeout: 300_000,
            format: SourceFormat::Edt,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
            source_sets: vec![
                SourceSetConfig {
                    name: "ext".to_owned(),
                    purpose: SourceSetPurpose::Extension,
                    path: PathBuf::from("ext"),
                },
                SourceSetConfig {
                    name: "main".to_owned(),
                    purpose: SourceSetPurpose::Configuration,
                    path: PathBuf::from("main"),
                },
            ],
            build: BuildConfig::default(),
            tools: ToolsConfig::default(),
            mcp: Default::default(),
            tests: TestsConfig::default(),
        }
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod");
    }

    #[cfg(unix)]
    fn write_one_shot_edt_script(path: &Path, calls_log: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create dirs");
        }
        fs::write(
            path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
                calls_log.display()
            ),
        )
        .expect("write edt script");
        make_executable(path);
    }

    #[cfg(unix)]
    fn write_interactive_edt_script(path: &Path, calls_log: &Path) {
        write_interactive_edt_script_with_startup_delay(path, calls_log, 0);
    }

    #[cfg(unix)]
    fn write_interactive_edt_script_with_startup_delay(
        path: &Path,
        calls_log: &Path,
        startup_delay_ms: u64,
    ) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create dirs");
        }
        fs::write(
            path,
            format!(
                "#!/bin/sh\nset -eu\nprompt() {{ printf '1C:EDT>'; }}\ncurrent_dir=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"-data\" ]; then current_dir=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nsleep {}\nprintf 'START\\n' >> '{}'\ntrap 'printf \"EXIT\\\\n\" >> \"{}\"' EXIT\nprompt\nwhile IFS= read -r line; do\n  printf '%s\\n' \"$line\" >> '{}'\n  eval \"set -- $line\"\n  cmd=\"${{1:-}}\"\n  if [ \"$#\" -gt 0 ]; then shift; fi\n  case \"$cmd\" in\n    cd)\n      if [ \"$#\" -eq 0 ]; then\n        printf '%s\\n' \"$current_dir\"\n      else\n        current_dir=\"$1\"\n      fi\n      prompt\n      ;;\n    import)\n      prompt\n      ;;\n    *)\n      prompt\n      ;;\n  esac\ndone\n",
                startup_delay_ms as f64 / 1000.0,
                calls_log.display(),
                calls_log.display(),
                calls_log.display()
            ),
        )
        .expect("write interactive edt script");
        make_executable(path);
    }

    #[test]
    fn infobase_marker_uses_1cv8_1cd_file() {
        assert_eq!(
            infobase_marker_path(Path::new("/tmp/ib")),
            PathBuf::from("/tmp/ib/1Cv8.1CD")
        );
    }

    #[test]
    fn edt_workspace_marker_uses_internal_file_name() {
        assert_eq!(
            edt_workspace_marker_path(Path::new("/tmp/ws")),
            PathBuf::from("/tmp/ws/.v8tr-initialized")
        );
    }

    #[test]
    fn ordered_source_sets_puts_configuration_before_extensions() {
        let config = sample_config();
        let ordered = ordered_source_sets(&config);
        assert_eq!(ordered[0].name, "main");
        assert_eq!(ordered[1].name, "ext");
    }

    #[test]
    fn init_skips_infobase_creation_for_server_connection() {
        let mut config = sample_config();
        config.format = SourceFormat::Designer;
        config.infobase.connection = "Srvr=server;Ref=demo".to_owned();

        let result = super::run_init(
            &crate::use_cases::context::ExecutionContext::cli(
                crate::use_cases::context::CommandName::Init,
            ),
            &config,
        )
        .expect("server init should skip infobase create");

        assert!(result.ok);
        assert_eq!(result.steps.len(), 2);
        assert_eq!(result.steps[0].target, "infobase");
        assert_eq!(result.steps[0].action, "create");
        assert_eq!(result.steps[0].status, InitStepStatus::Skipped);
        assert_eq!(
            result.steps[0].message.as_deref(),
            Some(
                "server infobase connection detected; automatic creation is not supported for builder=DESIGNER"
            )
        );
        assert_eq!(result.steps[1].target, "edt_workspace");
        assert_eq!(result.steps[1].status, InitStepStatus::Skipped);
    }

    #[test]
    fn init_honors_interruption_before_infobase_create_safe_point() {
        let dir = tempdir().expect("tempdir");
        let mut config = sample_config();
        config.base_path = dir.path().join("base");
        config.work_path = dir.path().join("work");
        config.format = SourceFormat::Designer;
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let failure = super::run_init(
            &crate::use_cases::context::ExecutionContext::cli(
                crate::use_cases::context::CommandName::Init,
            )
            .with_cancellation(cancellation),
            &config,
        )
        .expect_err("interrupted init");
        let payload = failure.payload.expect("payload");

        assert_eq!(payload.steps[0].status, InitStepStatus::Failed);
        assert!(payload.steps[0]
            .message
            .as_deref()
            .expect("message")
            .contains("before entering infobase create safe point"));
    }

    #[cfg(unix)]
    #[test]
    fn init_uses_one_shot_edt_when_interactive_mode_is_disabled() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let main_dir = base.join("main");
        let ext_dir = base.join("ext");
        let edt_script = dir.path().join("edt").join("1cedtcli");
        let edt_calls = dir.path().join("edt-calls.log");
        fs::create_dir_all(&main_dir).expect("main dir");
        fs::create_dir_all(&ext_dir).expect("ext dir");
        write_one_shot_edt_script(&edt_script, &edt_calls);

        let mut config = sample_config();
        config.base_path = base;
        config.work_path = work.clone();
        config.infobase.connection = "Srvr=server;Ref=demo".to_owned();
        config.tools.edt_cli.path = Some(edt_script);
        config.tools.edt_cli.interactive_mode = false;

        let result = super::run_init(
            &crate::use_cases::context::ExecutionContext::cli(
                crate::use_cases::context::CommandName::Init,
            ),
            &config,
        )
        .expect("init");

        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");
        assert!(result.ok);
        assert!(edt_calls_text.contains("-command import --project"));
        assert_eq!(
            edt_calls_text.matches("-command import --project").count(),
            2
        );
        assert!(!edt_calls_text.contains("START"));
        assert!(edt_workspace_marker_path(&work.join("edt-workspace")).exists());
    }

    #[cfg(unix)]
    #[test]
    fn init_imports_edt_client_mcp_tool_extension_source_project() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let main_dir = base.join("main");
        let tool_dir = base.join("tool-client-mcp");
        let edt_script = dir.path().join("edt").join("1cedtcli");
        let edt_calls = dir.path().join("edt-calls.log");
        fs::create_dir_all(&main_dir).expect("main dir");
        fs::create_dir_all(&tool_dir).expect("tool dir");
        write_one_shot_edt_script(&edt_script, &edt_calls);

        let mut config = sample_config();
        config.base_path = base;
        config.work_path = work.clone();
        config.infobase.connection = "Srvr=server;Ref=demo".to_owned();
        config.source_sets = vec![SourceSetConfig {
            name: "main".to_owned(),
            purpose: SourceSetPurpose::Configuration,
            path: PathBuf::from("main"),
        }];
        config.tools.edt_cli.path = Some(edt_script);
        config.tools.edt_cli.interactive_mode = false;
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Source(ToolExtensionSourceConfig {
                path: tool_dir.clone(),
                format: Some(SourceFormat::Edt),
            }),
        });

        let result = super::run_init(
            &crate::use_cases::context::ExecutionContext::cli(
                crate::use_cases::context::CommandName::Init,
            ),
            &config,
        )
        .expect("init");

        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");
        assert!(result.ok);
        assert_eq!(
            edt_calls_text.matches("-command import --project").count(),
            2
        );
        assert!(edt_calls_text.contains(main_dir.display().to_string().as_str()));
        assert!(edt_calls_text.contains(tool_dir.display().to_string().as_str()));
        assert!(edt_workspace_marker_path(&work.join("edt-workspace")).exists());
    }

    #[cfg(unix)]
    #[test]
    fn init_imports_edt_client_mcp_tool_extension_for_designer_project() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let main_dir = base.join("main");
        let tool_dir = base.join("tool-client-mcp");
        let edt_script = dir.path().join("edt").join("1cedtcli");
        let edt_calls = dir.path().join("edt-calls.log");
        fs::create_dir_all(&main_dir).expect("main dir");
        fs::create_dir_all(&tool_dir).expect("tool dir");
        write_one_shot_edt_script(&edt_script, &edt_calls);

        let mut config = sample_config();
        config.base_path = base;
        config.work_path = work.clone();
        config.format = SourceFormat::Designer;
        config.infobase.connection = "Srvr=server;Ref=demo".to_owned();
        config.source_sets = vec![SourceSetConfig {
            name: "main".to_owned(),
            purpose: SourceSetPurpose::Configuration,
            path: PathBuf::from("main"),
        }];
        config.tools.edt_cli.path = Some(edt_script);
        config.tools.edt_cli.interactive_mode = false;
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Source(ToolExtensionSourceConfig {
                path: tool_dir.clone(),
                format: Some(SourceFormat::Edt),
            }),
        });

        let result = super::run_init(
            &crate::use_cases::context::ExecutionContext::cli(
                crate::use_cases::context::CommandName::Init,
            ),
            &config,
        )
        .expect("init");

        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");
        assert!(result.ok);
        assert_eq!(
            edt_calls_text.matches("-command import --project").count(),
            1
        );
        assert!(!edt_calls_text.contains(main_dir.display().to_string().as_str()));
        assert!(edt_calls_text.contains(tool_dir.display().to_string().as_str()));
        assert!(edt_workspace_marker_path(&work.join("edt-workspace")).exists());
    }

    #[cfg(unix)]
    #[test]
    fn init_imports_edt_client_mcp_tool_extension_into_existing_workspace() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let main_dir = base.join("main");
        let tool_dir = base.join("tool-client-mcp");
        let edt_script = dir.path().join("edt").join("1cedtcli");
        let edt_calls = dir.path().join("edt-calls.log");
        fs::create_dir_all(&main_dir).expect("main dir");
        fs::create_dir_all(&tool_dir).expect("tool dir");
        let workspace = work.join("edt-workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::write(edt_workspace_marker_path(&workspace), "initialized\n").expect("marker");
        write_one_shot_edt_script(&edt_script, &edt_calls);

        let mut config = sample_config();
        config.base_path = base;
        config.work_path = work.clone();
        config.infobase.connection = "Srvr=server;Ref=demo".to_owned();
        config.source_sets = vec![SourceSetConfig {
            name: "main".to_owned(),
            purpose: SourceSetPurpose::Configuration,
            path: PathBuf::from("main"),
        }];
        config.tools.edt_cli.path = Some(edt_script);
        config.tools.edt_cli.interactive_mode = false;
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Source(ToolExtensionSourceConfig {
                path: tool_dir.clone(),
                format: Some(SourceFormat::Edt),
            }),
        });

        let result = super::run_init(
            &crate::use_cases::context::ExecutionContext::cli(
                crate::use_cases::context::CommandName::Init,
            ),
            &config,
        )
        .expect("init");

        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");
        assert!(result.ok);
        assert_eq!(
            edt_calls_text.matches("-command import --project").count(),
            1
        );
        assert!(!edt_calls_text.contains(main_dir.display().to_string().as_str()));
        assert!(edt_calls_text.contains(tool_dir.display().to_string().as_str()));
        assert!(edt_workspace_marker_path(&work.join("edt-workspace")).exists());
    }

    #[cfg(unix)]
    #[test]
    fn init_cli_interactive_auto_start_remains_lazy_without_edt_commands() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let edt_script = dir.path().join("edt").join("1cedtcli");
        let edt_calls = dir.path().join("edt-calls.log");
        fs::create_dir_all(&base).expect("base dir");
        write_interactive_edt_script(&edt_script, &edt_calls);

        let mut config = sample_config();
        config.base_path = base;
        config.work_path = work.clone();
        config.infobase.connection = "Srvr=server;Ref=demo".to_owned();
        config.source_sets = vec![];
        config.tools.edt_cli.path = Some(edt_script);
        config.tools.edt_cli.interactive_mode = true;
        config.tools.edt_cli.auto_start = true;

        let result = super::run_init(
            &crate::use_cases::context::ExecutionContext::cli(
                crate::use_cases::context::CommandName::Init,
            ),
            &config,
        )
        .expect("init");

        assert!(result.ok);
        assert!(
            !edt_calls.exists()
                || fs::read_to_string(&edt_calls)
                    .expect("edt calls")
                    .trim()
                    .is_empty()
        );
        assert!(edt_workspace_marker_path(&work.join("edt-workspace")).exists());
    }

    #[cfg(unix)]
    #[test]
    fn init_cli_shared_session_does_not_charge_startup_against_first_command_timeout() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let main_dir = base.join("main");
        let ext_dir = base.join("ext");
        let edt_script = dir.path().join("edt").join("1cedtcli");
        let edt_calls = dir.path().join("edt-calls.log");
        fs::create_dir_all(&main_dir).expect("main dir");
        fs::create_dir_all(&ext_dir).expect("ext dir");
        write_interactive_edt_script_with_startup_delay(&edt_script, &edt_calls, 150);

        let mut config = sample_config();
        config.base_path = base;
        config.work_path = work.clone();
        config.infobase.connection = "Srvr=server;Ref=demo".to_owned();
        config.tools.edt_cli.path = Some(edt_script);
        config.tools.edt_cli.interactive_mode = true;
        config.tools.edt_cli.startup_timeout_ms = 500;
        config.tools.edt_cli.command_timeout_ms = 50;

        let result = super::run_init(
            &crate::use_cases::context::ExecutionContext::cli(
                crate::use_cases::context::CommandName::Init,
            ),
            &config,
        )
        .expect("init");

        let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");
        assert!(result.ok);
        assert_eq!(edt_calls_text.matches("START").count(), 1);
        assert_eq!(edt_calls_text.matches("import --project").count(), 2);
    }
}
