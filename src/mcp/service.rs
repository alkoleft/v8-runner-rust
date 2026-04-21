use crate::config::model::AppConfig;
use crate::domain::build::{BuildMode, BuildResult, BuildStep};
use crate::domain::dump::{DumpMode, DumpResult};
use crate::domain::execution::StepResult;
use crate::domain::issue::{EdtIssue, Issue, IssueSeverity, ModuleIssue, ObjectIssue};
use crate::domain::launch::{LaunchMode, LaunchResult};
use crate::domain::runner::LaunchOptions;
use crate::domain::syntax::{SyntaxCheckResult, SyntaxCheckStatus};
use crate::domain::test::{TestCase, TestRunResult, TestStatus, TestSuite};
use crate::mcp::context::McpCallContext;
use crate::mcp::error::{
    McpBusinessError, McpBusinessFailure, McpInternalError, McpServiceError, McpServiceResult,
};
use crate::mcp::port::{DefaultMcpUseCasePort, McpUseCasePort};
use crate::mcp::request::{
    McpBuildProjectRequest, McpCheckSyntaxDesignerConfigRequest,
    McpCheckSyntaxDesignerModulesRequest, McpCheckSyntaxEdtRequest, McpDumpConfigRequest,
    McpLaunchAppRequest, McpRunAllTestsRequest, McpRunModuleTestsRequest,
};
use crate::mcp::response::{
    McpBuildMode, McpBuildResponse, McpBuildStep, McpDumpResponse, McpEdtIssue, McpIssue,
    McpIssueSeverity, McpLaunchResponse, McpModuleIssue, McpObjectIssue, McpStepResult,
    McpSyntaxCheckResponse, McpTestCase, McpTestResponse, McpTestStatus, McpTestSuite,
};
use crate::use_cases::context::{CommandName, ExecutionContext, ExecutionTransport};
use crate::use_cases::request::{
    BuildRequest, DesignerConfigSyntaxRequest, DesignerModulesSyntaxRequest, DumpModeRequest,
    DumpRequest, LaunchModeRequest, LaunchRequest, SyntaxRequest, SyntaxTargetRequest, TestRequest,
    TestScopeRequest,
};
use crate::use_cases::result::{UseCaseError, UseCaseFailure, UseCaseResult};

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
                McpServiceError::Business(McpBusinessFailure::new(
                    error,
                    McpTestResponse {
                        success: false,
                        message: "module_name must not be blank".to_owned(),
                        total_tests: None,
                        passed_tests: None,
                        failed_tests: None,
                        execution_time_ms: None,
                        enterprise_log_path: None,
                        log_file: None,
                        test_detail: None,
                        steps: None,
                        errors: Some(vec!["module_name must not be blank".to_owned()]),
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
            mode: dump_mode_from_raw(request.mode.as_deref()).map_err(|error| {
                let message = error.message.clone();
                McpServiceError::Business(McpBusinessFailure::new(
                    error,
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
            mode: launch_mode_from_raw(&request.utility_type).map_err(|error| {
                let message = error.message.clone();
                McpServiceError::Business(McpBusinessFailure::new(
                    error,
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
    #[cfg(test)]
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
        validate_extended_modules_dependencies(
            request.extended_modules_check,
            request.check_use_synchronous_calls,
            request.check_use_modality,
        )
        .map_err(invalid_syntax_request)?;
        let use_case_request = SyntaxRequest {
            target: SyntaxTargetRequest::DesignerConfig(map_designer_config_request(request)),
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
            target: SyntaxTargetRequest::DesignerModules(map_designer_modules_request(request)),
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
                .with_edt_timeout(call_context.edt_timeout()))
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
    let error = McpBusinessError::from_use_case(&failure.error);
    let response = failure
        .payload
        .map(payload_mapper)
        .unwrap_or_else(|| fallback_response(&failure.error));

    McpServiceError::Business(McpBusinessFailure::new(error, response))
}

fn normalize_required_string(
    value: &str,
    field_name: &'static str,
) -> Result<String, McpBusinessError> {
    normalize_optional_string(Some(value)).ok_or_else(|| {
        McpBusinessError::invalid_argument(format!("{field_name} must not be blank"))
    })
}

fn normalize_optional_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedExtensionScope {
    extension: Option<String>,
    all_extensions: bool,
}

fn dump_mode_from_raw(raw: Option<&str>) -> Result<DumpModeRequest, McpBusinessError> {
    match normalize_optional_string(raw) {
        None => Ok(DumpModeRequest::Incremental),
        Some(mode) => match mode.to_ascii_uppercase().as_str() {
            "FULL" => Ok(DumpModeRequest::Full),
            "INCREMENTAL" => Ok(DumpModeRequest::Incremental),
            "PARTIAL" => Ok(DumpModeRequest::Partial),
            _ => Err(McpBusinessError::unsupported_value(format!(
                "unsupported dump mode: {mode}"
            ))),
        },
    }
}

fn normalize_extension_scope(
    extension: Option<&str>,
    all_extensions: Option<bool>,
) -> NormalizedExtensionScope {
    let extension = normalize_optional_string(extension);
    let all_extensions = all_extensions.unwrap_or(extension.is_none());

    NormalizedExtensionScope {
        extension,
        all_extensions,
    }
}

fn launch_mode_from_raw(raw: &str) -> Result<LaunchModeRequest, McpBusinessError> {
    let normalized = normalize_required_string(raw, "utility_type")?.to_lowercase();
    match normalized.as_str() {
        "designer" | "configurator" | "1cv8" | "конфигуратор" => {
            Ok(LaunchModeRequest::Designer)
        }
        "thin"
        | "thin-client"
        | "thin client"
        | "thin_client"
        | "tc"
        | "1cv8c"
        | "тонкий клиент"
        | "тонкий" => Ok(LaunchModeRequest::Thin),
        "thick"
        | "thick-client"
        | "thick client"
        | "thick_client"
        | "толстый клиент"
        | "толстый" => Ok(LaunchModeRequest::Thick),
        _ => Err(McpBusinessError::unsupported_value(format!(
            "unsupported launch utility_type: {raw}"
        ))),
    }
}

fn validate_extended_modules_dependencies(
    extended_modules_check: Option<bool>,
    check_use_synchronous_calls: Option<bool>,
    check_use_modality: Option<bool>,
) -> Result<(), McpBusinessError> {
    if extended_modules_check == Some(false) && check_use_synchronous_calls == Some(true) {
        return Err(McpBusinessError::invalid_argument(
            "checkUseSynchronousCalls requires extendedModulesCheck=true".to_owned(),
        ));
    }

    if extended_modules_check == Some(false) && check_use_modality == Some(true) {
        return Err(McpBusinessError::invalid_argument(
            "checkUseModality requires extendedModulesCheck=true".to_owned(),
        ));
    }

    Ok(())
}

fn invalid_syntax_request(error: McpBusinessError) -> McpServiceError<McpSyntaxCheckResponse> {
    let message = error.message.clone();
    McpServiceError::Business(McpBusinessFailure::new(
        error,
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

fn map_designer_config_request(
    request: &McpCheckSyntaxDesignerConfigRequest,
) -> DesignerConfigSyntaxRequest {
    let scope = normalize_extension_scope(request.extension.as_deref(), request.all_extensions);
    DesignerConfigSyntaxRequest {
        config_log_integrity: request.config_log_integrity == Some(true),
        incorrect_references: request.incorrect_references == Some(true),
        thin_client: request.thin_client != Some(false),
        web_client: request.web_client == Some(true),
        mobile_client: request.mobile_client == Some(true),
        server: request.server != Some(false),
        external_connection: request.external_connection == Some(true),
        external_connection_server: request.external_connection_server == Some(true),
        mobile_app_client: request.mobile_app_client == Some(true),
        mobile_app_server: request.mobile_app_server == Some(true),
        thick_client_managed_application: request.thick_client_managed_application == Some(true),
        thick_client_server_managed_application: request.thick_client_server_managed_application
            == Some(true),
        thick_client_ordinary_application: request.thick_client_ordinary_application == Some(true),
        thick_client_server_ordinary_application: request.thick_client_server_ordinary_application
            == Some(true),
        mobile_client_digi_sign: request.mobile_client_digi_sign == Some(true),
        distributive_modules: request.distributive_modules == Some(true),
        unreference_procedures: request.unreference_procedures != Some(false),
        handlers_existence: request.handlers_existence != Some(false),
        empty_handlers: request.empty_handlers != Some(false),
        extended_modules_check: request.extended_modules_check != Some(false),
        check_use_synchronous_calls: request.check_use_synchronous_calls == Some(true),
        check_use_modality: request.check_use_modality == Some(true),
        unsupported_functional: request.unsupported_functional == Some(true),
        extension: scope.extension,
        all_extensions: scope.all_extensions,
    }
}

fn map_designer_modules_request(
    request: &McpCheckSyntaxDesignerModulesRequest,
) -> DesignerModulesSyntaxRequest {
    let scope = normalize_extension_scope(request.extension.as_deref(), request.all_extensions);
    DesignerModulesSyntaxRequest {
        thin_client: request.thin_client != Some(false),
        web_client: request.web_client == Some(true),
        server: request.server != Some(false),
        external_connection: request.external_connection == Some(true),
        thick_client_ordinary_application: request.thick_client_ordinary_application == Some(true),
        mobile_app_client: request.mobile_app_client == Some(true),
        mobile_app_server: request.mobile_app_server == Some(true),
        mobile_client: request.mobile_client == Some(true),
        extended_modules_check: request.extended_modules_check != Some(false),
        extension: scope.extension,
        all_extensions: scope.all_extensions,
    }
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
    let summary = result.report.as_ref().map(|report| &report.summary);
    let detail = result.report.as_ref().map(|report| report.suites.clone());
    let extracted_errors = result
        .report
        .as_ref()
        .map(|report| report.extracted_errors.clone())
        .unwrap_or_default();
    let mut errors = result.diagnostics.clone();
    errors.extend(extracted_errors);
    let retained_paths = result.retained_paths.as_ref();
    let success = result.ok;

    McpTestResponse {
        success,
        message: if success {
            "Tests completed successfully".to_owned()
        } else {
            "Tests failed".to_owned()
        },
        total_tests: summary.map(|summary| summary.total),
        passed_tests: summary.map(|summary| summary.passed),
        failed_tests: summary.map(|summary| summary.failed),
        execution_time_ms: Some(result.duration_ms),
        enterprise_log_path: retained_paths.map(|paths| paths.platform_log.display().to_string()),
        log_file: retained_paths.map(|paths| paths.yaxunit_log.display().to_string()),
        test_detail: detail.map(map_test_suites),
        steps: (!success && !result.steps.is_empty()).then(|| map_step_results(result.steps)),
        errors: (!errors.is_empty()).then_some(errors),
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
    let projects = normalize_optional_string(request.project_name.as_deref())
        .map_or_else(Vec::new, |project| vec![project]);
    SyntaxRequest {
        target: SyntaxTargetRequest::Edt { projects },
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
            duration_ms: step.duration_ms,
            message: step.message,
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

    use super::{normalize_extension_scope, McpService, NormalizedExtensionScope};
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, PlatformToolConfig, SourceFormat, SourceSetConfig,
        SourceSetPurpose, TestsConfig, ToolsConfig,
    };
    use crate::domain::build::{BuildMode, BuildResult, BuildStep};
    use crate::domain::dump::{DumpMode, DumpResult};
    use crate::domain::execution::StepResult;
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
        McpStepResult, McpSyntaxCheckResponse, McpTestResponse,
    };
    use crate::use_cases::context::{CommandName, ExecutionContext, ExecutionTransport};
    use crate::use_cases::request::{
        BuildRequest, DumpModeRequest, DumpRequest, LaunchModeRequest, LaunchRequest,
        SyntaxRequest, SyntaxTargetRequest, TestRequest, TestScopeRequest,
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
                .push((*context, request.clone()));
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
                .push((*context, request.clone()));
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
                .push((*context, request.clone()));
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
                .push((*context, request.clone()));
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
                .push((*context, request.clone()));
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
    fn run_all_tests_maps_failure_payload_into_business_failure() {
        let mut result = sample_test_result(false);
        result
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
                LaunchModeRequest::Designer,
            ),
            (
                "configurator",
                LaunchMode::Designer,
                LaunchModeRequest::Designer,
            ),
            (" 1CV8 ", LaunchMode::Designer, LaunchModeRequest::Designer),
            (
                "конфигуратор",
                LaunchMode::Designer,
                LaunchModeRequest::Designer,
            ),
            ("thin", LaunchMode::Thin, LaunchModeRequest::Thin),
            ("thin-client", LaunchMode::Thin, LaunchModeRequest::Thin),
            ("thin client", LaunchMode::Thin, LaunchModeRequest::Thin),
            ("THIN_CLIENT", LaunchMode::Thin, LaunchModeRequest::Thin),
            ("tc", LaunchMode::Thin, LaunchModeRequest::Thin),
            ("1cv8c", LaunchMode::Thin, LaunchModeRequest::Thin),
            (
                "  тонкий клиент  ",
                LaunchMode::Thin,
                LaunchModeRequest::Thin,
            ),
            ("тонкий", LaunchMode::Thin, LaunchModeRequest::Thin),
            ("thick", LaunchMode::Thick, LaunchModeRequest::Thick),
            ("thick-client", LaunchMode::Thick, LaunchModeRequest::Thick),
            ("thick client", LaunchMode::Thick, LaunchModeRequest::Thick),
            ("THICK_CLIENT", LaunchMode::Thick, LaunchModeRequest::Thick),
            (
                " толстый клиент ",
                LaunchMode::Thick,
                LaunchModeRequest::Thick,
            ),
            ("толстый", LaunchMode::Thick, LaunchModeRequest::Thick),
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
            assert_eq!(requests[0].1.mode, request_mode, "alias {alias}");
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
            (
                None,
                None,
                NormalizedExtensionScope {
                    extension: None,
                    all_extensions: true,
                },
            ),
            (
                None,
                Some(false),
                NormalizedExtensionScope {
                    extension: None,
                    all_extensions: false,
                },
            ),
            (
                None,
                Some(true),
                NormalizedExtensionScope {
                    extension: None,
                    all_extensions: true,
                },
            ),
            (
                Some(" Ext "),
                None,
                NormalizedExtensionScope {
                    extension: Some("Ext".to_owned()),
                    all_extensions: false,
                },
            ),
            (
                Some(" Ext "),
                Some(false),
                NormalizedExtensionScope {
                    extension: Some("Ext".to_owned()),
                    all_extensions: false,
                },
            ),
            (
                Some(" Ext "),
                Some(true),
                NormalizedExtensionScope {
                    extension: Some("Ext".to_owned()),
                    all_extensions: true,
                },
            ),
            (
                Some(" "),
                None,
                NormalizedExtensionScope {
                    extension: None,
                    all_extensions: true,
                },
            ),
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
                assert_eq!(request.thin_client, true);
                assert_eq!(request.server, true);
                assert_eq!(request.extended_modules_check, true);
                assert_eq!(request.all_extensions, true);
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
                assert_eq!(request.extension.as_deref(), Some("Ext"));
                assert_eq!(request.all_extensions, true);
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
                assert_eq!(request.thin_client, true);
                assert_eq!(request.server, true);
                assert_eq!(request.extension.as_deref(), Some("Ext"));
                assert_eq!(request.all_extensions, false);
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
                assert_eq!(request.extension, None);
                assert_eq!(request.all_extensions, true);
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
                duration_ms: 10,
                message: Some("boom".to_owned()),
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
                    "duration_ms": 10,
                    "message": "boom"
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
                vec![StepResult {
                    name: "test".to_owned(),
                    ok: false,
                    duration_ms: 120,
                    message: Some("boom".to_owned()),
                }]
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
