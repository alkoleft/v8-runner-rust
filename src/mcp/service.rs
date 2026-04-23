use crate::config::model::AppConfig;
use crate::domain::artifact::{ARTIFACT_ROLE_PLATFORM_LOG, ARTIFACT_ROLE_RUNNER_LOG};
use crate::domain::build::{BuildMode, BuildResult, BuildStep};
use crate::domain::dump::{DumpMode, DumpResult};
use crate::domain::execution::{
    ExecutionStatus, ExecutionStepKind, ExecutionStepStatus, StepResult,
};
use crate::domain::issue::{EdtIssue, Issue, IssueSeverity, ModuleIssue, ObjectIssue};
use crate::domain::launch::{LaunchMode, LaunchResult};
use crate::domain::runner::LaunchOptions;
use crate::domain::syntax::{SyntaxCheckResult, SyntaxCheckStatus};
use crate::domain::test::{TestCase, TestRunResult, TestStatus, TestSuite};
use crate::mcp::context::McpCallContext;
use crate::mcp::error::{
    McpBusinessError, McpBusinessErrorKind, McpBusinessFailure, McpInternalError, McpServiceError,
    McpServiceResult,
};
use crate::mcp::port::{DefaultMcpUseCasePort, McpUseCasePort};
use crate::mcp::request::{
    McpBuildProjectRequest, McpCheckSyntaxDesignerConfigRequest,
    McpCheckSyntaxDesignerModulesRequest, McpCheckSyntaxEdtRequest, McpDumpConfigRequest,
    McpLaunchAppRequest, McpRunAllTestsRequest, McpRunModuleTestsRequest,
};
use crate::mcp::response::{
    McpBuildMode, McpBuildResponse, McpBuildStep, McpDumpResponse, McpEdtIssue, McpIssue,
    McpIssueSeverity, McpLaunchResponse, McpModuleIssue, McpObjectIssue, McpStepKind,
    McpStepResult, McpStepStatus, McpSyntaxCheckResponse, McpTestCase, McpTestResponse,
    McpTestStatus, McpTestSuite,
};
use crate::support::adapter_input::{
    normalize_edt_projects, normalize_extension_scope, normalize_optional_string,
    normalize_required_string, parse_launch_target, parse_optional_dump_mode, LaunchModeAliases,
};
use crate::use_cases::context::{CommandName, ExecutionContext, ExecutionTransport};
use crate::use_cases::request::{
    BuildRequest, DesignerClientScope, DesignerClientScopes, DesignerConfigCheck,
    DesignerConfigChecks, DesignerConfigSyntaxRequest, DesignerModulesSyntaxRequest,
    DumpModeRequest, DumpRequest, LaunchRequest, SyntaxRequest, SyntaxTargetRequest, TestRequest,
    TestScopeRequest,
};
use crate::use_cases::result::{UseCaseError, UseCaseFailure, UseCaseResult};
use crate::use_cases::transport::map_failure_response;

/// MCP-facing service layer over transport-neutral use cases.
#[derive(Debug)]
pub struct McpService<'a, P = DefaultMcpUseCasePort> {
    config: &'a AppConfig,
    port: P,
}

impl<'a, P> McpService<'a, P>
where
    P: McpUseCasePort,
{
    /// Creates an MCP service with a custom use-case port.
    pub const fn with_port(config: &'a AppConfig, port: P) -> Self {
        Self { config, port }
    }

    /// Executes the `build_project` MCP tool.
    pub fn build_project(
        &self,
        call_context: McpCallContext,
        request: &McpBuildProjectRequest,
    ) -> McpServiceResult<McpBuildResponse> {
        let context = execution_context(call_context, CommandName::Build)
            .map_err(McpServiceError::Internal)?;
        let use_case_request = BuildRequest {
            full_rebuild: request.full_rebuild.unwrap_or(false),
        };

        match self
            .port
            .build_project(&context, self.config, &use_case_request)
        {
            Ok(result) => Ok(map_build_response(result)),
            Err(failure) => Err(map_use_case_failure(failure, map_build_response, |error| {
                McpBuildResponse {
                    success: false,
                    message: error.message().to_owned(),
                    build_time_ms: None,
                    steps: None,
                }
            })),
        }
    }

    /// Executes the `run_all_tests` MCP tool.
    pub fn run_all_tests(
        &self,
        call_context: McpCallContext,
        request: &McpRunAllTestsRequest,
    ) -> McpServiceResult<McpTestResponse> {
        let context = execution_context(call_context, CommandName::Test)
            .map_err(McpServiceError::Internal)?;
        let use_case_request = TestRequest {
            execution: TestRequest::default_execution(),
            full: request.full.unwrap_or(false),
            scope: TestScopeRequest::All,
        };

        match self
            .port
            .run_tests(&context, self.config, &use_case_request)
        {
            Ok(result) => Ok(map_test_response(result)),
            Err(failure) => Err(map_use_case_failure(failure, map_test_response, |error| {
                McpTestResponse {
                    success: false,
                    message: error.message().to_owned(),
                    total_tests: None,
                    passed_tests: None,
                    failed_tests: None,
                    execution_time_ms: None,
                    enterprise_log_path: None,
                    log_file: None,
                    test_detail: None,
                    steps: None,
                    errors: Some(vec![error.message().to_owned()]),
                }
            })),
        }
    }

    /// Executes the `run_module_tests` MCP tool.
    pub fn run_module_tests(
        &self,
        call_context: McpCallContext,
        request: &McpRunModuleTestsRequest,
    ) -> McpServiceResult<McpTestResponse> {
        let context = execution_context(call_context, CommandName::Test)
            .map_err(McpServiceError::Internal)?;
        let module_name =
            normalize_required_string(&request.module_name, "module_name").map_err(|error| {
                let message = error.message().to_owned();
                McpServiceError::Business(McpBusinessFailure::new(
                    McpBusinessError::from_use_case(&error),
                    McpTestResponse {
                        success: false,
                        message: message.clone(),
                        total_tests: None,
                        passed_tests: None,
                        failed_tests: None,
                        execution_time_ms: None,
                        enterprise_log_path: None,
                        log_file: None,
                        test_detail: None,
                        steps: None,
                        errors: Some(vec![message]),
                    },
                ))
            })?;
        let use_case_request = TestRequest {
            execution: TestRequest::default_execution(),
            full: request.full.unwrap_or(false),
            scope: TestScopeRequest::Module { name: module_name },
        };

        match self
            .port
            .run_tests(&context, self.config, &use_case_request)
        {
            Ok(result) => Ok(map_test_response(result)),
            Err(failure) => Err(map_use_case_failure(failure, map_test_response, |error| {
                McpTestResponse {
                    success: false,
                    message: error.message().to_owned(),
                    total_tests: None,
                    passed_tests: None,
                    failed_tests: None,
                    execution_time_ms: None,
                    enterprise_log_path: None,
                    log_file: None,
                    test_detail: None,
                    steps: None,
                    errors: Some(vec![error.message().to_owned()]),
                }
            })),
        }
    }

    /// Executes the `dump_config` MCP tool.
    pub fn dump_config(
        &self,
        call_context: McpCallContext,
        request: &McpDumpConfigRequest,
    ) -> McpServiceResult<McpDumpResponse> {
        let context = execution_context(call_context, CommandName::Dump)
            .map_err(McpServiceError::Internal)?;
        let use_case_request = DumpRequest {
            mode: parse_optional_dump_mode(request.mode.as_deref(), DumpModeRequest::Incremental)
                .map_err(|error| {
                let message = error.message().to_owned();
                McpServiceError::Business(McpBusinessFailure::new(
                    raw_value_business_error(&error, "dump mode"),
                    McpDumpResponse {
                        success: false,
                        message: request.mode.as_deref().map_or_else(
                            || "dump mode is invalid".to_owned(),
                            |value| format!("unsupported dump mode: {value}"),
                        ),
                        mode: request
                            .mode
                            .as_deref()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .unwrap_or("FULL")
                            .to_owned(),
                        dump_time_ms: None,
                        dumped_objects: None,
                        errors: Some(vec![message]),
                        steps: None,
                    },
                ))
            })?,
            source_set: None,
            extension: normalize_optional_string(request.extension.as_deref()),
            objects: request.objects.clone(),
        };

        match self
            .port
            .dump_config(&context, self.config, &use_case_request)
        {
            Ok(result) => Ok(map_dump_response(result)),
            Err(failure) => Err(map_use_case_failure(failure, map_dump_response, |error| {
                McpDumpResponse {
                    success: false,
                    message: error.message().to_owned(),
                    mode: render_dump_mode(use_case_request.mode).to_owned(),
                    dump_time_ms: None,
                    dumped_objects: None,
                    errors: Some(vec![error.message().to_owned()]),
                    steps: None,
                }
            })),
        }
    }

    /// Executes the `launch_app` MCP tool.
    pub fn launch_app(
        &self,
        call_context: McpCallContext,
        request: &McpLaunchAppRequest,
    ) -> McpServiceResult<McpLaunchResponse> {
        let context = execution_context(call_context, CommandName::Launch)
            .map_err(McpServiceError::Internal)?;
        let use_case_request = LaunchRequest {
            target: parse_launch_target(
                &request.utility_type,
                "utility_type",
                LaunchModeAliases::Mcp,
            )
            .map_err(|error| {
                let message = error.message().to_owned();
                McpServiceError::Business(McpBusinessFailure::new(
                    raw_value_business_error(&error, "utility_type"),
                    McpLaunchResponse {
                        success: false,
                        message,
                    },
                ))
            })?,
            launch: LaunchOptions::default(),
        };

        match self
            .port
            .launch_app(&context, self.config, &use_case_request)
        {
            Ok(result) => Ok(map_launch_response(result)),
            Err(failure) => Err(map_use_case_failure(
                failure,
                map_launch_response,
                |error| McpLaunchResponse {
                    success: false,
                    message: error.message().to_owned(),
                },
            )),
        }
    }

    /// Executes the `check_syntax_edt` MCP tool.
    pub fn check_syntax_edt(
        &self,
        call_context: McpCallContext,
        request: &McpCheckSyntaxEdtRequest,
    ) -> McpServiceResult<McpSyntaxCheckResponse> {
        let context = execution_context(call_context, CommandName::Syntax)
            .map_err(McpServiceError::Internal)?;
        let use_case_request = normalize_check_syntax_edt_request(request);

        map_syntax_use_case_result(
            self.port
                .check_syntax(&context, self.config, &use_case_request),
        )
    }

    /// Executes the `check_syntax_designer_config` MCP tool.
    pub fn check_syntax_designer_config(
        &self,
        call_context: McpCallContext,
        request: &McpCheckSyntaxDesignerConfigRequest,
    ) -> McpServiceResult<McpSyntaxCheckResponse> {
        let context = execution_context(call_context, CommandName::Syntax)
            .map_err(McpServiceError::Internal)?;
        let use_case_request = SyntaxRequest {
            target: SyntaxTargetRequest::DesignerConfig(
                map_designer_config_request(request).map_err(invalid_syntax_request)?,
            ),
        };

        match self
            .port
            .check_syntax(&context, self.config, &use_case_request)
        {
            Ok(result) => Ok(map_syntax_response(result)),
            Err(failure) => Err(map_use_case_failure(
                failure,
                map_syntax_response,
                |error| McpSyntaxCheckResponse {
                    success: false,
                    message: error.message().to_owned(),
                    check_result: None,
                    errors: Some(vec![error.message().to_owned()]),
                    issues: None,
                    duration_ms: None,
                },
            )),
        }
    }

    /// Executes the `check_syntax_designer_modules` MCP tool.
    pub fn check_syntax_designer_modules(
        &self,
        call_context: McpCallContext,
        request: &McpCheckSyntaxDesignerModulesRequest,
    ) -> McpServiceResult<McpSyntaxCheckResponse> {
        let context = execution_context(call_context, CommandName::Syntax)
            .map_err(McpServiceError::Internal)?;
        let use_case_request = SyntaxRequest {
            target: SyntaxTargetRequest::DesignerModules(
                map_designer_modules_request(request).map_err(invalid_syntax_request)?,
            ),
        };

        match self
            .port
            .check_syntax(&context, self.config, &use_case_request)
        {
            Ok(result) => Ok(map_syntax_response(result)),
            Err(failure) => Err(map_use_case_failure(
                failure,
                map_syntax_response,
                |error| McpSyntaxCheckResponse {
                    success: false,
                    message: error.message().to_owned(),
                    check_result: None,
                    errors: Some(vec![error.message().to_owned()]),
                    issues: None,
                    duration_ms: None,
                },
            )),
        }
    }
}

fn execution_context(
    call_context: McpCallContext,
    command: CommandName,
) -> Result<ExecutionContext, McpInternalError> {
    match call_context.transport() {
        transport @ (ExecutionTransport::McpStdio | ExecutionTransport::McpHttp) => {
            Ok(ExecutionContext::new(command, transport)
                .with_edt_timeout(call_context.edt_timeout())
                .with_deadline(call_context.deadline())
                .with_cancellation(call_context.cancellation()))
        }
        ExecutionTransport::Cli => Err(McpInternalError::new(format!(
            "mcp service received non-MCP transport for {}",
            command.as_str()
        ))),
    }
}

fn map_use_case_failure<TPayload, TResponse, FPayload, FFallback>(
    failure: UseCaseFailure<TPayload>,
    payload_mapper: FPayload,
    fallback_response: FFallback,
) -> McpServiceError<TResponse>
where
    FPayload: FnOnce(TPayload) -> TResponse,
    FFallback: FnOnce(&UseCaseError) -> TResponse,
{
    let (error, response) = map_failure_response(failure, payload_mapper, fallback_response);
    McpServiceError::Business(McpBusinessFailure::new(
        McpBusinessError::from_use_case(&error),
        response,
    ))
}

fn invalid_syntax_request(error: UseCaseError) -> McpServiceError<McpSyntaxCheckResponse> {
    let message = error.message().to_owned();
    McpServiceError::Business(McpBusinessFailure::new(
        McpBusinessError::from_use_case(&error),
        McpSyntaxCheckResponse {
            success: false,
            message: message.clone(),
            check_result: None,
            errors: Some(vec![message]),
            issues: None,
            duration_ms: None,
        },
    ))
}

fn raw_value_business_error(error: &UseCaseError, field_name: &'static str) -> McpBusinessError {
    let blank_message = format!("{field_name} must not be blank");
    let code = if error.message() == blank_message {
        crate::mcp::error::McpErrorCode::InvalidArgument
    } else {
        crate::mcp::error::McpErrorCode::UnsupportedValue
    };

    McpBusinessError {
        code,
        kind: McpBusinessErrorKind::Validation,
        message: error.message().to_owned(),
    }
}

fn map_designer_config_request(
    request: &McpCheckSyntaxDesignerConfigRequest,
) -> Result<DesignerConfigSyntaxRequest, UseCaseError> {
    let scope = normalize_extension_scope(request.extension.as_deref(), request.all_extensions);
    Ok(DesignerConfigSyntaxRequest::new(
        DesignerConfigChecks::new(
            [
                (request.config_log_integrity == Some(true))
                    .then_some(DesignerConfigCheck::ConfigLogIntegrity),
                (request.incorrect_references == Some(true))
                    .then_some(DesignerConfigCheck::IncorrectReferences),
                (request.mobile_client_digi_sign == Some(true))
                    .then_some(DesignerConfigCheck::MobileClientDigiSign),
                (request.distributive_modules == Some(true))
                    .then_some(DesignerConfigCheck::DistributiveModules),
                (request.unreference_procedures != Some(false))
                    .then_some(DesignerConfigCheck::UnreferenceProcedures),
                (request.handlers_existence != Some(false))
                    .then_some(DesignerConfigCheck::HandlersExistence),
                (request.empty_handlers != Some(false))
                    .then_some(DesignerConfigCheck::EmptyHandlers),
                (request.unsupported_functional == Some(true))
                    .then_some(DesignerConfigCheck::UnsupportedFunctional),
            ]
            .into_iter()
            .flatten(),
        ),
        DesignerClientScopes::new(
            [
                (request.thin_client != Some(false)).then_some(DesignerClientScope::ThinClient),
                (request.web_client == Some(true)).then_some(DesignerClientScope::WebClient),
                (request.mobile_client == Some(true)).then_some(DesignerClientScope::MobileClient),
                (request.server != Some(false)).then_some(DesignerClientScope::Server),
                (request.external_connection == Some(true))
                    .then_some(DesignerClientScope::ExternalConnection),
                (request.external_connection_server == Some(true))
                    .then_some(DesignerClientScope::ExternalConnectionServer),
                (request.mobile_app_client == Some(true))
                    .then_some(DesignerClientScope::MobileAppClient),
                (request.mobile_app_server == Some(true))
                    .then_some(DesignerClientScope::MobileAppServer),
                (request.thick_client_managed_application == Some(true))
                    .then_some(DesignerClientScope::ThickClientManagedApplication),
                (request.thick_client_server_managed_application == Some(true))
                    .then_some(DesignerClientScope::ThickClientServerManagedApplication),
                (request.thick_client_ordinary_application == Some(true))
                    .then_some(DesignerClientScope::ThickClientOrdinaryApplication),
                (request.thick_client_server_ordinary_application == Some(true))
                    .then_some(DesignerClientScope::ThickClientServerOrdinaryApplication),
            ]
            .into_iter()
            .flatten(),
        ),
        crate::use_cases::request::ExtendedModulesPolicy::from_mcp_flags(
            request.extended_modules_check,
            request.check_use_synchronous_calls,
            request.check_use_modality,
        )?,
        scope,
    ))
}

fn map_designer_modules_request(
    request: &McpCheckSyntaxDesignerModulesRequest,
) -> Result<DesignerModulesSyntaxRequest, UseCaseError> {
    let scope = normalize_extension_scope(request.extension.as_deref(), request.all_extensions);
    DesignerModulesSyntaxRequest::new(
        DesignerClientScopes::new(
            [
                (request.thin_client != Some(false)).then_some(DesignerClientScope::ThinClient),
                (request.web_client == Some(true)).then_some(DesignerClientScope::WebClient),
                (request.server != Some(false)).then_some(DesignerClientScope::Server),
                (request.external_connection == Some(true))
                    .then_some(DesignerClientScope::ExternalConnection),
                (request.thick_client_ordinary_application == Some(true))
                    .then_some(DesignerClientScope::ThickClientOrdinaryApplication),
                (request.mobile_app_client == Some(true))
                    .then_some(DesignerClientScope::MobileAppClient),
                (request.mobile_app_server == Some(true))
                    .then_some(DesignerClientScope::MobileAppServer),
                (request.mobile_client == Some(true)).then_some(DesignerClientScope::MobileClient),
            ]
            .into_iter()
            .flatten(),
        ),
        crate::use_cases::request::ExtendedModulesPolicy::basic(
            request.extended_modules_check != Some(false),
        ),
        scope,
    )
}

fn map_build_response(result: BuildResult) -> McpBuildResponse {
    let message = if result.ok {
        if result
            .steps
            .iter()
            .all(|step| step.ok && matches!(step.mode, BuildMode::Skipped))
        {
            "Build completed: no changes".to_owned()
        } else {
            "Build completed successfully".to_owned()
        }
    } else {
        "Build failed".to_owned()
    };

    McpBuildResponse {
        success: result.ok,
        message,
        build_time_ms: Some(result.duration_ms),
        steps: (!result.ok).then(|| map_build_steps(result.steps)),
    }
}

fn map_test_response(result: TestRunResult) -> McpTestResponse {
    let execution = &result.execution;
    let summary = execution
        .metrics
        .as_ref()
        .map(|metrics| (metrics.total, metrics.passed, metrics.failed))
        .or_else(|| {
            result.report.as_ref().map(|report| {
                (
                    report.summary.total,
                    report.summary.passed,
                    report.summary.failed,
                )
            })
        });
    let detail = result.report.as_ref().map(|report| report.suites.clone());
    let extracted_errors = result
        .report
        .as_ref()
        .map(|report| report.extracted_errors.clone())
        .unwrap_or_default();
    let mut errors = Vec::new();
    for error in &execution.errors {
        push_unique(&mut errors, error.message.clone());
        for detail in &error.details {
            push_unique(&mut errors, detail.clone());
        }
    }
    for diagnostic in &execution.diagnostics {
        push_unique(&mut errors, diagnostic.clone());
    }
    for extracted in extracted_errors {
        push_unique(&mut errors, extracted);
    }
    let artifacts = execution.artifacts.as_ref();
    let success = execution.status.is_ok();
    let include_steps = !result.steps.is_empty()
        && result
            .steps
            .iter()
            .any(|step| step.status != ExecutionStepStatus::Succeeded);

    McpTestResponse {
        success,
        message: test_status_message(execution.status).to_owned(),
        total_tests: summary.map(|summary| summary.0),
        passed_tests: summary.map(|summary| summary.1),
        failed_tests: summary.map(|summary| summary.2),
        execution_time_ms: Some(result.duration_ms),
        enterprise_log_path: artifacts.and_then(|artifacts| {
            artifacts
                .get_by_role(ARTIFACT_ROLE_PLATFORM_LOG)
                .map(|path| path.display().to_string())
        }),
        log_file: artifacts.and_then(|artifacts| {
            artifacts
                .get_by_role(ARTIFACT_ROLE_RUNNER_LOG)
                .map(|path| path.display().to_string())
        }),
        test_detail: detail.map(map_test_suites),
        steps: include_steps.then(|| map_step_results(result.steps)),
        errors: (!errors.is_empty()).then_some(errors),
    }
}

fn test_status_message(status: ExecutionStatus) -> &'static str {
    match status {
        ExecutionStatus::Succeeded => "Tests completed successfully",
        ExecutionStatus::Failed => "Tests failed",
        ExecutionStatus::Cancelled => "Tests cancelled",
        ExecutionStatus::TimedOut => "Tests timed out",
        ExecutionStatus::InvalidOutput => "Tests produced invalid output",
    }
}

fn push_unique(items: &mut Vec<String>, value: String) {
    if !items.contains(&value) {
        items.push(value);
    }
}

fn map_dump_response(result: DumpResult) -> McpDumpResponse {
    McpDumpResponse {
        success: result.ok,
        message: result.message.unwrap_or_else(|| {
            if result.ok {
                "Dump completed successfully".to_owned()
            } else {
                "Dump failed".to_owned()
            }
        }),
        mode: render_dump_mode_request_from_domain(result.mode).to_owned(),
        dump_time_ms: Some(result.duration_ms),
        dumped_objects: None,
        errors: None,
        steps: None,
    }
}

fn map_launch_response(result: LaunchResult) -> McpLaunchResponse {
    let default_message = match result.mode {
        LaunchMode::Designer => "Launched Designer successfully",
        LaunchMode::Thin => "Launched thin client successfully",
        LaunchMode::Thick => "Launched thick client successfully",
        LaunchMode::Ordinary => "Launched ordinary application successfully",
    };

    McpLaunchResponse {
        success: result.ok,
        message: result.message.unwrap_or_else(|| default_message.to_owned()),
    }
}

pub(crate) fn map_syntax_response(result: SyntaxCheckResult) -> McpSyntaxCheckResponse {
    let success = matches!(result.status, SyntaxCheckStatus::Clean);
    let mut errors = Vec::new();
    if let Some(stderr) = result.stderr.as_ref() {
        let trimmed = stderr.trim();
        if !trimmed.is_empty() {
            errors.push(trimmed.to_owned());
        }
    }
    if let Some(warning) = result.log_read_warning.as_ref() {
        errors.push(warning.clone());
    }

    McpSyntaxCheckResponse {
        success,
        message: match result.status {
            SyntaxCheckStatus::Clean => {
                format!("Syntax check {} completed successfully", result.check_name)
            }
            SyntaxCheckStatus::IssuesFound => {
                format!("Syntax check {} found issues", result.check_name)
            }
            SyntaxCheckStatus::ToolFailed => {
                format!("Syntax check {} failed", result.check_name)
            }
        },
        check_result: Some(render_syntax_status(result.status).to_owned()),
        errors: (!errors.is_empty()).then_some(errors),
        issues: (!result.issues.is_empty()).then(|| map_issues(result.issues)),
        duration_ms: Some(result.duration_ms),
    }
}

pub(crate) fn normalize_check_syntax_edt_request(
    request: &McpCheckSyntaxEdtRequest,
) -> SyntaxRequest {
    SyntaxRequest {
        target: SyntaxTargetRequest::Edt {
            projects: normalize_edt_projects(request.project_name.as_deref()),
        },
    }
}

pub(crate) fn map_syntax_use_case_result(
    result: UseCaseResult<SyntaxCheckResult>,
) -> McpServiceResult<McpSyntaxCheckResponse> {
    match result {
        Ok(result) => Ok(map_syntax_response(result)),
        Err(failure) => Err(map_use_case_failure(
            failure,
            map_syntax_response,
            |error| McpSyntaxCheckResponse {
                success: false,
                message: error.message().to_owned(),
                check_result: None,
                errors: Some(vec![error.message().to_owned()]),
                issues: None,
                duration_ms: None,
            },
        )),
    }
}

fn map_build_steps(steps: Vec<BuildStep>) -> Vec<McpBuildStep> {
    steps
        .into_iter()
        .map(|step| McpBuildStep {
            source_set: step.source_set,
            mode: match step.mode {
                BuildMode::EdtExport => McpBuildMode::EdtExport,
                BuildMode::Full => McpBuildMode::Full,
                BuildMode::Partial { file_count } => McpBuildMode::Partial { file_count },
                BuildMode::Skipped => McpBuildMode::Skipped,
            },
            ok: step.ok,
            message: step.message,
            duration_ms: step.duration_ms,
        })
        .collect()
}

fn map_step_results(steps: Vec<StepResult>) -> Vec<McpStepResult> {
    steps
        .into_iter()
        .map(|step| McpStepResult {
            name: step.name,
            ok: step.ok,
            status: match step.status {
                ExecutionStepStatus::Succeeded => McpStepStatus::Succeeded,
                ExecutionStepStatus::Failed => McpStepStatus::Failed,
                ExecutionStepStatus::Skipped => McpStepStatus::Skipped,
                ExecutionStepStatus::Degraded => McpStepStatus::Degraded,
            },
            kind: match step.kind {
                ExecutionStepKind::Validation => McpStepKind::Validation,
                ExecutionStepKind::ResolveTarget => McpStepKind::ResolveTarget,
                ExecutionStepKind::PrepareWorkspace => McpStepKind::PrepareWorkspace,
                ExecutionStepKind::PlatformCommand => McpStepKind::PlatformCommand,
                ExecutionStepKind::ParseOutput => McpStepKind::ParseOutput,
                ExecutionStepKind::Publish => McpStepKind::Publish,
                ExecutionStepKind::Cleanup => McpStepKind::Cleanup,
                ExecutionStepKind::Diagnostics => McpStepKind::Diagnostics,
                ExecutionStepKind::Other => McpStepKind::Other,
            },
            duration_ms: step.duration_ms,
            target: step.target,
            message: step.message,
            diagnostics: step.diagnostics,
            errors: step.errors.into_iter().map(|error| error.message).collect(),
            artifacts: step
                .artifacts
                .into_iter()
                .flat_map(|artifacts| artifacts.items.into_iter().map(|artifact| artifact.path))
                .map(|path| path.display().to_string())
                .collect(),
        })
        .collect()
}

fn map_test_suites(suites: Vec<TestSuite>) -> Vec<McpTestSuite> {
    suites
        .into_iter()
        .map(|suite| McpTestSuite {
            name: suite.name,
            cases: suite.cases.into_iter().map(map_test_case).collect(),
            duration_ms: suite.duration_ms,
        })
        .collect()
}

fn map_test_case(case: TestCase) -> McpTestCase {
    McpTestCase {
        name: case.name,
        class_name: case.class_name,
        status: match case.status {
            TestStatus::Passed => McpTestStatus::Passed,
            TestStatus::Failed => McpTestStatus::Failed,
            TestStatus::Skipped => McpTestStatus::Skipped,
            TestStatus::Error => McpTestStatus::Error,
        },
        duration_ms: case.duration_ms,
        failure_message: case.failure_message,
        stack_trace: case.stack_trace,
    }
}

fn map_issues(issues: Vec<Issue>) -> Vec<McpIssue> {
    issues.into_iter().map(map_issue).collect()
}

fn map_issue(issue: Issue) -> McpIssue {
    match issue {
        Issue::Module(issue) => McpIssue::Module(map_module_issue(issue)),
        Issue::Object(issue) => McpIssue::Object(map_object_issue(issue)),
        Issue::Edt(issue) => McpIssue::Edt(map_edt_issue(issue)),
    }
}

fn map_module_issue(issue: ModuleIssue) -> McpModuleIssue {
    McpModuleIssue {
        path: issue.path,
        line: issue.line,
        column: issue.column,
        message: issue.message,
        severity: map_issue_severity(issue.severity),
    }
}

fn map_object_issue(issue: ObjectIssue) -> McpObjectIssue {
    McpObjectIssue {
        object: issue.object,
        message: issue.message,
        severity: map_issue_severity(issue.severity),
    }
}

fn map_edt_issue(issue: EdtIssue) -> McpEdtIssue {
    McpEdtIssue {
        path: issue.path,
        line: issue.line,
        column: issue.column,
        message: issue.message,
        severity: map_issue_severity(issue.severity),
        check: issue.check,
    }
}

fn map_issue_severity(severity: IssueSeverity) -> McpIssueSeverity {
    match severity {
        IssueSeverity::Error => McpIssueSeverity::Error,
        IssueSeverity::Warning => McpIssueSeverity::Warning,
        IssueSeverity::Info => McpIssueSeverity::Info,
    }
}

fn render_dump_mode(mode: DumpModeRequest) -> &'static str {
    match mode {
        DumpModeRequest::Full => "FULL",
        DumpModeRequest::Incremental => "INCREMENTAL",
        DumpModeRequest::Partial => "PARTIAL",
    }
}

fn render_dump_mode_request_from_domain(mode: DumpMode) -> &'static str {
    match mode {
        DumpMode::Full => "FULL",
        DumpMode::Incremental => "INCREMENTAL",
        DumpMode::Partial => "PARTIAL",
    }
}

fn render_syntax_status(status: SyntaxCheckStatus) -> &'static str {
    match status {
        SyntaxCheckStatus::Clean => "clean",
        SyntaxCheckStatus::IssuesFound => "issues_found",
        SyntaxCheckStatus::ToolFailed => "tool_failed",
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::{map_test_response, McpService};
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, PlatformToolConfig, SourceFormat, SourceSetConfig,
        SourceSetPurpose, TestsConfig, ToolsConfig,
    };
    use crate::domain::build::{BuildMode, BuildResult, BuildStep};
    use crate::domain::dump::{DumpMode, DumpResult};
    use crate::domain::execution::{ExecutionStepKind, StepResult};
    use crate::domain::issue::{Issue, IssueSeverity, ModuleIssue};
    use crate::domain::launch::{LaunchMode, LaunchResult};
    use crate::domain::syntax::{SyntaxCheckResult, SyntaxCheckStatus, SyntaxIssueSummary};
    use crate::domain::test::{
        RetainedPaths, TestCase, TestOutputMode, TestReport, TestRunResult, TestStatus, TestSuite,
        TestSummary, TestTarget,
    };
    use crate::mcp::context::McpCallContext;
    use crate::mcp::error::{McpErrorCode, McpServiceError};
    use crate::mcp::port::McpUseCasePort;
    use crate::mcp::request::{
        McpBuildProjectRequest, McpCheckSyntaxDesignerConfigRequest,
        McpCheckSyntaxDesignerModulesRequest, McpCheckSyntaxEdtRequest, McpDumpConfigRequest,
        McpLaunchAppRequest, McpRunAllTestsRequest, McpRunModuleTestsRequest,
    };
    use crate::mcp::response::{
        McpBuildMode, McpBuildResponse, McpBuildStep, McpIssue, McpIssueSeverity, McpObjectIssue,
        McpStepKind, McpStepResult, McpStepStatus, McpSyntaxCheckResponse, McpTestResponse,
    };
    use crate::support::adapter_input::normalize_extension_scope;
    use crate::use_cases::context::{CommandName, ExecutionContext, ExecutionTransport};
    use crate::use_cases::request::{
        BuildRequest, DesignerClientScope, DumpModeRequest, DumpRequest, LaunchRequest,
        LaunchTargetRequest, SyntaxExtensionScope, SyntaxRequest, SyntaxTargetRequest, TestRequest,
        TestScopeRequest,
    };
    use crate::use_cases::result::{UseCaseError, UseCaseErrorKind, UseCaseFailure, UseCaseResult};

    #[derive(Default)]
    struct StubPort {
        build_result: RefCell<Option<UseCaseResult<BuildResult>>>,
        test_result: RefCell<Option<UseCaseResult<TestRunResult>>>,
        dump_result: RefCell<Option<UseCaseResult<DumpResult>>>,
        launch_result: RefCell<Option<UseCaseResult<LaunchResult>>>,
        syntax_result: RefCell<Option<UseCaseResult<SyntaxCheckResult>>>,
        build_requests: RefCell<Vec<(ExecutionContext, BuildRequest)>>,
        test_requests: RefCell<Vec<(ExecutionContext, TestRequest)>>,
        dump_requests: RefCell<Vec<(ExecutionContext, DumpRequest)>>,
        launch_requests: RefCell<Vec<(ExecutionContext, LaunchRequest)>>,
        syntax_requests: RefCell<Vec<(ExecutionContext, SyntaxRequest)>>,
    }

    impl StubPort {
        fn with_build_result(result: UseCaseResult<BuildResult>) -> Self {
            Self {
                build_result: RefCell::new(Some(result)),
                ..Self::default()
            }
        }

        fn with_test_result(result: UseCaseResult<TestRunResult>) -> Self {
            Self {
                test_result: RefCell::new(Some(result)),
                ..Self::default()
            }
        }

        fn with_dump_result(result: UseCaseResult<DumpResult>) -> Self {
            Self {
                dump_result: RefCell::new(Some(result)),
                ..Self::default()
            }
        }

        fn with_launch_result(result: UseCaseResult<LaunchResult>) -> Self {
            Self {
                launch_result: RefCell::new(Some(result)),
                ..Self::default()
            }
        }

        fn with_syntax_result(result: UseCaseResult<SyntaxCheckResult>) -> Self {
            Self {
                syntax_result: RefCell::new(Some(result)),
                ..Self::default()
            }
        }
    }

    impl McpUseCasePort for StubPort {
        fn build_project(
            &self,
            context: &ExecutionContext,
            _config: &AppConfig,
            request: &BuildRequest,
        ) -> UseCaseResult<BuildResult> {
            self.build_requests
                .borrow_mut()
                .push((context.clone(), request.clone()));
            self.build_result
                .borrow_mut()
                .take()
                .expect("missing build result")
        }

        fn run_tests(
            &self,
            context: &ExecutionContext,
            _config: &AppConfig,
            request: &TestRequest,
        ) -> UseCaseResult<TestRunResult> {
            self.test_requests
                .borrow_mut()
                .push((context.clone(), request.clone()));
            self.test_result
                .borrow_mut()
                .take()
                .expect("missing test result")
        }

        fn dump_config(
            &self,
            context: &ExecutionContext,
            _config: &AppConfig,
            request: &DumpRequest,
        ) -> UseCaseResult<DumpResult> {
            self.dump_requests
                .borrow_mut()
                .push((context.clone(), request.clone()));
            self.dump_result
                .borrow_mut()
                .take()
                .expect("missing dump result")
        }

        fn launch_app(
            &self,
            context: &ExecutionContext,
            _config: &AppConfig,
            request: &LaunchRequest,
        ) -> UseCaseResult<LaunchResult> {
            self.launch_requests
                .borrow_mut()
                .push((context.clone(), request.clone()));
            self.launch_result
                .borrow_mut()
                .take()
                .expect("missing launch result")
        }

        fn check_syntax(
            &self,
            context: &ExecutionContext,
            _config: &AppConfig,
            request: &SyntaxRequest,
        ) -> UseCaseResult<SyntaxCheckResult> {
            self.syntax_requests
                .borrow_mut()
                .push((context.clone(), request.clone()));
            self.syntax_result
                .borrow_mut()
                .take()
                .expect("missing syntax result")
        }
    }

    #[test]
    fn build_project_maps_success_request_and_response() {
        let port = StubPort::with_build_result(Ok(BuildResult {
            ok: true,
            steps: vec![BuildStep {
                source_set: "main".to_owned(),
                mode: BuildMode::Full,
                ok: true,
                message: Some("loaded".to_owned()),
                duration_ms: 17,
            }],
            duration_ms: 42,
        }));
        let config = sample_config();
        let service = McpService::with_port(&config, port);

        let response = service
            .build_project(
                McpCallContext::stdio(),
                &McpBuildProjectRequest {
                    full_rebuild: Some(true),
                },
            )
            .expect("success");

        assert_eq!(
            response,
            McpBuildResponse {
                success: true,
                message: "Build completed successfully".to_owned(),
                build_time_ms: Some(42),
                steps: None,
            }
        );
        let requests = service.port.build_requests.borrow();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0.command(), CommandName::Build);
        assert_eq!(requests[0].0.transport(), ExecutionTransport::McpStdio);
        assert_eq!(requests[0].1.full_rebuild, true);
    }

    #[test]
    fn build_project_maps_failure_payload_into_business_failure() {
        let port = StubPort::with_build_result(Err(UseCaseFailure::with_payload(
            UseCaseError::new(UseCaseErrorKind::Runtime, "builder failed"),
            BuildResult {
                ok: false,
                steps: vec![BuildStep {
                    source_set: "main".to_owned(),
                    mode: BuildMode::Full,
                    ok: false,
                    message: Some("broken".to_owned()),
                    duration_ms: 9,
                }],
                duration_ms: 19,
            },
        )));
        let config = sample_config();
        let service = McpService::with_port(&config, port);

        let error = service
            .build_project(McpCallContext::http(), &McpBuildProjectRequest::default())
            .expect_err("expected failure");

        match error {
            McpServiceError::Business(failure) => {
                assert_eq!(failure.error.code, McpErrorCode::RuntimeFailure);
                assert_eq!(failure.response.success, false);
                assert_eq!(failure.response.build_time_ms, Some(19));
                assert_eq!(failure.response.steps.expect("steps").len(), 1);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn run_all_tests_maps_success_response() {
        let mut result = sample_test_result(true);
        if let Some(report) = result.report.as_mut() {
            report.summary.errors = 2;
        }
        let port = StubPort::with_test_result(Ok(result));
        let config = sample_config();
        let service = McpService::with_port(&config, port);

        let response = service
            .run_all_tests(
                McpCallContext::stdio(),
                &McpRunAllTestsRequest { full: Some(true) },
            )
            .expect("success");

        assert_eq!(response.success, true);
        assert_eq!(response.total_tests, Some(3));
        assert_eq!(response.passed_tests, Some(2));
        assert_eq!(response.failed_tests, Some(1));
        assert_eq!(response.execution_time_ms, Some(120));
        assert!(response.errors.is_none());
        let requests = service.port.test_requests.borrow();
        assert_eq!(requests[0].1.full, true);
        assert_eq!(requests[0].1.scope, TestScopeRequest::All);
    }

    #[test]
    fn map_test_response_prefers_execution_over_legacy_projection() {
        let mut result = sample_test_result(false);
        result.ok = true;
        result.diagnostics.clear();
        result.retained_paths = None;
        if let Some(report) = result.report.as_mut() {
            report.summary.total = 99;
            report.summary.passed = 99;
            report.summary.failed = 0;
        }

        let response = map_test_response(result);

        assert!(!response.success);
        assert_eq!(response.message, "Tests failed");
        assert_eq!(response.total_tests, Some(3));
        assert_eq!(response.passed_tests, Some(2));
        assert_eq!(response.failed_tests, Some(1));
        assert_eq!(
            response.enterprise_log_path.as_deref(),
            Some("/tmp/platform.log")
        );
        assert_eq!(response.log_file.as_deref(), Some("/tmp/yaxunit.log"));
        assert!(response
            .errors
            .expect("errors")
            .contains(&"Tests failed".to_owned()));
    }

    #[test]
    fn run_all_tests_maps_failure_payload_into_business_failure() {
        let mut result = sample_test_result(false);
        result
            .execution
            .diagnostics
            .push("enterprise exited non-zero".to_owned());
        let port = StubPort::with_test_result(Err(UseCaseFailure::with_payload(
            UseCaseError::new(UseCaseErrorKind::Runtime, "tests failed"),
            result,
        )));
        let config = sample_config();
        let service = McpService::with_port(&config, port);

        let error = service
            .run_all_tests(McpCallContext::stdio(), &McpRunAllTestsRequest::default())
            .expect_err("expected failure");

        match error {
            McpServiceError::Business(failure) => {
                assert_eq!(failure.error.code, McpErrorCode::RuntimeFailure);
                assert_eq!(failure.response.success, false);
                assert!(failure
                    .response
                    .errors
                    .expect("errors")
                    .contains(&"enterprise exited non-zero".to_owned()));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn run_module_tests_maps_success_module_scope() {
        let port = StubPort::with_test_result(Ok(sample_test_result(true)));
        let config = sample_config();
        let service = McpService::with_port(&config, port);

        let response = service
            .run_module_tests(
                McpCallContext::http(),
                &McpRunModuleTestsRequest {
                    module_name: "Smoke".to_owned(),
                    full: Some(false),
                },
            )
            .expect("success");

        assert_eq!(response.success, true);
        let requests = service.port.test_requests.borrow();
        assert_eq!(requests[0].0.transport(), ExecutionTransport::McpHttp);
        assert_eq!(
            requests[0].1.scope,
            TestScopeRequest::Module {
                name: "Smoke".to_owned()
            }
        );
    }

    #[test]
    fn run_module_tests_rejects_blank_module_name_as_business_failure() {
        let config = sample_config();
        let service = McpService::with_port(
            &config,
            StubPort::with_test_result(Ok(sample_test_result(true))),
        );

        let error = service
            .run_module_tests(
                McpCallContext::stdio(),
                &McpRunModuleTestsRequest {
                    module_name: "   ".to_owned(),
                    full: None,
                },
            )
            .expect_err("expected failure");

        match error {
            McpServiceError::Business(failure) => {
                assert_eq!(failure.error.code, McpErrorCode::InvalidArgument);
                assert_eq!(failure.response.message, "module_name must not be blank");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn dump_config_maps_success_and_incremental_default_mode() {
        let port = StubPort::with_dump_result(Ok(DumpResult {
            ok: true,
            source_set: Some("main".to_owned()),
            extension: None,
            mode: DumpMode::Incremental,
            target_path: PathBuf::from("/tmp/out"),
            platform_log_path: None,
            duration_ms: 33,
            message: None,
        }));
        let config = sample_config();
        let service = McpService::with_port(&config, port);

        let response = service
            .dump_config(
                McpCallContext::stdio(),
                &McpDumpConfigRequest {
                    mode: None,
                    extension: Some(" ".to_owned()),
                    objects: vec!["Catalog.Item".to_owned()],
                },
            )
            .expect("success");

        assert_eq!(response.success, true);
        assert_eq!(response.mode, "INCREMENTAL");
        let requests = service.port.dump_requests.borrow();
        assert_eq!(requests[0].1.mode, DumpModeRequest::Incremental);
        assert_eq!(requests[0].1.extension, None);
        assert_eq!(requests[0].1.objects, vec!["Catalog.Item".to_owned()]);
    }

    #[test]
    fn dump_config_failure_with_missing_mode_keeps_incremental_request_and_payload_mode() {
        let config = sample_config();
        let service = McpService::with_port(
            &config,
            StubPort::with_dump_result(Err(UseCaseFailure::with_payload(
                UseCaseError::new(UseCaseErrorKind::Runtime, "dump failed"),
                DumpResult {
                    ok: false,
                    source_set: Some("main".to_owned()),
                    extension: None,
                    mode: DumpMode::Incremental,
                    target_path: PathBuf::from("/tmp/out"),
                    platform_log_path: None,
                    duration_ms: 3,
                    message: Some("dump failed".to_owned()),
                },
            ))),
        );

        let error = service
            .dump_config(
                McpCallContext::stdio(),
                &McpDumpConfigRequest {
                    mode: None,
                    extension: None,
                    objects: vec![],
                },
            )
            .expect_err("expected failure");

        let requests = service.port.dump_requests.borrow();
        assert_eq!(requests[0].1.mode, DumpModeRequest::Incremental);
        match error {
            McpServiceError::Business(failure) => {
                assert_eq!(failure.response.mode, "INCREMENTAL");
                assert_eq!(failure.response.message, "dump failed");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn dump_config_failure_with_blank_mode_uses_incremental_fallback_response() {
        let config = sample_config();
        let service = McpService::with_port(
            &config,
            StubPort::with_dump_result(Err(UseCaseFailure::without_payload(UseCaseError::new(
                UseCaseErrorKind::Validation,
                "dump failed",
            )))),
        );

        let error = service
            .dump_config(
                McpCallContext::stdio(),
                &McpDumpConfigRequest {
                    mode: Some("   ".to_owned()),
                    extension: None,
                    objects: vec![],
                },
            )
            .expect_err("expected failure");

        let requests = service.port.dump_requests.borrow();
        assert_eq!(requests[0].1.mode, DumpModeRequest::Incremental);
        match error {
            McpServiceError::Business(failure) => {
                assert_eq!(failure.response.mode, "INCREMENTAL");
                assert_eq!(failure.response.message, "dump failed");
                assert_eq!(
                    failure.response.errors,
                    Some(vec!["dump failed".to_owned()])
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn dump_config_rejects_invalid_mode_as_business_failure() {
        let config = sample_config();
        let service = McpService::with_port(
            &config,
            StubPort::with_dump_result(Ok(DumpResult {
                ok: true,
                source_set: None,
                extension: None,
                mode: DumpMode::Incremental,
                target_path: PathBuf::from("/tmp/out"),
                platform_log_path: None,
                duration_ms: 1,
                message: None,
            })),
        );

        let error = service
            .dump_config(
                McpCallContext::stdio(),
                &McpDumpConfigRequest {
                    mode: Some("garbage".to_owned()),
                    extension: None,
                    objects: vec![],
                },
            )
            .expect_err("expected failure");

        match error {
            McpServiceError::Business(failure) => {
                assert_eq!(failure.error.code, McpErrorCode::UnsupportedValue);
                assert_eq!(failure.response.mode, "garbage");
                assert_eq!(
                    failure.response.errors,
                    Some(vec!["unsupported dump mode: garbage".to_owned()])
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn dump_config_partial_success_preserves_partial_mode_and_warning_message() {
        let port = StubPort::with_dump_result(Ok(DumpResult {
            ok: true,
            source_set: Some("main".to_owned()),
            extension: None,
            mode: DumpMode::Partial,
            target_path: PathBuf::from("/tmp/out"),
            platform_log_path: None,
            duration_ms: 14,
            message: Some(
                "IBCMD does not support object-scoped partial dump; ran incremental export for source-set 'main' instead"
                    .to_owned(),
            ),
        }));
        let config = sample_config();
        let service = McpService::with_port(&config, port);

        let response = service
            .dump_config(
                McpCallContext::stdio(),
                &McpDumpConfigRequest {
                    mode: Some("PARTIAL".to_owned()),
                    extension: None,
                    objects: vec!["  Catalog.Item  ".to_owned()],
                },
            )
            .expect("success");

        assert!(response.success);
        assert_eq!(response.mode, "PARTIAL");
        assert!(response
            .message
            .contains("IBCMD does not support object-scoped partial dump"));
        let requests = service.port.dump_requests.borrow();
        assert_eq!(requests[0].1.mode, DumpModeRequest::Partial);
        assert_eq!(requests[0].1.objects, vec!["  Catalog.Item  ".to_owned()]);
    }

    #[test]
    fn dump_config_partial_failure_payload_preserves_partial_mode_and_warning() {
        let config = sample_config();
        let service = McpService::with_port(
            &config,
            StubPort::with_dump_result(Err(UseCaseFailure::with_payload(
                UseCaseError::new(
                    UseCaseErrorKind::Platform,
                    "IBCMD does not support object-scoped partial dump; export failed",
                ),
                DumpResult {
                    ok: false,
                    source_set: Some("main".to_owned()),
                    extension: None,
                    mode: DumpMode::Partial,
                    target_path: PathBuf::from("/tmp/out"),
                    platform_log_path: None,
                    duration_ms: 3,
                    message: Some(
                        "IBCMD does not support object-scoped partial dump; export failed"
                            .to_owned(),
                    ),
                },
            ))),
        );

        let error = service
            .dump_config(
                McpCallContext::stdio(),
                &McpDumpConfigRequest {
                    mode: Some("PARTIAL".to_owned()),
                    extension: None,
                    objects: vec!["Catalog.Item".to_owned()],
                },
            )
            .expect_err("expected failure");

        match error {
            McpServiceError::Business(failure) => {
                assert_eq!(failure.response.mode, "PARTIAL");
                assert!(failure
                    .response
                    .message
                    .contains("IBCMD does not support object-scoped partial dump"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn launch_app_maps_supported_aliases() {
        let cases = [
            (
                "designer",
                LaunchMode::Designer,
                LaunchTargetRequest::designer(),
            ),
            (
                "configurator",
                LaunchMode::Designer,
                LaunchTargetRequest::designer(),
            ),
            (
                " 1CV8 ",
                LaunchMode::Designer,
                LaunchTargetRequest::designer(),
            ),
            (
                "конфигуратор",
                LaunchMode::Designer,
                LaunchTargetRequest::designer(),
            ),
            ("thin", LaunchMode::Thin, LaunchTargetRequest::thin_client()),
            (
                "thin-client",
                LaunchMode::Thin,
                LaunchTargetRequest::thin_client(),
            ),
            (
                "thin client",
                LaunchMode::Thin,
                LaunchTargetRequest::thin_client(),
            ),
            (
                "THIN_CLIENT",
                LaunchMode::Thin,
                LaunchTargetRequest::thin_client(),
            ),
            ("tc", LaunchMode::Thin, LaunchTargetRequest::thin_client()),
            (
                "1cv8c",
                LaunchMode::Thin,
                LaunchTargetRequest::thin_client(),
            ),
            (
                "  тонкий клиент  ",
                LaunchMode::Thin,
                LaunchTargetRequest::thin_client(),
            ),
            (
                "тонкий",
                LaunchMode::Thin,
                LaunchTargetRequest::thin_client(),
            ),
            (
                "thick",
                LaunchMode::Thick,
                LaunchTargetRequest::thick_client(),
            ),
            (
                "thick-client",
                LaunchMode::Thick,
                LaunchTargetRequest::thick_client(),
            ),
            (
                "thick client",
                LaunchMode::Thick,
                LaunchTargetRequest::thick_client(),
            ),
            (
                "THICK_CLIENT",
                LaunchMode::Thick,
                LaunchTargetRequest::thick_client(),
            ),
            (
                " толстый клиент ",
                LaunchMode::Thick,
                LaunchTargetRequest::thick_client(),
            ),
            (
                "толстый",
                LaunchMode::Thick,
                LaunchTargetRequest::thick_client(),
            ),
        ];

        for (alias, result_mode, request_mode) in cases {
            let port = StubPort::with_launch_result(Ok(LaunchResult {
                ok: true,
                mode: result_mode,
                pid: Some(42),
                binary: PathBuf::from("/opt/1cv8"),
                message: None,
            }));
            let config = sample_config();
            let service = McpService::with_port(&config, port);

            let response = service
                .launch_app(
                    McpCallContext::http(),
                    &McpLaunchAppRequest {
                        utility_type: alias.to_owned(),
                    },
                )
                .expect("success");

            assert_eq!(response.success, true, "alias {alias}");
            let requests = service.port.launch_requests.borrow();
            assert_eq!(requests[0].1.target, request_mode, "alias {alias}");
        }
    }

    #[test]
    fn launch_app_rejects_blank_alias_with_consistent_validation_message() {
        let config = sample_config();
        let service = McpService::with_port(
            &config,
            StubPort::with_launch_result(Ok(LaunchResult {
                ok: true,
                mode: LaunchMode::Designer,
                pid: None,
                binary: PathBuf::from("/opt/1cv8"),
                message: None,
            })),
        );

        let error = service
            .launch_app(
                McpCallContext::stdio(),
                &McpLaunchAppRequest {
                    utility_type: "   ".to_owned(),
                },
            )
            .expect_err("expected failure");

        match error {
            McpServiceError::Business(failure) => {
                assert_eq!(failure.error.code, McpErrorCode::InvalidArgument);
                assert_eq!(failure.error.message, "utility_type must not be blank");
                assert_eq!(failure.response.message, "utility_type must not be blank");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn launch_app_rejects_unknown_alias_as_business_failure() {
        let config = sample_config();
        let service = McpService::with_port(
            &config,
            StubPort::with_launch_result(Ok(LaunchResult {
                ok: true,
                mode: LaunchMode::Designer,
                pid: None,
                binary: PathBuf::from("/opt/1cv8"),
                message: None,
            })),
        );

        let error = service
            .launch_app(
                McpCallContext::stdio(),
                &McpLaunchAppRequest {
                    utility_type: "unknown".to_owned(),
                },
            )
            .expect_err("expected failure");

        match error {
            McpServiceError::Business(failure) => {
                assert_eq!(failure.error.code, McpErrorCode::UnsupportedValue);
                assert_eq!(
                    failure.response.message,
                    "unsupported launch utility_type: unknown"
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn check_syntax_edt_maps_blank_project_to_all_projects() {
        let port = StubPort::with_syntax_result(Ok(sample_syntax_result(SyntaxCheckStatus::Clean)));
        let config = sample_config();
        let service = McpService::with_port(&config, port);

        let response = service
            .check_syntax_edt(
                McpCallContext::stdio(),
                &McpCheckSyntaxEdtRequest {
                    project_name: Some(" ".to_owned()),
                },
            )
            .expect("success");

        assert_eq!(response.success, true);
        let requests = service.port.syntax_requests.borrow();
        assert_eq!(
            requests[0].1.target,
            SyntaxTargetRequest::Edt { projects: vec![] }
        );
    }

    #[test]
    fn check_syntax_edt_maps_failure_without_payload() {
        let port = StubPort::with_syntax_result(Err(UseCaseFailure::without_payload(
            UseCaseError::new(UseCaseErrorKind::Validation, "edt project not found"),
        )));
        let config = sample_config();
        let service = McpService::with_port(&config, port);

        let error = service
            .check_syntax_edt(
                McpCallContext::stdio(),
                &McpCheckSyntaxEdtRequest::default(),
            )
            .expect_err("expected failure");

        match error {
            McpServiceError::Business(failure) => {
                assert_eq!(failure.error.code, McpErrorCode::InvalidArgument);
                assert_eq!(failure.response.message, "edt project not found");
                assert_eq!(
                    failure.response.errors,
                    Some(vec!["edt project not found".to_owned()])
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn normalize_extension_scope_covers_tri_state_matrix() {
        let cases = [
            (None, None, SyntaxExtensionScope::AllExtensions),
            (None, Some(false), SyntaxExtensionScope::MainConfiguration),
            (None, Some(true), SyntaxExtensionScope::AllExtensions),
            (
                Some(" Ext "),
                None,
                SyntaxExtensionScope::SingleExtension {
                    name: "Ext".to_owned(),
                },
            ),
            (
                Some(" Ext "),
                Some(false),
                SyntaxExtensionScope::SingleExtension {
                    name: "Ext".to_owned(),
                },
            ),
            (
                Some(" Ext "),
                Some(true),
                SyntaxExtensionScope::SingleExtensionAndAll {
                    name: "Ext".to_owned(),
                },
            ),
            (Some(" "), None, SyntaxExtensionScope::AllExtensions),
        ];

        for (extension, all_extensions, expected) in cases {
            assert_eq!(
                normalize_extension_scope(extension, all_extensions),
                expected
            );
        }
    }

    #[test]
    fn check_syntax_designer_config_maps_normalized_defaults() {
        let port = StubPort::with_syntax_result(Ok(sample_syntax_result(SyntaxCheckStatus::Clean)));
        let config = sample_config();
        let service = McpService::with_port(&config, port);

        let response = service
            .check_syntax_designer_config(
                McpCallContext::http(),
                &McpCheckSyntaxDesignerConfigRequest {
                    extension: None,
                    ..McpCheckSyntaxDesignerConfigRequest::default()
                },
            )
            .expect("success");

        assert_eq!(response.success, true);
        let requests = service.port.syntax_requests.borrow();
        match &requests[0].1.target {
            SyntaxTargetRequest::DesignerConfig(request) => {
                assert!(request.has_client_scope(DesignerClientScope::ThinClient));
                assert!(request.has_client_scope(DesignerClientScope::Server));
                assert!(request.extended_modules().is_enabled());
                assert!(request.extension_scope().includes_all_extensions());
            }
            other => panic!("unexpected target: {other:?}"),
        }
    }

    #[test]
    fn check_syntax_designer_config_uses_extension_scope_helper() {
        let port = StubPort::with_syntax_result(Ok(sample_syntax_result(SyntaxCheckStatus::Clean)));
        let config = sample_config();
        let service = McpService::with_port(&config, port);

        service
            .check_syntax_designer_config(
                McpCallContext::http(),
                &McpCheckSyntaxDesignerConfigRequest {
                    extension: Some(" Ext ".to_owned()),
                    all_extensions: Some(true),
                    ..McpCheckSyntaxDesignerConfigRequest::default()
                },
            )
            .expect("success");

        let requests = service.port.syntax_requests.borrow();
        match &requests[0].1.target {
            SyntaxTargetRequest::DesignerConfig(request) => {
                assert_eq!(request.extension_scope().extension(), Some("Ext"));
                assert!(request.extension_scope().includes_all_extensions());
            }
            other => panic!("unexpected target: {other:?}"),
        }
    }

    #[test]
    fn check_syntax_designer_config_rejects_sync_dependency_without_calling_use_case() {
        let port = StubPort::with_syntax_result(Ok(sample_syntax_result(SyntaxCheckStatus::Clean)));
        let config = sample_config();
        let service = McpService::with_port(&config, port);

        let error = service
            .check_syntax_designer_config(
                McpCallContext::stdio(),
                &McpCheckSyntaxDesignerConfigRequest {
                    extended_modules_check: Some(false),
                    check_use_synchronous_calls: Some(true),
                    ..McpCheckSyntaxDesignerConfigRequest::default()
                },
            )
            .expect_err("expected failure");

        assert!(service.port.syntax_requests.borrow().is_empty());
        match error {
            McpServiceError::Business(failure) => {
                assert_eq!(failure.error.code, McpErrorCode::InvalidArgument);
                assert_eq!(
                    failure.response.message,
                    "checkUseSynchronousCalls requires extendedModulesCheck=true"
                );
                assert_eq!(
                    failure.response.errors,
                    Some(vec![
                        "checkUseSynchronousCalls requires extendedModulesCheck=true".to_owned()
                    ])
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn check_syntax_designer_config_rejects_modality_dependency_without_calling_use_case() {
        let port = StubPort::with_syntax_result(Ok(sample_syntax_result(SyntaxCheckStatus::Clean)));
        let config = sample_config();
        let service = McpService::with_port(&config, port);

        let error = service
            .check_syntax_designer_config(
                McpCallContext::stdio(),
                &McpCheckSyntaxDesignerConfigRequest {
                    extended_modules_check: Some(false),
                    check_use_modality: Some(true),
                    ..McpCheckSyntaxDesignerConfigRequest::default()
                },
            )
            .expect_err("expected failure");

        assert!(service.port.syntax_requests.borrow().is_empty());
        match error {
            McpServiceError::Business(failure) => {
                assert_eq!(failure.error.code, McpErrorCode::InvalidArgument);
                assert_eq!(
                    failure.response.message,
                    "checkUseModality requires extendedModulesCheck=true"
                );
                assert_eq!(
                    failure.response.errors,
                    Some(vec![
                        "checkUseModality requires extendedModulesCheck=true".to_owned()
                    ])
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn check_syntax_designer_config_maps_failure_payload_into_business_failure() {
        let port = StubPort::with_syntax_result(Err(UseCaseFailure::with_payload(
            UseCaseError::new(UseCaseErrorKind::Platform, "designer failed"),
            sample_syntax_result(SyntaxCheckStatus::ToolFailed),
        )));
        let config = sample_config();
        let service = McpService::with_port(&config, port);

        let error = service
            .check_syntax_designer_config(
                McpCallContext::stdio(),
                &McpCheckSyntaxDesignerConfigRequest::default(),
            )
            .expect_err("expected failure");

        match error {
            McpServiceError::Business(failure) => {
                assert_eq!(failure.error.code, McpErrorCode::PlatformFailure);
                assert_eq!(
                    failure.response.check_result.as_deref(),
                    Some("tool_failed")
                );
                assert_eq!(
                    failure.response.errors,
                    Some(vec![
                        "designer stderr".to_owned(),
                        "log truncated".to_owned()
                    ])
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn check_syntax_designer_modules_maps_success_request() {
        let port =
            StubPort::with_syntax_result(Ok(sample_syntax_result(SyntaxCheckStatus::IssuesFound)));
        let config = sample_config();
        let service = McpService::with_port(&config, port);

        let response = service
            .check_syntax_designer_modules(
                McpCallContext::http(),
                &McpCheckSyntaxDesignerModulesRequest {
                    extension: Some("Ext".to_owned()),
                    all_extensions: Some(false),
                    ..McpCheckSyntaxDesignerModulesRequest::default()
                },
            )
            .expect("success");

        assert_eq!(response.success, false);
        let requests = service.port.syntax_requests.borrow();
        match &requests[0].1.target {
            SyntaxTargetRequest::DesignerModules(request) => {
                assert!(request.has_client_scope(DesignerClientScope::ThinClient));
                assert!(request.has_client_scope(DesignerClientScope::Server));
                assert_eq!(request.extension_scope().extension(), Some("Ext"));
                assert!(!request.extension_scope().includes_all_extensions());
            }
            other => panic!("unexpected target: {other:?}"),
        }
    }

    #[test]
    fn check_syntax_designer_modules_uses_extension_scope_helper() {
        let port = StubPort::with_syntax_result(Ok(sample_syntax_result(SyntaxCheckStatus::Clean)));
        let config = sample_config();
        let service = McpService::with_port(&config, port);

        service
            .check_syntax_designer_modules(
                McpCallContext::http(),
                &McpCheckSyntaxDesignerModulesRequest {
                    extension: Some(" ".to_owned()),
                    all_extensions: None,
                    ..McpCheckSyntaxDesignerModulesRequest::default()
                },
            )
            .expect("success");

        let requests = service.port.syntax_requests.borrow();
        match &requests[0].1.target {
            SyntaxTargetRequest::DesignerModules(request) => {
                assert_eq!(request.extension_scope().extension(), None);
                assert!(request.extension_scope().includes_all_extensions());
            }
            other => panic!("unexpected target: {other:?}"),
        }
    }

    #[test]
    fn check_syntax_designer_modules_maps_failure_without_payload() {
        let port = StubPort::with_syntax_result(Err(UseCaseFailure::without_payload(
            UseCaseError::new(UseCaseErrorKind::Validation, "no syntax modes selected"),
        )));
        let config = sample_config();
        let service = McpService::with_port(&config, port);

        let error = service
            .check_syntax_designer_modules(
                McpCallContext::stdio(),
                &McpCheckSyntaxDesignerModulesRequest::default(),
            )
            .expect_err("expected failure");

        match error {
            McpServiceError::Business(failure) => {
                assert_eq!(failure.error.code, McpErrorCode::InvalidArgument);
                assert_eq!(failure.response.message, "no syntax modes selected");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn rejects_non_mcp_transport_as_internal_error() {
        let config = sample_config();
        let service = McpService::with_port(
            &config,
            StubPort::with_build_result(Ok(BuildResult {
                ok: true,
                steps: vec![],
                duration_ms: 0,
            })),
        );

        let error = service
            .build_project(
                McpCallContext::new(ExecutionTransport::Cli),
                &McpBuildProjectRequest::default(),
            )
            .expect_err("expected failure");

        match error {
            McpServiceError::Internal(error) => {
                assert_eq!(error.code, McpErrorCode::Internal);
                assert!(error.message.contains("non-MCP transport"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn internal_error_is_not_confused_with_business_failure() {
        let config = sample_config();
        let service = McpService::with_port(
            &config,
            StubPort::with_build_result(Ok(BuildResult {
                ok: true,
                steps: vec![],
                duration_ms: 0,
            })),
        );

        let error = service
            .build_project(
                McpCallContext::new(ExecutionTransport::Cli),
                &McpBuildProjectRequest::default(),
            )
            .expect_err("expected failure");

        assert!(matches!(error, McpServiceError::Internal(_)));
        assert!(!matches!(error, McpServiceError::Business(_)));
    }

    #[test]
    fn serializes_test_response_shape() {
        let response = McpTestResponse {
            success: false,
            message: "Tests failed".to_owned(),
            total_tests: Some(3),
            passed_tests: Some(2),
            failed_tests: Some(1),
            execution_time_ms: Some(120),
            enterprise_log_path: Some("/tmp/platform.log".to_owned()),
            log_file: Some("/tmp/yaxunit.log".to_owned()),
            test_detail: None,
            steps: Some(vec![McpStepResult {
                name: "build".to_owned(),
                ok: false,
                status: McpStepStatus::Failed,
                kind: McpStepKind::PlatformCommand,
                duration_ms: 10,
                target: None,
                message: Some("boom".to_owned()),
                diagnostics: vec![],
                errors: vec!["boom".to_owned()],
                artifacts: vec![],
            }]),
            errors: Some(vec!["boom".to_owned()]),
        };

        let json = serde_json::to_value(response).expect("json");

        assert_eq!(
            json,
            json!({
                "success": false,
                "message": "Tests failed",
                "total_tests": 3,
                "passed_tests": 2,
                "failed_tests": 1,
                "execution_time_ms": 120,
                "enterprise_log_path": "/tmp/platform.log",
                "log_file": "/tmp/yaxunit.log",
                "steps": [{
                    "name": "build",
                    "ok": false,
                    "status": "failed",
                    "kind": "platform_command",
                    "duration_ms": 10,
                    "message": "boom",
                    "errors": ["boom"]
                }],
                "errors": ["boom"]
            })
        );
    }

    #[test]
    fn serializes_syntax_response_shape() {
        let response = McpSyntaxCheckResponse {
            success: false,
            message: "Syntax check CheckConfig failed".to_owned(),
            check_result: Some("tool_failed".to_owned()),
            errors: Some(vec!["stderr".to_owned()]),
            issues: Some(vec![McpIssue::Object(McpObjectIssue {
                object: "Catalog.Item".to_owned(),
                message: "broken".to_owned(),
                severity: McpIssueSeverity::Error,
            })]),
            duration_ms: Some(55),
        };

        let json = serde_json::to_value(response).expect("json");

        assert_eq!(
            json,
            json!({
                "success": false,
                "message": "Syntax check CheckConfig failed",
                "check_result": "tool_failed",
                "errors": ["stderr"],
                "issues": [{
                    "kind": "object",
                    "object": "Catalog.Item",
                    "message": "broken",
                    "severity": "ERROR"
                }],
                "duration_ms": 55
            })
        );
    }

    #[test]
    fn serializes_build_failure_response_shape() {
        let response = McpBuildResponse {
            success: false,
            message: "Build failed".to_owned(),
            build_time_ms: Some(19),
            steps: Some(vec![McpBuildStep {
                source_set: "main".to_owned(),
                mode: McpBuildMode::Full,
                ok: false,
                message: Some("broken".to_owned()),
                duration_ms: 9,
            }]),
        };

        let json = serde_json::to_value(response).expect("json");

        assert_eq!(
            json,
            json!({
                "success": false,
                "message": "Build failed",
                "build_time_ms": 19,
                "steps": [{
                    "source_set": "main",
                    "mode": "full",
                    "ok": false,
                    "message": "broken",
                    "duration_ms": 9
                }]
            })
        );
    }

    fn sample_config() -> AppConfig {
        AppConfig {
            base_path: PathBuf::from("/tmp/project"),
            work_path: PathBuf::from("/tmp/work"),
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
            source_sets: vec![SourceSetConfig {
                name: "main".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: Path::new("src").to_path_buf(),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig {
                platform: PlatformToolConfig::default(),
                enterprise: Default::default(),
                edt_cli: Default::default(),
            },
            mcp: Default::default(),
            tests: TestsConfig::default(),
        }
    }

    fn sample_test_result(ok: bool) -> TestRunResult {
        let retained = RetainedPaths {
            run_dir: PathBuf::from("/tmp/run"),
            config_json: PathBuf::from("/tmp/config.json"),
            junit_xml: PathBuf::from("/tmp/junit.xml"),
            yaxunit_log: PathBuf::from("/tmp/yaxunit.log"),
            platform_log: PathBuf::from("/tmp/platform.log"),
            sentinel: PathBuf::from("/tmp/sentinel"),
        };
        let report = TestReport {
            summary: TestSummary {
                total: 3,
                passed: 2,
                failed: 1,
                skipped: 0,
                errors: 0,
            },
            suites: vec![TestSuite {
                name: "Smoke".to_owned(),
                cases: vec![
                    TestCase {
                        name: "ok".to_owned(),
                        class_name: None,
                        status: TestStatus::Passed,
                        duration_ms: 10,
                        failure_message: None,
                        stack_trace: None,
                    },
                    TestCase {
                        name: "fail".to_owned(),
                        class_name: Some("Smoke".to_owned()),
                        status: TestStatus::Failed,
                        duration_ms: 11,
                        failure_message: Some("assert".to_owned()),
                        stack_trace: Some("trace".to_owned()),
                    },
                ],
                duration_ms: 21,
            }],
            extracted_errors: (!ok)
                .then_some(vec!["assert".to_owned()])
                .unwrap_or_default(),
        };
        let mut outcome = crate::domain::execution::ExecutionOutcome::new(if ok {
            crate::domain::execution::ExecutionStatus::Succeeded
        } else {
            crate::domain::execution::ExecutionStatus::Failed
        })
        .with_metrics(crate::domain::execution::ExecutionMetrics::from(
            &report.summary,
        ))
        .with_payload(report);
        if !ok {
            outcome = outcome.with_errors(vec![crate::domain::test::test_execution_error(
                crate::domain::test::TestErrorKind::TestFailures,
                "Tests failed",
            )]);
        }
        outcome = outcome.with_artifacts(retained.into_artifact_set());

        TestRunResult::from_outcome(
            outcome,
            TestTarget::All,
            TestOutputMode::Full,
            vec![],
            if ok {
                vec![]
            } else {
                vec![
                    StepResult::failed("test", ExecutionStepKind::PlatformCommand, 120)
                        .with_message("boom")
                        .with_errors(vec![crate::domain::test::test_execution_error(
                            crate::domain::test::TestErrorKind::TestFailures,
                            "boom",
                        )]),
                ]
            },
            120,
        )
    }

    fn sample_syntax_result(status: SyntaxCheckStatus) -> SyntaxCheckResult {
        SyntaxCheckResult {
            status,
            exit_code: if matches!(status, SyntaxCheckStatus::Clean) {
                0
            } else {
                1
            },
            check_name: "CheckConfig".to_owned(),
            issues: if matches!(status, SyntaxCheckStatus::IssuesFound) {
                vec![Issue::Module(ModuleIssue {
                    path: "src/CommonModule.bsl".to_owned(),
                    line: Some(10),
                    column: Some(4),
                    message: "broken".to_owned(),
                    severity: IssueSeverity::Error,
                })]
            } else {
                vec![]
            },
            summary: SyntaxIssueSummary {
                errors: usize::from(matches!(status, SyntaxCheckStatus::IssuesFound)),
                warnings: 0,
                info: 0,
            },
            duration_ms: 55,
            platform_log_path: None,
            stderr: matches!(status, SyntaxCheckStatus::ToolFailed)
                .then_some("designer stderr".to_owned()),
            log_read_warning: matches!(status, SyntaxCheckStatus::ToolFailed)
                .then_some("log truncated".to_owned()),
        }
    }
}
