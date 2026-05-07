use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Instant;

use regex::Regex;
use serde::Serialize;
use uuid::Uuid;

use crate::config::model::AppConfig;
use crate::domain::artifact::ArtifactSet;
use crate::domain::execution::{
    ExecutionMetrics, ExecutionOutcome, ExecutionStatus, ExecutionStepKind, StepResult,
};
use crate::domain::runner::LaunchClientModeRequest;
use crate::domain::test::{
    test_execution_error, test_execution_status, TestErrorKind, TestOutputMode, TestReport,
    TestRunResult, TestStatus, TestTarget,
};
use crate::parsers::junit;
use crate::parsers::vanessa_log;
use crate::parsers::yaxunit_log;
use crate::support::error::AppError;
use crate::use_cases::build_project;
use crate::use_cases::context::ExecutionContext;
use crate::use_cases::request::{BuildRequest as BuildArgs, TestRequest as TestArgs};
use crate::use_cases::result::{UseCaseFailure, UseCaseResult};
use crate::use_cases::vanessa::{self, VanessaTestArtifacts};
use tracing::debug;

const STACK_TRACE_LIMIT: usize = 500;

mod coordinator;
mod helpers;

use self::helpers::{
    apply_test_mcp_ws_payload, build_enterprise_dsl, build_platform_launch, build_summary,
    capped_timeout_ms, collect_diagnostics, degraded_step, enterprise_error_kind, failed_step,
    interrupted_test_failure, make_test_result, prepare_runner_artifacts, prepared_run_summary,
    succeeded_step, validate_runner_profile_id, validate_target, with_retained_artifacts,
};

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
    run_tests(context, config, args)
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

fn run_tests(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &TestArgs,
) -> UseCaseResult<TestRunResult> {
    coordinator::run_tests(context, config, args)
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
    let profile_name = args.execution.profile.id.as_str();
    let launch = vanessa::prepare_test_launch(
        config,
        profile_name,
        VanessaTestArtifacts {
            run_dir: &artifacts.run_dir,
            junit_dir: &artifacts.junit_dir,
            runner_log: &artifacts.runner_log,
        },
    )?;
    artifacts.config_json = launch.params_path.clone();

    Ok(PreparedRun::Vanessa {
        epf_path: launch.epf_path,
        params_path: launch.params_path,
    })
}

fn materialize_vanessa_runner_log(artifacts: &RunArtifacts) -> Result<(), String> {
    if artifacts
        .runner_log
        .metadata()
        .is_ok_and(|metadata| metadata.len() > 0)
    {
        return Ok(());
    }
    fs::copy(&artifacts.platform_log, &artifacts.runner_log).map_err(|error| {
        format!("failed to materialize Vanessa runner log from enterprise output: {error}")
    })?;
    set_file_permissions(&artifacts.runner_log)
        .map_err(|error| format!("failed to chmod Vanessa runner log: {error}"))
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
                steps.push(
                    succeeded_step(
                        "parse_log",
                        ExecutionStepKind::ParseOutput,
                        parse_log_started.elapsed().as_millis() as u64,
                        format!(
                            "extracted {} YaXUnit error block(s)",
                            report.extracted_errors.len()
                        ),
                    )
                    .with_target(runner_log_path.display().to_string()),
                );
            }
            Err(error) => {
                warnings.push(format!("failed to read YaXUnit log: {error}"));
                steps.push(
                    degraded_step(
                        "parse_log",
                        ExecutionStepKind::ParseOutput,
                        parse_log_started.elapsed().as_millis() as u64,
                        format!("failed to read YaXUnit log: {error}"),
                    )
                    .with_target(runner_log_path.display().to_string()),
                );
            }
        },
        PreparedRun::Vanessa { .. } => match vanessa_log::normalize_file(runner_log_path) {
            Ok(parsed) => {
                if let Some(errors) = parsed.payload {
                    report.extracted_errors = errors;
                }
                warnings.extend(parsed.warnings);
                steps.push(
                    succeeded_step(
                        "parse_log",
                        ExecutionStepKind::ParseOutput,
                        parse_log_started.elapsed().as_millis() as u64,
                        format!(
                            "extracted {} Vanessa Automation log line(s)",
                            report.extracted_errors.len()
                        ),
                    )
                    .with_target(runner_log_path.display().to_string()),
                );
            }
            Err(error) => {
                warnings.push(format!("failed to read Vanessa Automation log: {error}"));
                steps.push(
                    degraded_step(
                        "parse_log",
                        ExecutionStepKind::ParseOutput,
                        parse_log_started.elapsed().as_millis() as u64,
                        format!("failed to read Vanessa Automation log: {error}"),
                    )
                    .with_target(runner_log_path.display().to_string()),
                );
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
        parse_junit_report, retain_run_artifacts, run_tests, sanitize_text, sanitize_text_full,
        truncate_stack_trace, RunArtifacts,
    };
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, PlatformToolConfig, SourceFormat, SourceSetConfig,
        SourceSetPurpose, TestsConfig, ToolsConfig, VanessaProfileConfig,
    };
    use crate::domain::execution::{ExecutionStatus, ExecutionTimeouts};
    use crate::domain::runner::{
        ExecutionPolicy, LaunchClientModeRequest, LaunchOptions, RunnerKind, RunnerProfile,
        ScenarioExecutionRequest,
    };
    use crate::domain::test::{
        TestCase, TestErrorKind, TestReport, TestStatus, TestSuite, TestSummary, TestTarget,
    };
    use crate::use_cases::context::{CommandName, ExecutionContext};
    use crate::use_cases::request::{TestRequest, TestScopeRequest};
    use std::path::PathBuf;
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    fn config(work_path: &std::path::Path) -> AppConfig {
        let base = work_path.join("base");
        std::fs::create_dir_all(base.join("main")).expect("base");
        AppConfig {
            base_path: base.clone(),
            work_path: work_path.to_path_buf(),
            execution_timeout: 300_000,
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
    fn materialize_vanessa_runner_log_falls_back_when_runner_log_is_empty() {
        let dir = tempdir().expect("tempdir");
        let artifacts = create_artifacts(dir.path());
        std::fs::create_dir_all(&artifacts.run_dir).expect("run dir");
        std::fs::write(&artifacts.platform_log, b"enterprise /Out").expect("write platform log");
        std::fs::write(&artifacts.runner_log, b"").expect("write empty runner log");

        materialize_vanessa_runner_log(&artifacts).expect("materialize log");

        let copied = std::fs::read(&artifacts.runner_log).expect("read runner log");
        assert_eq!(copied, b"enterprise /Out");
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

        config.tools.va.epf_path = Some(epf);
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
            mcp_ws: crate::use_cases::request::McpClientWsRequest::default(),
        };

        let context = ExecutionContext::cli(CommandName::Test);
        let result = super::run_tests(&context, &config, &args);
        assert!(result.is_err());
        let error = result.err().expect("error");
        assert!(error.error.to_string().contains("unsafe path characters"));
    }

    #[test]
    fn run_tests_reports_cancelled_execution_before_first_safe_point() {
        let dir = tempdir().expect("tempdir");
        let config = config(dir.path());
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let context = ExecutionContext::cli(CommandName::Test).with_cancellation(cancellation);
        let args = TestRequest {
            full: false,
            scope: TestScopeRequest::All,
            execution: ScenarioExecutionRequest {
                profile: RunnerProfile {
                    id: "yaxunit".to_owned(),
                    kind: RunnerKind::YaXUnit,
                    output_formats: vec![],
                    backend_hint: Some("enterprise".to_owned()),
                },
                client_mode: Some(LaunchClientModeRequest::Thin),
                timeouts: ExecutionTimeouts::default(),
                policy: ExecutionPolicy::default(),
                launch: LaunchOptions::default(),
            },
            mcp_ws: crate::use_cases::request::McpClientWsRequest::default(),
        };

        let failure = run_tests(&context, &config, &args).expect_err("cancelled");
        let payload = failure.payload.expect("payload");

        assert_eq!(payload.execution.status, ExecutionStatus::Cancelled);
        assert_eq!(payload.execution.interruptions.len(), 1);
        assert!(payload.execution.errors.is_empty());
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
