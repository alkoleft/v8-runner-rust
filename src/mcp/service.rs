use serde::Serialize;
use serde_json::{json, Value};
use std::time::Instant;

use crate::command_envelope::{test_envelope, Envelope, EnvelopeError};
use crate::config::model::AppConfig;
use crate::domain::runner::LaunchOptions;
use crate::domain::syntax::SyntaxCheckResult;
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

type McpCommandEnvelope = Envelope<Value>;

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
    ) -> McpServiceResult<McpCommandEnvelope> {
        let context = execution_context(call_context, CommandName::Build)
            .map_err(McpServiceError::Internal)?;
        let use_case_request = BuildRequest {
            full_rebuild: request.full_rebuild.unwrap_or(false),
            source_set: request.source_set.clone(),
        };

        match self
            .port
            .build_project(&context, self.config, &use_case_request)
        {
            Ok(result) => ok_envelope(CommandName::Build, result.duration_ms, result)
                .map_err(McpServiceError::Internal),
            Err(failure) => Err(map_use_case_failure_envelope(
                failure,
                |result| err_envelope(CommandName::Build, result.duration_ms, result),
                |error| fallback_error_envelope(CommandName::Build, "build_project", error),
            )),
        }
    }

    /// Executes the `run_all_tests` MCP tool.
    pub fn run_all_tests(
        &self,
        call_context: McpCallContext,
        request: &McpRunAllTestsRequest,
    ) -> McpServiceResult<McpCommandEnvelope> {
        let context = execution_context(call_context, CommandName::Test)
            .map_err(McpServiceError::Internal)?;
        let use_case_request = TestRequest {
            execution: TestRequest::default_execution(),
            full: request.full.unwrap_or(false),
            scope: TestScopeRequest::All,
            mcp_ws: crate::use_cases::request::McpClientWsRequest::default(),
        };

        match self
            .port
            .run_tests(&context, self.config, &use_case_request)
        {
            Ok(result) => {
                mcp_value_envelope(test_envelope(&result)).map_err(McpServiceError::Internal)
            }
            Err(failure) => Err(map_use_case_failure_envelope(
                failure,
                |result| mcp_value_envelope(test_envelope(&result)),
                |error| fallback_error_envelope(CommandName::Test, "run_all_tests", error),
            )),
        }
    }

    /// Executes the `run_module_tests` MCP tool.
    pub fn run_module_tests(
        &self,
        call_context: McpCallContext,
        request: &McpRunModuleTestsRequest,
    ) -> McpServiceResult<McpCommandEnvelope> {
        let context = execution_context(call_context, CommandName::Test)
            .map_err(McpServiceError::Internal)?;
        let module_name =
            normalize_required_string(&request.module_name, "module_name").map_err(|error| {
                let message = error.message().to_owned();
                let business_error = McpBusinessError::from_use_case(&error);
                McpServiceError::Business(McpBusinessFailure::new(
                    business_error.clone(),
                    adapter_error_envelope(
                        CommandName::Test,
                        "run_module_tests",
                        &message,
                        business_error,
                        json!({ "field": "module_name" }),
                    ),
                ))
            })?;
        let use_case_request = TestRequest {
            execution: TestRequest::default_execution(),
            full: request.full.unwrap_or(false),
            scope: TestScopeRequest::Module { name: module_name },
            mcp_ws: crate::use_cases::request::McpClientWsRequest::default(),
        };

        match self
            .port
            .run_tests(&context, self.config, &use_case_request)
        {
            Ok(result) => {
                mcp_value_envelope(test_envelope(&result)).map_err(McpServiceError::Internal)
            }
            Err(failure) => Err(map_use_case_failure_envelope(
                failure,
                |result| mcp_value_envelope(test_envelope(&result)),
                |error| fallback_error_envelope(CommandName::Test, "run_module_tests", error),
            )),
        }
    }

    /// Executes the `dump_config` MCP tool.
    pub fn dump_config(
        &self,
        call_context: McpCallContext,
        request: &McpDumpConfigRequest,
    ) -> McpServiceResult<McpCommandEnvelope> {
        let context = execution_context(call_context, CommandName::Dump)
            .map_err(McpServiceError::Internal)?;
        let use_case_request = DumpRequest {
            mode: parse_optional_dump_mode(request.mode.as_deref(), DumpModeRequest::Incremental)
                .map_err(|error| {
                let message = error.message().to_owned();
                let business_error = raw_value_business_error(&error, "dump mode");
                let mode = request
                    .mode
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or("FULL")
                    .to_owned();
                let data_message = request.mode.as_deref().map_or_else(
                    || "dump mode is invalid".to_owned(),
                    |value| format!("unsupported dump mode: {value}"),
                );
                McpServiceError::Business(McpBusinessFailure::new(
                    business_error.clone(),
                    adapter_error_envelope(
                        CommandName::Dump,
                        "dump_config",
                        &data_message,
                        business_error,
                        json!({ "field": "mode", "mode": mode, "errors": [message] }),
                    ),
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
            Ok(result) => ok_envelope(CommandName::Dump, result.duration_ms, result)
                .map_err(McpServiceError::Internal),
            Err(failure) => Err(map_use_case_failure_envelope(
                failure,
                |result| err_envelope(CommandName::Dump, result.duration_ms, result),
                |error| {
                    let mut envelope =
                        fallback_error_envelope(CommandName::Dump, "dump_config", error)?;
                    envelope.data["mode"] = json!(render_dump_mode(use_case_request.mode));
                    Ok(envelope)
                },
            )),
        }
    }

    /// Executes the `launch_app` MCP tool.
    pub fn launch_app(
        &self,
        call_context: McpCallContext,
        request: &McpLaunchAppRequest,
    ) -> McpServiceResult<McpCommandEnvelope> {
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
                let business_error = raw_value_business_error(&error, "utility_type");
                McpServiceError::Business(McpBusinessFailure::new(
                    business_error.clone(),
                    adapter_error_envelope(
                        CommandName::Launch,
                        "launch_app",
                        &message,
                        business_error,
                        json!({ "field": "utility_type" }),
                    ),
                ))
            })?,
            launch: LaunchOptions::default(),
            client_mcp: None,
            mcp_ws: crate::use_cases::request::McpClientWsRequest::default(),
        };
        let started = Instant::now();

        match self
            .port
            .launch_app(&context, self.config, &use_case_request)
        {
            Ok(result) => ok_envelope(
                CommandName::Launch,
                started.elapsed().as_millis() as u64,
                result,
            )
            .map_err(McpServiceError::Internal),
            Err(failure) => Err(map_use_case_failure_envelope(
                failure,
                |result| {
                    err_envelope(
                        CommandName::Launch,
                        started.elapsed().as_millis() as u64,
                        result,
                    )
                },
                |error| fallback_error_envelope(CommandName::Launch, "launch_app", error),
            )),
        }
    }

    /// Executes the `check_syntax_edt` MCP tool.
    pub fn check_syntax_edt(
        &self,
        call_context: McpCallContext,
        request: &McpCheckSyntaxEdtRequest,
    ) -> McpServiceResult<McpCommandEnvelope> {
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
    ) -> McpServiceResult<McpCommandEnvelope> {
        let context = execution_context(call_context, CommandName::Syntax)
            .map_err(McpServiceError::Internal)?;
        let use_case_request = SyntaxRequest {
            target: SyntaxTargetRequest::DesignerConfig(
                map_designer_config_request(request).map_err(|error| {
                    invalid_syntax_request(error, "check_syntax_designer_config")
                })?,
            ),
        };

        match self
            .port
            .check_syntax(&context, self.config, &use_case_request)
        {
            Ok(result) => ok_envelope(CommandName::Syntax, result.duration_ms, result)
                .map_err(McpServiceError::Internal),
            Err(failure) => Err(map_use_case_failure_envelope(
                failure,
                |result| err_envelope(CommandName::Syntax, result.duration_ms, result),
                |error| {
                    fallback_error_envelope(
                        CommandName::Syntax,
                        "check_syntax_designer_config",
                        error,
                    )
                },
            )),
        }
    }

    /// Executes the `check_syntax_designer_modules` MCP tool.
    pub fn check_syntax_designer_modules(
        &self,
        call_context: McpCallContext,
        request: &McpCheckSyntaxDesignerModulesRequest,
    ) -> McpServiceResult<McpCommandEnvelope> {
        let context = execution_context(call_context, CommandName::Syntax)
            .map_err(McpServiceError::Internal)?;
        let use_case_request = SyntaxRequest {
            target: SyntaxTargetRequest::DesignerModules(
                map_designer_modules_request(request).map_err(|error| {
                    invalid_syntax_request(error, "check_syntax_designer_modules")
                })?,
            ),
        };

        match self
            .port
            .check_syntax(&context, self.config, &use_case_request)
        {
            Ok(result) => ok_envelope(CommandName::Syntax, result.duration_ms, result)
                .map_err(McpServiceError::Internal),
            Err(failure) => Err(map_use_case_failure_envelope(
                failure,
                |result| err_envelope(CommandName::Syntax, result.duration_ms, result),
                |error| {
                    fallback_error_envelope(
                        CommandName::Syntax,
                        "check_syntax_designer_modules",
                        error,
                    )
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

fn ok_envelope<T: Serialize>(
    command: CommandName,
    duration_ms: u64,
    data: T,
) -> Result<McpCommandEnvelope, McpInternalError> {
    mcp_value_envelope(Envelope::ok(command.as_str(), duration_ms, data))
}

fn err_envelope<T: Serialize>(
    command: CommandName,
    duration_ms: u64,
    data: T,
) -> Result<McpCommandEnvelope, McpInternalError> {
    mcp_value_envelope(Envelope::err(command.as_str(), duration_ms, data))
}

fn mcp_value_envelope<T: Serialize>(
    envelope: Envelope<T>,
) -> Result<McpCommandEnvelope, McpInternalError> {
    let Envelope {
        ok,
        command,
        duration_ms,
        data,
        warnings,
        steps,
        error,
    } = envelope;
    let data = serde_json::to_value(data).map_err(|error| {
        McpInternalError::new(format!("failed to serialize command data: {error}"))
    })?;
    Ok(Envelope {
        ok,
        command,
        duration_ms,
        data,
        warnings,
        steps,
        error,
    })
}

fn map_use_case_failure_envelope<TPayload, FPayload, FFallback>(
    failure: UseCaseFailure<TPayload>,
    payload_mapper: FPayload,
    fallback_response: FFallback,
) -> McpServiceError<McpCommandEnvelope>
where
    FPayload: FnOnce(TPayload) -> Result<McpCommandEnvelope, McpInternalError>,
    FFallback: FnOnce(&UseCaseError) -> Result<McpCommandEnvelope, McpInternalError>,
{
    let error = failure.error;
    let business_error = McpBusinessError::from_use_case(&error);
    let response = match failure.payload {
        Some(payload) => payload_mapper(payload),
        None => fallback_response(&error),
    };
    match response {
        Ok(response) => McpServiceError::Business(McpBusinessFailure::new(
            business_error.clone(),
            response.with_error(envelope_error(&business_error)),
        )),
        Err(error) => McpServiceError::Internal(error),
    }
}

fn fallback_error_envelope(
    command: CommandName,
    tool: &'static str,
    error: &UseCaseError,
) -> Result<McpCommandEnvelope, McpInternalError> {
    mcp_value_envelope(Envelope::err(
        command.as_str(),
        0,
        json!({
            "message": error.message(),
            "tool": tool,
        }),
    ))
}

fn adapter_error_envelope(
    command: CommandName,
    tool: &'static str,
    message: &str,
    business_error: McpBusinessError,
    mut extra: Value,
) -> McpCommandEnvelope {
    let mut data = json!({
        "message": message,
        "tool": tool,
    });
    if let (Some(data_object), Some(extra_object)) = (data.as_object_mut(), extra.as_object_mut()) {
        data_object.extend(extra_object.clone());
    }
    Envelope::err(command.as_str(), 0, data).with_error(envelope_error(&business_error))
}

fn envelope_error(error: &McpBusinessError) -> EnvelopeError {
    EnvelopeError::new(
        error.code.as_str(),
        error.kind.as_str(),
        error.message.clone(),
    )
}

fn invalid_syntax_request(
    error: UseCaseError,
    tool: &'static str,
) -> McpServiceError<McpCommandEnvelope> {
    let message = error.message().to_owned();
    let business_error = McpBusinessError::from_use_case(&error);
    McpServiceError::Business(McpBusinessFailure::new(
        business_error.clone(),
        adapter_error_envelope(
            CommandName::Syntax,
            tool,
            &message,
            business_error,
            json!({ "errors": [message] }),
        ),
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
) -> McpServiceResult<McpCommandEnvelope> {
    match result {
        Ok(result) => ok_envelope(CommandName::Syntax, result.duration_ms, result)
            .map_err(McpServiceError::Internal),
        Err(failure) => Err(map_use_case_failure_envelope(
            failure,
            |result| err_envelope(CommandName::Syntax, result.duration_ms, result),
            |error| fallback_error_envelope(CommandName::Syntax, "check_syntax_edt", error),
        )),
    }
}

fn render_dump_mode(mode: DumpModeRequest) -> &'static str {
    match mode {
        DumpModeRequest::Full => "FULL",
        DumpModeRequest::Incremental => "INCREMENTAL",
        DumpModeRequest::Partial => "PARTIAL",
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::McpService;
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
                    source_set: Some("main".to_owned()),
                },
            )
            .expect("success");

        assert!(response.ok);
        assert_eq!(response.command, "build");
        assert_eq!(response.duration_ms, 42);
        assert_eq!(response.data["ok"], true);
        assert_eq!(response.data["steps"][0]["mode"], "full");
        let requests = service.port.build_requests.borrow();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0.command(), CommandName::Build);
        assert_eq!(requests[0].0.transport(), ExecutionTransport::McpStdio);
        assert_eq!(requests[0].1.full_rebuild, true);
        assert_eq!(requests[0].1.source_set.as_deref(), Some("main"));
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
                assert!(!failure.response.ok);
                assert_eq!(failure.response.command, "build");
                assert_eq!(failure.response.duration_ms, 19);
                assert_eq!(failure.response.data["steps"][0]["ok"], false);
                assert_eq!(
                    failure
                        .response
                        .error
                        .as_ref()
                        .map(|error| error.code.as_str()),
                    Some("runtime_failure")
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn run_all_tests_maps_success_response() {
        let mut result = sample_test_result(true);
        if let Some(report) = result.execution.payload.as_mut() {
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

        assert!(response.ok);
        assert_eq!(response.command, "test");
        assert_eq!(response.duration_ms, 120);
        assert_eq!(response.data["report"]["summary"]["total"], 3);
        assert_eq!(response.data["report"]["summary"]["passed"], 2);
        assert_eq!(response.data["report"]["summary"]["failed"], 1);
        assert!(response.error.is_none());
        let requests = service.port.test_requests.borrow();
        assert_eq!(requests[0].1.full, true);
        assert_eq!(requests[0].1.scope, TestScopeRequest::All);
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
                assert!(!failure.response.ok);
                assert_eq!(failure.response.command, "test");
                assert!(failure.response.data["diagnostics"]
                    .as_array()
                    .expect("diagnostics")
                    .contains(&json!("enterprise exited non-zero")));
                assert_eq!(
                    failure
                        .response
                        .error
                        .as_ref()
                        .map(|error| error.code.as_str()),
                    Some("runtime_failure")
                );
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

        assert!(response.ok);
        assert_eq!(response.command, "test");
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
                assert_eq!(failure.response.command, "test");
                assert_eq!(
                    failure.response.data["message"],
                    "module_name must not be blank"
                );
                assert_eq!(failure.response.data["tool"], "run_module_tests");
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

        assert!(response.ok);
        assert_eq!(response.command, "dump");
        assert_eq!(response.data["mode"], "INCREMENTAL");
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
                assert_eq!(failure.response.command, "dump");
                assert_eq!(failure.response.data["mode"], "INCREMENTAL");
                assert_eq!(failure.response.data["message"], "dump failed");
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
                assert_eq!(failure.response.command, "dump");
                assert_eq!(failure.response.data["mode"], "INCREMENTAL");
                assert_eq!(failure.response.data["message"], "dump failed");
                assert_eq!(failure.response.data["tool"], "dump_config");
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
                assert_eq!(failure.response.command, "dump");
                assert_eq!(failure.response.data["mode"], "garbage");
                assert_eq!(
                    failure.response.data["errors"][0],
                    "unsupported dump mode: garbage"
                );
                assert_eq!(
                    failure
                        .response
                        .error
                        .as_ref()
                        .map(|error| error.code.as_str()),
                    Some("unsupported_value")
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

        assert!(response.ok);
        assert_eq!(response.data["mode"], "PARTIAL");
        assert!(response.data["message"]
            .as_str()
            .expect("message")
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
                assert_eq!(failure.response.data["mode"], "PARTIAL");
                assert!(failure.response.data["message"]
                    .as_str()
                    .expect("message")
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
                transport: None,
                client_uid: None,
                kind: None,
                manager_url: None,
                corr_id: None,
                mcp_port: None,
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

            assert!(response.ok, "alias {alias}");
            assert_eq!(response.command, "launch");
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
                transport: None,
                client_uid: None,
                kind: None,
                manager_url: None,
                corr_id: None,
                mcp_port: None,
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
                assert_eq!(
                    failure.response.data["message"],
                    "utility_type must not be blank"
                );
                assert_eq!(failure.response.data["tool"], "launch_app");
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
                transport: None,
                client_uid: None,
                kind: None,
                manager_url: None,
                corr_id: None,
                mcp_port: None,
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
                    failure.response.data["message"],
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

        assert!(response.ok);
        assert_eq!(response.command, "syntax");
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
                assert_eq!(failure.response.data["message"], "edt project not found");
                assert_eq!(failure.response.data["tool"], "check_syntax_edt");
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

        assert!(response.ok);
        assert_eq!(response.command, "syntax");
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
                    failure.response.data["message"],
                    "checkUseSynchronousCalls requires extendedModulesCheck=true"
                );
                assert_eq!(
                    failure.response.data["errors"][0],
                    "checkUseSynchronousCalls requires extendedModulesCheck=true"
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
                    failure.response.data["message"],
                    "checkUseModality requires extendedModulesCheck=true"
                );
                assert_eq!(
                    failure.response.data["errors"][0],
                    "checkUseModality requires extendedModulesCheck=true"
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
                assert_eq!(failure.response.data["status"], "tool_failed");
                assert_eq!(failure.response.data["stderr"], "designer stderr");
                assert_eq!(failure.response.data["log_read_warning"], "log truncated");
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

        assert!(response.ok);
        assert_eq!(response.data["status"], "issues_found");
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
                assert_eq!(failure.response.data["message"], "no syntax modes selected");
                assert_eq!(
                    failure.response.data["tool"],
                    "check_syntax_designer_modules"
                );
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
                ..Default::default()
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
