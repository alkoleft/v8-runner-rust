use super::*;
use crate::use_cases::progress::log_live_stage;

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
        interrupted_test_failure(context, &target, &mode, &warnings, &steps, started)
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
    steps.push(succeeded_step(
        "build",
        ExecutionStepKind::PlatformCommand,
        build_started.elapsed().as_millis() as u64,
        build_summary(&build_result),
    ));

    debug!("preparing test run artifacts");
    let prepare_artifacts_started = Instant::now();
    let mut artifacts = match create_run_artifacts(config, &runner_id) {
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

    let prepare_runner_started = Instant::now();
    if let Some(failure) =
        interrupted_test_failure(context, &target, &mode, &warnings, &steps, started)
    {
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
    let mut platform_launch =
        build_platform_launch(&args.execution.launch, &prepared_run, &artifacts);
    apply_test_mcp_ws_payload(config, &args.mcp_ws, &prepared_run, &mut platform_launch);
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

    if let Some(failure) =
        interrupted_test_failure(context, &target, &mode, &warnings, &steps, started)
    {
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
            steps.push(
                succeeded_step(
                    "parse_junit",
                    ExecutionStepKind::ParseOutput,
                    parse_junit_started.elapsed().as_millis() as u64,
                    format!("parsed {} test cases", report.summary.total),
                )
                .with_target(artifacts.junit_xml.display().to_string()),
            );
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
            steps.push(
                failed_step(
                    "parse_junit",
                    ExecutionStepKind::ParseOutput,
                    parse_junit_started.elapsed().as_millis() as u64,
                    message.clone(),
                )
                .with_target(artifacts.junit_xml.display().to_string())
                .with_errors(vec![error
                    .clone()
                    .with_details(junit_parse.diagnostics.clone())]),
            );
            let retained_paths = retain_run_artifacts(config, &artifacts).ok();
            let diagnostics = collect_diagnostics(&platform_result, vec![message.clone()], config);
            let outcome = with_retained_artifacts(
                ExecutionOutcome::new(test_execution_status(Some(kind.clone()), false))
                    .with_diagnostics(diagnostics)
                    .with_errors(vec![error.with_details(junit_parse.diagnostics)]),
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
        let outcome = with_retained_artifacts(
            ExecutionOutcome::new(test_execution_status(Some(kind.clone()), false))
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
