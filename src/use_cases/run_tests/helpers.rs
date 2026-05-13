use std::time::{Duration, Instant};

use crate::config::model::AppConfig;
use crate::domain::artifact::ArtifactSet;
use crate::domain::execution::{
    ExecutionInterruptionDetails, ExecutionOutcome, ExecutionStatus, ExecutionStepKind, StepResult,
};
use crate::domain::runner::{LaunchClientModeRequest, LaunchOptions, RunnerKind};
use crate::domain::test::{TestErrorKind, TestOutputMode, TestReport, TestRunResult, TestTarget};
use crate::platform::enterprise::{EnterpriseDsl, EnterpriseError};
use crate::platform::locator::UtilityType;
use crate::platform::process::{ProcessError, ProcessInterruptionReason};
use crate::platform::utilities::PlatformUtilities;
use crate::support::error::AppError;
use crate::support::path::is_safe_path_segment;
use crate::use_cases::context::{ExecutionContext, ExecutionInterruption, InterruptionSafetyClass};
use crate::use_cases::interruption::{
    command_interruption_details, command_interruption_message, command_interruption_status,
    process_interruption_details,
};
use crate::use_cases::launch_keys::vanessa_enterprise_launch_keys;
use crate::use_cases::request::{TestRequest as TestArgs, TestScopeRequest as TestScope};

use super::{build_yaxunit_config, prepare_vanessa_run, PreparedRun, RunArtifacts};

pub(super) fn make_test_result(
    target: TestTarget,
    mode: TestOutputMode,
    outcome: ExecutionOutcome<TestReport>,
    warnings: Vec<String>,
    steps: Vec<StepResult>,
    duration_ms: u64,
) -> TestRunResult {
    TestRunResult::from_outcome(outcome, target, mode, warnings, steps, duration_ms)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::enterprise_error_kind;
    use crate::domain::execution::ExecutionStatus;
    use crate::domain::test::TestErrorKind;
    use crate::platform::enterprise::EnterpriseError;
    use crate::platform::process::ProcessError;
    use crate::support::error::AppError;

    fn assert_process_mapping(
        process_error: ProcessError,
        expected_kind: TestErrorKind,
        assert_typed_error: impl FnOnce(AppError),
    ) {
        let (kind, app_error, interruption, status) =
            enterprise_error_kind(EnterpriseError::Spawn(process_error));

        assert_eq!(kind, Some(expected_kind));
        assert_typed_error(app_error);
        assert!(interruption.is_none());
        assert_eq!(status, ExecutionStatus::Failed);
    }

    #[test]
    fn enterprise_process_errors_keep_distinct_test_error_kinds() {
        assert_process_mapping(
            ProcessError::SpawnFailed {
                cmd: "1cv8c ENTERPRISE".to_owned(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "missing"),
            },
            TestErrorKind::EnterpriseSpawnFailed,
            |error| {
                assert!(matches!(
                    error,
                    AppError::PlatformProcess(ProcessError::SpawnFailed { .. })
                ));
            },
        );
        assert_process_mapping(
            ProcessError::StartupCheckFailed {
                cmd: "1cv8c ENTERPRISE".to_owned(),
                source: std::io::Error::other("probe failed"),
            },
            TestErrorKind::EnterpriseStartupCheckFailed,
            |error| {
                assert!(matches!(
                    error,
                    AppError::PlatformProcess(ProcessError::StartupCheckFailed { .. })
                ));
            },
        );
        assert_process_mapping(
            ProcessError::ExitedEarly {
                cmd: "1cv8c ENTERPRISE".to_owned(),
                exit_code: 17,
            },
            TestErrorKind::EnterpriseExitedEarly,
            |error| {
                assert!(matches!(
                    error,
                    AppError::PlatformProcess(ProcessError::ExitedEarly { .. })
                ));
            },
        );
        assert_process_mapping(
            ProcessError::StdoutLogIo {
                path: PathBuf::from("stdout.log"),
                source: std::io::Error::other("stdout write"),
            },
            TestErrorKind::EnterpriseStdoutLogIo,
            |error| {
                assert!(matches!(
                    error,
                    AppError::PlatformProcess(ProcessError::StdoutLogIo { .. })
                ));
            },
        );
        assert_process_mapping(
            ProcessError::StderrLogIo {
                path: PathBuf::from("stderr.log"),
                source: std::io::Error::other("stderr write"),
            },
            TestErrorKind::EnterpriseStderrLogIo,
            |error| {
                assert!(matches!(
                    error,
                    AppError::PlatformProcess(ProcessError::StderrLogIo { .. })
                ));
            },
        );
    }

    #[test]
    fn enterprise_timeout_keeps_interruption_contract() {
        let (kind, app_error, interruption, status) =
            enterprise_error_kind(EnterpriseError::Spawn(ProcessError::TimedOut {
                cmd: "1cv8c ENTERPRISE".to_owned(),
                timeout_ms: 500,
            }));

        assert_eq!(kind, None);
        assert!(matches!(app_error, AppError::Runtime(_)));
        assert!(interruption.is_some());
        assert_eq!(status, ExecutionStatus::TimedOut);
    }
}

pub(super) fn succeeded_step(
    name: &str,
    kind: ExecutionStepKind,
    duration_ms: u64,
    message: impl Into<String>,
) -> StepResult {
    StepResult::succeeded(name, kind, duration_ms).with_message(message)
}

pub(super) fn failed_step(
    name: &str,
    kind: ExecutionStepKind,
    duration_ms: u64,
    message: impl Into<String>,
) -> StepResult {
    let message = message.into();
    StepResult::failed(name, kind, duration_ms)
        .with_message(message.clone())
        .with_diagnostics(vec![message])
}

pub(super) fn degraded_step(
    name: &str,
    kind: ExecutionStepKind,
    duration_ms: u64,
    message: impl Into<String>,
) -> StepResult {
    let message = message.into();
    StepResult::degraded(name, kind, duration_ms)
        .with_message(message.clone())
        .with_diagnostics(vec![message])
}

pub(super) fn with_retained_artifacts(
    mut outcome: ExecutionOutcome<TestReport>,
    retained_paths: Option<ArtifactSet>,
) -> ExecutionOutcome<TestReport> {
    if let Some(retained_paths) = retained_paths {
        outcome = outcome.with_artifacts(retained_paths);
    }
    outcome
}

pub(super) fn interrupted_test_failure(
    context: &ExecutionContext,
    target: &TestTarget,
    mode: &TestOutputMode,
    warnings: &[String],
    steps: &[StepResult],
    started: Instant,
) -> Option<super::TestExecutionFailure> {
    let interruption = context.interruption()?;
    let message = interruption_message(context, interruption);
    let outcome = ExecutionOutcome::new(command_interruption_status(interruption))
        .with_diagnostics(vec![message.clone()])
        .with_interruptions(vec![command_interruption_details(
            interruption,
            "command_boundary",
            message.clone(),
        )]);
    let result = make_test_result(
        target.clone(),
        mode.clone(),
        outcome,
        warnings.to_vec(),
        steps.to_vec(),
        started.elapsed().as_millis() as u64,
    );
    Some(super::TestExecutionFailure::with_payload(
        AppError::Runtime(message),
        result,
    ))
}

pub(super) fn capped_timeout_ms(
    timeout_override_ms: Option<u64>,
    context: &ExecutionContext,
) -> Option<u64> {
    let remaining_budget_ms = context.remaining_budget().map(|duration| {
        u64::try_from(duration.as_millis())
            .unwrap_or(u64::MAX)
            .max(1)
    });
    match (timeout_override_ms, remaining_budget_ms) {
        (Some(timeout_ms), Some(remaining_ms)) => Some(timeout_ms.min(remaining_ms)),
        (Some(timeout_ms), None) => Some(timeout_ms),
        (None, Some(remaining_ms)) => Some(remaining_ms),
        (None, None) => None,
    }
}

pub(super) fn interruption_message(
    context: &ExecutionContext,
    interruption: ExecutionInterruption,
) -> String {
    command_interruption_message(context, interruption)
}

pub(super) fn validate_runner_profile_id(profile_id: &str) -> Result<&str, AppError> {
    if !is_safe_path_segment(profile_id) {
        return Err(AppError::Validation(format!(
            "runner profile contains unsafe path characters: {profile_id}"
        )));
    }
    Ok(profile_id)
}

pub(super) fn build_summary(result: &crate::domain::build::BuildResult) -> String {
    if result.ok {
        "build completed".to_owned()
    } else {
        result
            .steps
            .iter()
            .find(|step| !step.ok)
            .map(|step| {
                format!(
                    "build failed at source-set '{}' ({})",
                    step.source_set,
                    step.message.as_deref().unwrap_or("unknown error")
                )
            })
            .unwrap_or_else(|| "build failed".to_owned())
    }
}

pub(super) fn prepared_run_summary(prepared_run: &PreparedRun) -> String {
    match prepared_run {
        PreparedRun::YaXUnit => "YaXUnit config written".to_owned(),
        PreparedRun::Vanessa { .. } => "Vanessa Automation params written".to_owned(),
    }
}

pub(super) fn validate_target(
    runner_kind: &RunnerKind,
    scope: &TestScope,
) -> Result<TestTarget, AppError> {
    match scope {
        TestScope::All => Ok(TestTarget::All),
        TestScope::Module { name } => {
            if *runner_kind == RunnerKind::Vanessa {
                return Err(AppError::Validation(
                    "Vanessa Automation supports only 'test va' without module scope".to_owned(),
                ));
            }
            let trimmed = name.trim();
            if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
                return Err(AppError::Validation(
                    "test module requires a non-empty module name".to_owned(),
                ));
            }
            Ok(TestTarget::Module {
                name: trimmed.to_owned(),
            })
        }
    }
}

pub(super) fn prepare_runner_artifacts(
    config: &AppConfig,
    args: &TestArgs,
    target: &TestTarget,
    artifacts: &mut RunArtifacts,
) -> Result<PreparedRun, AppError> {
    match args.execution.profile.kind {
        RunnerKind::YaXUnit => {
            tracing::debug!(
                path = %artifacts.config_json.display(),
                "writing YaXUnit configuration"
            );
            let config_payload = build_yaxunit_config(target, artifacts);
            super::write_json_file(&artifacts.config_json, &config_payload).map_err(|error| {
                AppError::Runtime(format!("failed to write YaXUnit config: {error}"))
            })?;
            Ok(PreparedRun::YaXUnit)
        }
        RunnerKind::Vanessa => prepare_vanessa_run(config, args, artifacts),
        ref other => Err(AppError::Validation(format!(
            "unsupported test runner kind: {other:?}"
        ))),
    }
}

pub(super) fn build_enterprise_dsl<'a>(
    context: &ExecutionContext,
    config: &AppConfig,
    artifacts: &'a RunArtifacts,
    prepared_run: &PreparedRun,
    launch: &LaunchOptions,
    runner: &'a dyn crate::platform::process::ProcessRunner,
    client_mode: LaunchClientModeRequest,
    timeout_override_ms: Option<u64>,
) -> Result<EnterpriseDsl<'a>, AppError> {
    let mut utilities = PlatformUtilities::from_config(config);
    let utility = match client_mode {
        LaunchClientModeRequest::Designer => UtilityType::V8,
        LaunchClientModeRequest::Thin => UtilityType::V8C,
        LaunchClientModeRequest::Thick | LaunchClientModeRequest::Ordinary => UtilityType::V8,
    };
    let location = utilities.locate(utility).map_err(AppError::from)?;
    tracing::debug!(
        additional_launch_keys = ?config.tools.enterprise.additional_launch_keys,
        "resolved enterprise additional launch keys"
    );
    let additional_launch_keys = effective_enterprise_launch_keys(config, prepared_run, launch);
    Ok(EnterpriseDsl::new(
        location.path,
        config.v8_connection(),
        additional_launch_keys,
        client_mode.into(),
        runner,
        artifacts.platform_log.clone(),
        timeout_override_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(config.tests.execution_timeout_seconds)),
    )
    .with_execution_policy(
        context.process_policy(
            InterruptionSafetyClass::GracefulThenKill,
            Some(
                timeout_override_ms
                    .map(Duration::from_millis)
                    .unwrap_or_else(|| Duration::from_secs(config.tests.execution_timeout_seconds)),
            ),
        ),
    ))
}

fn effective_enterprise_launch_keys(
    config: &AppConfig,
    prepared_run: &PreparedRun,
    launch: &LaunchOptions,
) -> Vec<String> {
    if matches!(prepared_run, PreparedRun::Vanessa { .. }) {
        return vanessa_enterprise_launch_keys(
            &config.tools.enterprise.additional_launch_keys,
            launch,
        );
    }
    config.tools.enterprise.additional_launch_keys.clone()
}

pub(super) fn build_platform_launch(
    base: &LaunchOptions,
    prepared_run: &PreparedRun,
    artifacts: &RunArtifacts,
) -> LaunchOptions {
    let mut launch = base.clone();
    match prepared_run {
        PreparedRun::YaXUnit => {
            launch.c = Some(format!(
                "RunUnitTests={}",
                crate::platform::enterprise::normalize_launch_payload_path(&artifacts.config_json)
            ));
            launch.execute = None;
        }
        PreparedRun::Vanessa {
            epf_path,
            params_path,
        } => {
            launch = crate::use_cases::vanessa::apply_test_player_launch(
                base,
                &crate::use_cases::vanessa::VanessaLaunch {
                    epf_path: epf_path.clone(),
                    params_path: params_path.clone(),
                },
            );
        }
    }
    launch
}

/// Appends the `mcpMode=ws;...` snippet to an existing `/C` payload, joining
/// with `;` and preserving the leading payload (e.g. `RunUnitTests=...` or
/// the Vanessa player payload). When the `/C` value is absent, the snippet
/// becomes the entire `/C`.
pub(super) fn append_mcp_ws_snippet(launch: &mut LaunchOptions, snippet: &str) {
    let combined = match launch.c.take() {
        Some(existing) if !existing.is_empty() => format!("{existing};{snippet}"),
        _ => snippet.to_owned(),
    };
    launch.c = Some(combined);
}

/// If the WS-mode resolution succeeds for this test run, append the
/// `mcpMode=ws;...` snippet to the platform `/C` so the BSL devkit registers
/// with `v8-client-session-manager` instead of starting a local HTTP MCP.
///
/// Errors from explicit WS resolution are returned to the caller. Auto mode
/// still falls back to the regular `/C` payload when the manager is down.
pub(super) fn apply_test_mcp_ws_payload(
    config: &AppConfig,
    mcp_ws: &crate::use_cases::request::McpClientWsRequest,
    prepared_run: &PreparedRun,
    launch: &mut LaunchOptions,
) -> Result<(), AppError> {
    let kind = match prepared_run {
        PreparedRun::YaXUnit => crate::use_cases::mcp_ws::ClientKind::YaxunitRunner,
        PreparedRun::Vanessa { .. } => crate::use_cases::mcp_ws::ClientKind::VanessaTestClient,
    };
    let decision = crate::use_cases::launch_app::decide_mcp_transport(config, mcp_ws)?;
    if !matches!(decision, crate::use_cases::mcp_ws::TransportDecision::Ws) {
        return Ok(());
    }
    let params = crate::use_cases::launch_app::resolve_ws_launch_params(config, mcp_ws, kind);
    append_mcp_ws_snippet(launch, &params.payload_snippet());
    Ok(())
}

pub(super) fn collect_diagnostics(
    platform_result: &crate::platform::result::PlatformCommandResult,
    mut diagnostics: Vec<String>,
    config: &AppConfig,
) -> Vec<String> {
    if !platform_result.process.stderr.trim().is_empty() {
        diagnostics.push(super::sanitize_text(
            &platform_result.process.stderr,
            config,
        ));
    }
    if let Some(log) = &platform_result.platform_log {
        let trimmed = log.trim();
        if !trimmed.is_empty() {
            diagnostics.push(super::limit_excerpt(&super::sanitize_text(trimmed, config)));
        }
    }
    diagnostics
}

pub(super) fn enterprise_error_kind(
    error: EnterpriseError,
) -> (
    Option<TestErrorKind>,
    AppError,
    Option<ExecutionInterruptionDetails>,
    ExecutionStatus,
) {
    match error {
        EnterpriseError::Spawn(ProcessError::Cancelled { .. }) => (
            None,
            AppError::Runtime("enterprise test run cancelled".to_owned()),
            Some(process_interruption_details(
                ProcessInterruptionReason::Cancelled,
                "run",
                false,
                "enterprise test run cancelled",
            )),
            ExecutionStatus::Cancelled,
        ),
        EnterpriseError::Spawn(ProcessError::TimedOut { .. }) => (
            None,
            AppError::Runtime("enterprise test run timed out".to_owned()),
            Some(process_interruption_details(
                ProcessInterruptionReason::TimedOut,
                "run",
                false,
                "enterprise test run timed out",
            )),
            ExecutionStatus::TimedOut,
        ),
        EnterpriseError::Spawn(process_error @ ProcessError::SpawnFailed { .. }) => (
            Some(TestErrorKind::EnterpriseSpawnFailed),
            AppError::PlatformProcess(process_error),
            None,
            ExecutionStatus::Failed,
        ),
        EnterpriseError::Spawn(process_error @ ProcessError::StartupCheckFailed { .. }) => (
            Some(TestErrorKind::EnterpriseStartupCheckFailed),
            AppError::PlatformProcess(process_error),
            None,
            ExecutionStatus::Failed,
        ),
        EnterpriseError::Spawn(process_error @ ProcessError::ExitedEarly { .. }) => (
            Some(TestErrorKind::EnterpriseExitedEarly),
            AppError::PlatformProcess(process_error),
            None,
            ExecutionStatus::Failed,
        ),
        EnterpriseError::Spawn(process_error @ ProcessError::StdoutLogIo { .. }) => (
            Some(TestErrorKind::EnterpriseStdoutLogIo),
            AppError::PlatformProcess(process_error),
            None,
            ExecutionStatus::Failed,
        ),
        EnterpriseError::Spawn(process_error @ ProcessError::StderrLogIo { .. }) => (
            Some(TestErrorKind::EnterpriseStderrLogIo),
            AppError::PlatformProcess(process_error),
            None,
            ExecutionStatus::Failed,
        ),
    }
}

#[cfg(test)]
mod append_ws_tests {
    use super::append_mcp_ws_snippet;
    use crate::domain::runner::LaunchOptions;

    #[test]
    fn append_appends_after_existing_payload() {
        let mut launch = LaunchOptions {
            c: Some("RunUnitTests=/tmp/cfg.json".to_owned()),
            ..Default::default()
        };
        append_mcp_ws_snippet(&mut launch, "mcpMode=ws;manager_url=ws://m:1/s");
        assert_eq!(
            launch.c.as_deref(),
            Some("RunUnitTests=/tmp/cfg.json;mcpMode=ws;manager_url=ws://m:1/s")
        );
    }

    #[test]
    fn append_uses_snippet_alone_when_c_missing() {
        let mut launch = LaunchOptions::default();
        append_mcp_ws_snippet(&mut launch, "mcpMode=ws");
        assert_eq!(launch.c.as_deref(), Some("mcpMode=ws"));
    }

    #[test]
    fn append_replaces_empty_c() {
        let mut launch = LaunchOptions {
            c: Some(String::new()),
            ..Default::default()
        };
        append_mcp_ws_snippet(&mut launch, "mcpMode=ws");
        assert_eq!(launch.c.as_deref(), Some("mcpMode=ws"));
    }
}
