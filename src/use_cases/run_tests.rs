use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Serialize;
use uuid::Uuid;

use crate::cli::args::{BuildArgs, TestArgs, TestScope};
use crate::config::model::AppConfig;
use crate::domain::test::{
    RetainedPaths, TestErrorKind, TestOutputMode, TestReport, TestRunResult, TestStatus, TestTarget,
};
use crate::output::json::{Envelope, StepResult};
use crate::output::presenter::Presenter;
use crate::parsers::junit::{self, JunitError};
use crate::parsers::yaxunit_log;
use crate::platform::connection::V8Connection;
use crate::platform::enterprise::{EnterpriseDsl, EnterpriseError};
use crate::platform::locator::UtilityType;
use crate::platform::process::ProcessError;
use crate::platform::utilities::PlatformUtilities;
use crate::support::error::AppError;
use crate::use_cases::build_project;
use tracing::info;

const TEST_COMMAND: &str = "test";
const STACK_TRACE_LIMIT: usize = 500;

pub fn execute(config: &AppConfig, args: &TestArgs, presenter: &Presenter) -> Result<(), AppError> {
    let result = match run_tests(config, args) {
        Ok(result) => result,
        Err((error, result)) => {
            if presenter.is_json() {
                presenter.print_envelope(&build_envelope(result.clone(), false));
            } else {
                render_text_result(&result, presenter);
                presenter.print_error(&error.to_string());
            }
            return Err(error);
        }
    };

    if presenter.is_json() {
        presenter.print_envelope(&build_envelope(result.clone(), true));
    } else {
        render_text_result(&result, presenter);
    }

    Ok(())
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
    yaxunit_log: PathBuf,
    platform_log: PathBuf,
    sentinel: PathBuf,
}

fn run_tests(
    config: &AppConfig,
    args: &TestArgs,
) -> Result<TestRunResult, (AppError, TestRunResult)> {
    let started = Instant::now();
    info!(full = args.full, scope = ?args.scope, "starting test run");
    let mode = if args.full {
        TestOutputMode::Full
    } else {
        TestOutputMode::Compact
    };
    let target = match &args.scope {
        TestScope::All => TestTarget::All,
        TestScope::Module { name } => {
            let trimmed = name.trim();
            if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
                let error =
                    AppError::Validation("test module requires a non-empty module name".to_owned());
                let result = TestRunResult {
                    ok: false,
                    target: TestTarget::Module { name: name.clone() },
                    mode,
                    error_kind: None,
                    diagnostics: vec![error.to_string()],
                    retained_paths: None,
                    report: None,
                    warnings: Vec::new(),
                    steps: Vec::new(),
                    duration_ms: started.elapsed().as_millis() as u64,
                };
                return Err((error, result));
            }
            TestTarget::Module {
                name: trimmed.to_owned(),
            }
        }
    };

    let mut steps = Vec::new();
    let mut warnings = Vec::new();

    info!("running build prerequisite for tests");
    let build_started = Instant::now();
    let build_result = match build_project::run_build(
        config,
        &BuildArgs {
            full_rebuild: false,
        },
    ) {
        Ok(result) => result,
        Err(failure) => {
            steps.push(StepResult {
                name: "build".to_owned(),
                ok: false,
                duration_ms: build_started.elapsed().as_millis() as u64,
                message: Some(build_summary(&failure.result)),
            });
            let result = TestRunResult {
                ok: false,
                target,
                mode,
                error_kind: Some(TestErrorKind::BuildFailed),
                diagnostics: vec![build_summary(&failure.result)],
                retained_paths: None,
                report: None,
                warnings,
                steps,
                duration_ms: started.elapsed().as_millis() as u64,
            };
            return Err((failure.error, result));
        }
    };
    steps.push(StepResult {
        name: "build".to_owned(),
        ok: true,
        duration_ms: build_started.elapsed().as_millis() as u64,
        message: Some(build_summary(&build_result)),
    });

    info!("preparing test run artifacts");
    let artifacts = match create_run_artifacts(config) {
        Ok(artifacts) => artifacts,
        Err(error) => {
            let app_error =
                AppError::Runtime(format!("failed to prepare test run directory: {error}"));
            let result = TestRunResult {
                ok: false,
                target,
                mode,
                error_kind: Some(TestErrorKind::EnterpriseSpawnFailed),
                diagnostics: vec![app_error.to_string()],
                retained_paths: None,
                report: None,
                warnings,
                steps,
                duration_ms: started.elapsed().as_millis() as u64,
            };
            return Err((app_error, result));
        }
    };

    info!(path = %artifacts.config_json.display(), "writing YaXUnit configuration");
    let config_payload = build_yaxunit_config(&target, &artifacts);
    if let Err(error) = write_json_file(&artifacts.config_json, &config_payload) {
        let app_error = AppError::Runtime(format!("failed to write YaXUnit config: {error}"));
        let retained_paths = retain_run_artifacts(config, &artifacts).ok();
        let result = TestRunResult {
            ok: false,
            target,
            mode,
            error_kind: Some(TestErrorKind::EnterpriseSpawnFailed),
            diagnostics: vec![app_error.to_string()],
            retained_paths,
            report: None,
            warnings,
            steps,
            duration_ms: started.elapsed().as_millis() as u64,
        };
        return Err((app_error, result));
    }

    info!(path = %artifacts.run_dir.display(), "launching enterprise test run");
    let run_started = Instant::now();
    let enterprise_runner = crate::platform::process::ProcessExecutor;
    let enterprise = match build_enterprise_dsl(config, &artifacts, &enterprise_runner) {
        Ok(dsl) => dsl,
        Err(error) => {
            let retained_paths = retain_run_artifacts(config, &artifacts).ok();
            let result = TestRunResult {
                ok: false,
                target,
                mode,
                error_kind: Some(TestErrorKind::EnterpriseSpawnFailed),
                diagnostics: vec![error.to_string()],
                retained_paths,
                report: None,
                warnings,
                steps,
                duration_ms: started.elapsed().as_millis() as u64,
            };
            return Err((error, result));
        }
    };

    let platform_result = match enterprise.run_unit_tests(&artifacts.config_json) {
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
            let result = TestRunResult {
                ok: false,
                target,
                mode,
                error_kind: Some(kind),
                diagnostics: vec![app_error.to_string()],
                retained_paths,
                report: None,
                warnings,
                steps,
                duration_ms: started.elapsed().as_millis() as u64,
            };
            return Err((app_error, result));
        }
    };

    info!(path = %artifacts.junit_xml.display(), "parsing JUnit report");
    let parse_junit_started = Instant::now();
    let mut report = match parse_junit_report(&artifacts) {
        Ok(report) => {
            steps.push(StepResult {
                name: "parse_junit".to_owned(),
                ok: true,
                duration_ms: parse_junit_started.elapsed().as_millis() as u64,
                message: Some(format!("parsed {} test cases", report.summary.total)),
            });
            report
        }
        Err((kind, message)) => {
            steps.push(StepResult {
                name: "parse_junit".to_owned(),
                ok: false,
                duration_ms: parse_junit_started.elapsed().as_millis() as u64,
                message: Some(message.clone()),
            });
            let retained_paths = retain_run_artifacts(config, &artifacts).ok();
            let diagnostics = collect_diagnostics(&platform_result, vec![message.clone()], config);
            let result = TestRunResult {
                ok: false,
                target,
                mode,
                error_kind: Some(kind),
                diagnostics,
                retained_paths,
                report: None,
                warnings,
                steps,
                duration_ms: started.elapsed().as_millis() as u64,
            };
            return Err((AppError::Runtime(message), result));
        }
    };

    info!(path = %artifacts.yaxunit_log.display(), "parsing YaXUnit log");
    let parse_log_started = Instant::now();
    match yaxunit_log::parse_file(&artifacts.yaxunit_log) {
        Ok(errors) => {
            if artifacts.yaxunit_log.exists() {
                report.extracted_errors = errors;
            } else {
                warnings.push("YaXUnit log file was not produced".to_owned());
            }
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
    }

    let rendered_report = match mode {
        TestOutputMode::Full => report.clone(),
        TestOutputMode::Compact => compact_report(&report),
    };

    let has_test_failures = report.summary.failed > 0 || report.summary.errors > 0;
    let process_failed = platform_result.process.exit_code != 0;
    let diagnostics = collect_diagnostics(&platform_result, Vec::new(), config);

    if process_failed || has_test_failures {
        info!(
            process_failed,
            has_test_failures, "retaining failed test artifacts"
        );
        let retained_paths = retain_run_artifacts(config, &artifacts).ok();
        let result = TestRunResult {
            ok: false,
            target,
            mode,
            error_kind: Some(if process_failed {
                TestErrorKind::EnterpriseExitedNonZero
            } else {
                TestErrorKind::TestFailures
            }),
            diagnostics,
            retained_paths,
            report: Some(rendered_report),
            warnings,
            steps,
            duration_ms: started.elapsed().as_millis() as u64,
        };
        return Err((
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

    info!(path = %artifacts.run_dir.display(), "cleaning successful test run directory");
    cleanup_run_dir(&artifacts);
    Ok(TestRunResult {
        ok: true,
        target,
        mode,
        error_kind: None,
        diagnostics,
        retained_paths: None,
        report: Some(rendered_report),
        warnings,
        steps,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

fn build_envelope(result: TestRunResult, ok: bool) -> Envelope<TestRunResult> {
    Envelope {
        ok,
        command: TEST_COMMAND.to_owned(),
        duration_ms: result.duration_ms,
        warnings: result.warnings.clone(),
        steps: result.steps.clone(),
        data: result,
    }
}

fn render_text_result(result: &TestRunResult, presenter: &Presenter) {
    let target = match &result.target {
        TestTarget::All => "all".to_owned(),
        TestTarget::Module { name } => format!("module {name}"),
    };
    presenter.print_info(&format!("Test target: {target}"));

    if let Some(report) = &result.report {
        presenter.print_info(&format!(
            "Summary: total={}, passed={}, failed={}, skipped={}, errors={}",
            report.summary.total,
            report.summary.passed,
            report.summary.failed,
            report.summary.skipped,
            report.summary.errors
        ));

        for suite in &report.suites {
            presenter.print_info(&format!("Suite: {}", suite.name));
            for case in &suite.cases {
                presenter.print_info(&format!("  {} {}", status_label(&case.status), case.name));
                if let Some(message) = &case.failure_message {
                    presenter.print_info(&format!("    {message}"));
                }
                if let Some(trace) = &case.stack_trace {
                    presenter.print_info(&format!("    {trace}"));
                }
            }
        }
    }

    for diagnostic in &result.diagnostics {
        presenter.print_info(&format!("Diagnostic: {diagnostic}"));
    }
    for warning in &result.warnings {
        presenter.print_info(&format!("Warning: {warning}"));
    }

    if result.ok {
        presenter.print_ok("Tests completed successfully");
    } else {
        presenter.print_info("Tests failed");
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
            file: artifacts.yaxunit_log.display().to_string(),
            console: false,
            level: "info",
        },
    }
}

fn build_enterprise_dsl<'a>(
    config: &AppConfig,
    artifacts: &'a RunArtifacts,
    runner: &'a dyn crate::platform::process::ProcessRunner,
) -> Result<EnterpriseDsl<'a>, AppError> {
    let mut utilities = PlatformUtilities::from_config(config);
    let location = utilities
        .locate(UtilityType::V8C)
        .map_err(|error| AppError::Platform(error.to_string()))?;
    Ok(EnterpriseDsl::new(
        location.path,
        config.v8_connection(),
        runner,
        artifacts.platform_log.clone(),
        Duration::from_secs(config.tests.execution_timeout_seconds),
    ))
}

fn create_run_artifacts(config: &AppConfig) -> std::io::Result<RunArtifacts> {
    let run_id = format!(
        "{}-{}-{}",
        chrono::Utc::now().timestamp_millis(),
        std::process::id(),
        Uuid::new_v4().simple()
    );
    let run_dir = config
        .work_path
        .join("temp")
        .join("yaxunit")
        .join("runs")
        .join(&run_id);
    info!(path = %run_dir.display(), "creating test artifact directory");
    fs::create_dir_all(&run_dir)?;
    set_dir_permissions(&run_dir)?;

    let sentinel = run_dir.join("run.inprogress");
    fs::write(&sentinel, &run_id)?;
    set_file_permissions(&sentinel)?;

    let artifacts = RunArtifacts {
        run_dir: run_dir.clone(),
        config_json: run_dir.join("config.json"),
        junit_xml: run_dir.join("report.xml"),
        yaxunit_log: run_dir.join("yaxunit.log"),
        platform_log: run_dir.join("enterprise.out.log"),
        sentinel,
    };
    Ok(artifacts)
}

fn write_json_file(path: &Path, payload: &impl Serialize) -> std::io::Result<()> {
    fs::write(path, serde_json::to_vec_pretty(payload)?)?;
    set_file_permissions(path)
}

fn parse_junit_report(artifacts: &RunArtifacts) -> Result<TestReport, (TestErrorKind, String)> {
    if !artifacts.junit_xml.exists() {
        return Err((
            TestErrorKind::JunitNotProduced,
            "JUnit report was not produced".to_owned(),
        ));
    }
    if fs::metadata(&artifacts.junit_xml)
        .map(|meta| meta.len() == 0)
        .unwrap_or(false)
    {
        return Err((
            TestErrorKind::JunitEmpty,
            "JUnit report is empty".to_owned(),
        ));
    }
    let file = fs::File::open(&artifacts.junit_xml)
        .map_err(|error| (TestErrorKind::JunitNotProduced, error.to_string()))?;
    let reader = BufReader::new(file);
    junit::parse(reader).map_err(|error| match error {
        JunitError::Empty => (TestErrorKind::JunitEmpty, error.to_string()),
        JunitError::Malformed(_) => (TestErrorKind::JunitMalformed, error.to_string()),
    })
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
) -> std::io::Result<RetainedPaths> {
    Ok(RetainedPaths {
        run_dir: artifacts.run_dir.clone(),
        config_json: artifacts.config_json.clone(),
        junit_xml: artifacts.junit_xml.clone(),
        yaxunit_log: artifacts.yaxunit_log.clone(),
        platform_log: artifacts.platform_log.clone(),
        sentinel: artifacts.sentinel.clone(),
    })
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
        build_yaxunit_config, compact_report, create_run_artifacts, retain_run_artifacts,
        sanitize_text, sanitize_text_full, truncate_stack_trace, RunArtifacts,
    };
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, PlatformToolConfig, SourceFormat, SourceSetConfig,
        SourceSetPurpose, TestsConfig, ToolsConfig,
    };
    use crate::domain::test::{
        TestCase, TestReport, TestStatus, TestSuite, TestSummary, TestTarget,
    };
    use std::fs;
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
            connection: "File=/tmp/ib".to_owned(),
            credentials: Default::default(),
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
            tests: TestsConfig::default(),
        }
    }

    #[test]
    fn creates_distinct_run_dirs() {
        let dir = tempdir().expect("tempdir");
        let config = config(dir.path());
        let first = create_run_artifacts(&config).expect("first");
        let second = create_run_artifacts(&config).expect("second");
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

    fn create_artifacts(root: &std::path::Path) -> RunArtifacts {
        RunArtifacts {
            run_dir: root.join("run"),
            config_json: root.join("run/config.json"),
            junit_xml: root.join("run/report.xml"),
            yaxunit_log: root.join("run/yax.log"),
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
