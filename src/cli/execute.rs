use std::path::Path;
use std::time::Instant;

use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::cli::args::{
    ArtifactsArgs, BuildArgs, Command, ConvertArgs, DesignerConfigSyntaxArgs,
    DesignerModulesSyntaxArgs, DumpArgs, ExtensionsArgs, LaunchArgs, LaunchOptionsArgs, LoadArgs,
    SyntaxArgs, SyntaxTarget, TestArgs, TestRunner, TestScope, TestYaxunitArgs,
};
use crate::cli::signal::CliSignalGuard;
use crate::config::model::{AppConfig, SourceSetPurpose};
use crate::domain::artifact::{
    ArtifactRef, ARTIFACT_ROLE_PACKAGE_FILE, ARTIFACT_ROLE_PLATFORM_LOG,
};
use crate::domain::artifacts::{ArtifactBuildMode, ArtifactsResult};
use crate::domain::build::{BuildMode, BuildResult};
use crate::domain::convert::{ConvertDirection, ConvertResult, ConvertScope};
use crate::domain::dump::{DumpMode, DumpResult};
use crate::domain::execution::{
    ExecutionError, ExecutionInterruptionDetails, ExecutionStepStatus, ExecutionTimeouts,
    StepResult,
};
use crate::domain::init::{InitResult, InitStep, InitStepStatus};
use crate::domain::issue::{Issue, IssueSeverity};
use crate::domain::launch::{LaunchMode, LaunchResult};
use crate::domain::load::{LoadMode, LoadResult};
use crate::domain::runner::{
    ExecutionPolicy, LaunchClientModeRequest, LaunchOptions, RunnerKind, RunnerOutputFormat,
    RunnerProfile,
};
use crate::domain::syntax::{SyntaxCheckResult, SyntaxCheckStatus};
use crate::domain::test::{TestRunResult, TestStatus, TestTarget};
use crate::output::json::Envelope;
use crate::output::presenter::Presenter;
use crate::output::text::{TimelineItem, TimelineStatus};
use crate::support::adapter_input::{
    parse_launch_mode, parse_required_dump_mode, LaunchModeAliases,
};
use crate::support::error::AppError;
use crate::support::fs::clean_dir;
use crate::support::path::is_safe_path_segment;
use crate::support::temp::platform_logs_dir;
use crate::use_cases::artifacts;
use crate::use_cases::build_project;
use crate::use_cases::check_syntax;
use crate::use_cases::configure_extensions;
use crate::use_cases::context::{CommandName, ExecutionContext};
use crate::use_cases::convert_sources;
use crate::use_cases::dump_config;
use crate::use_cases::init_project;
use crate::use_cases::launch_app;
use crate::use_cases::load_artifact;
use crate::use_cases::request::{
    ArtifactsModeRequest, ArtifactsRequest, BuildRequest, ConfigureExtensionsRequest,
    ConvertRequest, ConvertScopeRequest, DesignerConfigSyntaxRequest,
    DesignerConfigSyntaxSelection, DesignerModulesSyntaxRequest, DesignerModulesSyntaxSelection,
    DumpRequest, InitRequest, LaunchRequest, LoadRequest, SyntaxRequest, SyntaxTargetRequest,
    TestRequest, TestScopeRequest,
};
use crate::use_cases::result::{UseCaseError, UseCaseErrorKind};
use crate::use_cases::run_tests;
use crate::use_cases::transport::dispatch_with_workspace_lock;

/// Executes a parsed CLI command by mapping it into transport-neutral requests and
/// rendering the resulting command output.
pub fn execute_command(
    config: &AppConfig,
    command: &Command,
    presenter: &Presenter,
    clean_before_execution: bool,
) -> Result<(), UseCaseError> {
    let cancellation = CancellationToken::new();
    let _signal_guard = CliSignalGuard::install(cancellation.clone());
    match command {
        Command::Config(_) => unreachable!("config commands are handled outside cli::execute"),
        Command::Init => execute_init(config, presenter, clean_before_execution, cancellation),
        Command::Extensions(args) => execute_extensions(
            config,
            args,
            presenter,
            clean_before_execution,
            cancellation,
        ),
        Command::Build(args) => execute_build(
            config,
            args,
            presenter,
            clean_before_execution,
            cancellation,
        ),
        Command::Load(args) => execute_load(
            config,
            args,
            presenter,
            clean_before_execution,
            cancellation,
        ),
        Command::Test(args) => execute_test(
            config,
            args,
            presenter,
            clean_before_execution,
            cancellation,
        ),
        Command::Dump(args) => execute_dump(
            config,
            args,
            presenter,
            clean_before_execution,
            cancellation,
        ),
        Command::Convert(args) => execute_convert(
            config,
            args,
            presenter,
            clean_before_execution,
            cancellation,
        ),
        Command::Artifacts(args) => execute_artifacts(
            config,
            args,
            presenter,
            clean_before_execution,
            cancellation,
        ),
        Command::Syntax(args) => execute_syntax(
            config,
            args,
            presenter,
            clean_before_execution,
            cancellation,
        ),
        Command::Launch(args) => execute_launch(
            config,
            args,
            presenter,
            clean_before_execution,
            cancellation,
        ),
        Command::Mcp(_) => unreachable!("mcp commands are handled outside cli::execute"),
    }
}

/// Returns the canonical command identifier for a parsed CLI command.
pub fn command_name(command: &Command) -> CommandName {
    match command {
        Command::Config(_) => unreachable!("config commands do not map to execution use cases"),
        Command::Init => CommandName::Init,
        Command::Extensions(_) => CommandName::Extensions,
        Command::Build(_) => CommandName::Build,
        Command::Load(_) => CommandName::Load,
        Command::Test(_) => CommandName::Test,
        Command::Dump(_) => CommandName::Dump,
        Command::Convert(_) => CommandName::Convert,
        Command::Artifacts(_) => CommandName::Artifacts,
        Command::Syntax(_) => CommandName::Syntax,
        Command::Launch(_) => CommandName::Launch,
        Command::Mcp(_) => unreachable!("mcp commands do not map to CLI command names"),
    }
}

fn execute_extensions(
    config: &AppConfig,
    args: &ExtensionsArgs,
    presenter: &Presenter,
    clean_before_execution: bool,
    cancellation: CancellationToken,
) -> Result<(), UseCaseError> {
    let request = map_extensions_request(args);
    let context = cli_context(config, CommandName::Extensions, cancellation);
    with_cli_workspace_lock(
        config,
        presenter,
        CommandName::Extensions,
        clean_before_execution,
        || match configure_extensions::execute(&context, config, &request) {
            Ok(result) => {
                if presenter.is_json() {
                    presenter.print_envelope(&Envelope::ok(
                        CommandName::Extensions.as_str(),
                        result.duration_ms,
                        result,
                    ));
                }
                Ok(())
            }
            Err(failure) => {
                let error = failure.error;
                if presenter.is_json() {
                    if let Some(result) = failure.payload {
                        presenter.print_envelope(&Envelope::err(
                            CommandName::Extensions.as_str(),
                            result.duration_ms,
                            result,
                        ));
                    }
                } else {
                    presenter.print_error(&error.to_string());
                }
                Err(error)
            }
        },
    )
}

fn execute_init(
    config: &AppConfig,
    presenter: &Presenter,
    clean_before_execution: bool,
    cancellation: CancellationToken,
) -> Result<(), UseCaseError> {
    let request = InitRequest;
    let context = cli_context(config, CommandName::Init, cancellation);
    with_cli_workspace_lock(
        config,
        presenter,
        CommandName::Init,
        clean_before_execution,
        || match init_project::execute(&context, config, &request) {
            Ok(result) => {
                if presenter.is_json() {
                    presenter.print_envelope(&Envelope::ok(
                        CommandName::Init.as_str(),
                        result.duration_ms,
                        result,
                    ));
                } else {
                    render_init_text(&result, presenter);
                }
                Ok(())
            }
            Err(failure) => {
                let error = failure.error;
                if presenter.is_json() {
                    if let Some(result) = failure.payload {
                        presenter.print_envelope(&Envelope::err(
                            CommandName::Init.as_str(),
                            result.duration_ms,
                            result,
                        ));
                    }
                } else {
                    if let Some(result) = failure.payload.as_ref() {
                        render_init_text(result, presenter);
                    }
                    presenter.print_error(&error.to_string());
                }
                Err(error)
            }
        },
    )
}

fn execute_build(
    config: &AppConfig,
    args: &BuildArgs,
    presenter: &Presenter,
    clean_before_execution: bool,
    cancellation: CancellationToken,
) -> Result<(), UseCaseError> {
    let request = map_build_request(args);
    let context = cli_context(config, CommandName::Build, cancellation);
    with_cli_workspace_lock(
        config,
        presenter,
        CommandName::Build,
        clean_before_execution,
        || match build_project::execute(&context, config, &request) {
            Ok(result) => {
                if presenter.is_json() {
                    presenter.print_envelope(&Envelope::ok(
                        CommandName::Build.as_str(),
                        result.duration_ms,
                        result,
                    ));
                } else {
                    render_build_text(&result, presenter, true);
                }
                Ok(())
            }
            Err(failure) => {
                let error = failure.error;
                if presenter.is_json() {
                    if let Some(result) = failure.payload {
                        presenter.print_envelope(&Envelope::err(
                            CommandName::Build.as_str(),
                            result.duration_ms,
                            result,
                        ));
                    }
                } else {
                    if let Some(result) = failure.payload.as_ref() {
                        render_build_text(result, presenter, false);
                    }
                    presenter.print_error(&error.to_string());
                }
                Err(error)
            }
        },
    )
}

fn execute_test(
    config: &AppConfig,
    args: &TestArgs,
    presenter: &Presenter,
    clean_before_execution: bool,
    cancellation: CancellationToken,
) -> Result<(), UseCaseError> {
    let request = map_test_request(config, args)?;
    let context = cli_context(config, CommandName::Test, cancellation);
    with_cli_workspace_lock(
        config,
        presenter,
        CommandName::Test,
        clean_before_execution,
        || match run_tests::execute(&context, config, &request) {
            Ok(result) => {
                if presenter.is_json() {
                    presenter.print_envelope(&build_test_envelope(result, true));
                } else {
                    render_test_text(&result, presenter);
                }
                Ok(())
            }
            Err(failure) => {
                let error = failure.error;
                if presenter.is_json() {
                    if let Some(result) = failure.payload {
                        presenter.print_envelope(&build_test_envelope(result, false));
                    }
                } else {
                    if let Some(result) = failure.payload.as_ref() {
                        render_test_text(result, presenter);
                    }
                    presenter.print_error(&error.to_string());
                }
                Err(error)
            }
        },
    )
}

fn execute_load(
    config: &AppConfig,
    args: &LoadArgs,
    presenter: &Presenter,
    clean_before_execution: bool,
    cancellation: CancellationToken,
) -> Result<(), UseCaseError> {
    let request = map_load_request(args)?;
    let context = cli_context(config, CommandName::Load, cancellation);
    with_cli_workspace_lock(
        config,
        presenter,
        CommandName::Load,
        clean_before_execution,
        || match load_artifact::execute(&context, config, &request) {
            Ok(result) => {
                if presenter.is_json() {
                    presenter.print_envelope(&Envelope::ok(
                        CommandName::Load.as_str(),
                        result.duration_ms,
                        result,
                    ));
                } else {
                    render_load_text(&result, presenter, true);
                }
                Ok(())
            }
            Err(failure) => {
                let error = failure.error;
                if presenter.is_json() {
                    if let Some(result) = failure.payload {
                        presenter.print_envelope(&Envelope::err(
                            CommandName::Load.as_str(),
                            result.duration_ms,
                            result,
                        ));
                    }
                } else {
                    if let Some(result) = failure.payload.as_ref() {
                        render_load_text(result, presenter, false);
                    }
                    presenter.print_error(&error.to_string());
                }
                Err(error)
            }
        },
    )
}

fn execute_dump(
    config: &AppConfig,
    args: &DumpArgs,
    presenter: &Presenter,
    clean_before_execution: bool,
    cancellation: CancellationToken,
) -> Result<(), UseCaseError> {
    let request = map_dump_request(args)?;
    let context = cli_context(config, CommandName::Dump, cancellation);
    with_cli_workspace_lock(
        config,
        presenter,
        CommandName::Dump,
        clean_before_execution,
        || match dump_config::execute(&context, config, &request) {
            Ok(result) => {
                if presenter.is_json() {
                    presenter.print_envelope(&Envelope::ok(
                        CommandName::Dump.as_str(),
                        result.duration_ms,
                        result,
                    ));
                } else {
                    render_dump_text(&result, presenter, true);
                }
                Ok(())
            }
            Err(failure) => {
                let error = failure.error;
                if presenter.is_json() {
                    if let Some(result) = failure.payload {
                        presenter.print_envelope(&Envelope::err(
                            CommandName::Dump.as_str(),
                            result.duration_ms,
                            result,
                        ));
                    }
                } else {
                    if let Some(result) = failure.payload.as_ref() {
                        render_dump_text(result, presenter, false);
                    }
                    presenter.print_error(&error.to_string());
                }
                Err(error)
            }
        },
    )
}

fn execute_convert(
    config: &AppConfig,
    args: &ConvertArgs,
    presenter: &Presenter,
    clean_before_execution: bool,
    cancellation: CancellationToken,
) -> Result<(), UseCaseError> {
    let request = map_convert_request(args);
    if let Err(error) = convert_sources::preflight_validate(config, &request) {
        return Err(render_pre_dispatch_error(
            presenter,
            CommandName::Convert,
            error,
        ));
    }
    let context = cli_context(config, CommandName::Convert, cancellation);
    with_cli_workspace_lock(
        config,
        presenter,
        CommandName::Convert,
        clean_before_execution,
        || match convert_sources::execute(&context, config, &request) {
            Ok(result) => {
                if presenter.is_json() {
                    presenter.print_envelope(&Envelope::ok(
                        CommandName::Convert.as_str(),
                        result.duration_ms,
                        result,
                    ));
                } else {
                    render_convert_text(&result, presenter, true);
                }
                Ok(())
            }
            Err(failure) => {
                let error = failure.error;
                if presenter.is_json() {
                    if let Some(result) = failure.payload {
                        presenter.print_envelope(&Envelope::err(
                            CommandName::Convert.as_str(),
                            result.duration_ms,
                            result,
                        ));
                    }
                } else {
                    if let Some(result) = failure.payload.as_ref() {
                        render_convert_text(result, presenter, false);
                    }
                    presenter.print_error(&error.to_string());
                }
                Err(error)
            }
        },
    )
}

fn execute_artifacts(
    config: &AppConfig,
    args: &ArtifactsArgs,
    presenter: &Presenter,
    clean_before_execution: bool,
    cancellation: CancellationToken,
) -> Result<(), UseCaseError> {
    let request = map_artifacts_request_with_config(config, args)?;
    let context = cli_context(config, CommandName::Artifacts, cancellation);
    with_cli_workspace_lock(
        config,
        presenter,
        CommandName::Artifacts,
        clean_before_execution,
        || match artifacts::execute(&context, config, &request) {
            Ok(result) => {
                if presenter.is_json() {
                    presenter.print_envelope(&Envelope::ok(
                        CommandName::Artifacts.as_str(),
                        result.duration_ms,
                        result,
                    ));
                } else {
                    render_artifacts_text(&result, presenter, true);
                }
                Ok(())
            }
            Err(failure) => {
                let error = failure.error;
                if presenter.is_json() {
                    if let Some(result) = failure.payload {
                        presenter.print_envelope(&Envelope::err(
                            CommandName::Artifacts.as_str(),
                            result.duration_ms,
                            result,
                        ));
                    }
                } else {
                    if let Some(result) = failure.payload.as_ref() {
                        render_artifacts_text(result, presenter, false);
                    }
                    presenter.print_error(&error.to_string());
                }
                Err(error)
            }
        },
    )
}

fn execute_syntax(
    config: &AppConfig,
    args: &SyntaxArgs,
    presenter: &Presenter,
    clean_before_execution: bool,
    cancellation: CancellationToken,
) -> Result<(), UseCaseError> {
    let request = map_syntax_request(args);
    let context = cli_context(config, CommandName::Syntax, cancellation);
    with_cli_workspace_lock(
        config,
        presenter,
        CommandName::Syntax,
        clean_before_execution,
        || match check_syntax::execute(&context, config, &request) {
            Ok(result) => {
                if presenter.is_json() {
                    presenter.print_envelope(&Envelope::ok(
                        CommandName::Syntax.as_str(),
                        result.duration_ms,
                        result,
                    ));
                } else {
                    render_syntax_text(&result, presenter);
                }
                Ok(())
            }
            Err(failure) => {
                let error = failure.error;
                if presenter.is_json() {
                    if let Some(result) = failure.payload {
                        presenter.print_envelope(&Envelope::err(
                            CommandName::Syntax.as_str(),
                            result.duration_ms,
                            result,
                        ));
                    }
                } else {
                    if let Some(result) = failure.payload.as_ref() {
                        render_syntax_text(result, presenter);
                    }
                    presenter.print_error(&error.to_string());
                }
                Err(error)
            }
        },
    )
}

fn execute_launch(
    config: &AppConfig,
    args: &LaunchArgs,
    presenter: &Presenter,
    clean_before_execution: bool,
    cancellation: CancellationToken,
) -> Result<(), UseCaseError> {
    let request = map_launch_request(args)?;
    let context = cli_context(config, CommandName::Launch, cancellation);
    let started = Instant::now();
    with_cli_workspace_lock(
        config,
        presenter,
        CommandName::Launch,
        clean_before_execution,
        || match launch_app::execute(&context, config, &request) {
            Ok(result) => {
                if presenter.is_json() {
                    presenter.print_envelope(&Envelope::ok(
                        CommandName::Launch.as_str(),
                        started.elapsed().as_millis() as u64,
                        result,
                    ));
                } else {
                    render_launch_text(&result, presenter);
                }
                Ok(())
            }
            Err(failure) => {
                let error = failure.error;
                if !presenter.is_json() {
                    presenter.print_error(&error.to_string());
                }
                Err(error)
            }
        },
    )
}

fn with_cli_workspace_lock<T>(
    config: &AppConfig,
    presenter: &Presenter,
    command: CommandName,
    clean_before_execution: bool,
    run: impl FnOnce() -> Result<T, UseCaseError>,
) -> Result<T, UseCaseError> {
    dispatch_with_workspace_lock(
        config,
        command,
        || {
            if clean_before_execution {
                clean_platform_logs_under_lock(config)
            } else {
                Ok(())
            }
        },
        run,
    )
    .map_err(|error| render_pre_dispatch_error(presenter, command, error))?
}

fn clean_platform_logs_under_lock(config: &AppConfig) -> Result<(), UseCaseError> {
    platform_logs_dir(&config.work_path)
        .and_then(|dir| clean_dir(&dir))
        .map_err(|error| {
            UseCaseError::from(AppError::Runtime(format!(
                "failed to clean platform logs: {error}"
            )))
        })
}

fn render_pre_dispatch_error(
    presenter: &Presenter,
    command: CommandName,
    error: impl Into<UseCaseError>,
) -> UseCaseError {
    let error = error.into();
    if presenter.is_json() {
        presenter.print_envelope(&pre_dispatch_error_envelope(command, error.message()));
    } else {
        presenter.print_error(&error.to_string());
    }
    error
}

fn pre_dispatch_error_envelope(command: CommandName, message: &str) -> Envelope<serde_json::Value> {
    Envelope::err(command.as_str(), 0, json!({ "message": message }))
}

fn map_build_request(args: &BuildArgs) -> BuildRequest {
    BuildRequest {
        full_rebuild: args.full_rebuild,
    }
}

fn map_extensions_request(args: &ExtensionsArgs) -> ConfigureExtensionsRequest {
    ConfigureExtensionsRequest {
        names: args.names.clone(),
    }
}

fn map_test_request(config: &AppConfig, args: &TestArgs) -> Result<TestRequest, UseCaseError> {
    let client_mode = map_test_client_mode(args.client_mode.as_deref())?;
    match &args.runner {
        TestRunner::Yaxunit(TestYaxunitArgs { scope }) => {
            let scope = map_yaxunit_scope(scope)?;
            Ok(TestRequest {
                execution: build_yaxunit_execution(config, &args.launch, client_mode)?,
                full: args.full,
                scope,
            })
        }
        TestRunner::Va => Ok(TestRequest {
            execution: build_vanessa_execution(config, &args.launch, client_mode)?,
            full: args.full,
            scope: TestScopeRequest::All,
        }),
    }
}

fn map_test_client_mode(
    client_mode: Option<&str>,
) -> Result<Option<LaunchClientModeRequest>, UseCaseError> {
    Ok(match client_mode {
        Some("designer") => Some(LaunchClientModeRequest::Designer),
        Some("thin") => Some(LaunchClientModeRequest::Thin),
        Some("thick") => Some(LaunchClientModeRequest::Thick),
        Some("ordinary") => Some(LaunchClientModeRequest::Ordinary),
        Some(other) => {
            return Err(UseCaseError::new(
                UseCaseErrorKind::Validation,
                format!("unsupported test client mode: {other}"),
            ));
        }
        None => None,
    })
}

fn map_yaxunit_scope(scope: &TestScope) -> Result<TestScopeRequest, UseCaseError> {
    Ok(match scope {
        TestScope::All => TestScopeRequest::All,
        TestScope::Module { name } => {
            let trimmed = name.trim();
            if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
                return Err(UseCaseError::new(
                    UseCaseErrorKind::Validation,
                    "test module requires a non-empty module name",
                ));
            }
            TestScopeRequest::Module {
                name: trimmed.to_owned(),
            }
        }
    })
}

fn build_yaxunit_execution(
    config: &AppConfig,
    launch_args: &LaunchOptionsArgs,
    client_mode: Option<LaunchClientModeRequest>,
) -> Result<crate::domain::runner::ScenarioExecutionRequest, UseCaseError> {
    validate_test_launch_options(launch_args)?;
    let mut execution = TestRequest::default_execution();
    execution.timeouts = effective_test_timeouts(
        config.tests.execution_timeout_seconds,
        &config.tests.yaxunit.timeouts,
    );
    execution.launch = map_test_launch_options(launch_args)?;
    execution.client_mode = client_mode.or(Some(LaunchClientModeRequest::Thin));
    execution.launch.c = Some("RunUnitTests={config_path}".to_owned());
    Ok(execution)
}

fn build_vanessa_execution(
    config: &AppConfig,
    launch_args: &LaunchOptionsArgs,
    client_mode: Option<LaunchClientModeRequest>,
) -> Result<crate::domain::runner::ScenarioExecutionRequest, UseCaseError> {
    validate_test_launch_options(launch_args)?;
    let profile_id = config.tests.va.profile.as_deref().ok_or_else(|| {
        UseCaseError::new(
            UseCaseErrorKind::Validation,
            "tests.va.profile is not configured",
        )
    })?;
    if !config.tests.va.profiles.contains_key(profile_id) {
        return Err(UseCaseError::new(
            UseCaseErrorKind::Validation,
            format!("unknown Vanessa Automation profile '{profile_id}'"),
        ));
    }
    if !is_safe_path_segment(profile_id) {
        return Err(UseCaseError::new(
            UseCaseErrorKind::Validation,
            format!("tests.va.profile contains unsafe path characters: {profile_id}"),
        ));
    }
    if config.tests.va.epf_path.is_none() {
        return Err(UseCaseError::new(
            UseCaseErrorKind::Validation,
            "tests.va.epf_path is not configured",
        ));
    }
    if config.tests.va.params_path.is_none() {
        return Err(UseCaseError::new(
            UseCaseErrorKind::Validation,
            "tests.va.params_path is not configured",
        ));
    }

    let mut execution = crate::domain::runner::ScenarioExecutionRequest {
        profile: RunnerProfile {
            id: profile_id.to_owned(),
            kind: RunnerKind::Vanessa,
            output_formats: vec![
                RunnerOutputFormat::JunitXml,
                RunnerOutputFormat::PlainTextLog,
            ],
            backend_hint: Some("enterprise".to_owned()),
        },
        client_mode: Some(LaunchClientModeRequest::Thin),
        timeouts: effective_test_timeouts(
            config.tests.execution_timeout_seconds,
            &config.tests.va.timeouts,
        ),
        policy: ExecutionPolicy {
            retain_artifacts_on_failure: true,
            retain_artifacts_on_success: false,
        },
        launch: LaunchOptions::default(),
    };
    execution.launch = map_test_launch_options(launch_args)?;
    execution.client_mode = client_mode.or(Some(LaunchClientModeRequest::Thin));
    execution.launch.c = Some("StartFeaturePlayer;VAParams={params_path}".to_owned());
    execution.launch.execute = Some("{epf_path}".to_owned());
    Ok(execution)
}

fn map_test_launch_options(args: &LaunchOptionsArgs) -> Result<LaunchOptions, UseCaseError> {
    if args.c.is_some() {
        return Err(UseCaseError::new(
            UseCaseErrorKind::Validation,
            "--c is not supported for test; it is reserved for the internal runner payload",
        ));
    }
    if args.execute.is_some() {
        return Err(UseCaseError::new(
            UseCaseErrorKind::Validation,
            "--execute is not supported for test; it is reserved for the internal runner payload",
        ));
    }
    if args.output.is_some() {
        return Err(UseCaseError::new(
            UseCaseErrorKind::Validation,
            "--output is not supported for test; the platform log path is managed internally",
        ));
    }
    Ok(LaunchOptions {
        c: None,
        execute: None,
        use_privileged_mode: args.use_privileged_mode,
        out: None,
        internal_out: None,
        raw_args: args.raw_keys.clone(),
    })
}

fn validate_test_launch_options(args: &LaunchOptionsArgs) -> Result<(), UseCaseError> {
    if args.c.is_some() || args.execute.is_some() || args.output.is_some() {
        return Err(UseCaseError::new(
            UseCaseErrorKind::Validation,
            "test accepts only --use-privileged-mode and raw launch keys; /C, /Execute, and /Out are managed by the runner",
        ));
    }
    Ok(())
}

fn effective_test_timeouts(
    legacy_total_seconds: u64,
    runner_timeouts: &ExecutionTimeouts,
) -> ExecutionTimeouts {
    let mut timeouts = runner_timeouts.clone();
    if timeouts.total_ms.is_none() {
        timeouts.total_ms = Some(legacy_total_seconds.saturating_mul(1_000));
    }
    timeouts
}

fn cli_context(
    config: &AppConfig,
    command: CommandName,
    cancellation: CancellationToken,
) -> ExecutionContext {
    ExecutionContext::cli(command)
        .with_deadline(Some(Instant::now() + config.execution_timeout_duration()))
        .with_cancellation(cancellation)
}

fn map_load_request(args: &LoadArgs) -> Result<LoadRequest, UseCaseError> {
    Ok(LoadRequest {
        mode: match args.mode.as_str() {
            "load" => LoadMode::Load,
            "merge" => LoadMode::Merge,
            "update" => LoadMode::Update,
            other => {
                return Err(UseCaseError::new(
                    UseCaseErrorKind::Validation,
                    format!("unsupported load mode: {other}"),
                ));
            }
        },
        artifact_path: args.path.clone(),
        settings_path: args.settings.clone(),
        extension: args.extension.clone(),
    })
}

fn map_dump_request(args: &DumpArgs) -> Result<DumpRequest, UseCaseError> {
    Ok(DumpRequest {
        mode: parse_required_dump_mode(&args.mode)?,
        source_set: args.source_set.clone(),
        extension: args.extension.clone(),
        objects: args.objects.clone(),
    })
}

fn map_convert_request(args: &ConvertArgs) -> ConvertRequest {
    ConvertRequest {
        scope: match args.source_set.as_deref() {
            Some(name) => ConvertScopeRequest::SourceSet {
                name: name.to_owned(),
            },
            None => ConvertScopeRequest::All,
        },
    }
}

fn map_artifacts_request_with_config(
    config: &AppConfig,
    args: &ArtifactsArgs,
) -> Result<ArtifactsRequest, UseCaseError> {
    let mode = match (args.source_set.as_deref(), args.extension.is_some()) {
        (_, true) => ArtifactsModeRequest::ExtensionCfe,
        (Some(source_set_name), false) => {
            let source_set = config
                .source_sets
                .iter()
                .find(|source_set| source_set.name == source_set_name)
                .ok_or_else(|| {
                    UseCaseError::new(
                        UseCaseErrorKind::Validation,
                        format!("unknown source-set '{source_set_name}'"),
                    )
                })?;
            match source_set.purpose {
                SourceSetPurpose::Configuration => ArtifactsModeRequest::ConfigurationCf,
                SourceSetPurpose::Extension => ArtifactsModeRequest::ExtensionCfe,
                SourceSetPurpose::ExternalDataProcessors => {
                    ArtifactsModeRequest::ExternalDataProcessorEpf
                }
                SourceSetPurpose::ExternalReports => ArtifactsModeRequest::ExternalReportErf,
            }
        }
        (None, false) => ArtifactsModeRequest::ConfigurationCf,
    };

    Ok(ArtifactsRequest {
        execution: ArtifactsRequest::default_execution(mode),
        mode,
        output_path: args.output.clone(),
        source_set: args.source_set.clone(),
        extension: args.extension.clone(),
    })
}

fn map_syntax_request(args: &SyntaxArgs) -> SyntaxRequest {
    SyntaxRequest {
        target: match &args.target {
            SyntaxTarget::DesignerConfig(config) => {
                SyntaxTargetRequest::DesignerConfig(map_designer_config_request(config))
            }
            SyntaxTarget::DesignerModules(modules) => {
                SyntaxTargetRequest::DesignerModules(map_designer_modules_request(modules))
            }
            SyntaxTarget::Edt { projects } => SyntaxTargetRequest::Edt {
                projects: projects.clone(),
            },
        },
    }
}

fn map_designer_config_request(args: &DesignerConfigSyntaxArgs) -> DesignerConfigSyntaxRequest {
    DesignerConfigSyntaxRequest::from_selection(
        DesignerConfigSyntaxSelection {
            config_log_integrity: args.config_log_integrity,
            incorrect_references: args.incorrect_references,
            thin_client: args.thin_client,
            web_client: args.web_client,
            mobile_client: args.mobile_client,
            server: args.server,
            external_connection: args.external_connection,
            external_connection_server: args.external_connection_server,
            mobile_app_client: args.mobile_app_client,
            mobile_app_server: args.mobile_app_server,
            thick_client_managed_application: args.thick_client_managed_application,
            thick_client_server_managed_application: args.thick_client_server_managed_application,
            thick_client_ordinary_application: args.thick_client_ordinary_application,
            thick_client_server_ordinary_application: args.thick_client_server_ordinary_application,
            mobile_client_digi_sign: args.mobile_client_digi_sign,
            distributive_modules: args.distributive_modules,
            unreference_procedures: args.unreference_procedures,
            handlers_existence: args.handlers_existence,
            empty_handlers: args.empty_handlers,
            extended_modules_check: args.extended_modules_check,
            check_use_synchronous_calls: args.check_use_synchronous_calls,
            check_use_modality: args.check_use_modality,
            unsupported_functional: args.unsupported_functional,
        },
        args.extension.clone(),
        args.all_extensions,
    )
}

fn map_designer_modules_request(args: &DesignerModulesSyntaxArgs) -> DesignerModulesSyntaxRequest {
    DesignerModulesSyntaxRequest::from_selection(
        DesignerModulesSyntaxSelection {
            thin_client: args.thin_client,
            web_client: args.web_client,
            server: args.server,
            external_connection: args.external_connection,
            thick_client_ordinary_application: args.thick_client_ordinary_application,
            mobile_app_client: args.mobile_app_client,
            mobile_app_server: args.mobile_app_server,
            mobile_client: args.mobile_client,
            extended_modules_check: args.extended_modules_check,
        },
        args.extension.clone(),
        args.all_extensions,
    )
}

fn map_launch_request(args: &LaunchArgs) -> Result<LaunchRequest, UseCaseError> {
    let Some(mode) = args.mode.as_deref().or(args.target.as_deref()) else {
        return Err(UseCaseError::new(
            UseCaseErrorKind::Validation,
            "launch mode is required",
        ));
    };

    Ok(LaunchRequest {
        mode: parse_launch_mode(mode, "mode", LaunchModeAliases::Cli)?,
        launch: map_direct_launch_options(&args.launch),
    })
}

fn map_direct_launch_options(args: &LaunchOptionsArgs) -> LaunchOptions {
    LaunchOptions {
        c: args.c.clone(),
        execute: args.execute.clone(),
        use_privileged_mode: args.use_privileged_mode,
        out: args.output.clone(),
        internal_out: None,
        raw_args: args.raw_keys.clone(),
    }
}

fn build_test_envelope(result: TestRunResult, ok: bool) -> Envelope<TestRunResult> {
    Envelope {
        ok,
        command: CommandName::Test.as_str().to_owned(),
        duration_ms: result.duration_ms,
        warnings: result.warnings.clone(),
        steps: result.steps.clone(),
        data: result,
    }
}

fn render_build_text(result: &BuildResult, presenter: &Presenter, succeeded: bool) {
    let summary = if !succeeded {
        TimelineItem::new(TimelineStatus::Failed, "Build failed")
    } else if result
        .steps
        .iter()
        .all(|step| matches!(step.mode, BuildMode::Skipped) && step.ok)
    {
        TimelineItem::new(TimelineStatus::Succeeded, "Build completed: no changes")
    } else {
        TimelineItem::new(TimelineStatus::Succeeded, "Build completed successfully")
    };
    presenter.print_timeline(&[summary]);
}

fn timeline_status(ok: bool) -> TimelineStatus {
    if ok {
        TimelineStatus::Succeeded
    } else {
        TimelineStatus::Failed
    }
}

fn timeline_item_with_details(
    status: TimelineStatus,
    label: impl Into<String>,
    details: Vec<String>,
) -> TimelineItem {
    let item = TimelineItem::new(status, label);
    if details.is_empty() {
        item
    } else {
        item.with_detail(details.join("\n"))
    }
}

fn step_status_detail(status: &InitStepStatus, message: impl AsRef<str>) -> String {
    match status {
        InitStepStatus::Ok => format!("✓ {}", message.as_ref()),
        InitStepStatus::Skipped => format!("○ {}", message.as_ref()),
        InitStepStatus::Failed => format!("✗ {}", message.as_ref()),
    }
}

fn bracketed_detail(kind: &str, message: impl AsRef<str>) -> String {
    format!("[{kind}] {}", message.as_ref())
}

fn single_timeline(
    presenter: &Presenter,
    status: TimelineStatus,
    label: impl Into<String>,
    details: Vec<String>,
) {
    presenter.print_timeline(&[timeline_item_with_details(status, label, details)]);
}

fn append_if_present(details: &mut Vec<String>, line: Option<String>) {
    if let Some(line) = line.filter(|value| !value.is_empty()) {
        push_unique_detail(details, line);
    }
}

fn push_unique_detail(details: &mut Vec<String>, line: impl Into<String>) {
    let line = line.into();
    if !details.contains(&line) {
        details.push(line);
    }
}

fn append_error_details(details: &mut Vec<String>, errors: &[ExecutionError]) {
    for error in errors {
        push_unique_detail(
            details,
            bracketed_detail(&format!("error:{}", error.code), &error.message),
        );
        for detail in &error.details {
            push_unique_detail(details, bracketed_detail("detail", detail));
        }
        if let Some(artifact) = error.artifact.as_ref() {
            push_unique_detail(details, render_artifact_ref("diagnostic", artifact));
        }
    }
}

fn append_diagnostics(details: &mut Vec<String>, diagnostics: &[String]) {
    for diagnostic in diagnostics {
        push_unique_detail(details, bracketed_detail("diagnostic", diagnostic));
    }
}

fn append_interruptions(details: &mut Vec<String>, interruptions: &[ExecutionInterruptionDetails]) {
    for interruption in interruptions {
        if let Some(message) = interruption.message.as_deref() {
            push_unique_detail(details, bracketed_detail("warning", message));
            continue;
        }

        let kind = match interruption.kind {
            crate::domain::execution::ExecutionInterruptionKind::Cancelled => "cancelled",
            crate::domain::execution::ExecutionInterruptionKind::TimedOut => "timed_out",
        };
        let phase = interruption.phase.as_deref().unwrap_or("unknown_phase");
        let detail = if interruption.deferred {
            format!("deferred {kind} interruption during {phase}")
        } else {
            format!("{kind} interruption during {phase}")
        };
        push_unique_detail(details, bracketed_detail("warning", detail));
    }
}

fn render_artifact_ref(kind: &str, artifact: &ArtifactRef) -> String {
    let role = artifact.role.as_deref().unwrap_or(kind);
    format!("[{kind}] {role} -> {}", artifact.path.display())
}

fn render_output_artifact(path: &Path) -> String {
    format!("[artifact] {}", path.display())
}

fn render_step_signal(step: &StepResult) -> String {
    let label = render_test_step_label(&step.name);
    let message = step
        .message
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or("completed");
    match step.status {
        ExecutionStepStatus::Failed => format!("✗ {label}: {message}"),
        ExecutionStepStatus::Skipped => format!("○ {label}: {message}"),
        ExecutionStepStatus::Degraded => {
            bracketed_detail("step:degraded", format!("{label}: {message}"))
        }
        ExecutionStepStatus::Succeeded => format!("✓ {label}: {message}"),
    }
}

fn append_step_signals(details: &mut Vec<String>, steps: &[StepResult]) {
    for step in steps {
        let is_interesting = !matches!(step.status, ExecutionStepStatus::Succeeded)
            || !step.diagnostics.is_empty()
            || !step.errors.is_empty()
            || step.artifacts.is_some();
        if !is_interesting {
            continue;
        }

        push_unique_detail(details, render_step_signal(step));
        if let Some(target) = step.target.as_deref() {
            push_unique_detail(details, bracketed_detail("target", target));
        }
        append_diagnostics(details, &step.diagnostics);
        append_error_details(details, &step.errors);
        if let Some(artifacts) = step.artifacts.as_ref() {
            for artifact in &artifacts.items {
                push_unique_detail(details, render_artifact_ref("artifact", artifact));
            }
        }
    }
}

fn append_report_failures(details: &mut Vec<String>, result: &TestRunResult) {
    let Some(report) = result.report.as_ref() else {
        return;
    };

    for extracted in &report.extracted_errors {
        push_unique_detail(details, bracketed_detail("error:test_report", extracted));
    }

    for suite in &report.suites {
        for case in &suite.cases {
            if matches!(case.status, TestStatus::Passed) {
                continue;
            }

            push_unique_detail(
                details,
                bracketed_detail(
                    "case",
                    format!(
                        "{} :: {} {}",
                        suite.name,
                        status_label(&case.status),
                        case.name
                    ),
                ),
            );
            if let Some(message) = case.failure_message.as_deref() {
                push_unique_detail(details, bracketed_detail("detail", message));
            }
            if let Some(trace) = case.stack_trace.as_deref() {
                push_unique_detail(details, bracketed_detail("detail", trace));
            }
        }
    }
}

fn append_retained_test_artifacts(details: &mut Vec<String>, result: &TestRunResult) {
    let Some(paths) = result.retained_paths.as_ref() else {
        return;
    };

    push_unique_detail(
        details,
        format!("[artifact] run_dir -> {}", paths.run_dir.display()),
    );
    push_unique_detail(
        details,
        format!("[artifact] report -> {}", paths.junit_xml.display()),
    );
    push_unique_detail(
        details,
        format!("[artifact] runner_log -> {}", paths.yaxunit_log.display()),
    );
    push_unique_detail(
        details,
        format!(
            "[diagnostic] platform_log -> {}",
            paths.platform_log.display()
        ),
    );
}

fn should_hide_success_test_diagnostic(diagnostic: &str) -> bool {
    diagnostic.trim_start().starts_with("platform ")
}

fn visible_test_diagnostics(result: &TestRunResult) -> Vec<String> {
    if !result.ok {
        return result.diagnostics.clone();
    }

    result
        .diagnostics
        .iter()
        .filter(|diagnostic| !should_hide_success_test_diagnostic(diagnostic))
        .cloned()
        .collect()
}

fn test_has_actionable_success_signal(result: &TestRunResult) -> bool {
    result
        .report
        .as_ref()
        .is_some_and(|report| !report.extracted_errors.is_empty())
        || !visible_test_diagnostics(result).is_empty()
}

fn dump_has_warning(result: &DumpResult) -> bool {
    result
        .message
        .as_deref()
        .is_some_and(|message| message != "dump completed successfully")
}

fn execution_has_warning(
    diagnostics: &[String],
    interruptions: &[ExecutionInterruptionDetails],
) -> bool {
    !diagnostics.is_empty() || !interruptions.is_empty()
}

fn render_artifact_mode(mode: ArtifactBuildMode) -> &'static str {
    match mode {
        ArtifactBuildMode::Unknown => "unknown",
        ArtifactBuildMode::ConfigurationCf => "cf",
        ArtifactBuildMode::ExtensionCfe => "cfe",
        ArtifactBuildMode::ExternalDataProcessorEpf => "epf",
        ArtifactBuildMode::ExternalReportErf => "erf",
    }
}

fn render_load_text(result: &LoadResult, presenter: &Presenter, succeeded: bool) {
    let mode = match result.mode {
        LoadMode::Load => "load",
        LoadMode::Merge => "merge",
        LoadMode::Update => "update",
    };
    let target = match result.target_kind {
        crate::domain::load::LoadTargetKind::Configuration => "configuration".to_owned(),
        crate::domain::load::LoadTargetKind::Extension => format!(
            "extension {}",
            result.extension.as_deref().unwrap_or("<unknown>")
        ),
        crate::domain::load::LoadTargetKind::Unknown => "unknown".to_owned(),
    };
    let warning = succeeded
        && execution_has_warning(
            &result.execution.diagnostics,
            &result.execution.interruptions,
        );
    let label = if !succeeded {
        "Artifact load failed"
    } else if warning {
        "Artifact load completed with warnings"
    } else {
        "Artifact load completed successfully"
    };
    let mut details = vec![
        format!("target: {target}"),
        format!(
            "action: {mode} {}",
            render_artifact_mode(result.artifact_type)
        ),
        format!("artifact: {}", result.artifact_path.display()),
    ];
    if !succeeded || warning {
        let prefix = if succeeded { "warning" } else { "error" };
        append_if_present(
            &mut details,
            result
                .message
                .as_deref()
                .map(|message| bracketed_detail(prefix, message)),
        );
        append_error_details(&mut details, &result.execution.errors);
        append_diagnostics(&mut details, &result.execution.diagnostics);
        append_interruptions(&mut details, &result.execution.interruptions);
        append_if_present(
            &mut details,
            result
                .platform_log_path
                .as_deref()
                .map(|path| format!("[diagnostic] platform log -> {}", path.display())),
        );
    }
    single_timeline(presenter, timeline_status(succeeded), label, details);
}

fn render_init_text(result: &InitResult, presenter: &Presenter) {
    let mut details = Vec::new();
    for step in &result.steps {
        if is_designer_edt_workspace_noop(step) {
            continue;
        }

        let line = format!(
            "{}: {} - {}",
            step.target,
            step.action,
            step.message.as_deref().unwrap_or("ok")
        );
        details.push(step_status_detail(&step.status, line));
    }

    let succeeded = result
        .steps
        .iter()
        .all(|step| matches!(step.status, InitStepStatus::Ok | InitStepStatus::Skipped));
    let mut timeline = vec![timeline_item_with_details(
        timeline_status(succeeded),
        "init:",
        details,
    )];
    timeline.push(if succeeded {
        TimelineItem::new(TimelineStatus::Succeeded, "Init completed successfully")
    } else {
        TimelineItem::new(TimelineStatus::Failed, "Init failed")
    });
    presenter.print_timeline(&timeline);
}

fn is_designer_edt_workspace_noop(step: &InitStep) -> bool {
    matches!(step.status, InitStepStatus::Skipped)
        && step.target == "edt_workspace"
        && step.action == "import"
        && step
            .message
            .as_deref()
            .is_some_and(|message| message.contains("format=DESIGNER"))
}

fn render_dump_text(result: &DumpResult, presenter: &Presenter, succeeded: bool) {
    let mode = match result.mode {
        DumpMode::Full => "full",
        DumpMode::Incremental => "incremental",
        DumpMode::Partial => "partial",
    };
    let source_set = result.source_set.as_deref().unwrap_or("<unresolved>");
    let warning = succeeded && dump_has_warning(result);
    let label = if !succeeded {
        "Dump failed"
    } else if warning {
        "Dump completed with warnings"
    } else {
        "Dump completed successfully"
    };
    let mut details = vec![
        format!("source-set: {source_set}"),
        format!("mode: {mode}"),
        format!("output: {}", result.target_path.display()),
    ];
    if let Some(extension) = result.extension.as_deref() {
        details.push(format!("extension: {extension}"));
    }
    if !succeeded || warning {
        let prefix = if succeeded { "warning" } else { "error" };
        append_if_present(
            &mut details,
            result
                .message
                .as_deref()
                .map(|message| bracketed_detail(prefix, message)),
        );
        append_if_present(
            &mut details,
            result
                .platform_log_path
                .as_deref()
                .map(|path| format!("[diagnostic] platform log -> {}", path.display())),
        );
    }
    single_timeline(presenter, timeline_status(succeeded), label, details);
}

fn render_convert_text(result: &ConvertResult, presenter: &Presenter, succeeded: bool) {
    let label = if succeeded {
        if result.message.is_some() {
            "Convert completed with warnings"
        } else {
            "Convert completed successfully"
        }
    } else {
        "Convert failed"
    };
    let mut details = vec![
        format!("direction: {}", render_convert_direction(result.direction)),
        format!(
            "scope: {}",
            render_convert_scope(result.scope, result.source_set.as_deref())
        ),
        format!("workspace: {}", result.workspace_path.display()),
    ];
    for output in &result.outputs {
        details.push(format!(
            "source-set {}: {} -> {}",
            output.source_set,
            output.source_path.display(),
            output.target_path.display()
        ));
    }
    if !succeeded || result.message.is_some() {
        let prefix = if succeeded { "warning" } else { "error" };
        append_if_present(
            &mut details,
            result
                .message
                .as_deref()
                .map(|message| bracketed_detail(prefix, message)),
        );
    }
    single_timeline(presenter, timeline_status(succeeded), label, details);
}

fn render_artifacts_text(result: &ArtifactsResult, presenter: &Presenter, succeeded: bool) {
    let source_set = result.source_set.as_deref().unwrap_or("<unresolved>");
    let warning = succeeded
        && (result.message.is_some()
            || execution_has_warning(
                &result.execution.diagnostics,
                &result.execution.interruptions,
            ));
    let label = if !succeeded {
        "Artifacts export failed"
    } else if warning {
        "Artifacts export completed with warnings"
    } else {
        "Artifacts export completed successfully"
    };
    let mut details = vec![
        format!("source-set: {source_set}"),
        format!("mode: {}", render_artifact_mode(result.mode)),
        format!("output: {}", result.output_path.display()),
    ];
    if let Some(extension) = result.extension.as_deref() {
        details.push(format!("extension: {extension}"));
    }
    let package_artifacts = result
        .artifacts
        .items
        .iter()
        .filter(|artifact| artifact.role.as_deref() == Some(ARTIFACT_ROLE_PACKAGE_FILE))
        .collect::<Vec<_>>();
    if package_artifacts.is_empty() {
        details.push(render_output_artifact(&result.output_path));
    } else {
        for artifact in package_artifacts {
            details.push(render_artifact_ref("artifact", artifact));
        }
    }
    if !succeeded || warning {
        let prefix = if succeeded { "warning" } else { "error" };
        append_if_present(
            &mut details,
            result
                .message
                .as_deref()
                .map(|message| bracketed_detail(prefix, message)),
        );
        append_error_details(&mut details, &result.execution.errors);
        append_diagnostics(&mut details, &result.execution.diagnostics);
        append_interruptions(&mut details, &result.execution.interruptions);
        append_if_present(
            &mut details,
            result
                .platform_log_path
                .as_deref()
                .map(|path| format!("[diagnostic] platform log -> {}", path.display())),
        );
        for artifact in result
            .artifacts
            .items
            .iter()
            .filter(|artifact| artifact.role.as_deref() == Some(ARTIFACT_ROLE_PLATFORM_LOG))
        {
            details.push(render_artifact_ref("diagnostic", artifact));
        }
    }
    single_timeline(presenter, timeline_status(succeeded), label, details);
}

fn render_convert_direction(direction: ConvertDirection) -> &'static str {
    match direction {
        ConvertDirection::EdtToDesigner => "edt-to-designer",
        ConvertDirection::DesignerToEdt => "designer-to-edt",
    }
}

fn render_convert_scope(scope: ConvertScope, source_set: Option<&str>) -> String {
    match (scope, source_set) {
        (ConvertScope::All, _) => "all source-sets".to_owned(),
        (ConvertScope::Single, Some(source_set)) => format!("source-set {source_set}"),
        (ConvertScope::Single, None) => "single source-set".to_owned(),
    }
}

fn render_syntax_text(result: &SyntaxCheckResult, presenter: &Presenter) {
    let succeeded = matches!(result.status, SyntaxCheckStatus::Clean);
    let label = match result.status {
        SyntaxCheckStatus::Clean => {
            format!("Syntax check {} completed successfully", result.check_name)
        }
        SyntaxCheckStatus::IssuesFound => {
            format!("Syntax check {} found issues", result.check_name)
        }
        SyntaxCheckStatus::ToolFailed => format!("Syntax check {} failed", result.check_name),
    };
    let mut details = vec![format!(
        "status: {} (exit {}, errors {}, warnings {}, info {}, duration {} ms)",
        render_syntax_status(result.status),
        result.exit_code,
        result.summary.errors,
        result.summary.warnings,
        result.summary.info,
        result.duration_ms
    )];

    if !succeeded {
        for issue in &result.issues {
            details.push(bracketed_detail("issue", render_issue(issue)));
        }
    }

    append_if_present(
        &mut details,
        result
            .log_read_warning
            .as_deref()
            .map(|warning| bracketed_detail("warning", format!("log {warning}"))),
    );

    if !succeeded || result.log_read_warning.is_some() {
        append_if_present(
            &mut details,
            result
                .platform_log_path
                .as_deref()
                .map(|path| format!("[diagnostic] platform log -> {}", path.display())),
        );
    }

    if matches!(result.status, SyntaxCheckStatus::ToolFailed) {
        append_if_present(
            &mut details,
            result
                .stderr
                .as_deref()
                .map(|stderr| bracketed_detail("diagnostic", format!("stderr: {}", stderr.trim()))),
        );
    }

    single_timeline(presenter, timeline_status(succeeded), label, details);
}

fn render_syntax_status(status: SyntaxCheckStatus) -> &'static str {
    match status {
        SyntaxCheckStatus::Clean => "clean",
        SyntaxCheckStatus::IssuesFound => "issues_found",
        SyntaxCheckStatus::ToolFailed => "tool_failed",
    }
}

fn render_launch_text(result: &LaunchResult, presenter: &Presenter) {
    let mut details = vec![
        format!("mode: {}", render_launch_mode(&result.mode)),
        format!("binary: {}", result.binary.display()),
    ];
    append_if_present(
        &mut details,
        result
            .message
            .as_deref()
            .map(|message| bracketed_detail("status", message)),
    );
    if let Some(pid) = result.pid {
        details.push(format!("pid: {pid}"));
    }
    single_timeline(
        presenter,
        TimelineStatus::Succeeded,
        "Launch completed successfully",
        details,
    );
}

fn render_launch_mode(mode: &LaunchMode) -> &'static str {
    match mode {
        LaunchMode::Designer => "конфигуратор",
        LaunchMode::Thin => "тонкий клиент",
        LaunchMode::Thick => "толстый клиент",
        LaunchMode::Ordinary => "обычное приложение",
    }
}

fn render_test_text(result: &TestRunResult, presenter: &Presenter) {
    let diagnostics = visible_test_diagnostics(result);
    let has_warning = result.ok
        && (!result.warnings.is_empty()
            || test_has_actionable_success_signal(result)
            || !result.execution.interruptions.is_empty()
            || result
                .steps
                .iter()
                .any(|step| !matches!(step.status, ExecutionStepStatus::Succeeded)));
    let label = if result.ok {
        if has_warning {
            "Tests completed with warnings"
        } else {
            "Tests completed successfully"
        }
    } else {
        "Tests failed"
    };
    let mut details = vec![format!("target: {}", render_test_target(&result.target))];
    if let Some(report) = &result.report {
        details.push(format!(
            "summary: total={}, passed={}, failed={}, skipped={}, errors={}",
            report.summary.total,
            report.summary.passed,
            report.summary.failed,
            report.summary.skipped,
            report.summary.errors
        ));
    }

    if !result.ok || has_warning {
        append_step_signals(&mut details, &result.steps);
        append_report_failures(&mut details, result);
        append_error_details(&mut details, &result.execution.errors);
        append_diagnostics(&mut details, &diagnostics);
        append_interruptions(&mut details, &result.execution.interruptions);
        for warning in &result.warnings {
            push_unique_detail(&mut details, bracketed_detail("warning", warning));
        }
        append_retained_test_artifacts(&mut details, result);
    }

    single_timeline(presenter, timeline_status(result.ok), label, details);
}

fn render_test_target(target: &TestTarget) -> String {
    match target {
        TestTarget::All => "all".to_owned(),
        TestTarget::Module { name } => format!("module {name}"),
    }
}

fn render_test_step_label(name: &str) -> String {
    match name {
        "build" => "build prerequisite".to_owned(),
        "prepare_artifacts" => "prepare artifacts".to_owned(),
        "prepare_runner" => "prepare runner".to_owned(),
        "run" => "enterprise run".to_owned(),
        "parse_junit" => "parse JUnit report".to_owned(),
        "parse_log" => "parse runner log".to_owned(),
        other => other.to_owned(),
    }
}

fn render_issue(issue: &Issue) -> String {
    match issue {
        Issue::Module(issue) => {
            let location = match (issue.line, issue.column) {
                (Some(line), Some(column)) => format!("{}:{}:{}", issue.path, line, column),
                (Some(line), None) => format!("{}:{}", issue.path, line),
                _ => issue.path.clone(),
            };
            format!(
                "{} {} {}",
                render_severity(&issue.severity),
                location,
                issue.message
            )
        }
        Issue::Object(issue) => format!(
            "{} {} {}",
            render_severity(&issue.severity),
            issue.object,
            issue.message
        ),
        Issue::Edt(issue) => {
            let location = match (issue.line, issue.column) {
                (Some(line), Some(column)) => format!("{}:{}:{}", issue.path, line, column),
                (Some(line), None) => format!("{}:{}", issue.path, line),
                _ => issue.path.clone(),
            };
            format!(
                "{} {} {}",
                render_severity(&issue.severity),
                location,
                issue.message
            )
        }
    }
}

fn render_severity(severity: &IssueSeverity) -> &'static str {
    match severity {
        IssueSeverity::Error => "ERROR",
        IssueSeverity::Warning => "WARNING",
        IssueSeverity::Info => "INFO",
    }
}

fn status_label(status: &TestStatus) -> &'static str {
    match status {
        TestStatus::Passed => "PASSED",
        TestStatus::Failed => "FAILED",
        TestStatus::Skipped => "SKIPPED",
        TestStatus::Error => "ERROR",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        command_name, execute_command, map_artifacts_request_with_config, map_build_request,
        map_designer_config_request, map_dump_request, map_extensions_request, map_launch_request,
        map_load_request, map_syntax_request, map_test_request, pre_dispatch_error_envelope,
    };
    use crate::cli::args::{
        ArtifactsArgs, BuildArgs, Command, DesignerConfigSyntaxArgs, DesignerModulesSyntaxArgs,
        DumpArgs, ExtensionsArgs, LaunchArgs, LaunchOptionsArgs, LoadArgs, SyntaxArgs,
        SyntaxTarget, TestArgs, TestRunner, TestScope, TestYaxunitArgs,
    };
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, SourceFormat, SourceSetConfig, SourceSetPurpose,
        TestsConfig, ToolsConfig,
    };
    use crate::domain::load::LoadMode;
    use crate::domain::runner::{LaunchOptions, RunnerKind};
    use crate::output::presenter::{ColorMode, Presenter};
    use crate::support::fs::acquire_advisory_lock;
    use crate::support::temp::platform_logs_dir;
    use crate::use_cases::context::CommandName;
    use crate::use_cases::request::{
        ArtifactsModeRequest, DumpModeRequest, LaunchModeRequest, LaunchRequest,
        SyntaxTargetRequest, TestScopeRequest,
    };
    use crate::use_cases::result::UseCaseErrorKind;
    use crate::use_cases::workspace_lock::workspace_lock_path;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    #[test]
    fn maps_test_module_request() {
        let work = tempdir().expect("tempdir");
        let config = sample_config(work.path());
        let request = map_test_request(
            &config,
            &TestArgs {
                full: true,
                client_mode: None,
                launch: LaunchOptionsArgs::default(),
                runner: TestRunner::Yaxunit(TestYaxunitArgs {
                    scope: TestScope::Module {
                        name: "ModuleA".to_owned(),
                    },
                }),
            },
        )
        .expect("request");

        assert!(request.full);
        assert_eq!(
            request.scope,
            TestScopeRequest::Module {
                name: "ModuleA".to_owned()
            }
        );
    }

    #[test]
    fn rejects_blank_test_module_request() {
        let work = tempdir().expect("tempdir");
        let config = sample_config(work.path());
        let error = map_test_request(
            &config,
            &TestArgs {
                full: false,
                client_mode: None,
                launch: LaunchOptionsArgs::default(),
                runner: TestRunner::Yaxunit(TestYaxunitArgs {
                    scope: TestScope::Module {
                        name: "   ".to_owned(),
                    },
                }),
            },
        )
        .expect_err("blank module should be rejected");

        assert_eq!(error.kind(), UseCaseErrorKind::Validation);
        assert_eq!(
            error.message(),
            "test module requires a non-empty module name"
        );
    }

    #[test]
    fn maps_vanessa_request_from_configured_profile() {
        let work = tempdir().expect("tempdir");
        let base = work.path().join("base");
        let features = work.path().join("features");
        let epf = work.path().join("va.epf");
        let params = work.path().join("va.json");
        std::fs::create_dir_all(base.join("src")).expect("src");
        std::fs::create_dir_all(&features).expect("features");
        std::fs::write(&epf, "epf").expect("epf");
        std::fs::write(&params, "{}").expect("params");

        let mut config = sample_config(work.path());
        config.base_path = base;
        config.tests.va.epf_path = Some(epf);
        config.tests.va.params_path = Some(params);
        config.tests.va.profile = Some("smoke".to_owned());
        config.tests.va.profiles.insert(
            "smoke".to_owned(),
            crate::config::model::VanessaProfileConfig {
                feature_path: Some(features),
                ..Default::default()
            },
        );

        let request = map_test_request(
            &config,
            &TestArgs {
                full: false,
                client_mode: None,
                launch: LaunchOptionsArgs::default(),
                runner: TestRunner::Va,
            },
        )
        .expect("request");

        assert_eq!(request.execution.profile.kind, RunnerKind::Vanessa);
        assert_eq!(request.execution.profile.id, "smoke");
        assert_eq!(request.scope, TestScopeRequest::All);
        assert_eq!(request.execution.timeouts.total_ms, Some(300_000));
    }

    #[test]
    fn maps_syntax_request() {
        let request = map_syntax_request(&SyntaxArgs {
            target: SyntaxTarget::DesignerModules(DesignerModulesSyntaxArgs {
                thin_client: true,
                web_client: false,
                server: true,
                external_connection: false,
                thick_client_ordinary_application: false,
                mobile_app_client: false,
                mobile_app_server: false,
                mobile_client: false,
                extended_modules_check: true,
                extension: Some("Ext".to_owned()),
                all_extensions: false,
            }),
        });

        assert!(matches!(
            request.target,
            SyntaxTargetRequest::DesignerModules(ref modules)
                if modules.thin_client && modules.server && modules.extension.as_deref() == Some("Ext")
        ));
    }

    #[test]
    fn maps_build_dump_launch_and_load_requests() {
        assert!(map_build_request(&BuildArgs { full_rebuild: true }).full_rebuild);
        assert_eq!(
            map_extensions_request(&ExtensionsArgs {
                names: vec!["client_mcp".to_owned()],
            })
            .names,
            vec!["client_mcp"]
        );
        assert_eq!(
            map_dump_request(&DumpArgs {
                mode: "incremental".to_owned(),
                source_set: Some("main".to_owned()),
                extension: Some("Ext".to_owned()),
                objects: vec!["Catalog.Item".to_owned()],
            })
            .expect("request")
            .mode,
            DumpModeRequest::Incremental
        );
        assert_eq!(
            map_dump_request(&DumpArgs {
                mode: "incremental".to_owned(),
                source_set: Some("main".to_owned()),
                extension: Some("Ext".to_owned()),
                objects: vec!["Catalog.Item".to_owned()],
            })
            .expect("request")
            .source_set
            .as_deref(),
            Some("main")
        );
        assert_eq!(
            map_launch_request(&LaunchArgs {
                target: None,
                mode: Some("thin".to_owned()),
                launch: LaunchOptionsArgs {
                    c: Some("Command".to_owned()),
                    execute: Some("tool.epf".to_owned()),
                    use_privileged_mode: true,
                    output: Some("launch.log".to_owned()),
                    raw_keys: vec!["/WA-".to_owned(), "/DisplayAllFunctions".to_owned()],
                },
            })
            .expect("request"),
            LaunchRequest {
                mode: LaunchModeRequest::Thin,
                launch: LaunchOptions {
                    c: Some("Command".to_owned()),
                    execute: Some("tool.epf".to_owned()),
                    use_privileged_mode: true,
                    out: Some("launch.log".to_owned()),
                    internal_out: None,
                    raw_args: vec!["/WA-".to_owned(), "/DisplayAllFunctions".to_owned()],
                },
            }
        );
        assert_eq!(
            map_launch_request(&LaunchArgs {
                target: Some("ordinary".to_owned()),
                mode: None,
                launch: LaunchOptionsArgs::default(),
            })
            .expect("request")
            .mode,
            LaunchModeRequest::Ordinary
        );
        assert_eq!(
            map_launch_request(&LaunchArgs {
                target: None,
                mode: Some("thin".to_owned()),
                launch: LaunchOptionsArgs::default(),
            })
            .expect("request")
            .mode,
            LaunchModeRequest::Thin
        );
        let load = map_load_request(&LoadArgs {
            path: "dist/main.cf".to_owned(),
            mode: "merge".to_owned(),
            settings: Some("merge.xml".to_owned()),
            extension: Some("Ext".to_owned()),
        })
        .expect("load request");
        assert_eq!(load.mode, LoadMode::Merge);
        assert_eq!(load.artifact_path, "dist/main.cf");
        assert_eq!(load.settings_path.as_deref(), Some("merge.xml"));
        assert_eq!(load.extension.as_deref(), Some("Ext"));
        let artifacts = map_artifacts_request_with_config(
            &sample_config(Path::new("/tmp/work")),
            &ArtifactsArgs {
                output: "dist/ext.cfe".to_owned(),
                source_set: Some("ext-sales".to_owned()),
                extension: Some("SalesAddon".to_owned()),
            },
        )
        .expect("request");
        assert_eq!(artifacts.mode, ArtifactsModeRequest::ExtensionCfe);
        assert_eq!(artifacts.source_set.as_deref(), Some("ext-sales"));
        assert_eq!(artifacts.extension.as_deref(), Some("SalesAddon"));
    }

    #[test]
    fn maps_artifacts_request_keeps_blank_extension_in_cfe_mode() {
        let artifacts = map_artifacts_request_with_config(
            &sample_config(Path::new("/tmp/work")),
            &ArtifactsArgs {
                output: "dist/main.cf".to_owned(),
                source_set: Some("main".to_owned()),
                extension: Some("   ".to_owned()),
            },
        )
        .expect("request");

        assert_eq!(artifacts.mode, ArtifactsModeRequest::ExtensionCfe);
        assert_eq!(artifacts.extension.as_deref(), Some("   "));
        assert_eq!(artifacts.source_set.as_deref(), Some("main"));
    }

    #[test]
    fn rejects_invalid_mode_mapping() {
        let dump_error = map_dump_request(&DumpArgs {
            mode: "garbage".to_owned(),
            source_set: None,
            extension: None,
            objects: vec![],
        })
        .expect_err("dump mode should be rejected");
        let launch_error = map_launch_request(&LaunchArgs {
            target: None,
            mode: Some("garbage".to_owned()),
            launch: LaunchOptionsArgs::default(),
        })
        .expect_err("launch mode should be rejected");

        assert_eq!(dump_error.kind(), UseCaseErrorKind::Validation);
        assert_eq!(launch_error.kind(), UseCaseErrorKind::Validation);
    }

    #[test]
    fn rejects_invalid_load_mode_mapping() {
        let error = map_load_request(&LoadArgs {
            path: "dist/main.cf".to_owned(),
            mode: "garbage".to_owned(),
            settings: None,
            extension: None,
        })
        .expect_err("load mode should be rejected");

        assert_eq!(error.kind(), UseCaseErrorKind::Validation);
        assert_eq!(error.message(), "unsupported load mode: garbage");
    }

    #[test]
    fn maps_designer_config_request() {
        let request = map_designer_config_request(&DesignerConfigSyntaxArgs {
            config_log_integrity: true,
            incorrect_references: false,
            thin_client: true,
            web_client: false,
            mobile_client: false,
            server: true,
            external_connection: false,
            external_connection_server: false,
            mobile_app_client: false,
            mobile_app_server: false,
            thick_client_managed_application: false,
            thick_client_server_managed_application: false,
            thick_client_ordinary_application: false,
            thick_client_server_ordinary_application: false,
            mobile_client_digi_sign: false,
            distributive_modules: false,
            unreference_procedures: false,
            handlers_existence: false,
            empty_handlers: false,
            extended_modules_check: true,
            check_use_synchronous_calls: true,
            check_use_modality: false,
            unsupported_functional: false,
            extension: Some("Ext".to_owned()),
            all_extensions: false,
        });

        assert!(request.config_log_integrity);
        assert!(request.thin_client);
        assert!(request.server);
        assert!(request.extended_modules_check);
        assert!(request.check_use_synchronous_calls);
    }

    #[test]
    fn resolves_command_name() {
        assert_eq!(command_name(&Command::Init), CommandName::Init);
        assert_eq!(
            command_name(&Command::Extensions(ExtensionsArgs { names: vec![] })),
            CommandName::Extensions
        );
        assert_eq!(
            command_name(&Command::Build(BuildArgs {
                full_rebuild: false
            })),
            CommandName::Build
        );
        assert_eq!(
            command_name(&Command::Load(LoadArgs {
                path: "dist/main.cf".to_owned(),
                mode: "load".to_owned(),
                settings: None,
                extension: None,
            })),
            CommandName::Load
        );
        assert_eq!(
            command_name(&Command::Artifacts(ArtifactsArgs {
                output: "dist/main.cf".to_owned(),
                source_set: None,
                extension: None,
            })),
            CommandName::Artifacts
        );
    }

    fn sample_config(work_path: &Path) -> AppConfig {
        AppConfig {
            base_path: work_path.join("base"),
            work_path: work_path.to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
            source_sets: vec![
                SourceSetConfig {
                    name: "main".to_owned(),
                    purpose: SourceSetPurpose::Configuration,
                    path: PathBuf::from("main"),
                },
                SourceSetConfig {
                    name: "ext-sales".to_owned(),
                    purpose: SourceSetPurpose::Extension,
                    path: PathBuf::from("ext-sales"),
                },
                SourceSetConfig {
                    name: "external-processors".to_owned(),
                    purpose: SourceSetPurpose::ExternalDataProcessors,
                    path: PathBuf::from("external-processors"),
                },
            ],
            build: BuildConfig::default(),
            tools: ToolsConfig::default(),
            mcp: Default::default(),
            tests: TestsConfig::default(),
        }
    }

    #[test]
    fn execute_command_reports_workspace_lock_conflict_before_dispatch() {
        let dir = tempdir().expect("tempdir");
        let work = dir.path().join("work");
        fs::create_dir_all(&work).expect("work dir");
        let config = sample_config(&work);
        let canonical_work = fs::canonicalize(&config.work_path).expect("canonical work");
        let lock_path = workspace_lock_path(&canonical_work);
        let _guard = acquire_advisory_lock(&lock_path).expect("workspace lock");
        let presenter = Presenter::new("text".to_owned(), ColorMode::Disabled);

        let error = execute_command(
            &config,
            &Command::Build(BuildArgs { full_rebuild: true }),
            &presenter,
            false,
        )
        .expect_err("busy workspace");

        assert_eq!(error.kind(), UseCaseErrorKind::Runtime);
        assert!(error.to_string().contains("workspace"));
        assert!(error.to_string().contains("already"));
    }

    #[test]
    fn execute_command_reports_workspace_lock_conflict_for_test_command() {
        let dir = tempdir().expect("tempdir");
        let work = dir.path().join("work");
        fs::create_dir_all(&work).expect("work dir");
        let config = sample_config(&work);
        let canonical_work = fs::canonicalize(&config.work_path).expect("canonical work");
        let lock_path = workspace_lock_path(&canonical_work);
        let _guard = acquire_advisory_lock(&lock_path).expect("workspace lock");
        let presenter = Presenter::new("text".to_owned(), ColorMode::Disabled);

        let error = execute_command(
            &config,
            &Command::Test(TestArgs {
                full: false,
                client_mode: None,
                launch: LaunchOptionsArgs::default(),
                runner: TestRunner::Yaxunit(TestYaxunitArgs {
                    scope: TestScope::All,
                }),
            }),
            &presenter,
            false,
        )
        .expect_err("busy workspace");

        assert_eq!(error.kind(), UseCaseErrorKind::Runtime);
        assert!(error.to_string().contains("workspace"));
        assert!(error.to_string().contains("already"));
    }

    #[test]
    fn execute_command_validates_before_trying_workspace_lock() {
        let dir = tempdir().expect("tempdir");
        let work = dir.path().join("work");
        fs::create_dir_all(&work).expect("work dir");
        let config = sample_config(&work);
        let canonical_work = fs::canonicalize(&config.work_path).expect("canonical work");
        let lock_path = workspace_lock_path(&canonical_work);
        let _guard = acquire_advisory_lock(&lock_path).expect("workspace lock");
        let presenter = Presenter::new("text".to_owned(), ColorMode::Disabled);

        let error = execute_command(
            &config,
            &Command::Launch(LaunchArgs {
                target: None,
                mode: Some("garbage".to_owned()),
                launch: LaunchOptionsArgs::default(),
            }),
            &presenter,
            false,
        )
        .expect_err("invalid mode");

        assert_eq!(error.kind(), UseCaseErrorKind::Validation);
        assert!(!error.to_string().contains("workspace"));
    }

    #[test]
    fn execute_command_validates_test_module_before_trying_workspace_lock() {
        let dir = tempdir().expect("tempdir");
        let work = dir.path().join("work");
        fs::create_dir_all(&work).expect("work dir");
        let config = sample_config(&work);
        let canonical_work = fs::canonicalize(&config.work_path).expect("canonical work");
        let lock_path = workspace_lock_path(&canonical_work);
        let _guard = acquire_advisory_lock(&lock_path).expect("workspace lock");
        let presenter = Presenter::new("text".to_owned(), ColorMode::Disabled);

        let error = execute_command(
            &config,
            &Command::Test(TestArgs {
                full: false,
                client_mode: None,
                launch: LaunchOptionsArgs::default(),
                runner: TestRunner::Yaxunit(TestYaxunitArgs {
                    scope: TestScope::Module {
                        name: "   ".to_owned(),
                    },
                }),
            }),
            &presenter,
            false,
        )
        .expect_err("invalid module");

        assert_eq!(error.kind(), UseCaseErrorKind::Validation);
        assert!(!error.to_string().contains("workspace"));
    }

    #[test]
    fn execute_command_does_not_clean_logs_when_workspace_is_busy() {
        let dir = tempdir().expect("tempdir");
        let work = dir.path().join("work");
        fs::create_dir_all(&work).expect("work dir");
        let config = sample_config(&work);
        let logs_dir = platform_logs_dir(&config.work_path).expect("logs dir");
        fs::create_dir_all(&logs_dir).expect("create logs dir");
        let stale_log = logs_dir.join("stale.log");
        fs::write(&stale_log, "old").expect("stale log");
        let canonical_work = fs::canonicalize(&config.work_path).expect("canonical work");
        let lock_path = workspace_lock_path(&canonical_work);
        let _guard = acquire_advisory_lock(&lock_path).expect("workspace lock");
        let presenter = Presenter::new("text".to_owned(), ColorMode::Disabled);

        let _ = execute_command(
            &config,
            &Command::Build(BuildArgs { full_rebuild: true }),
            &presenter,
            true,
        )
        .expect_err("busy workspace");

        assert!(stale_log.exists());
    }

    #[test]
    fn pre_dispatch_json_error_keeps_command_identity() {
        let envelope = pre_dispatch_error_envelope(CommandName::Build, "workspace is busy");
        let json = serde_json::to_value(envelope).expect("json");

        assert_eq!(json["command"], "build");
        assert_eq!(json["data"]["message"], "workspace is busy");
    }
}
