use super::*;
use crate::use_cases::progress::log_live_stage;
use crate::use_cases::request::TestBuildPolicy;

pub(super) fn run_tests(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &TestArgs,
) -> UseCaseResult<TestRunResult> {
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
    if let Some(failure) =
        interrupted_test_failure(context, &target, &mode, &warnings, &steps, started, None)
    {
        return Err(failure);
    }
    let runner_id = match validate_runner_profile_id(&args.execution.profile.id) {
        Ok(runner_id) => runner_id,
        Err(error) => {
            let outcome = ExecutionOutcome::new(ExecutionStatus::Failed)
                .with_diagnostics(vec![error.to_string()])
                .with_errors(vec![test_execution_error(
                    TestErrorKind::TestSetupFailed,
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

    match args.build_policy {
        TestBuildPolicy::BuildFirst => {}
        TestBuildPolicy::Skip => {
            steps.push(skipped_step(
                "build",
                ExecutionStepKind::PlatformCommand,
                0,
                "build prerequisite explicitly skipped by --no-build",
            ));
            if let Err(error) = validate_prepared_infobase(config) {
                let message = error.to_string();
                steps.push(
                    failed_step(
                        "preflight_infobase",
                        ExecutionStepKind::Validation,
                        0,
                        message.clone(),
                    )
                    .with_errors(vec![test_execution_error(
                        TestErrorKind::InfobaseUnavailable,
                        message.clone(),
                    )]),
                );
                let outcome = ExecutionOutcome::new(ExecutionStatus::Failed)
                    .with_diagnostics(vec![message.clone()])
                    .with_errors(vec![test_execution_error(
                        TestErrorKind::InfobaseUnavailable,
                        message,
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
        }
    }

    debug!("preparing test run artifacts");
    let prepare_artifacts_started = Instant::now();
    let mut artifacts = match create_run_artifacts(config, runner_id) {
        Ok(artifacts) => artifacts,
        Err(error) => {
            let app_error =
                AppError::Runtime(format!("failed to prepare test run directory: {error}"));
            steps.push(
                failed_step(
                    "prepare_artifacts",
                    ExecutionStepKind::PrepareWorkspace,
                    prepare_artifacts_started.elapsed().as_millis() as u64,
                    app_error.to_string(),
                )
                .with_errors(vec![test_execution_error(
                    TestErrorKind::TestSetupFailed,
                    app_error.to_string(),
                )]),
            );
            let outcome = ExecutionOutcome::new(ExecutionStatus::Failed)
                .with_diagnostics(vec![app_error.to_string()])
                .with_errors(vec![test_execution_error(
                    TestErrorKind::TestSetupFailed,
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
    steps.push(
        succeeded_step(
            "prepare_artifacts",
            ExecutionStepKind::PrepareWorkspace,
            prepare_artifacts_started.elapsed().as_millis() as u64,
            format!("created {}", artifacts.run_dir.display()),
        )
        .with_target(artifacts.run_dir.display().to_string()),
    );

    match args.build_policy {
        TestBuildPolicy::BuildFirst => {
            debug!("running build prerequisite for tests");
            log_live_stage(
                "test: build prerequisite",
                "[Build] preparing test infobase",
            );
            let build_started = Instant::now();
            let build_result = match build_project::execute(
                context,
                config,
                &BuildArgs {
                    full_rebuild: false,
                    source_set: None,
                },
            ) {
                Ok(result) => result,
                Err(failure) => {
                    let summary = failure
                        .payload
                        .as_ref()
                        .map(build_summary)
                        .unwrap_or_else(|| failure.error.to_string());
                    steps.push(
                        failed_step(
                            "build",
                            ExecutionStepKind::PlatformCommand,
                            build_started.elapsed().as_millis() as u64,
                            summary.clone(),
                        )
                        .with_errors(vec![test_execution_error(
                            TestErrorKind::BuildFailed,
                            summary.clone(),
                        )]),
                    );
                    let retained_paths = retain_run_artifacts(config, &artifacts).ok();
                    let outcome = with_retained_artifacts(
                        ExecutionOutcome::new(ExecutionStatus::Failed)
                            .with_diagnostics(vec![summary.clone()])
                            .with_errors(vec![test_execution_error(
                                TestErrorKind::BuildFailed,
                                summary.clone(),
                            )]),
                        retained_paths,
                    );
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
            steps.push(succeeded_step(
                "build",
                ExecutionStepKind::PlatformCommand,
                build_started.elapsed().as_millis() as u64,
                build_summary(&build_result),
            ));
        }
        TestBuildPolicy::Skip => {}
    }

    let prepare_runner_started = Instant::now();
    if let Some(failure) = interrupted_test_failure(
        context,
        &target,
        &mode,
        &warnings,
        &steps,
        started,
        retain_run_artifacts(config, &artifacts).ok(),
    ) {
        return Err(failure);
    }
    let prepared_run = match prepare_runner_artifacts(config, args, &target, &mut artifacts) {
        Ok(prepared_run) => {
            steps.push(
                succeeded_step(
                    "prepare_runner",
                    ExecutionStepKind::PrepareWorkspace,
                    prepare_runner_started.elapsed().as_millis() as u64,
                    prepared_run_summary(&prepared_run),
                )
                .with_target(artifacts.config_json.display().to_string()),
            );
            prepared_run
        }
        Err(error) => {
            steps.push(
                failed_step(
                    "prepare_runner",
                    ExecutionStepKind::PrepareWorkspace,
                    prepare_runner_started.elapsed().as_millis() as u64,
                    error.to_string(),
                )
                .with_target(artifacts.config_json.display().to_string())
                .with_errors(vec![test_execution_error(
                    TestErrorKind::TestSetupFailed,
                    error.to_string(),
                )]),
            );
            let retained_paths = retain_run_artifacts(config, &artifacts).ok();
            let outcome = with_retained_artifacts(
                ExecutionOutcome::new(ExecutionStatus::Failed)
                    .with_diagnostics(vec![error.to_string()])
                    .with_errors(vec![test_execution_error(
                        TestErrorKind::TestSetupFailed,
                        error.to_string(),
                    )]),
                retained_paths,
            );
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
    log_live_stage("test: enterprise run", "[Enterprise] running test runner");
    let run_started = Instant::now();
    let enterprise_runner = crate::platform::process::ProcessExecutor;
    let platform_launch = build_platform_launch(&args.execution.launch, &prepared_run, &artifacts);
    let enterprise = match build_enterprise_dsl(
        context,
        config,
        &artifacts,
        &prepared_run,
        &platform_launch,
        &enterprise_runner,
        args.execution
            .client_mode
            .unwrap_or(LaunchClientModeRequest::Thin),
        capped_timeout_ms(args.execution.timeouts.total_ms, context),
    ) {
        Ok(dsl) => dsl,
        Err(error) => {
            steps.push(
                failed_step(
                    "run",
                    ExecutionStepKind::PlatformCommand,
                    run_started.elapsed().as_millis() as u64,
                    error.to_string(),
                )
                .with_target(artifacts.platform_log.display().to_string())
                .with_errors(vec![test_execution_error(
                    TestErrorKind::TestSetupFailed,
                    error.to_string(),
                )]),
            );
            let retained_paths = retain_run_artifacts(config, &artifacts).ok();
            let outcome = with_retained_artifacts(
                ExecutionOutcome::new(ExecutionStatus::Failed)
                    .with_diagnostics(vec![error.to_string()])
                    .with_errors(vec![test_execution_error(
                        TestErrorKind::TestSetupFailed,
                        error.to_string(),
                    )]),
                retained_paths,
            );
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

    if let Some(failure) = interrupted_test_failure(
        context,
        &target,
        &mode,
        &warnings,
        &steps,
        started,
        retain_run_artifacts(config, &artifacts).ok(),
    ) {
        return Err(failure);
    }
    let platform_result = match enterprise.run_launch(&platform_launch) {
        Ok(result) => {
            steps.push(
                if result.process.exit_code == 0 {
                    succeeded_step(
                        "run",
                        ExecutionStepKind::PlatformCommand,
                        run_started.elapsed().as_millis() as u64,
                        format!("enterprise exit code {}", result.process.exit_code),
                    )
                } else {
                    failed_step(
                        "run",
                        ExecutionStepKind::PlatformCommand,
                        run_started.elapsed().as_millis() as u64,
                        format!("enterprise exit code {}", result.process.exit_code),
                    )
                }
                .with_target(artifacts.platform_log.display().to_string()),
            );
            result
        }
        Err(error) => {
            let (kind, app_error, interruption, status) = enterprise_error_kind(error);
            let mut step = failed_step(
                "run",
                ExecutionStepKind::PlatformCommand,
                run_started.elapsed().as_millis() as u64,
                app_error.to_string(),
            )
            .with_target(artifacts.platform_log.display().to_string());
            if let Some(kind) = kind.clone() {
                step = step.with_errors(vec![test_execution_error(kind, app_error.to_string())]);
            }
            steps.push(step);
            let retained_paths = retain_run_artifacts(config, &artifacts).ok();
            let mut outcome =
                ExecutionOutcome::new(status).with_diagnostics(vec![app_error.to_string()]);
            if let Some(kind) = kind {
                outcome =
                    outcome.with_errors(vec![test_execution_error(kind, app_error.to_string())]);
            }
            if let Some(interruption) = interruption {
                outcome = outcome.with_interruptions(vec![interruption]);
            }
            let outcome = with_retained_artifacts(outcome, retained_paths);
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
        if let Err(warning) = materialize_vanessa_runner_log(&artifacts) {
            warnings.push(warning);
        }
    }

    debug!(path = %artifacts.junit_dir.display(), "parsing JUnit reports");
    let parse_junit_started = Instant::now();
    let junit_parse = parse_junit_report(&artifacts);
    let mut report = match junit_parse.payload {
        Some(report) => {
            steps.push(
                succeeded_step(
                    "parse_junit",
                    ExecutionStepKind::ParseOutput,
                    parse_junit_started.elapsed().as_millis() as u64,
                    format!(
                        "parsed {} test cases from native JUnit reports",
                        report.summary.total
                    ),
                )
                .with_target(artifacts.junit_dir.display().to_string()),
            );
            report
        }
        None => {
            let errors = if junit_parse.errors.is_empty() {
                vec![test_execution_error(
                    TestErrorKind::JunitMalformed,
                    "JUnit report parsing returned no report or error",
                )]
            } else {
                junit_parse.errors
            };
            let error = errors[0].clone();
            let kind =
                TestErrorKind::from_code(&error.code).unwrap_or(TestErrorKind::JunitMalformed);
            let message = error.message.clone();
            steps.push(
                failed_step(
                    "parse_junit",
                    ExecutionStepKind::ParseOutput,
                    parse_junit_started.elapsed().as_millis() as u64,
                    message.clone(),
                )
                .with_target(artifacts.junit_dir.display().to_string())
                .with_errors(errors.clone()),
            );
            let retained_paths = retain_run_artifacts(config, &artifacts).ok();
            let diagnostics = collect_diagnostics(&platform_result, vec![message.clone()], config);
            let outcome = with_retained_artifacts(
                ExecutionOutcome::new(test_execution_status(Some(kind.clone()), false))
                    .with_diagnostics(diagnostics)
                    .with_errors(errors),
                retained_paths,
            );
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

    let validate_allure_started = Instant::now();
    if let Err(failure) = validate_allure_results(&artifacts.allure_results_dir) {
        let kind = failure.kind;
        let message = failure.message;
        let error =
            test_execution_error(kind.clone(), message.clone()).with_details(failure.details);
        steps.push(
            failed_step(
                "validate_allure",
                ExecutionStepKind::ParseOutput,
                validate_allure_started.elapsed().as_millis() as u64,
                message.clone(),
            )
            .with_target(artifacts.allure_results_dir.display().to_string())
            .with_errors(vec![error.clone()]),
        );
        let retained_paths = retain_run_artifacts(config, &artifacts).ok();
        let diagnostics = collect_diagnostics(&platform_result, vec![message.clone()], config);
        let outcome = with_retained_artifacts(
            ExecutionOutcome::new(test_execution_status(Some(kind), false))
                .with_diagnostics(diagnostics)
                .with_errors(vec![error]),
            retained_paths,
        );
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
    steps.push(
        succeeded_step(
            "validate_allure",
            ExecutionStepKind::ParseOutput,
            validate_allure_started.elapsed().as_millis() as u64,
            "validated native Allure results",
        )
        .with_target(artifacts.allure_results_dir.display().to_string()),
    );

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

    let classification =
        classify_test_completion(&report.summary, platform_result.process.exit_code);
    let mut diagnostics = collect_diagnostics(&platform_result, Vec::new(), config);

    if let Some(kind) = classification {
        debug!(
            error_kind = kind.clone().code(),
            "retaining failed test artifacts"
        );
        if matches!(&kind, TestErrorKind::TestFailures) && platform_result.process.exit_code != 0 {
            diagnostics.push(format!(
                "enterprise test run exited with code {}",
                platform_result.process.exit_code
            ));
        }
        let retained_paths = retain_run_artifacts(config, &artifacts).ok();
        let message = match &kind {
            TestErrorKind::EnterpriseExitedNonZero => format!(
                "enterprise test run exited with code {}",
                platform_result.process.exit_code
            ),
            TestErrorKind::TestFailures => "test run reported failures".to_owned(),
            TestErrorKind::BuildFailed
            | TestErrorKind::InfobaseUnavailable
            | TestErrorKind::TestSetupFailed
            | TestErrorKind::EnterpriseSpawnFailed
            | TestErrorKind::EnterpriseStartupCheckFailed
            | TestErrorKind::EnterpriseExitedEarly
            | TestErrorKind::EnterpriseStdoutLogIo
            | TestErrorKind::EnterpriseStderrLogIo
            | TestErrorKind::EnterpriseTimedOut
            | TestErrorKind::JunitNotProduced
            | TestErrorKind::JunitEmpty
            | TestErrorKind::JunitMalformed
            | TestErrorKind::AllureNotProduced
            | TestErrorKind::AllureEmpty => "test run failed".to_owned(),
        };
        let outcome = with_retained_artifacts(
            ExecutionOutcome::new(test_execution_status(Some(kind.clone()), false))
                .with_diagnostics(diagnostics)
                .with_errors(vec![test_execution_error(kind, message.clone())])
                .with_metrics(ExecutionMetrics::from(&report.summary))
                .with_payload(rendered_report),
            retained_paths,
        );
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

    let retained_paths = retain_run_artifacts(config, &artifacts).ok();
    Ok(make_test_result(
        target,
        mode,
        with_retained_artifacts(
            ExecutionOutcome::new(ExecutionStatus::Succeeded)
                .with_diagnostics(diagnostics)
                .with_metrics(ExecutionMetrics::from(&report.summary))
                .with_payload(rendered_report),
            retained_paths,
        ),
        warnings,
        steps,
        started.elapsed().as_millis() as u64,
    ))
}

fn validate_prepared_infobase(config: &AppConfig) -> Result<(), AppError> {
    let connection = config.v8_connection();
    let Some(file_path) = connection.file_path() else {
        return Ok(());
    };
    let marker = Path::new(file_path).join("1Cv8.1CD");
    if marker.is_file() {
        Ok(())
    } else {
        Err(AppError::Runtime(format!(
            "prepared file infobase is unavailable: expected '{}'",
            marker.display()
        )))
    }
}
