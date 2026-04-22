use std::time::{Duration, Instant};

use crate::config::model::AppConfig;
use crate::domain::artifact::ArtifactSet;
use crate::domain::execution::{
    ExecutionInterruptionDetails, ExecutionInterruptionKind, ExecutionOutcome, ExecutionStatus,
    ExecutionStepKind, StepResult,
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

pub(super) fn command_interruption_status(interruption: ExecutionInterruption) -> ExecutionStatus {
    match interruption {
        ExecutionInterruption::Cancelled => ExecutionStatus::Cancelled,
        ExecutionInterruption::TimedOut => ExecutionStatus::TimedOut,
    }
}

pub(super) fn command_interruption_details(
    interruption: ExecutionInterruption,
    phase: &str,
    message: impl Into<String>,
) -> ExecutionInterruptionDetails {
    ExecutionInterruptionDetails::new(
        match interruption {
            ExecutionInterruption::Cancelled => ExecutionInterruptionKind::Cancelled,
            ExecutionInterruption::TimedOut => ExecutionInterruptionKind::TimedOut,
        },
        false,
    )
    .with_phase(phase)
    .with_message(message)
}

pub(super) fn process_interruption_details(
    interruption: ProcessInterruptionReason,
    phase: &str,
    deferred: bool,
    message: impl Into<String>,
) -> ExecutionInterruptionDetails {
    ExecutionInterruptionDetails::new(
        match interruption {
            ProcessInterruptionReason::Cancelled => ExecutionInterruptionKind::Cancelled,
            ProcessInterruptionReason::TimedOut => ExecutionInterruptionKind::TimedOut,
        },
        deferred,
    )
    .with_phase(phase)
    .with_message(message)
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
    format!(
        "{} for command '{}'",
        interruption.message(context.command()),
        context.command().as_str()
    )
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
    let location = utilities
        .locate(utility)
        .map_err(|error| AppError::Platform(error.to_string()))?;
    tracing::debug!(
        additional_launch_keys = ?config.tools.enterprise.additional_launch_keys,
        "resolved enterprise additional launch keys"
    );
    Ok(EnterpriseDsl::new(
        location.path,
        config.v8_connection(),
        config.tools.enterprise.additional_launch_keys.clone(),
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
            launch.execute = Some(crate::platform::enterprise::normalize_launch_payload_path(
                epf_path,
            ));
            launch.c = Some(format!(
                "StartFeaturePlayer;VAParams={}",
                crate::platform::enterprise::normalize_launch_payload_path(params_path)
            ));
        }
    }
    launch
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
        EnterpriseError::Spawn(process_error @ ProcessError::SpawnFailed { .. })
        | EnterpriseError::Spawn(process_error @ ProcessError::ExitedEarly { .. }) => (
            Some(TestErrorKind::EnterpriseSpawnFailed),
            AppError::Platform(process_error.to_string()),
            None,
            ExecutionStatus::Failed,
        ),
        EnterpriseError::Spawn(process_error) => (
            Some(TestErrorKind::EnterpriseSpawnFailed),
            AppError::Platform(process_error.to_string()),
            None,
            ExecutionStatus::Failed,
        ),
    }
}
