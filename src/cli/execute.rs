use std::time::Instant;

use serde_json::json;

use crate::cli::args::{
    ArtifactsArgs, BuildArgs, Command, DesignerConfigSyntaxArgs, DesignerModulesSyntaxArgs,
    DumpArgs, ExtensionsArgs, LaunchArgs, LaunchOptionsArgs, LoadArgs, SyntaxArgs, SyntaxTarget,
    TestArgs, TestRunner, TestScope, TestYaxunitArgs,
};
use crate::config::model::{AppConfig, SourceSetPurpose};
use crate::domain::artifacts::{ArtifactBuildMode, ArtifactsResult};
use crate::domain::build::{BuildMode, BuildResult};
use crate::domain::dump::{DumpMode, DumpResult};
use crate::domain::execution::ExecutionTimeouts;
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
use crate::support::error::AppError;
use crate::support::fs::clean_dir;
use crate::support::path::is_safe_path_segment;
use crate::support::temp::platform_logs_dir;
use crate::use_cases::artifacts;
use crate::use_cases::build_project;
use crate::use_cases::check_syntax;
use crate::use_cases::configure_extensions;
use crate::use_cases::context::{CommandName, ExecutionContext};
use crate::use_cases::dump_config;
use crate::use_cases::init_project;
use crate::use_cases::launch_app;
use crate::use_cases::load_artifact;
use crate::use_cases::request::{
    ArtifactsModeRequest, ArtifactsRequest, BuildRequest, ConfigureExtensionsRequest,
    DesignerConfigSyntaxRequest, DesignerModulesSyntaxRequest, DumpModeRequest, DumpRequest,
    InitRequest, LaunchModeRequest, LaunchRequest, LoadRequest, SyntaxRequest, SyntaxTargetRequest,
    TestRequest, TestScopeRequest,
};
use crate::use_cases::result::{UseCaseError, UseCaseErrorKind};
use crate::use_cases::run_tests;
use crate::use_cases::workspace_lock::acquire_workspace_lock;

/// Executes a parsed CLI command by mapping it into transport-neutral requests and
/// rendering the resulting command output.
pub fn execute_command(
    config: &AppConfig,
    command: &Command,
    presenter: &Presenter,
    clean_before_execution: bool,
) -> Result<(), UseCaseError> {
    match command {
        Command::Config(_) => unreachable!("config commands are handled outside cli::execute"),
        Command::Init => execute_init(config, presenter, clean_before_execution),
        Command::Extensions(args) => {
            execute_extensions(config, args, presenter, clean_before_execution)
        }
        Command::Build(args) => execute_build(config, args, presenter, clean_before_execution),
        Command::Load(args) => execute_load(config, args, presenter, clean_before_execution),
        Command::Test(args) => execute_test(config, args, presenter, clean_before_execution),
        Command::Dump(args) => execute_dump(config, args, presenter, clean_before_execution),
        Command::Artifacts(args) => {
            execute_artifacts(config, args, presenter, clean_before_execution)
        }
        Command::Syntax(args) => execute_syntax(config, args, presenter, clean_before_execution),
        Command::Launch(args) => execute_launch(config, args, presenter, clean_before_execution),
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
) -> Result<(), UseCaseError> {
    let request = map_extensions_request(args);
    let context = ExecutionContext::cli(CommandName::Extensions);
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
) -> Result<(), UseCaseError> {
    let request = InitRequest;
    let context = ExecutionContext::cli(CommandName::Init);
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
) -> Result<(), UseCaseError> {
    let request = map_build_request(args);
    let context = ExecutionContext::cli(CommandName::Build);
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
) -> Result<(), UseCaseError> {
    let request = map_test_request(config, args)?;
    let context = ExecutionContext::cli(CommandName::Test);
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
) -> Result<(), UseCaseError> {
    let request = map_load_request(args)?;
    let context = ExecutionContext::cli(CommandName::Load);
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
) -> Result<(), UseCaseError> {
    let request = map_dump_request(args)?;
    let context = ExecutionContext::cli(CommandName::Dump);
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

fn execute_artifacts(
    config: &AppConfig,
    args: &ArtifactsArgs,
    presenter: &Presenter,
    clean_before_execution: bool,
) -> Result<(), UseCaseError> {
    let request = map_artifacts_request_with_config(config, args)?;
    let context = ExecutionContext::cli(CommandName::Artifacts);
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
) -> Result<(), UseCaseError> {
    let request = map_syntax_request(args);
    let context = ExecutionContext::cli(CommandName::Syntax);
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
) -> Result<(), UseCaseError> {
    let request = map_launch_request(args)?;
    let context = ExecutionContext::cli(CommandName::Launch);
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
    let _workspace_lock = acquire_workspace_lock(config, command.as_str())
        .map_err(|error| render_pre_dispatch_error(presenter, command, error))?;
    if clean_before_execution {
        clean_platform_logs_under_lock(config, presenter, command)?;
    }
    run()
}

fn clean_platform_logs_under_lock(
    config: &AppConfig,
    presenter: &Presenter,
    command: CommandName,
) -> Result<(), UseCaseError> {
    platform_logs_dir(&config.work_path)
        .and_then(|dir| clean_dir(&dir))
        .map_err(|error| {
            render_pre_dispatch_error(
                presenter,
                command,
                AppError::Runtime(format!("failed to clean platform logs: {error}")),
            )
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
    if args.out.is_some() {
        return Err(UseCaseError::new(
            UseCaseErrorKind::Validation,
            "--out is not supported for test; the platform log path is managed internally",
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
    if args.c.is_some() || args.execute.is_some() || args.out.is_some() {
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
        mode: match args.mode.as_str() {
            "full" => DumpModeRequest::Full,
            "incremental" => DumpModeRequest::Incremental,
            "partial" => DumpModeRequest::Partial,
            other => {
                return Err(UseCaseError::new(
                    UseCaseErrorKind::Validation,
                    format!("unsupported dump mode: {other}"),
                ));
            }
        },
        source_set: args.source_set.clone(),
        extension: args.extension.clone(),
        objects: args.objects.clone(),
    })
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
    DesignerConfigSyntaxRequest {
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
        extension: args.extension.clone(),
        all_extensions: args.all_extensions,
    }
}

fn map_designer_modules_request(args: &DesignerModulesSyntaxArgs) -> DesignerModulesSyntaxRequest {
    DesignerModulesSyntaxRequest {
        thin_client: args.thin_client,
        web_client: args.web_client,
        server: args.server,
        external_connection: args.external_connection,
        thick_client_ordinary_application: args.thick_client_ordinary_application,
        mobile_app_client: args.mobile_app_client,
        mobile_app_server: args.mobile_app_server,
        mobile_client: args.mobile_client,
        extended_modules_check: args.extended_modules_check,
        extension: args.extension.clone(),
        all_extensions: args.all_extensions,
    }
}

fn map_launch_request(args: &LaunchArgs) -> Result<LaunchRequest, UseCaseError> {
    let Some(mode) = args.mode.as_deref().or(args.target.as_deref()) else {
        return Err(UseCaseError::new(
            UseCaseErrorKind::Validation,
            "launch mode is required",
        ));
    };

    Ok(LaunchRequest {
        mode: match mode {
            "designer" => LaunchModeRequest::Designer,
            "thin" => LaunchModeRequest::Thin,
            "thick" => LaunchModeRequest::Thick,
            "ordinary" => LaunchModeRequest::Ordinary,
            other => {
                return Err(UseCaseError::new(
                    UseCaseErrorKind::Validation,
                    format!("unsupported launch mode: {other}"),
                ));
            }
        },
        launch: map_direct_launch_options(&args.launch),
    })
}

fn map_direct_launch_options(args: &LaunchOptionsArgs) -> LaunchOptions {
    LaunchOptions {
        c: args.c.clone(),
        execute: args.execute.clone(),
        use_privileged_mode: args.use_privileged_mode,
        out: args.out.clone(),
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

fn status_detail(ok: bool, message: impl AsRef<str>) -> String {
    if ok {
        format!("✓ {}", message.as_ref())
    } else {
        format!("✗ {}", message.as_ref())
    }
}

fn step_status_detail(status: &InitStepStatus, message: impl AsRef<str>) -> String {
    match status {
        InitStepStatus::Ok => format!("✓ {}", message.as_ref()),
        InitStepStatus::Skipped => format!("○ {}", message.as_ref()),
        InitStepStatus::Failed => format!("✗ {}", message.as_ref()),
    }
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
    let mut details = vec![format!(
        "[Конфигуратор] {mode} {} <- {}",
        render_artifact_mode(result.artifact_type),
        result.artifact_path.display()
    )];
    if let Some(message) = result.message.as_deref() {
        details.push(status_detail(succeeded, message));
    }
    if let Some(path) = result.platform_log_path.as_deref() {
        details.push(format!("platform log: {}", path.display()));
    }
    let mut timeline = vec![timeline_item_with_details(
        timeline_status(succeeded),
        format!("{target}:"),
        details,
    )];
    timeline.push(if succeeded {
        TimelineItem::new(
            TimelineStatus::Succeeded,
            "Artifact load completed successfully",
        )
    } else {
        TimelineItem::new(TimelineStatus::Failed, "Artifact load failed")
    });
    presenter.print_timeline(&timeline);
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
    let mut details = vec![format!(
        "[Конфигуратор] Выгрузка {mode} -> {}",
        result.target_path.display()
    )];
    if let Some(extension) = result.extension.as_deref() {
        details.push(format!("extension: {extension}"));
    }
    if let Some(message) = result.message.as_deref() {
        details.push(status_detail(succeeded, message));
    }
    if let Some(path) = result.platform_log_path.as_deref() {
        details.push(format!("platform log: {}", path.display()));
    }
    let mut timeline = vec![timeline_item_with_details(
        timeline_status(succeeded),
        format!("{source_set}:"),
        details,
    )];

    timeline.push(if succeeded {
        TimelineItem::new(TimelineStatus::Succeeded, "Dump completed successfully")
    } else {
        TimelineItem::new(TimelineStatus::Failed, "Dump failed")
    });
    presenter.print_timeline(&timeline);
}

fn render_artifacts_text(result: &ArtifactsResult, presenter: &Presenter, succeeded: bool) {
    let source_set = result.source_set.as_deref().unwrap_or("<unresolved>");
    let mut details = vec![format!(
        "[Конфигуратор] Сборка {} -> {}",
        render_artifact_mode(result.mode),
        result.output_path.display()
    )];
    if let Some(extension) = result.extension.as_deref() {
        details.push(format!("extension: {extension}"));
    }
    let published_count = result
        .artifacts
        .items
        .iter()
        .filter(|artifact| artifact.role.as_deref() == Some("package_file"))
        .count();
    if published_count > 1 {
        details.push(format!("published files: {published_count}"));
    }
    if let Some(message) = result.message.as_deref() {
        details.push(status_detail(succeeded, message));
    }
    if let Some(path) = result.platform_log_path.as_deref() {
        details.push(format!("platform log: {}", path.display()));
    }
    let mut timeline = vec![timeline_item_with_details(
        timeline_status(succeeded),
        format!("{source_set}:"),
        details,
    )];
    timeline.push(if succeeded {
        TimelineItem::new(
            TimelineStatus::Succeeded,
            "Artifacts export completed successfully",
        )
    } else {
        TimelineItem::new(TimelineStatus::Failed, "Artifacts export failed")
    });
    presenter.print_timeline(&timeline);
}

fn render_syntax_text(result: &SyntaxCheckResult, presenter: &Presenter) {
    let succeeded = matches!(result.status, SyntaxCheckStatus::Clean);
    let mut details = vec![
        format!("[Синтаксис] Проверка: {}", result.check_name),
        status_detail(
            succeeded,
            format!(
                "{} (exit {}, errors {}, warnings {}, info {}, duration {} ms)",
                render_syntax_status(result.status),
                result.exit_code,
                result.summary.errors,
                result.summary.warnings,
                result.summary.info,
                result.duration_ms
            ),
        ),
    ];
    if let Some(path) = result.platform_log_path.as_deref() {
        details.push(format!("platform log: {}", path.display()));
    }
    for issue in &result.issues {
        details.push(render_issue(issue));
    }

    if let Some(log_read_warning) = &result.log_read_warning {
        details.push(format!("Warning: log {log_read_warning}"));
    }

    if matches!(result.status, SyntaxCheckStatus::ToolFailed) {
        if let Some(stderr) = &result.stderr {
            details.push(format!("stderr: {}", stderr.trim()));
        }
    }

    let mut timeline = vec![timeline_item_with_details(
        timeline_status(succeeded),
        "syntax:",
        details,
    )];
    timeline.push(if succeeded {
        TimelineItem::new(
            TimelineStatus::Succeeded,
            format!("Syntax check {} completed successfully", result.check_name),
        )
    } else {
        TimelineItem::new(
            TimelineStatus::Failed,
            format!("Syntax check {} failed", result.check_name),
        )
    });
    presenter.print_timeline(&timeline);
}

fn render_syntax_status(status: SyntaxCheckStatus) -> &'static str {
    match status {
        SyntaxCheckStatus::Clean => "clean",
        SyntaxCheckStatus::IssuesFound => "issues_found",
        SyntaxCheckStatus::ToolFailed => "tool_failed",
    }
}

fn render_launch_text(result: &LaunchResult, presenter: &Presenter) {
    let message = result
        .message
        .as_deref()
        .unwrap_or("Launched application successfully");
    let mut details = vec![
        format!("[Запуск] Приложение: {}", render_launch_mode(&result.mode)),
        status_detail(true, message),
        format!("binary: {}", result.binary.display()),
    ];
    if let Some(pid) = result.pid {
        details.push(format!("pid: {pid}"));
    }

    let timeline = vec![
        timeline_item_with_details(TimelineStatus::Succeeded, "launch:", details),
        TimelineItem::new(TimelineStatus::Succeeded, "Launch completed successfully"),
    ];
    presenter.print_timeline(&timeline);
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
    let mut details = vec![format!("target: {}", render_test_target(&result.target))];
    for step in &result.steps {
        let message = step
            .message
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or("ok");
        details.push(status_detail(
            step.ok,
            format!("{}: {message}", render_test_step_label(&step.name)),
        ));
    }

    let status = timeline_status(result.ok);
    let mut timeline = vec![timeline_item_with_details(status, "tests:", details)];

    let mut summary_details = Vec::new();
    if let Some(report) = &result.report {
        summary_details.push(format!(
            "total={}, passed={}, failed={}, skipped={}, errors={}",
            report.summary.total,
            report.summary.passed,
            report.summary.failed,
            report.summary.skipped,
            report.summary.errors
        ));
        for suite in &report.suites {
            summary_details.push(format!("Suite: {}", suite.name));
            for case in &suite.cases {
                summary_details.push(format!("  {} {}", status_label(&case.status), case.name));
                if let Some(message) = &case.failure_message {
                    summary_details.push(format!("    {message}"));
                }
                if let Some(trace) = &case.stack_trace {
                    summary_details.push(format!("    {trace}"));
                }
            }
        }
    }
    for diagnostic in &result.diagnostics {
        summary_details.push(format!("Diagnostic: {diagnostic}"));
    }
    for warning in &result.warnings {
        summary_details.push(format!("Warning: {warning}"));
    }

    timeline.push(timeline_item_with_details(
        status,
        if result.ok {
            "Tests completed successfully"
        } else {
            "Tests failed"
        },
        summary_details,
    ));
    presenter.print_timeline(&timeline);
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
                    out: Some("launch.log".to_owned()),
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
