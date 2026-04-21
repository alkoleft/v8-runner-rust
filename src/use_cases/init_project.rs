use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::Instant;

use tracing::debug;

use crate::config::model::{
    AppConfig, BuilderBackend, SourceFormat, SourceSetConfig, SourceSetPurpose,
};
use crate::domain::init::{InitResult, InitStep, InitStepStatus};
use crate::platform::designer::DesignerDsl;
use crate::platform::edt::EdtDsl;
use crate::platform::ibcmd::{
    IbcmdConnection, IbcmdDsl, IbcmdInfobaseCreateOutcome, IbcmdInfobaseCreateStatus,
};
use crate::platform::locator::UtilityType;
use crate::platform::result::PlatformCommandResult;
use crate::platform::utilities::PlatformUtilities;
use crate::support::error::AppError;
use crate::use_cases::context::ExecutionContext;
use crate::use_cases::request::InitRequest;
use crate::use_cases::result::{UseCaseError, UseCaseFailure, UseCaseResult};

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
    run_init(config)
}

pub(crate) type InitExecutionFailure = UseCaseFailure<InitResult>;
const EDT_WORKSPACE_MARKER: &str = ".v8tr-initialized";

fn run_init(config: &AppConfig) -> UseCaseResult<InitResult> {
    let started = Instant::now();
    let mut utilities = PlatformUtilities::from_config(config);
    let mut steps = Vec::new();
    let mut first_error: Option<UseCaseError> = None;

    record_step(
        &mut steps,
        &mut first_error,
        ensure_infobase(config, &mut utilities),
    );
    record_step(
        &mut steps,
        &mut first_error,
        ensure_edt_workspace(config, &mut utilities),
    );

    let result = InitResult {
        ok: first_error.is_none(),
        steps,
        duration_ms: started.elapsed().as_millis() as u64,
    };

    match first_error {
        Some(error) => Err(InitExecutionFailure::with_payload(error, result)),
        None => Ok(result),
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
    steps.push(outcome.step);
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

fn ensure_infobase(config: &AppConfig, utilities: &mut PlatformUtilities) -> StepOutcome {
    let Some(infobase_dir) = config.v8_connection().file_path().map(PathBuf::from) else {
        return match config.builder {
            BuilderBackend::Designer => StepOutcome::skipped(
                "infobase",
                "create",
                Instant::now(),
                "server infobase connection detected; automatic creation is not supported for builder=DESIGNER",
            ),
            BuilderBackend::Ibcmd => ensure_server_infobase(config, utilities),
        };
    };

    ensure_file_infobase(config, utilities, &infobase_dir)
}

fn ensure_file_infobase(
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

    let command_result = match create_infobase(config, utilities) {
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
                format!("infobase created: {}", marker.display()),
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

fn ensure_server_infobase(config: &AppConfig, utilities: &mut PlatformUtilities) -> StepOutcome {
    let started = Instant::now();
    match create_infobase(config, utilities) {
        Ok(outcome) => match outcome.status {
            IbcmdInfobaseCreateStatus::Created => StepOutcome::ok(
                "infobase",
                "create",
                started,
                format!(
                    "server infobase ensured via ibcmd: {}",
                    config.infobase.connection
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

fn ensure_edt_workspace(config: &AppConfig, utilities: &mut PlatformUtilities) -> StepOutcome {
    let started = Instant::now();
    if config.format != SourceFormat::Edt {
        return StepOutcome::skipped(
            "edt_workspace",
            "import",
            started,
            "EDT workspace initialization is not applicable for format=DESIGNER",
        );
    }

    let workspace = config.work_path.join("edt-workspace");
    let marker = edt_workspace_marker_path(&workspace);
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
        return StepOutcome::skipped(
            "edt_workspace",
            "import",
            started,
            format!("workspace already initialized: {}", workspace.display()),
        );
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

    let binary = match utilities.locate(UtilityType::EdtCli) {
        Ok(location) => location.path,
        Err(error) => {
            return StepOutcome::failed(
                "edt_workspace",
                "import",
                started,
                AppError::Platform(error.to_string()),
            )
        }
    };

    let dsl = if config.tools.edt_cli.interactive_mode {
        match EdtDsl::new_interactive(
            binary,
            workspace.clone(),
            Duration::from_millis(config.tools.edt_cli.startup_timeout_ms),
            Duration::from_millis(config.tools.edt_cli.command_timeout_ms),
        ) {
            Ok(dsl) => dsl,
            Err(error) => {
                return StepOutcome::failed(
                    "edt_workspace",
                    "import",
                    started,
                    AppError::Platform(error.to_string()),
                )
            }
        }
    } else {
        EdtDsl::new(
            binary,
            workspace.clone(),
            utilities.runner_for(UtilityType::EdtCli),
        )
    };
    debug!("[EDT] Инициализация workspace: {}", workspace.display());
    for source_set in ordered_source_sets(config) {
        let source_path = resolve_source_set_path(config, source_set);
        debug!("[EDT] Импорт проекта: {}", source_set.name);
        match dsl.import_project(&source_path) {
            Ok(result) => {
                if let Err(error) =
                    ensure_platform_success("import EDT project", &source_set.name, &result)
                {
                    return StepOutcome::failed("edt_workspace", "import", started, error);
                }
            }
            Err(error) => {
                return StepOutcome::failed(
                    "edt_workspace",
                    "import",
                    started,
                    AppError::Platform(error.to_string()),
                )
            }
        }
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
        format!("workspace initialized: {}", workspace.display()),
    )
}

fn create_infobase_via_designer(
    config: &AppConfig,
    utilities: &mut PlatformUtilities,
) -> Result<IbcmdInfobaseCreateOutcome, AppError> {
    let binary = utilities
        .locate(UtilityType::V8)
        .map_err(|error| AppError::Platform(error.to_string()))?
        .path;
    DesignerDsl::new(
        binary,
        config.v8_connection(),
        utilities.runner_for(UtilityType::V8),
        None,
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
    .map_err(|error| AppError::Platform(error.to_string()))
}

fn create_infobase_via_ibcmd(
    config: &AppConfig,
    utilities: &mut PlatformUtilities,
) -> Result<IbcmdInfobaseCreateOutcome, AppError> {
    let binary = utilities
        .locate(UtilityType::Ibcmd)
        .map_err(|error| AppError::Platform(error.to_string()))?
        .path;
    let connection = IbcmdConnection::from_infobase(&config.infobase).map_err(|error| match error {
        crate::platform::ibcmd::IbcmdError::MissingServerDbmsField(_) => {
            AppError::Validation(error.to_string())
        }
        crate::platform::ibcmd::IbcmdError::Spawn(_) => AppError::Platform(error.to_string()),
    })?;
    IbcmdDsl::new(binary, connection, utilities.runner_for(UtilityType::Ibcmd))
        .ensure_infobase_create()
        .map_err(|error| AppError::Platform(error.to_string()))
}

fn create_infobase(
    config: &AppConfig,
    utilities: &mut PlatformUtilities,
) -> Result<IbcmdInfobaseCreateOutcome, AppError> {
    match config.builder {
        BuilderBackend::Designer => create_infobase_via_designer(config, utilities),
        BuilderBackend::Ibcmd => create_infobase_via_ibcmd(config, utilities),
    }
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
        TestsConfig, ToolsConfig,
    };
    use std::path::{Path, PathBuf};

    fn sample_config() -> AppConfig {
        AppConfig {
            base_path: PathBuf::from("/tmp/base"),
            work_path: PathBuf::from("/tmp/work"),
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

        let result = super::run_init(&config).expect("server init should skip infobase create");

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
}
