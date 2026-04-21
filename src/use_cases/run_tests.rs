use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::config::model::{AppConfig, VanessaProfileConfig};
use crate::domain::artifact::ArtifactSet;
use crate::domain::execution::{ExecutionMetrics, ExecutionOutcome, ExecutionStatus, StepResult};
use crate::domain::runner::{LaunchClientModeRequest, LaunchOptions, RunnerKind};
use crate::domain::test::{
    test_execution_error, test_execution_status, TestErrorKind, TestOutputMode, TestReport,
    TestRunResult, TestStatus, TestTarget,
};
use crate::parsers::junit;
use crate::parsers::vanessa_log;
use crate::parsers::yaxunit_log;
use crate::platform::enterprise::EnterpriseDsl;
use crate::platform::enterprise::EnterpriseError;
use crate::platform::locator::UtilityType;
use crate::platform::process::ProcessError;
use crate::platform::utilities::PlatformUtilities;
use crate::support::error::AppError;
use crate::support::path::is_safe_path_segment;
use crate::use_cases::build_project;
use crate::use_cases::context::ExecutionContext;
use crate::use_cases::request::{
    BuildRequest as BuildArgs, TestRequest as TestArgs, TestScopeRequest as TestScope,
};
use crate::use_cases::result::{UseCaseFailure, UseCaseResult};
use tracing::debug;

const STACK_TRACE_LIMIT: usize = 500;

pub fn execute(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &TestArgs,
) -> UseCaseResult<TestRunResult> {
    debug!(
        command = context.command().as_str(),
        transport = ?context.transport(),
        "executing test use case"
    );
    run_tests(config, args)
}

#[derive(Debug, Serialize)]
struct YaXUnitConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    filter: Option<YaXUnitFilter>,
    #[serde(rename = "reportFormat")]
    report_format: &'static str,
    #[serde(rename = "reportPath")]
    report_path: String,
    #[serde(rename = "closeAfterTests")]
    close_after_tests: bool,
    #[serde(rename = "showReport")]
    show_report: bool,
    logging: YaXUnitLogging,
}

#[derive(Debug, Serialize)]
struct YaXUnitFilter {
    modules: Vec<String>,
}

#[derive(Debug, Serialize)]
struct YaXUnitLogging {
    file: String,
    console: bool,
    level: &'static str,
}

#[derive(Debug)]
struct RunArtifacts {
    run_dir: PathBuf,
    config_json: PathBuf,
    junit_xml: PathBuf,
    junit_dir: PathBuf,
    runner_log: PathBuf,
    platform_log: PathBuf,
    sentinel: PathBuf,
}

enum PreparedRun {
    YaXUnit,
    Vanessa {
        epf_path: PathBuf,
        params_path: PathBuf,
    },
}

type TestExecutionFailure = UseCaseFailure<TestRunResult>;

fn make_test_result(
    target: TestTarget,
    mode: TestOutputMode,
    outcome: ExecutionOutcome<TestReport>,
    warnings: Vec<String>,
    steps: Vec<StepResult>,
    duration_ms: u64,
) -> TestRunResult {
    TestRunResult::from_outcome(outcome, target, mode, warnings, steps, duration_ms)
}

fn run_tests(config: &AppConfig, args: &TestArgs) -> UseCaseResult<TestRunResult> {
    let started = Instant::now();
    let runner_kind = args.execution.profile.kind.clone();
    debug!(
        full = args.full,
        scope = ?args.scope,
        runner = ?runner_kind,
        "starting test run"
    );
    let mode = if args.full {
        TestOutputMode::Full
    } else {
        TestOutputMode::Compact
    };
    let target = match validate_target(&runner_kind, &args.scope) {
        Ok(target) => target,
        Err(error) => {
            let outcome = ExecutionOutcome::new(ExecutionStatus::Failed)
                .with_diagnostics(vec![error.to_string()]);
            let result = make_test_result(
                TestTarget::All,
                mode,
                outcome,
                Vec::new(),
                Vec::new(),
                started.elapsed().as_millis() as u64,
            );
            return Err(TestExecutionFailure::with_payload(error, result));
        }
    };

    let mut steps = Vec::new();
    let mut warnings = Vec::new();
    let runner_id = match validate_runner_profile_id(&args.execution.profile.id) {
        Ok(runner_id) => runner_id,
        Err(error) => {
            let outcome = ExecutionOutcome::new(ExecutionStatus::Failed)
                .with_diagnostics(vec![error.to_string()])
                .with_errors(vec![test_execution_error(
                    TestErrorKind::EnterpriseSpawnFailed,
                    error.to_string(),
                )]);
            let result = make_test_result(
                target,
                mode,
                outcome,
                warnings,
                steps,
                started.elapsed().as_millis() as u64,
            );
            return Err(TestExecutionFailure::with_payload(error, result));
        }
    };

    debug!("running build prerequisite for tests");
    let build_started = Instant::now();
    let build_result = match build_project::run_build_unlocked(
        config,
        &BuildArgs {
            full_rebuild: false,
        },
    ) {
        Ok(result) => result,
        Err(failure) => {
            let summary = failure
                .payload
                .as_ref()
                .map(build_summary)
                .unwrap_or_else(|| failure.error.to_string());
            steps.push(StepResult {
                name: "build".to_owned(),
                ok: false,
                duration_ms: build_started.elapsed().as_millis() as u64,
                message: Some(summary.clone()),
            });
            let outcome = ExecutionOutcome::new(ExecutionStatus::Failed)
                .with_diagnostics(vec![summary.clone()])
                .with_errors(vec![test_execution_error(
                    TestErrorKind::BuildFailed,
                    summary.clone(),
                )]);
            let result = make_test_result(
                target,
                mode,
                outcome,
                warnings,
                steps,
                started.elapsed().as_millis() as u64,
            );
            return Err(TestExecutionFailure::with_payload(failure.error, result));
        }
    };
    steps.push(StepResult {
        name: "build".to_owned(),
        ok: true,
        duration_ms: build_started.elapsed().as_millis() as u64,
        message: Some(build_summary(&build_result)),
    });

    debug!("preparing test run artifacts");
    let prepare_artifacts_started = Instant::now();
    let mut artifacts = match create_run_artifacts(config, &runner_id) {
        Ok(artifacts) => artifacts,
        Err(error) => {
            let app_error =
                AppError::Runtime(format!("failed to prepare test run directory: {error}"));
            steps.push(StepResult {
                name: "prepare_artifacts".to_owned(),
                ok: false,
                duration_ms: prepare_artifacts_started.elapsed().as_millis() as u64,
                message: Some(app_error.to_string()),
            });
            let outcome = ExecutionOutcome::new(ExecutionStatus::Failed)
                .with_diagnostics(vec![app_error.to_string()])
                .with_errors(vec![test_execution_error(
                    TestErrorKind::EnterpriseSpawnFailed,
                    app_error.to_string(),
                )]);
            let result = make_test_result(
                target,
                mode,
                outcome,
                warnings,
                steps,
                started.elapsed().as_millis() as u64,
            );
            return Err(TestExecutionFailure::with_payload(app_error, result));
        }
    };
    steps.push(StepResult {
        name: "prepare_artifacts".to_owned(),
        ok: true,
        duration_ms: prepare_artifacts_started.elapsed().as_millis() as u64,
        message: Some(format!("created {}", artifacts.run_dir.display())),
    });

    let prepare_runner_started = Instant::now();
    let prepared_run = match prepare_runner_artifacts(config, args, &target, &mut artifacts) {
        Ok(prepared_run) => {
            steps.push(StepResult {
                name: "prepare_runner".to_owned(),
                ok: true,
                duration_ms: prepare_runner_started.elapsed().as_millis() as u64,
                message: Some(prepared_run_summary(&prepared_run)),
            });
            prepared_run
        }
        Err(error) => {
            steps.push(StepResult {
                name: "prepare_runner".to_owned(),
                ok: false,
                duration_ms: prepare_runner_started.elapsed().as_millis() as u64,
                message: Some(error.to_string()),
            });
            let retained_paths = retain_run_artifacts(config, &artifacts).ok();
            let mut outcome = ExecutionOutcome::new(ExecutionStatus::Failed)
                .with_diagnostics(vec![error.to_string()])
                .with_errors(vec![test_execution_error(
                    TestErrorKind::EnterpriseSpawnFailed,
                    error.to_string(),
                )]);
            if let Some(retained_paths) = retained_paths {
                outcome = outcome.with_artifacts(retained_paths);
            }
            let result = make_test_result(
                target.clone(),
                mode,
                outcome,
                warnings,
                steps,
                started.elapsed().as_millis() as u64,
            );
            return Err(TestExecutionFailure::with_payload(error, result));
        }
    };

    debug!(path = %artifacts.run_dir.display(), "launching enterprise test run");
    let run_started = Instant::now();
    let enterprise_runner = crate::platform::process::ProcessExecutor;
    let enterprise = match build_enterprise_dsl(
        config,
        &artifacts,
        &enterprise_runner,
        args.execution
            .client_mode
            .unwrap_or(LaunchClientModeRequest::Thin),
        args.execution.timeouts.total_ms,
    ) {
        Ok(dsl) => dsl,
        Err(error) => {
            steps.push(StepResult {
                name: "run".to_owned(),
                ok: false,
                duration_ms: run_started.elapsed().as_millis() as u64,
                message: Some(error.to_string()),
            });
            let retained_paths = retain_run_artifacts(config, &artifacts).ok();
            let mut outcome = ExecutionOutcome::new(ExecutionStatus::Failed)
                .with_diagnostics(vec![error.to_string()])
                .with_errors(vec![test_execution_error(
                    TestErrorKind::EnterpriseSpawnFailed,
                    error.to_string(),
                )]);
            if let Some(retained_paths) = retained_paths {
                outcome = outcome.with_artifacts(retained_paths);
            }
            let result = make_test_result(
                target,
                mode,
                outcome,
                warnings,
                steps,
                started.elapsed().as_millis() as u64,
            );
            return Err(TestExecutionFailure::with_payload(error, result));
        }
    };

    let platform_launch = build_platform_launch(&args.execution.launch, &prepared_run, &artifacts);

    let platform_result = match enterprise.run_launch(&platform_launch) {
        Ok(result) => {
            steps.push(StepResult {
                name: "run".to_owned(),
                ok: result.process.exit_code == 0,
                duration_ms: run_started.elapsed().as_millis() as u64,
                message: Some(format!("enterprise exit code {}", result.process.exit_code)),
            });
            result
        }
        Err(error) => {
            let (kind, app_error) = enterprise_error_kind(error);
            steps.push(StepResult {
                name: "run".to_owned(),
                ok: false,
                duration_ms: run_started.elapsed().as_millis() as u64,
                message: Some(app_error.to_string()),
            });
            let retained_paths = retain_run_artifacts(config, &artifacts).ok();
            let mut outcome =
                ExecutionOutcome::new(test_execution_status(Some(kind.clone()), false))
                    .with_diagnostics(vec![app_error.to_string()])
                    .with_errors(vec![test_execution_error(kind, app_error.to_string())]);
            if let Some(retained_paths) = retained_paths {
                outcome = outcome.with_artifacts(retained_paths);
            }
            let result = make_test_result(
                target,
                mode,
                outcome,
                warnings,
                steps,
                started.elapsed().as_millis() as u64,
            );
            return Err(TestExecutionFailure::with_payload(app_error, result));
        }
    };

    if matches!(prepared_run, PreparedRun::Vanessa { .. }) {
        resolve_vanessa_junit_path(&mut artifacts);
        if let Err(warning) = materialize_vanessa_runner_log(&artifacts) {
            warnings.push(warning);
        }
    }

    debug!(path = %artifacts.junit_xml.display(), "parsing JUnit report");
    let parse_junit_started = Instant::now();
    let junit_parse = parse_junit_report(&artifacts);
    let mut report = match junit_parse.payload {
        Some(report) => {
            steps.push(StepResult {
                name: "parse_junit".to_owned(),
                ok: true,
                duration_ms: parse_junit_started.elapsed().as_millis() as u64,
                message: Some(format!("parsed {} test cases", report.summary.total)),
            });
            report
        }
        None => {
            let error = junit_parse
                .errors
                .first()
                .cloned()
                .expect("junit parse error");
            let kind =
                TestErrorKind::from_code(&error.code).unwrap_or(TestErrorKind::JunitMalformed);
            let message = error.message.clone();
            steps.push(StepResult {
                name: "parse_junit".to_owned(),
                ok: false,
                duration_ms: parse_junit_started.elapsed().as_millis() as u64,
                message: Some(message.clone()),
            });
            let retained_paths = retain_run_artifacts(config, &artifacts).ok();
            let diagnostics = collect_diagnostics(&platform_result, vec![message.clone()], config);
            let mut outcome =
                ExecutionOutcome::new(test_execution_status(Some(kind.clone()), false))
                    .with_diagnostics(diagnostics)
                    .with_errors(vec![error.with_details(junit_parse.diagnostics)]);
            if let Some(retained_paths) = retained_paths {
                outcome = outcome.with_artifacts(retained_paths);
            }
            let result = make_test_result(
                target,
                mode,
                outcome,
                warnings,
                steps,
                started.elapsed().as_millis() as u64,
            );
            return Err(TestExecutionFailure::with_payload(
                AppError::Runtime(message),
                result,
            ));
        }
    };

    parse_runner_log(
        &prepared_run,
        &artifacts.runner_log,
        &mut report,
        &mut warnings,
        &mut steps,
    );

    let rendered_report = match mode {
        TestOutputMode::Full => report.clone(),
        TestOutputMode::Compact => compact_report(&report),
    };

    let has_test_failures = report.summary.failed > 0 || report.summary.errors > 0;
    let process_failed = platform_result.process.exit_code != 0;
    let diagnostics = collect_diagnostics(&platform_result, Vec::new(), config);

    if process_failed || has_test_failures {
        debug!(
            process_failed,
            has_test_failures, "retaining failed test artifacts"
        );
        let retained_paths = retain_run_artifacts(config, &artifacts).ok();
        let kind = if process_failed {
            TestErrorKind::EnterpriseExitedNonZero
        } else {
            TestErrorKind::TestFailures
        };
        let mut outcome = ExecutionOutcome::new(test_execution_status(Some(kind.clone()), false))
            .with_diagnostics(diagnostics)
            .with_errors(vec![test_execution_error(
                kind,
                if process_failed {
                    format!(
                        "enterprise test run exited with code {}",
                        platform_result.process.exit_code
                    )
                } else {
                    "test run reported failures".to_owned()
                },
            )])
            .with_metrics(ExecutionMetrics::from(&report.summary))
            .with_payload(rendered_report);
        if let Some(retained_paths) = retained_paths {
            outcome = outcome.with_artifacts(retained_paths);
        }
        let result = make_test_result(
            target,
            mode,
            outcome,
            warnings,
            steps,
            started.elapsed().as_millis() as u64,
        );
        return Err(TestExecutionFailure::with_payload(
            AppError::Runtime(if process_failed {
                format!(
                    "enterprise test run exited with code {}",
                    platform_result.process.exit_code
                )
            } else {
                "test run reported failures".to_owned()
            }),
            result,
        ));
    }

    debug!(path = %artifacts.run_dir.display(), "cleaning successful test run directory");
    cleanup_run_dir(&artifacts);
    Ok(make_test_result(
        target,
        mode,
        ExecutionOutcome::new(ExecutionStatus::Succeeded)
            .with_diagnostics(diagnostics)
            .with_metrics(ExecutionMetrics::from(&report.summary))
            .with_payload(rendered_report),
        warnings,
        steps,
        started.elapsed().as_millis() as u64,
    ))
}

fn validate_runner_profile_id(profile_id: &str) -> Result<&str, AppError> {
    if !is_safe_path_segment(profile_id) {
        return Err(AppError::Validation(format!(
            "runner profile contains unsafe path characters: {profile_id}"
        )));
    }
    Ok(profile_id)
}

fn build_summary(result: &crate::domain::build::BuildResult) -> String {
    if result.ok {
        "build completed".to_owned()
    } else {
        let failed = result
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
            .unwrap_or_else(|| "build failed".to_owned());
        failed
    }
}

fn prepared_run_summary(prepared_run: &PreparedRun) -> String {
    match prepared_run {
        PreparedRun::YaXUnit => "YaXUnit config written".to_owned(),
        PreparedRun::Vanessa { .. } => "Vanessa Automation params written".to_owned(),
    }
}

fn validate_target(runner_kind: &RunnerKind, scope: &TestScope) -> Result<TestTarget, AppError> {
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

fn prepare_runner_artifacts(
    config: &AppConfig,
    args: &TestArgs,
    target: &TestTarget,
    artifacts: &mut RunArtifacts,
) -> Result<PreparedRun, AppError> {
    match args.execution.profile.kind {
        RunnerKind::YaXUnit => {
            debug!(path = %artifacts.config_json.display(), "writing YaXUnit configuration");
            let config_payload = build_yaxunit_config(target, artifacts);
            write_json_file(&artifacts.config_json, &config_payload).map_err(|error| {
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

fn build_yaxunit_config(target: &TestTarget, artifacts: &RunArtifacts) -> YaXUnitConfig {
    YaXUnitConfig {
        filter: match target {
            TestTarget::All => None,
            TestTarget::Module { name } => Some(YaXUnitFilter {
                modules: vec![name.clone()],
            }),
        },
        report_format: "jUnit",
        report_path: artifacts.junit_xml.display().to_string(),
        close_after_tests: true,
        show_report: false,
        logging: YaXUnitLogging {
            file: artifacts.runner_log.display().to_string(),
            console: false,
            level: "info",
        },
    }
}

fn build_enterprise_dsl<'a>(
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
    debug!(
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
    ))
}

fn create_run_artifacts(config: &AppConfig, runner_id: &str) -> std::io::Result<RunArtifacts> {
    let run_id = format!(
        "{}-{}-{}",
        chrono::Utc::now().timestamp_millis(),
        std::process::id(),
        Uuid::new_v4().simple()
    );
    let run_dir = config
        .work_path
        .join("temp")
        .join(runner_id)
        .join("runs")
        .join(&run_id);
    debug!(path = %run_dir.display(), "creating test artifact directory");
    fs::create_dir_all(&run_dir)?;
    set_dir_permissions(&run_dir)?;

    let sentinel = run_dir.join("run.inprogress");
    fs::write(&sentinel, &run_id)?;
    set_file_permissions(&sentinel)?;

    let artifacts = RunArtifacts {
        run_dir: run_dir.clone(),
        config_json: run_dir.join("config.json"),
        junit_xml: run_dir.join("report.xml"),
        junit_dir: run_dir.join("junit"),
        runner_log: run_dir.join("runner.log"),
        platform_log: run_dir.join("enterprise.out.log"),
        sentinel,
    };
    Ok(artifacts)
}

fn write_json_file(path: &Path, payload: &impl Serialize) -> std::io::Result<()> {
    fs::write(path, serde_json::to_vec_pretty(payload)?)?;
    set_file_permissions(path)
}

fn prepare_vanessa_run(
    config: &AppConfig,
    args: &TestArgs,
    artifacts: &mut RunArtifacts,
) -> Result<PreparedRun, AppError> {
    let va = &config.tests.va;
    let epf_path = va
        .epf_path
        .clone()
        .ok_or_else(|| AppError::Validation("tests.va.epf_path is not configured".to_owned()))?;
    let params_path = va
        .params_path
        .as_ref()
        .ok_or_else(|| AppError::Validation("tests.va.params_path is not configured".to_owned()))?;
    let profile_name = args.execution.profile.id.as_str();
    if !is_safe_path_segment(profile_name) {
        return Err(AppError::Validation(format!(
            "tests.va.profile contains unsafe path characters: {profile_name}"
        )));
    }
    let profile = va.profiles.get(profile_name).ok_or_else(|| {
        AppError::Validation(format!(
            "unknown Vanessa Automation profile '{profile_name}'"
        ))
    })?;

    fs::create_dir_all(&artifacts.junit_dir)
        .map_err(|error| AppError::Runtime(format!("failed to create JUnit directory: {error}")))?;

    let runtime_params_path = artifacts.run_dir.join("va-params.json");
    let base = fs::read_to_string(params_path).map_err(|error| {
        AppError::Runtime(format!("failed to read Vanessa params template: {error}"))
    })?;
    let mut payload: Value = serde_json::from_str(&base).map_err(|error| {
        AppError::Runtime(format!("failed to parse Vanessa params JSON: {error}"))
    })?;
    let object = payload
        .as_object_mut()
        .ok_or_else(|| AppError::Runtime("Vanessa params JSON must be an object".to_owned()))?;
    apply_vanessa_overlay(object, profile, va.fail_fast, artifacts);
    write_json_file(&runtime_params_path, &payload)
        .map_err(|error| AppError::Runtime(format!("failed to write Vanessa params: {error}")))?;
    artifacts.config_json = runtime_params_path.clone();

    Ok(PreparedRun::Vanessa {
        epf_path,
        params_path: runtime_params_path,
    })
}

fn materialize_vanessa_runner_log(artifacts: &RunArtifacts) -> Result<(), String> {
    fs::copy(&artifacts.platform_log, &artifacts.runner_log).map_err(|error| {
        format!("failed to materialize Vanessa runner log from enterprise output: {error}")
    })?;
    set_file_permissions(&artifacts.runner_log)
        .map_err(|error| format!("failed to chmod Vanessa runner log: {error}"))
}

fn build_platform_launch(
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

fn apply_vanessa_overlay(
    object: &mut Map<String, Value>,
    profile: &VanessaProfileConfig,
    fail_fast: bool,
    artifacts: &RunArtifacts,
) {
    object.insert("stoponerror".to_owned(), Value::Bool(fail_fast));
    object.insert("junitcreatereport".to_owned(), Value::Bool(true));
    object.insert(
        "junitpath".to_owned(),
        Value::String(artifacts.junit_dir.display().to_string()),
    );

    if let Some(feature_path) = profile.feature_path.as_ref() {
        object.insert(
            "featurepath".to_owned(),
            Value::String(feature_path.display().to_string()),
        );
    }
    insert_string_array_if_non_empty(object, "FeaturesToRun", &profile.features_to_run);
    insert_string_array_if_non_empty(object, "filtertags", &profile.filter_tags);
    insert_string_array_if_non_empty(object, "ignoretags", &profile.ignore_tags);
    insert_string_array_if_non_empty(object, "scenariofilter", &profile.scenario_filter);
}

fn insert_string_array_if_non_empty(object: &mut Map<String, Value>, key: &str, values: &[String]) {
    if values.is_empty() {
        object.remove(key);
        return;
    }

    object.insert(
        key.to_owned(),
        Value::Array(values.iter().cloned().map(Value::String).collect()),
    );
}

fn parse_runner_log(
    prepared_run: &PreparedRun,
    runner_log_path: &Path,
    report: &mut TestReport,
    warnings: &mut Vec<String>,
    steps: &mut Vec<StepResult>,
) {
    let parse_log_started = Instant::now();
    match prepared_run {
        PreparedRun::YaXUnit => match yaxunit_log::normalize_file(runner_log_path) {
            Ok(parsed) => {
                if let Some(errors) = parsed.payload {
                    report.extracted_errors = errors;
                }
                warnings.extend(parsed.warnings);
                steps.push(StepResult {
                    name: "parse_log".to_owned(),
                    ok: true,
                    duration_ms: parse_log_started.elapsed().as_millis() as u64,
                    message: Some(format!(
                        "extracted {} YaXUnit error block(s)",
                        report.extracted_errors.len()
                    )),
                });
            }
            Err(error) => {
                warnings.push(format!("failed to read YaXUnit log: {error}"));
                steps.push(StepResult {
                    name: "parse_log".to_owned(),
                    ok: false,
                    duration_ms: parse_log_started.elapsed().as_millis() as u64,
                    message: Some(format!("failed to read YaXUnit log: {error}")),
                });
            }
        },
        PreparedRun::Vanessa { .. } => match vanessa_log::normalize_file(runner_log_path) {
            Ok(parsed) => {
                if let Some(errors) = parsed.payload {
                    report.extracted_errors = errors;
                }
                warnings.extend(parsed.warnings);
                steps.push(StepResult {
                    name: "parse_log".to_owned(),
                    ok: true,
                    duration_ms: parse_log_started.elapsed().as_millis() as u64,
                    message: Some(format!(
                        "extracted {} Vanessa Automation log line(s)",
                        report.extracted_errors.len()
                    )),
                });
            }
            Err(error) => {
                warnings.push(format!("failed to read Vanessa Automation log: {error}"));
                steps.push(StepResult {
                    name: "parse_log".to_owned(),
                    ok: false,
                    duration_ms: parse_log_started.elapsed().as_millis() as u64,
                    message: Some(format!("failed to read Vanessa Automation log: {error}")),
                });
            }
        },
    }
}

fn resolve_vanessa_junit_path(artifacts: &mut RunArtifacts) {
    if artifacts.junit_xml.exists() {
        return;
    }
    if let Some(path) = discover_junit_report(&artifacts.junit_dir) {
        artifacts.junit_xml = path;
    }
}

fn discover_junit_report(root: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("xml"))
        {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(found) = discover_junit_report(&path) {
                return Some(found);
            }
        }
    }
    None
}

fn parse_junit_report(artifacts: &RunArtifacts) -> crate::parsers::NormalizedParse<TestReport> {
    if !artifacts.junit_xml.exists() {
        return crate::parsers::NormalizedParse::default().with_errors(vec![test_execution_error(
            TestErrorKind::JunitNotProduced,
            "JUnit report was not produced",
        )]);
    }
    if fs::metadata(&artifacts.junit_xml)
        .map(|meta| meta.len() == 0)
        .unwrap_or(false)
    {
        return crate::parsers::NormalizedParse::default().with_errors(vec![test_execution_error(
            TestErrorKind::JunitEmpty,
            "JUnit report is empty",
        )]);
    }
    let file = fs::File::open(&artifacts.junit_xml).map_err(|error| error.to_string());
    let file = match file {
        Ok(file) => file,
        Err(error) => {
            return crate::parsers::NormalizedParse::default().with_errors(vec![
                test_execution_error(TestErrorKind::JunitNotProduced, error),
            ]);
        }
    };
    let reader = BufReader::new(file);
    let mut normalized = junit::parse_normalized(reader);
    if normalized.errors.is_empty() {
        return normalized;
    }
    normalized.errors = normalized
        .errors
        .into_iter()
        .map(|error| match error.code.as_str() {
            "junit_empty" => test_execution_error(TestErrorKind::JunitEmpty, error.message)
                .with_details(error.details),
            "junit_malformed" => test_execution_error(TestErrorKind::JunitMalformed, error.message)
                .with_details(error.details),
            _ => error,
        })
        .collect();
    normalized
}

fn compact_report(report: &TestReport) -> TestReport {
    let mut compact = report.clone();
    compact.suites = compact
        .suites
        .into_iter()
        .map(|mut suite| {
            suite.cases = suite
                .cases
                .into_iter()
                .filter(|case| case.status != TestStatus::Passed)
                .map(|mut case| {
                    if let Some(trace) = &case.stack_trace {
                        case.stack_trace = Some(truncate_stack_trace(trace));
                    }
                    case
                })
                .collect();
            suite
        })
        .filter(|suite| !suite.cases.is_empty())
        .collect();
    compact
}

fn truncate_stack_trace(trace: &str) -> String {
    if trace.chars().count() <= STACK_TRACE_LIMIT {
        return trace.to_owned();
    }
    let truncated: String = trace.chars().take(STACK_TRACE_LIMIT).collect();
    format!("{truncated}... (truncated, use --full to see complete trace)")
}

fn collect_diagnostics(
    platform_result: &crate::platform::result::PlatformCommandResult,
    mut diagnostics: Vec<String>,
    config: &AppConfig,
) -> Vec<String> {
    if !platform_result.process.stderr.trim().is_empty() {
        diagnostics.push(sanitize_text(&platform_result.process.stderr, config));
    }
    if let Some(log) = &platform_result.platform_log {
        let trimmed = log.trim();
        if !trimmed.is_empty() {
            diagnostics.push(limit_excerpt(&sanitize_text(trimmed, config)));
        }
    }
    diagnostics
}

fn enterprise_error_kind(error: EnterpriseError) -> (TestErrorKind, AppError) {
    match error {
        EnterpriseError::Spawn(ProcessError::TimedOut { .. }) => (
            TestErrorKind::EnterpriseTimedOut,
            AppError::Runtime("enterprise test run timed out".to_owned()),
        ),
        EnterpriseError::Spawn(process_error @ ProcessError::SpawnFailed { .. })
        | EnterpriseError::Spawn(process_error @ ProcessError::ExitedEarly { .. }) => (
            TestErrorKind::EnterpriseSpawnFailed,
            AppError::Platform(process_error.to_string()),
        ),
        EnterpriseError::Spawn(process_error) => (
            TestErrorKind::EnterpriseSpawnFailed,
            AppError::Platform(process_error.to_string()),
        ),
    }
}

fn retain_run_artifacts(
    _config: &AppConfig,
    artifacts: &RunArtifacts,
) -> std::io::Result<ArtifactSet> {
    Ok(crate::domain::test::RetainedPaths {
        run_dir: artifacts.run_dir.clone(),
        config_json: artifacts.config_json.clone(),
        junit_xml: artifacts.junit_xml.clone(),
        yaxunit_log: artifacts.runner_log.clone(),
        platform_log: artifacts.platform_log.clone(),
        sentinel: artifacts.sentinel.clone(),
    }
    .into_artifact_set())
}

fn cleanup_run_dir(artifacts: &RunArtifacts) {
    let _ = fs::remove_file(&artifacts.sentinel);
    let _ = fs::remove_dir_all(&artifacts.run_dir);
}

fn sanitize_text(text: &str, config: &AppConfig) -> String {
    limit_excerpt(&sanitize_text_full(text, config))
}

fn sanitize_text_full(text: &str, config: &AppConfig) -> String {
    let mut value = text.to_owned();
    value = Regex::new(r#"(?i)(/P\s+)("[^"]*"|\S+)"#)
        .expect("regex")
        .replace_all(&value, "$1***")
        .into_owned();
    value = Regex::new(r#"(?i)(/N\s+)("[^"]*"|\S+)"#)
        .expect("regex")
        .replace_all(&value, "$1***")
        .into_owned();
    value = Regex::new(r#"(?i)(password=)("[^"]*"|[^;\s]+)"#)
        .expect("regex")
        .replace_all(&value, "$1***")
        .into_owned();
    value = Regex::new(r#"(?i)(pwd=)("[^"]*"|[^;\s]+)"#)
        .expect("regex")
        .replace_all(&value, "$1***")
        .into_owned();
    value = Regex::new(r"(?i)(://[^:/\s]+:)([^@/\s]+)(@)")
        .expect("regex")
        .replace_all(&value, "$1***$3")
        .into_owned();
    if let Some(work_path) = config.work_path.to_str() {
        value = value.replace(work_path, "<workPath>");
    }
    value = redact_unix_paths(&value, &config.work_path);
    value = redact_quoted_windows_paths(&value);
    value = redact_windows_paths(&value);
    value
}

fn redact_unix_paths(text: &str, work_path: &Path) -> String {
    let work_path = work_path.to_string_lossy();
    Regex::new(r#"(/[^\s;,:"']+)"#)
        .expect("regex")
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let candidate = captures
                .get(1)
                .map(|value| value.as_str())
                .unwrap_or_default();
            if candidate.starts_with("/tmp/ib") {
                candidate.to_owned()
            } else if candidate.starts_with(work_path.as_ref()) {
                candidate.replacen(work_path.as_ref(), "<workPath>", 1)
            } else {
                "<path>".to_owned()
            }
        })
        .into_owned()
}

fn redact_windows_paths(text: &str) -> String {
    Regex::new(r#"([A-Za-z]:(?:\\[^\\\r\n";,]+)+)"#)
        .expect("regex")
        .replace_all(text, "<path>")
        .into_owned()
}

fn redact_quoted_windows_paths(text: &str) -> String {
    Regex::new(r#""[A-Za-z]:(?:\\[^"\r\n]+)+""#)
        .expect("regex")
        .replace_all(text, "<path>")
        .into_owned()
}

fn limit_excerpt(text: &str) -> String {
    let limit = 1_000;
    if text.chars().count() <= limit {
        text.to_owned()
    } else {
        format!(
            "{}... (truncated)",
            text.chars().take(limit).collect::<String>()
        )
    }
}

fn set_dir_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

fn set_file_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        build_yaxunit_config, compact_report, create_run_artifacts, materialize_vanessa_runner_log,
        parse_junit_report, retain_run_artifacts, sanitize_text, sanitize_text_full,
        truncate_stack_trace, RunArtifacts,
    };
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, PlatformToolConfig, SourceFormat, SourceSetConfig,
        SourceSetPurpose, TestsConfig, ToolsConfig, VanessaProfileConfig,
    };
    use crate::domain::execution::ExecutionTimeouts;
    use crate::domain::runner::{
        ExecutionPolicy, LaunchClientModeRequest, LaunchOptions, RunnerKind, RunnerProfile,
    };
    use crate::domain::test::{
        TestCase, TestErrorKind, TestReport, TestStatus, TestSuite, TestSummary, TestTarget,
    };
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn config(work_path: &std::path::Path) -> AppConfig {
        let base = work_path.join("base");
        std::fs::create_dir_all(base.join("main")).expect("base");
        AppConfig {
            base_path: base.clone(),
            work_path: work_path.to_path_buf(),
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
            source_sets: vec![SourceSetConfig {
                name: "main".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: PathBuf::from("main"),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig {
                platform: PlatformToolConfig::default(),
                ..ToolsConfig::default()
            },
            mcp: Default::default(),
            tests: TestsConfig::default(),
        }
    }

    #[test]
    fn creates_distinct_run_dirs() {
        let dir = tempdir().expect("tempdir");
        let config = config(dir.path());
        let first = create_run_artifacts(&config, "yaxunit").expect("first");
        let second = create_run_artifacts(&config, "yaxunit").expect("second");
        assert_ne!(first.run_dir, second.run_dir);
    }

    #[test]
    fn module_config_serializes_filter() {
        let dir = tempdir().expect("tempdir");
        let artifacts = create_artifacts(dir.path());
        let payload = build_yaxunit_config(
            &TestTarget::Module {
                name: "Foo Бар".to_owned(),
            },
            &artifacts,
        );
        let json = serde_json::to_value(payload).expect("json");
        assert_eq!(json["filter"]["modules"][0], "Foo Бар");
    }

    #[test]
    fn sanitizer_masks_passwords() {
        let dir = tempdir().expect("tempdir");
        let config = config(dir.path());
        let sanitized = sanitize_text(
            "cmd /N \"Domain User\" /P \"very secret\" File=/tmp/ib password=\"hidden value\" pwd=\"another secret\" /home/user/project C:\\Secrets\\ib \"C:\\Program Files\\1cv8\\conf\" http://user:pass@example",
            &config,
        );
        assert!(!sanitized.contains("very secret"));
        assert!(!sanitized.contains("hidden value"));
        assert!(!sanitized.contains("another secret"));
        assert!(!sanitized.contains("Domain User"));
        assert!(!sanitized.contains("pass@example"));
        assert!(!sanitized.contains("/home/user/project"));
        assert!(!sanitized.contains("C:\\Secrets\\ib"));
        assert!(!sanitized.contains("C:\\Program Files\\1cv8\\conf"));
        assert!(sanitized.contains("<path>"));
    }

    #[test]
    fn diagnostics_are_truncated_but_full_sanitizer_is_not() {
        let dir = tempdir().expect("tempdir");
        let config = config(dir.path());
        let input = format!("prefix {} suffix", "x".repeat(1_500));

        let excerpt = sanitize_text(&input, &config);
        let full = sanitize_text_full(&input, &config);

        assert!(excerpt.contains("(truncated)"));
        assert!(!full.contains("(truncated)"));
        assert!(full.len() > excerpt.len());
    }

    #[test]
    fn compact_report_hides_passed_cases() {
        let report = sample_report();
        let compact = compact_report(&report);
        assert_eq!(compact.suites[0].cases.len(), 1);
        assert_eq!(compact.suites[0].cases[0].status, TestStatus::Failed);
    }

    #[test]
    fn stack_trace_is_truncated() {
        let trace = "a".repeat(700);
        let truncated = truncate_stack_trace(&trace);
        assert!(truncated.contains("truncated"));
        assert!(truncated.len() < trace.len());
    }

    #[test]
    fn materialize_vanessa_runner_log_copies_raw_bytes() {
        let dir = tempdir().expect("tempdir");
        let artifacts = create_artifacts(dir.path());
        std::fs::create_dir_all(&artifacts.run_dir).expect("run dir");
        let payload = [0xff, 0xfe, 0x00, b'J', b'u', b'n'];
        std::fs::write(&artifacts.platform_log, payload).expect("write platform log");

        materialize_vanessa_runner_log(&artifacts).expect("materialize log");

        let copied = std::fs::read(&artifacts.runner_log).expect("read runner log");
        assert_eq!(copied, payload);
    }

    #[test]
    fn materialize_vanessa_runner_log_returns_warning_on_missing_source() {
        let dir = tempdir().expect("tempdir");
        let artifacts = create_artifacts(dir.path());
        std::fs::create_dir_all(&artifacts.run_dir).expect("run dir");

        let warning = materialize_vanessa_runner_log(&artifacts).expect_err("warning");
        assert!(warning.contains("failed to materialize Vanessa runner log"));
    }

    #[test]
    fn vanessa_junit_parse_failure_retains_materialized_runner_log() {
        let dir = tempdir().expect("tempdir");
        let config = config(dir.path());
        let artifacts = create_artifacts(dir.path());
        std::fs::create_dir_all(&artifacts.run_dir).expect("run dir");
        std::fs::write(&artifacts.platform_log, b"enterprise /Out").expect("platform log");

        materialize_vanessa_runner_log(&artifacts).expect("materialize log");
        let junit_parse = parse_junit_report(&artifacts);
        assert!(junit_parse.payload.is_none());
        assert_eq!(
            junit_parse.errors[0].code,
            TestErrorKind::JunitNotProduced.code()
        );

        let retained = retain_run_artifacts(&config, &artifacts).expect("retain artifacts");
        let retained_paths = crate::domain::test::RetainedPaths::from_artifact_set(&retained)
            .expect("retained paths");
        assert!(retained_paths.yaxunit_log.exists());
        assert_eq!(retained_paths.yaxunit_log, artifacts.runner_log);
    }

    #[test]
    fn unsafe_vanessa_profile_name_is_rejected() {
        let dir = tempdir().expect("tempdir");
        let mut config = config(dir.path());
        let epf = dir.path().join("runner.epf");
        let params = dir.path().join("params.json");
        let feature = dir.path().join("features");
        std::fs::write(&epf, "epf").expect("epf");
        std::fs::write(&params, "{}").expect("params");
        std::fs::create_dir_all(&feature).expect("feature dir");

        config.tests.va.epf_path = Some(epf);
        config.tests.va.params_path = Some(params);
        config.tests.va.profile = Some("bad/name".to_owned());
        config.tests.va.profiles.insert(
            "bad/name".to_owned(),
            VanessaProfileConfig {
                feature_path: Some(feature),
                ..VanessaProfileConfig::default()
            },
        );

        let args = crate::use_cases::request::TestRequest {
            full: false,
            scope: crate::use_cases::request::TestScopeRequest::All,
            execution: crate::domain::runner::ScenarioExecutionRequest {
                profile: RunnerProfile {
                    id: "bad/name".to_owned(),
                    kind: RunnerKind::Vanessa,
                    output_formats: vec![],
                    backend_hint: Some("enterprise".to_owned()),
                },
                client_mode: Some(LaunchClientModeRequest::Thin),
                timeouts: ExecutionTimeouts::default(),
                policy: ExecutionPolicy::default(),
                launch: LaunchOptions::default(),
            },
        };

        let result = super::run_tests(&config, &args);
        assert!(result.is_err());
        let error = result.err().expect("error");
        assert!(error.error.to_string().contains("unsafe path characters"));
    }

    fn create_artifacts(root: &std::path::Path) -> RunArtifacts {
        RunArtifacts {
            run_dir: root.join("run"),
            config_json: root.join("run/config.json"),
            junit_xml: root.join("run/report.xml"),
            junit_dir: root.join("run/junit"),
            runner_log: root.join("run/yax.log"),
            platform_log: root.join("run/platform.log"),
            sentinel: root.join("run/run.inprogress"),
        }
    }

    fn sample_report() -> TestReport {
        TestReport {
            summary: TestSummary {
                total: 2,
                passed: 1,
                failed: 1,
                skipped: 0,
                errors: 0,
            },
            suites: vec![TestSuite {
                name: "suite".to_owned(),
                duration_ms: 10,
                cases: vec![
                    TestCase {
                        name: "ok".to_owned(),
                        class_name: None,
                        status: TestStatus::Passed,
                        duration_ms: 1,
                        failure_message: None,
                        stack_trace: None,
                    },
                    TestCase {
                        name: "bad".to_owned(),
                        class_name: None,
                        status: TestStatus::Failed,
                        duration_ms: 2,
                        failure_message: Some("boom".to_owned()),
                        stack_trace: Some("trace".to_owned()),
                    },
                ],
            }],
            extracted_errors: vec![],
        }
    }
}
