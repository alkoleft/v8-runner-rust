use std::sync::Arc;
use std::time::Duration;

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ErrorData, ServerHandler, ServiceExt,
};
use serde_json::json;
use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::error;

use crate::config::model::AppConfig;
use crate::mcp::context::McpCallContext;
use crate::mcp::edt_session::{EdtRequestCompletion, EdtSessionManager};
use crate::mcp::edt_syntax;
use crate::mcp::error::{McpInternalError, McpServiceResult};
use crate::mcp::port::{DefaultMcpUseCasePort, McpUseCasePort};
use crate::mcp::request::{
    McpBuildProjectRequest, McpCheckSyntaxDesignerConfigRequest,
    McpCheckSyntaxDesignerModulesRequest, McpCheckSyntaxEdtRequest, McpDumpConfigRequest,
    McpLaunchAppRequest, McpRunAllTestsRequest, McpRunModuleTestsRequest,
};
use crate::mcp::service::McpService;
use crate::mcp::service::{map_syntax_use_case_result, normalize_check_syntax_edt_request};
use crate::mcp::tool_result::McpToolResult;

type SharedMcpUseCasePort = Arc<dyn McpUseCasePort + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum McpTool {
    RunAllTests,
    RunModuleTests,
    BuildProject,
    DumpConfig,
    LaunchApp,
    CheckSyntaxEdt,
    CheckSyntaxDesignerConfig,
    CheckSyntaxDesignerModules,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExecutionStage {
    Queued,
    Running,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorReason {
    Cancelled,
    Timeout,
    JoinFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExecutionPolicy {
    timeout: Option<Duration>,
}

impl ExecutionPolicy {
    const fn standard() -> Self {
        Self { timeout: None }
    }

    const fn bounded(timeout: Duration) -> Self {
        Self {
            timeout: Some(timeout),
        }
    }
}

impl McpTool {
    fn execution_policy(self, config: &AppConfig) -> ExecutionPolicy {
        match self {
            Self::CheckSyntaxEdt => ExecutionPolicy::bounded(Duration::from_millis(
                config.tools.edt_cli.command_timeout_ms,
            )),
            Self::RunAllTests
            | Self::RunModuleTests
            | Self::BuildProject
            | Self::DumpConfig
            | Self::LaunchApp
            | Self::CheckSyntaxDesignerConfig
            | Self::CheckSyntaxDesignerModules => ExecutionPolicy::standard(),
        }
    }
}

/// Bootstrap errors returned by the MCP stdio server.
#[derive(Debug, Error)]
pub enum McpServerError {
    #[error("failed to build tokio runtime for MCP stdio: {0}")]
    BuildRuntime(std::io::Error),

    #[error("failed to initialize MCP stdio server: {0}")]
    Bootstrap(String),

    #[error("failed to start MCP stdio server: {0}")]
    Start(String),

    #[error("MCP stdio server task failed: {0}")]
    Task(String),
}

/// Runs the MCP stdio server until the transport closes.
pub fn serve_stdio(config: AppConfig) -> Result<(), McpServerError> {
    let shutdown_timeout = shutdown_grace_period(&config);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("v8tr-mcp")
        .build()
        .map_err(McpServerError::BuildRuntime)?;

    let result = runtime.block_on(async move {
        let server = McpStdioServer::new(Arc::new(config))?;
        let running = server
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|error| McpServerError::Start(error.to_string()))?;

        running
            .waiting()
            .await
            .map_err(|error| McpServerError::Task(error.to_string()))?;
        Ok(())
    });

    runtime.shutdown_timeout(shutdown_timeout);
    result
}

/// rmcp-backed stdio transport adapter over the MCP service layer.
#[derive(Clone)]
pub struct McpStdioServer {
    config: Arc<AppConfig>,
    port: SharedMcpUseCasePort,
    edt_session: Arc<EdtSessionManager>,
    concurrency_limit: Arc<Semaphore>,
    tool_router: ToolRouter<Self>,
}

impl McpStdioServer {
    /// Creates a stdio server using the production use-case port.
    pub fn new(config: Arc<AppConfig>) -> Result<Self, McpServerError> {
        Self::with_port(config, Arc::new(DefaultMcpUseCasePort))
    }

    /// Creates a stdio server with an injected MCP use-case port.
    pub fn with_port(
        config: Arc<AppConfig>,
        port: SharedMcpUseCasePort,
    ) -> Result<Self, McpServerError> {
        Ok(Self {
            edt_session: Arc::new(
                EdtSessionManager::for_config(config.as_ref())
                    .map_err(|error| McpServerError::Bootstrap(error.to_string()))?,
            ),
            concurrency_limit: Arc::new(Semaphore::new(max_concurrent_calls(config.as_ref()))),
            config,
            port,
            tool_router: Self::tool_router(),
        })
    }

    async fn execute_tool<TRequest, TResponse>(
        &self,
        tool: McpTool,
        request: TRequest,
        cancellation: CancellationToken,
        method: impl FnOnce(
                Arc<AppConfig>,
                SharedMcpUseCasePort,
                McpCallContext,
                TRequest,
            ) -> McpServiceResult<TResponse>
            + Send
            + 'static,
    ) -> Result<CallToolResult, ErrorData>
    where
        TRequest: Send + 'static,
        TResponse: serde::Serialize + Send + 'static,
    {
        let policy = tool.execution_policy(self.config.as_ref());
        let timeout = policy.timeout;
        let deadline = timeout.map(|value| Instant::now() + value);
        let permit = self
            .acquire_execution_slot(cancellation.clone(), deadline, timeout)
            .await?;
        let remaining_timeout = remaining_timeout(deadline);
        if cancellation.is_cancelled() {
            return Err(execution_error(
                ErrorReason::Cancelled,
                ExecutionStage::Queued,
                timeout,
            ));
        }
        if timeout.is_some() && remaining_timeout.is_some_and(|value| value.is_zero()) {
            return Err(execution_error(
                ErrorReason::Timeout,
                ExecutionStage::Queued,
                timeout,
            ));
        }

        let config = self.config.clone();
        let port = self.port.clone();
        let call_context = McpCallContext::stdio().with_edt_timeout(remaining_timeout);
        let mut handle =
            tokio::task::spawn_blocking(move || method(config, port, call_context, request));

        tokio::select! {
            biased;
            result = &mut handle => {
                let result = result
                    .map_err(|_| execution_error(ErrorReason::JoinFailure, ExecutionStage::Running, timeout))?;
                map_tool_result(result)
            }
            _ = cancellation.cancelled() => {
                reap_detached_call(handle, permit);
                Err(execution_error(ErrorReason::Cancelled, ExecutionStage::Running, timeout))
            }
            _ = wait_for_deadline(deadline), if deadline.is_some() => {
                reap_detached_call(handle, permit);
                Err(execution_error(ErrorReason::Timeout, ExecutionStage::Running, timeout))
            }
        }
    }

    async fn acquire_execution_slot(
        &self,
        cancellation: CancellationToken,
        deadline: Option<Instant>,
        timeout: Option<Duration>,
    ) -> Result<OwnedSemaphorePermit, ErrorData> {
        let acquire = self.concurrency_limit.clone().acquire_owned();
        tokio::pin!(acquire);

        match deadline {
            Some(deadline) => {
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => {
                        Err(execution_error(ErrorReason::Cancelled, ExecutionStage::Queued, timeout))
                    }
                    _ = tokio::time::sleep_until(deadline) => {
                        Err(execution_error(ErrorReason::Timeout, ExecutionStage::Queued, timeout))
                    }
                    permit = &mut acquire => permit
                        .map_err(|error| ErrorData::internal_error(error.to_string(), None)),
                }
            }
            None => {
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => {
                        Err(execution_error(ErrorReason::Cancelled, ExecutionStage::Queued, timeout))
                    }
                    permit = &mut acquire => permit
                        .map_err(|error| ErrorData::internal_error(error.to_string(), None)),
                }
            }
        }
    }

    async fn execute_edt_syntax_tool(
        &self,
        request: McpCheckSyntaxEdtRequest,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult, ErrorData> {
        let timeout = McpTool::CheckSyntaxEdt
            .execution_policy(self.config.as_ref())
            .timeout;
        let deadline = timeout.map(|value| Instant::now() + value);
        let mut permit = Some(
            self.acquire_execution_slot(cancellation.clone(), deadline, timeout)
                .await?,
        );
        if cancellation.is_cancelled() {
            return Err(execution_error(
                ErrorReason::Cancelled,
                ExecutionStage::Queued,
                timeout,
            ));
        }

        let remaining_timeout = remaining_timeout(deadline);
        if remaining_timeout.is_some_and(|value| value.is_zero()) {
            return Err(execution_error(
                ErrorReason::Timeout,
                ExecutionStage::Queued,
                timeout,
            ));
        }

        let use_case_request = normalize_check_syntax_edt_request(&request);
        let result = edt_syntax::execute(
            self.edt_session.as_ref(),
            self.config.as_ref(),
            &use_case_request,
            remaining_timeout.unwrap_or_else(|| Duration::from_millis(1)),
            cancellation,
        )
        .await;

        match result {
            Ok(use_case_result) => {
                permit.take();
                map_tool_result(map_syntax_use_case_result(use_case_result))
            }
            Err(edt_syntax::EdtSyntaxTransportError::QueuedCancelled) => {
                permit.take();
                Err(execution_error(
                    ErrorReason::Cancelled,
                    ExecutionStage::Queued,
                    timeout,
                ))
            }
            Err(edt_syntax::EdtSyntaxTransportError::RunningCancelledDetached { completion }) => {
                if let Some(permit) = permit.take() {
                    reap_detached_edt_call(completion, permit);
                }
                Err(execution_error(
                    ErrorReason::Cancelled,
                    ExecutionStage::Running,
                    timeout,
                ))
            }
            Err(edt_syntax::EdtSyntaxTransportError::RunningCancelledCompleted) => {
                permit.take();
                Err(execution_error(
                    ErrorReason::Cancelled,
                    ExecutionStage::Running,
                    timeout,
                ))
            }
            Err(edt_syntax::EdtSyntaxTransportError::QueuedTimeout) => {
                permit.take();
                Err(execution_error(
                    ErrorReason::Timeout,
                    ExecutionStage::Queued,
                    timeout,
                ))
            }
            Err(edt_syntax::EdtSyntaxTransportError::RunningTimeoutDetached { completion }) => {
                if let Some(permit) = permit.take() {
                    reap_detached_edt_call(completion, permit);
                }
                Err(execution_error(
                    ErrorReason::Timeout,
                    ExecutionStage::Running,
                    timeout,
                ))
            }
            Err(edt_syntax::EdtSyntaxTransportError::RunningTimeoutCompleted) => {
                permit.take();
                Err(execution_error(
                    ErrorReason::Timeout,
                    ExecutionStage::Running,
                    timeout,
                ))
            }
        }
    }
}

#[tool_router(router = tool_router)]
impl McpStdioServer {
    #[tool(description = "Run all tests")]
    async fn run_all_tests(
        &self,
        Parameters(request): Parameters<McpRunAllTestsRequest>,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute_tool(
            McpTool::RunAllTests,
            request,
            cancellation,
            |config, port, call_context, request| {
                let service = McpService::with_port(config.as_ref(), port);
                service.run_all_tests(call_context, &request)
            },
        )
        .await
    }

    #[tool(description = "Run tests for a specific module")]
    async fn run_module_tests(
        &self,
        Parameters(request): Parameters<McpRunModuleTestsRequest>,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute_tool(
            McpTool::RunModuleTests,
            request,
            cancellation,
            |config, port, call_context, request| {
                let service = McpService::with_port(config.as_ref(), port);
                service.run_module_tests(call_context, &request)
            },
        )
        .await
    }

    #[tool(description = "Build the project")]
    async fn build_project(
        &self,
        Parameters(request): Parameters<McpBuildProjectRequest>,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute_tool(
            McpTool::BuildProject,
            request,
            cancellation,
            |config, port, call_context, request| {
                let service = McpService::with_port(config.as_ref(), port);
                service.build_project(call_context, &request)
            },
        )
        .await
    }

    #[tool(description = "Dump configuration to files")]
    async fn dump_config(
        &self,
        Parameters(request): Parameters<McpDumpConfigRequest>,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute_tool(
            McpTool::DumpConfig,
            request,
            cancellation,
            |config, port, call_context, request| {
                let service = McpService::with_port(config.as_ref(), port);
                service.dump_config(call_context, &request)
            },
        )
        .await
    }

    #[tool(description = "Launch a 1C application")]
    async fn launch_app(
        &self,
        Parameters(request): Parameters<McpLaunchAppRequest>,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute_tool(
            McpTool::LaunchApp,
            request,
            cancellation,
            |config, port, call_context, request| {
                let service = McpService::with_port(config.as_ref(), port);
                service.launch_app(call_context, &request)
            },
        )
        .await
    }

    #[tool(description = "Run EDT syntax check")]
    async fn check_syntax_edt(
        &self,
        Parameters(request): Parameters<McpCheckSyntaxEdtRequest>,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute_edt_syntax_tool(request, cancellation).await
    }

    #[tool(description = "Run Designer configuration syntax check")]
    async fn check_syntax_designer_config(
        &self,
        Parameters(request): Parameters<McpCheckSyntaxDesignerConfigRequest>,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute_tool(
            McpTool::CheckSyntaxDesignerConfig,
            request,
            cancellation,
            |config, port, call_context, request| {
                let service = McpService::with_port(config.as_ref(), port);
                service.check_syntax_designer_config(call_context, &request)
            },
        )
        .await
    }

    #[tool(description = "Run Designer modules syntax check")]
    async fn check_syntax_designer_modules(
        &self,
        Parameters(request): Parameters<McpCheckSyntaxDesignerModulesRequest>,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute_tool(
            McpTool::CheckSyntaxDesignerModules,
            request,
            cancellation,
            |config, port, call_context, request| {
                let service = McpService::with_port(config.as_ref(), port);
                service.check_syntax_designer_modules(call_context, &request)
            },
        )
        .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpStdioServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}

fn internal_error_to_mcp(error: McpInternalError) -> ErrorData {
    ErrorData::internal_error(error.message, None)
}

fn map_tool_result<TResponse>(
    result: McpServiceResult<TResponse>,
) -> Result<CallToolResult, ErrorData>
where
    TResponse: serde::Serialize + Send + 'static,
{
    match result {
        Ok(response) => Ok(CallToolResult::structured(
            serde_json::to_value(McpToolResult::success(response))
                .map_err(|error| ErrorData::internal_error(error.to_string(), None))?,
        )),
        Err(crate::mcp::error::McpServiceError::Business(failure)) => {
            Ok(CallToolResult::structured_error(
                serde_json::to_value(McpToolResult::business_failure(failure))
                    .map_err(|error| ErrorData::internal_error(error.to_string(), None))?,
            ))
        }
        Err(crate::mcp::error::McpServiceError::Internal(error)) => {
            Err(internal_error_to_mcp(error))
        }
    }
}

fn execution_error(
    reason: ErrorReason,
    stage: ExecutionStage,
    timeout: Option<Duration>,
) -> ErrorData {
    let message = match (reason, stage) {
        (ErrorReason::Cancelled, ExecutionStage::Queued) => {
            "MCP call cancelled while waiting for execution slot"
        }
        (ErrorReason::Cancelled, ExecutionStage::Running) => "MCP call cancelled during execution",
        (ErrorReason::Timeout, ExecutionStage::Queued) => {
            "MCP call timed out while waiting for execution slot"
        }
        (ErrorReason::Timeout, ExecutionStage::Running) => "MCP call timed out during execution",
        (ErrorReason::JoinFailure, ExecutionStage::Queued) => "MCP queue task failed unexpectedly",
        (ErrorReason::JoinFailure, ExecutionStage::Running) => {
            "MCP execution task failed unexpectedly"
        }
    };
    let data = json!({
        "reason": match reason {
            ErrorReason::Cancelled => "cancelled",
            ErrorReason::Timeout => "timeout",
            ErrorReason::JoinFailure => "join_failure",
        },
        "stage": match stage {
            ExecutionStage::Queued => "queued",
            ExecutionStage::Running => "running",
        },
        "timeoutMs": timeout.map(duration_to_millis),
    });
    ErrorData::internal_error(message, Some(data))
}

fn remaining_timeout(deadline: Option<Instant>) -> Option<Duration> {
    deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()))
}

fn duration_to_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

async fn wait_for_deadline(deadline: Option<Instant>) {
    if let Some(deadline) = deadline {
        tokio::time::sleep_until(deadline).await;
    }
}

fn reap_detached_call<TResponse>(
    handle: JoinHandle<McpServiceResult<TResponse>>,
    permit: OwnedSemaphorePermit,
) where
    TResponse: Send + 'static,
{
    tokio::spawn(async move {
        let _permit = permit;
        if let Err(join_error) = handle.await {
            error!(?join_error, "detached MCP execution task failed");
        }
    });
}

fn reap_detached_edt_call(completion: EdtRequestCompletion, permit: OwnedSemaphorePermit) {
    tokio::spawn(async move {
        let _permit = permit;
        completion.wait().await;
    });
}

fn max_concurrent_calls(config: &AppConfig) -> usize {
    config.mcp.execution.max_concurrent_calls.max(1)
}

fn shutdown_grace_period(config: &AppConfig) -> Duration {
    Duration::from_secs(config.mcp.execution.shutdown_grace_period_secs.max(1))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use std::time::Instant;

    use super::{
        execution_error, max_concurrent_calls, shutdown_grace_period, ErrorReason, ExecutionStage,
        McpStdioServer, McpTool,
    };
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, McpConfig, McpExecutionConfig, McpHttpConfig,
        PlatformToolConfig, SourceFormat, SourceSetConfig, SourceSetPurpose, TestsConfig,
        ToolsConfig,
    };
    use crate::mcp::port::DefaultMcpUseCasePort;
    use tokio_util::sync::CancellationToken;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn execute_tool_respects_configured_concurrency_limit() {
        let server =
            McpStdioServer::with_port(Arc::new(test_config(1, 9)), Arc::new(DefaultMcpUseCasePort))
                .expect("server");
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));

        let first = tokio::spawn(run_probe_call(
            server.clone(),
            McpTool::RunAllTests,
            active.clone(),
            max_active.clone(),
            CancellationToken::new(),
        ));
        let second = tokio::spawn(run_probe_call(
            server,
            McpTool::CheckSyntaxEdt,
            active,
            max_active.clone(),
            CancellationToken::new(),
        ));

        first.await.expect("first task join").expect("first call");
        second
            .await
            .expect("second task join")
            .expect("second call");

        assert_eq!(max_active.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn shutdown_grace_period_uses_configured_value() {
        let config = test_config(3, 42);

        assert_eq!(max_concurrent_calls(&config), 3);
        assert_eq!(shutdown_grace_period(&config), Duration::from_secs(42));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queued_cancellation_returns_transport_error_without_running_call() {
        let server =
            McpStdioServer::with_port(Arc::new(test_config(1, 9)), Arc::new(DefaultMcpUseCasePort))
                .expect("server");
        let started = Arc::new(AtomicUsize::new(0));

        let first = tokio::spawn(run_probe_call(
            server.clone(),
            McpTool::RunAllTests,
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            CancellationToken::new(),
        ));
        tokio::time::sleep(Duration::from_millis(10)).await;

        let cancellation = CancellationToken::new();
        let started_clone = started.clone();
        let queued_cancellation = cancellation.clone();
        let second = tokio::spawn(async move {
            server
                .execute_tool(
                    McpTool::RunAllTests,
                    (),
                    queued_cancellation,
                    move |_, _, _, ()| {
                        started_clone.fetch_add(1, Ordering::SeqCst);
                        Ok(String::from("unexpected"))
                    },
                )
                .await
                .expect_err("queued call must be cancelled")
        });

        tokio::time::sleep(Duration::from_millis(10)).await;
        cancellation.cancel();

        let error = second.await.expect("second task join");
        first.await.expect("first task join").expect("first call");

        assert_eq!(
            error,
            execution_error(ErrorReason::Cancelled, ExecutionStage::Queued, None)
        );
        assert_eq!(started.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queued_timeout_returns_transport_error() {
        let server = McpStdioServer::with_port(
            Arc::new(test_config_with_edt_timeout(1, 9, 20)),
            Arc::new(DefaultMcpUseCasePort),
        )
        .expect("server");

        let first = tokio::spawn(run_probe_call(
            server.clone(),
            McpTool::RunAllTests,
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            CancellationToken::new(),
        ));
        tokio::time::sleep(Duration::from_millis(10)).await;

        let started = Arc::new(AtomicUsize::new(0));
        let started_clone = started.clone();
        let error = server
            .execute_tool(
                McpTool::CheckSyntaxEdt,
                (),
                CancellationToken::new(),
                move |_, _, _, ()| {
                    started_clone.fetch_add(1, Ordering::SeqCst);
                    Ok(String::from("unexpected"))
                },
            )
            .await
            .expect_err("queued call must time out");

        first.await.expect("first task join").expect("first call");

        assert_eq!(
            error,
            execution_error(
                ErrorReason::Timeout,
                ExecutionStage::Queued,
                Some(Duration::from_millis(20))
            )
        );
        assert_eq!(started.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bounded_queued_cancellation_wins_before_deadline() {
        let server = McpStdioServer::with_port(
            Arc::new(test_config_with_edt_timeout(1, 9, 80)),
            Arc::new(DefaultMcpUseCasePort),
        )
        .expect("server");

        let first = tokio::spawn(run_probe_call(
            server.clone(),
            McpTool::RunAllTests,
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            CancellationToken::new(),
        ));
        tokio::time::sleep(Duration::from_millis(10)).await;

        let started = Arc::new(AtomicUsize::new(0));
        let started_clone = started.clone();
        let cancellation = CancellationToken::new();
        let queued_cancellation = cancellation.clone();
        let second = tokio::spawn(async move {
            server
                .execute_tool(
                    McpTool::CheckSyntaxEdt,
                    (),
                    queued_cancellation,
                    move |_, _, _, ()| {
                        started_clone.fetch_add(1, Ordering::SeqCst);
                        Ok(String::from("unexpected"))
                    },
                )
                .await
                .expect_err("queued bounded call must be cancelled")
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        cancellation.cancel();

        let error = second.await.expect("second task join");
        first.await.expect("first task join").expect("first call");

        assert_eq!(
            error,
            execution_error(
                ErrorReason::Cancelled,
                ExecutionStage::Queued,
                Some(Duration::from_millis(80))
            )
        );
        assert_eq!(started.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn standard_tools_ignore_edt_timeout_budget_when_not_cancelled() {
        let server = McpStdioServer::with_port(
            Arc::new(test_config_with_edt_timeout(1, 9, 20)),
            Arc::new(DefaultMcpUseCasePort),
        )
        .expect("server");

        let started = Instant::now();
        let result = server
            .execute_tool(
                McpTool::RunAllTests,
                (),
                CancellationToken::new(),
                move |_, _, _, ()| {
                    std::thread::sleep(Duration::from_millis(40));
                    Ok(String::from("ok"))
                },
            )
            .await;

        assert!(result.is_ok(), "standard tool should not time out");
        assert!(started.elapsed() >= Duration::from_millis(40));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn standard_running_cancellation_returns_early_and_retains_capacity_until_worker_finishes(
    ) {
        let server =
            McpStdioServer::with_port(Arc::new(test_config(1, 9)), Arc::new(DefaultMcpUseCasePort))
                .expect("server");
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let cancellation = CancellationToken::new();

        let first_server = server.clone();
        let first_active = active.clone();
        let first_max = max_active.clone();
        let first_cancel = cancellation.clone();
        let started = Instant::now();
        let first = tokio::spawn(async move {
            first_server
                .execute_tool(
                    McpTool::RunAllTests,
                    (),
                    first_cancel,
                    move |_, _, _, ()| {
                        let current = first_active.fetch_add(1, Ordering::SeqCst) + 1;
                        first_max.fetch_max(current, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(80));
                        first_active.fetch_sub(1, Ordering::SeqCst);
                        Ok(String::from("ok"))
                    },
                )
                .await
                .expect_err("running call must be cancelled")
        });

        tokio::time::sleep(Duration::from_millis(15)).await;
        cancellation.cancel();
        let error = first.await.expect("first task join");
        assert!(started.elapsed() < Duration::from_millis(80));
        assert_eq!(
            error,
            execution_error(ErrorReason::Cancelled, ExecutionStage::Running, None)
        );

        run_probe_call(
            server,
            McpTool::CheckSyntaxEdt,
            active,
            max_active.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("second call");

        assert_eq!(max_active.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn running_cancellation_returns_early_and_retains_capacity_until_worker_finishes() {
        let server = McpStdioServer::with_port(
            Arc::new(test_config_with_edt_timeout(1, 9, 500)),
            Arc::new(DefaultMcpUseCasePort),
        )
        .expect("server");
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let cancellation = CancellationToken::new();

        let first_server = server.clone();
        let first_active = active.clone();
        let first_max = max_active.clone();
        let first_cancel = cancellation.clone();
        let started = Instant::now();
        let first = tokio::spawn(async move {
            first_server
                .execute_tool(
                    McpTool::CheckSyntaxEdt,
                    (),
                    first_cancel,
                    move |_, _, _, ()| {
                        let current = first_active.fetch_add(1, Ordering::SeqCst) + 1;
                        first_max.fetch_max(current, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(80));
                        first_active.fetch_sub(1, Ordering::SeqCst);
                        Ok(String::from("ok"))
                    },
                )
                .await
                .expect_err("running call must be cancelled")
        });

        tokio::time::sleep(Duration::from_millis(15)).await;
        cancellation.cancel();
        let error = first.await.expect("first task join");
        assert!(started.elapsed() < Duration::from_millis(80));
        assert_eq!(
            error,
            execution_error(
                ErrorReason::Cancelled,
                ExecutionStage::Running,
                Some(Duration::from_millis(500))
            )
        );

        run_probe_call(
            server,
            McpTool::RunAllTests,
            active,
            max_active.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("second call");

        assert_eq!(max_active.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn running_timeout_returns_early_and_capacity_recovers() {
        let server = McpStdioServer::with_port(
            Arc::new(test_config_with_edt_timeout(1, 9, 20)),
            Arc::new(DefaultMcpUseCasePort),
        )
        .expect("server");

        for _ in 0..2 {
            let error = server
                .execute_tool(
                    McpTool::CheckSyntaxEdt,
                    (),
                    CancellationToken::new(),
                    move |_, _, _, ()| {
                        std::thread::sleep(Duration::from_millis(60));
                        Ok(String::from("ok"))
                    },
                )
                .await
                .expect_err("call must time out");

            assert_eq!(
                error,
                execution_error(
                    ErrorReason::Timeout,
                    ExecutionStage::Running,
                    Some(Duration::from_millis(20))
                )
            );
            tokio::time::sleep(Duration::from_millis(70)).await;
        }

        run_probe_call(
            server,
            McpTool::RunAllTests,
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            CancellationToken::new(),
        )
        .await
        .expect("capacity must recover after timed out calls");
    }

    async fn run_probe_call(
        server: McpStdioServer,
        tool: McpTool,
        active: Arc<AtomicUsize>,
        max_active: Arc<AtomicUsize>,
        cancellation: CancellationToken,
    ) -> Result<(), rmcp::ErrorData> {
        server
            .execute_tool(tool, (), cancellation, move |_, _, _, ()| {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_active.fetch_max(current, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(50));
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(String::from("ok"))
            })
            .await
            .map(|_| ())
    }

    fn test_config(max_concurrent_calls: usize, shutdown_grace_period_secs: u64) -> AppConfig {
        test_config_with_edt_timeout(max_concurrent_calls, shutdown_grace_period_secs, 300_000)
    }

    fn test_config_with_edt_timeout(
        max_concurrent_calls: usize,
        shutdown_grace_period_secs: u64,
        edt_timeout_ms: u64,
    ) -> AppConfig {
        AppConfig {
            base_path: PathBuf::from("/tmp/project"),
            work_path: PathBuf::from("/tmp/work"),
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            connection: String::from("File=/tmp/ib"),
            credentials: Default::default(),
            source_sets: vec![SourceSetConfig {
                name: String::from("main"),
                purpose: SourceSetPurpose::Configuration,
                path: PathBuf::from("."),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig {
                platform: PlatformToolConfig::default(),
                edt_cli: crate::config::model::EdtCliConfig {
                    command_timeout_ms: edt_timeout_ms,
                    ..Default::default()
                },
            },
            mcp: McpConfig {
                http: McpHttpConfig::default(),
                execution: McpExecutionConfig {
                    max_concurrent_calls,
                    shutdown_grace_period_secs,
                },
            },
            tests: TestsConfig::default(),
        }
    }
}
