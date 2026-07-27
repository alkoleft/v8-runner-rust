use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Instant;

use regex::Regex;
use serde::Serialize;
use uuid::Uuid;

use crate::config::model::AppConfig;
use crate::domain::artifact::{
    ArtifactKind, ArtifactRef, ArtifactSet, ARTIFACT_ROLE_ALLURE_RESULTS, ARTIFACT_ROLE_CONFIG,
    ARTIFACT_ROLE_ERROR_DETAILS, ARTIFACT_ROLE_JUNIT_XML, ARTIFACT_ROLE_PLATFORM_LOG,
    ARTIFACT_ROLE_RUNNER_LOG, ARTIFACT_ROLE_RUN_DIR, ARTIFACT_ROLE_SCREENSHOT,
};
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
const OPTIONAL_DIAGNOSTIC_FILE_LIMIT: usize = 100;

mod coordinator;
mod helpers;

use self::helpers::{
    build_enterprise_dsl, build_platform_launch, build_summary, capped_timeout_ms,
    collect_diagnostics, degraded_step, enterprise_error_kind, failed_step,
    interrupted_test_failure, make_test_result, prepare_runner_artifacts, prepared_run_summary,
    skipped_step, succeeded_step, validate_runner_profile_id, validate_target,
    with_retained_artifacts,
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
    reports: Vec<YaXUnitReportConfig>,
    #[serde(rename = "closeAfterTests")]
    close_after_tests: bool,
    #[serde(rename = "showReport")]
    show_report: bool,
    logging: YaXUnitLogging,
}

#[derive(Debug, Serialize)]
struct YaXUnitReportConfig {
    format: &'static str,
    path: String,
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
    allure_results_dir: PathBuf,
    error_details_dir: PathBuf,
    screenshots_dir: PathBuf,
    runner_log: PathBuf,
    platform_log: PathBuf,
    sentinel: PathBuf,
}

impl Drop for RunArtifacts {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.sentinel) {
            if self.sentinel.exists() {
                debug!(path = %self.sentinel.display(), %error, "failed to remove test run sentinel");
            }
        }
    }
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
        reports: vec![
            YaXUnitReportConfig {
                format: "jUnit",
                path: artifacts.junit_xml.display().to_string(),
            },
            YaXUnitReportConfig {
                format: "allure",
                path: artifacts.allure_results_dir.display().to_string(),
            },
        ],
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

    let junit_dir = run_dir.join("junit");
    let artifacts = RunArtifacts {
        run_dir: run_dir.clone(),
        config_json: run_dir.join("config.json"),
        junit_xml: junit_dir.join("report.xml"),
        junit_dir,
        allure_results_dir: run_dir.join("allure-results"),
        error_details_dir: run_dir.join("error-details"),
        screenshots_dir: run_dir.join("screenshots"),
        runner_log: run_dir.join("runner.log"),
        platform_log: run_dir.join("enterprise.out.log"),
        sentinel,
    };
    fs::create_dir_all(&artifacts.junit_dir)?;
    set_dir_permissions(&artifacts.junit_dir)?;
    fs::create_dir_all(&artifacts.allure_results_dir)?;
    set_dir_permissions(&artifacts.allure_results_dir)?;
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
            allure_results_dir: &artifacts.allure_results_dir,
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
    match fs::symlink_metadata(&artifacts.runner_log) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!(
                "failed to materialize Vanessa runner log: destination '{}' is a symlink",
                artifacts.runner_log.display()
            ));
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(format!(
                "failed to materialize Vanessa runner log: destination '{}' is not a regular file",
                artifacts.runner_log.display()
            ));
        }
        Ok(metadata) if metadata.len() > 0 => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "failed to inspect Vanessa runner log destination '{}': {error}",
                artifacts.runner_log.display()
            ));
        }
    }

    let source_metadata = fs::symlink_metadata(&artifacts.platform_log).map_err(|error| {
        format!(
            "failed to materialize Vanessa runner log from enterprise output '{}': {error}",
            artifacts.platform_log.display()
        )
    })?;
    if source_metadata.file_type().is_symlink() {
        return Err(format!(
            "failed to materialize Vanessa runner log: source '{}' is a symlink",
            artifacts.platform_log.display()
        ));
    }
    if !source_metadata.is_file() {
        return Err(format!(
            "failed to materialize Vanessa runner log: source '{}' is not a regular file",
            artifacts.platform_log.display()
        ));
    }

    let temp_path = artifacts
        .run_dir
        .join(format!(".runner-log-{}.tmp", Uuid::new_v4().simple()));
    let result = (|| -> std::io::Result<()> {
        let mut source = open_file_no_follow(&artifacts.platform_log)?;
        let mut temp = create_private_file(&temp_path)?;
        std::io::copy(&mut source, &mut temp)?;
        set_open_file_permissions(&temp)?;
        temp.sync_all()?;
        drop(temp);
        replace_file(&temp_path, &artifacts.runner_log)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result.map_err(|error| {
        format!("failed to materialize Vanessa runner log from enterprise output: {error}")
    })
}

#[cfg(unix)]
fn open_file_no_follow(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = fs::OpenOptions::new();
    options.read(true);
    options.custom_flags(libc::O_NOFOLLOW);
    options.open(path)
}

#[cfg(windows)]
fn open_file_no_follow(path: &Path) -> std::io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    ensure_windows_handle_is_not_reparse_point(&file)?;
    Ok(file)
}

#[cfg(windows)]
fn ensure_windows_handle_is_not_reparse_point(file: &fs::File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT,
    };

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `file` owns a valid live handle and `information` points to writable storage
    // of the exact structure expected by `GetFileInformationByHandle`.
    let succeeded = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "source is a Windows reparse point",
        ));
    }
    Ok(())
}

fn create_private_file(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

fn set_open_file_permissions(file: &fs::File) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = file.metadata()?.permissions();
        permissions.set_mode(0o600);
        file.set_permissions(permissions)?;
    }
    Ok(())
}

#[cfg(unix)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    fn wide_path(path: &Path) -> std::io::Result<Vec<u16>> {
        let parent = path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Windows path has no parent: {}", path.display()),
            )
        })?;
        let file_name = path.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Windows path has no file name: {}", path.display()),
            )
        })?;
        let normalized = fs::canonicalize(parent)?.join(file_name);
        let mut wide = normalized.as_os_str().encode_wide().collect::<Vec<_>>();
        if wide.contains(&0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Windows path contains a NUL code unit",
            ));
        }
        wide.push(0);
        Ok(wide)
    }

    let source = wide_path(source)?;
    let destination = wide_path(destination)?;
    // SAFETY: both vectors are NUL-terminated, contain no interior NULs, and remain alive
    // for the duration of the synchronous `MoveFileExW` call.
    let succeeded = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if succeeded == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
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
                    report.extracted_errors.extend(errors);
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
                    report.extracted_errors.extend(errors);
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

fn discover_junit_reports(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    Ok(collect_regular_files(root)?
        .into_iter()
        .filter(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("xml"))
        })
        .collect())
}

fn parse_junit_reports(reports: &[PathBuf]) -> crate::parsers::NormalizedParse<TestReport> {
    if reports.is_empty() {
        return crate::parsers::NormalizedParse::default().with_errors(vec![test_execution_error(
            TestErrorKind::JunitNotProduced,
            "JUnit report was not produced",
        )]);
    }

    let mut report = TestReport {
        summary: crate::domain::test::TestSummary {
            total: 0,
            passed: 0,
            failed: 0,
            skipped: 0,
            errors: 0,
        },
        suites: Vec::new(),
        extracted_errors: Vec::new(),
    };
    let mut errors = Vec::new();

    for path in reports {
        match fs::File::open(path) {
            Ok(file) => {
                let normalized = junit::parse_normalized(BufReader::new(file));
                if let Some(parsed) = normalized.payload {
                    report.summary.total =
                        report.summary.total.saturating_add(parsed.summary.total);
                    report.summary.passed =
                        report.summary.passed.saturating_add(parsed.summary.passed);
                    report.summary.failed =
                        report.summary.failed.saturating_add(parsed.summary.failed);
                    report.summary.skipped = report
                        .summary
                        .skipped
                        .saturating_add(parsed.summary.skipped);
                    report.summary.errors =
                        report.summary.errors.saturating_add(parsed.summary.errors);
                    report.suites.extend(parsed.suites);
                    report.extracted_errors.extend(parsed.extracted_errors);
                }
                errors.extend(normalized.errors.into_iter().map(|error| {
                    let mut details = vec![format!("JUnit report: {}", path.display())];
                    details.extend(error.details.clone());
                    match error.code.as_str() {
                        "junit_empty" => {
                            test_execution_error(TestErrorKind::JunitEmpty, error.message)
                                .with_details(details)
                        }
                        "junit_malformed" => {
                            test_execution_error(TestErrorKind::JunitMalformed, error.message)
                                .with_details(details)
                        }
                        _ => error.with_details(details),
                    }
                }));
            }
            Err(error) => errors.push(
                test_execution_error(TestErrorKind::JunitNotProduced, error.to_string())
                    .with_details(vec![format!("JUnit report: {}", path.display())]),
            ),
        }
    }

    if !errors.is_empty() {
        return crate::parsers::NormalizedParse::default().with_errors(errors);
    }

    crate::parsers::NormalizedParse::default()
        .with_metrics(ExecutionMetrics::from(&report.summary))
        .with_payload(report)
}

fn parse_junit_report(artifacts: &RunArtifacts) -> crate::parsers::NormalizedParse<TestReport> {
    match discover_junit_reports(&artifacts.junit_dir) {
        Ok(reports) if reports.is_empty() => crate::parsers::NormalizedParse::default()
            .with_errors(vec![test_execution_error(
                TestErrorKind::JunitNotProduced,
                "JUnit report was not produced",
            )
            .with_details(vec![format!(
                "JUnit report directory: {}",
                artifacts.junit_dir.display()
            )])]),
        Ok(reports) => parse_junit_reports(&reports),
        Err(error) => {
            crate::parsers::NormalizedParse::default().with_errors(vec![test_execution_error(
                TestErrorKind::JunitNotProduced,
                error.to_string(),
            )
            .with_details(vec![format!(
                "JUnit report directory: {}",
                artifacts.junit_dir.display()
            )])])
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct AllureValidationFailure {
    kind: TestErrorKind,
    message: String,
    details: Vec<String>,
}

fn validate_allure_results(root: &Path) -> Result<(), AllureValidationFailure> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(AllureValidationFailure {
                kind: TestErrorKind::AllureNotProduced,
                message: "Allure results directory was not produced".to_owned(),
                details: vec![format!("Allure results directory: {}", root.display())],
            });
        }
        Err(error) => {
            return Err(allure_io_failure(root, error));
        }
    };
    if !metadata.is_dir() {
        return Err(AllureValidationFailure {
            kind: TestErrorKind::AllureNotProduced,
            message: "Allure results directory was not produced".to_owned(),
            details: vec![format!("Allure results directory: {}", root.display())],
        });
    }
    let files = collect_regular_files(root).map_err(|error| allure_io_failure(root, error))?;
    if files.is_empty() {
        Err(AllureValidationFailure {
            kind: TestErrorKind::AllureEmpty,
            message: "Allure results directory is empty".to_owned(),
            details: vec![format!("Allure results directory: {}", root.display())],
        })
    } else {
        Ok(())
    }
}

fn allure_io_failure(root: &Path, error: std::io::Error) -> AllureValidationFailure {
    AllureValidationFailure {
        kind: TestErrorKind::TestSetupFailed,
        message: "failed to inspect Allure results".to_owned(),
        details: vec![
            format!("Allure results directory: {}", root.display()),
            error.to_string(),
        ],
    }
}

fn collect_regular_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let root_metadata = fs::symlink_metadata(root).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!("failed to inspect '{}': {error}", root.display()),
        )
    })?;
    if !root_metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("'{}' is not a directory", root.display()),
        ));
    }

    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory).map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!(
                    "failed to read directory '{}': {error}",
                    directory.display()
                ),
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!("failed to read entry in '{}': {error}", directory.display()),
                )
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!("failed to inspect '{}': {error}", path.display()),
                )
            })?;
            if metadata.is_file() {
                files.push(path);
            } else if metadata.is_dir() {
                pending.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn classify_test_completion(
    summary: &crate::domain::test::TestSummary,
    exit_code: i32,
) -> Option<TestErrorKind> {
    if summary.failed > 0 || summary.errors > 0 {
        Some(TestErrorKind::TestFailures)
    } else if exit_code != 0 {
        Some(TestErrorKind::EnterpriseExitedNonZero)
    } else {
        None
    }
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
    Ok(collect_run_artifacts(artifacts))
}

fn collect_run_artifacts(artifacts: &RunArtifacts) -> ArtifactSet {
    let mut collected = if is_existing_dir(&artifacts.run_dir) {
        let mut set = ArtifactSet::with_root(artifacts.run_dir.clone());
        set.push(
            ArtifactRef::new(ArtifactKind::RunDirectory, artifacts.run_dir.clone())
                .with_role(ARTIFACT_ROLE_RUN_DIR),
        );
        set
    } else {
        ArtifactSet::default()
    };

    push_existing_file(
        &mut collected,
        ArtifactKind::Config,
        ARTIFACT_ROLE_CONFIG,
        &artifacts.config_json,
    );

    let mut junit_reports = collect_existing_junit_reports(&artifacts.junit_dir);
    if is_existing_file(&artifacts.junit_xml) && !junit_reports.contains(&artifacts.junit_xml) {
        junit_reports.push(artifacts.junit_xml.clone());
    }
    junit_reports.sort();
    for report in junit_reports {
        push_existing_file(
            &mut collected,
            ArtifactKind::JunitXml,
            ARTIFACT_ROLE_JUNIT_XML,
            &report,
        );
    }

    push_existing_dir(
        &mut collected,
        ArtifactKind::AllureResults,
        ARTIFACT_ROLE_ALLURE_RESULTS,
        &artifacts.allure_results_dir,
    );
    let mut remaining_optional_diagnostics = OPTIONAL_DIAGNOSTIC_FILE_LIMIT;
    push_optional_diagnostics(
        &mut collected,
        ArtifactKind::ErrorDetails,
        ARTIFACT_ROLE_ERROR_DETAILS,
        &artifacts.error_details_dir,
        &mut remaining_optional_diagnostics,
    );
    push_optional_diagnostics(
        &mut collected,
        ArtifactKind::Screenshot,
        ARTIFACT_ROLE_SCREENSHOT,
        &artifacts.screenshots_dir,
        &mut remaining_optional_diagnostics,
    );
    push_existing_file(
        &mut collected,
        ArtifactKind::RunnerLog,
        ARTIFACT_ROLE_RUNNER_LOG,
        &artifacts.runner_log,
    );
    push_existing_file(
        &mut collected,
        ArtifactKind::PlatformLog,
        ARTIFACT_ROLE_PLATFORM_LOG,
        &artifacts.platform_log,
    );

    collected.items.sort_by(|left, right| {
        artifact_kind_sort_key(&left.kind)
            .cmp(artifact_kind_sort_key(&right.kind))
            .then_with(|| left.role.cmp(&right.role))
            .then_with(|| left.path.cmp(&right.path))
    });

    collected
}

#[allow(deprecated)]
fn artifact_kind_sort_key(kind: &ArtifactKind) -> &str {
    match kind {
        ArtifactKind::RunDirectory => "run_directory",
        ArtifactKind::Config => "config",
        ArtifactKind::Package => "package",
        ArtifactKind::Report => "report",
        ArtifactKind::JunitXml => "junit_xml",
        ArtifactKind::AllureResults => "allure_results",
        ArtifactKind::ErrorDetails => "error_details",
        ArtifactKind::Screenshot => "screenshot",
        ArtifactKind::RunnerLog => "runner_log",
        ArtifactKind::PlatformLog => "platform_log",
        ArtifactKind::Sentinel => "sentinel",
        ArtifactKind::Other(value) => value,
    }
}

fn collect_existing_junit_reports(root: &Path) -> Vec<PathBuf> {
    discover_junit_reports(root).unwrap_or_default()
}

fn push_optional_diagnostics(
    set: &mut ArtifactSet,
    kind: ArtifactKind,
    role: &str,
    root: &Path,
    remaining: &mut usize,
) {
    let Ok(paths) = collect_regular_files(root) else {
        return;
    };
    let added = paths.len().min(*remaining);
    for path in paths.iter().take(added) {
        set.push(ArtifactRef::new(kind.clone(), path.clone()).with_role(role));
    }
    *remaining -= added;
    if paths.len() > added {
        set.push(ArtifactRef::new(kind, root.to_path_buf()).with_role(role));
    }
}

fn push_existing_file(set: &mut ArtifactSet, kind: ArtifactKind, role: &str, path: &Path) {
    if is_existing_file(path) {
        set.push(ArtifactRef::new(kind, path).with_role(role));
    }
}

fn push_existing_dir(set: &mut ArtifactSet, kind: ArtifactKind, role: &str, path: &Path) {
    if is_existing_dir(path) {
        set.push(ArtifactRef::new(kind, path).with_role(role));
    }
}

fn is_existing_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
}

fn is_existing_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
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
        build_yaxunit_config, classify_test_completion, collect_run_artifacts, compact_report,
        create_run_artifacts, discover_junit_reports, materialize_vanessa_runner_log,
        parse_junit_report, parse_junit_reports, retain_run_artifacts, run_tests, sanitize_text,
        sanitize_text_full, truncate_stack_trace, validate_allure_results, RunArtifacts,
    };
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, PlatformToolConfig, SourceFormat, SourceSetConfig,
        SourceSetPurpose, TestsConfig, ToolsConfig, VanessaProfileConfig,
    };
    use crate::domain::artifact::{
        ArtifactKind, ARTIFACT_ROLE_ALLURE_RESULTS, ARTIFACT_ROLE_CONFIG,
        ARTIFACT_ROLE_ERROR_DETAILS, ARTIFACT_ROLE_JUNIT_XML, ARTIFACT_ROLE_PLATFORM_LOG,
        ARTIFACT_ROLE_RUNNER_LOG, ARTIFACT_ROLE_SCREENSHOT,
    };
    use crate::domain::execution::{ExecutionStatus, ExecutionTimeouts};
    use crate::domain::runner::{
        ExecutionPolicy, LaunchClientModeRequest, LaunchOptions, RunnerKind, RunnerProfile,
        ScenarioExecutionRequest,
    };
    use crate::domain::test::{
        test_execution_status, TestCase, TestErrorKind, TestReport, TestStatus, TestSuite,
        TestSummary, TestTarget,
    };
    use crate::use_cases::context::{CommandName, ExecutionContext};
    use crate::use_cases::request::{TestRequest, TestScopeRequest};
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[cfg(windows)]
    use super::{open_file_no_follow, replace_file};
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
        assert!(!first.error_details_dir.exists());
        assert!(!first.screenshots_dir.exists());
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
    fn yaxunit_config_serializes_simultaneous_junit_and_allure_reports() {
        let dir = tempdir().expect("tempdir");
        let artifacts = create_artifacts(dir.path());
        let payload = build_yaxunit_config(&TestTarget::All, &artifacts);

        let json = serde_json::to_value(payload).expect("json");

        assert_eq!(json["reports"][0]["format"], "jUnit");
        assert_eq!(
            json["reports"][0]["path"],
            artifacts.junit_xml.display().to_string()
        );
        assert_eq!(json["reports"][1]["format"], "allure");
        assert_eq!(
            json["reports"][1]["path"],
            artifacts.allure_results_dir.display().to_string()
        );
        assert!(json.get("reportFormat").is_none());
    }

    #[test]
    fn collect_run_artifacts_omits_missing_junit_and_keeps_existing_outputs() {
        let dir = tempdir().expect("tempdir");
        let artifacts = create_artifacts(dir.path());
        std::fs::create_dir_all(&artifacts.allure_results_dir).expect("allure dir");
        std::fs::write(&artifacts.config_json, b"{}").expect("config");
        std::fs::write(&artifacts.runner_log, b"runner").expect("runner log");
        std::fs::write(&artifacts.platform_log, b"platform").expect("platform log");

        let collected = collect_run_artifacts(&artifacts);

        assert!(collected.get_by_role(ARTIFACT_ROLE_JUNIT_XML).is_none());
        assert_eq!(
            collected.get_by_role(ARTIFACT_ROLE_CONFIG),
            Some(artifacts.config_json.as_path())
        );
        assert_eq!(
            collected.get_by_role(ARTIFACT_ROLE_ALLURE_RESULTS),
            Some(artifacts.allure_results_dir.as_path())
        );
        assert_eq!(
            collected.get_by_role(ARTIFACT_ROLE_RUNNER_LOG),
            Some(artifacts.runner_log.as_path())
        );
        assert_eq!(
            collected.get_by_role(ARTIFACT_ROLE_PLATFORM_LOG),
            Some(artifacts.platform_log.as_path())
        );
    }

    #[test]
    fn collect_run_artifacts_sorts_public_inventory_by_kind_role_and_path() {
        // Break caught: append order leaking implementation details makes JSON artifacts unstable.
        let dir = tempdir().expect("tempdir");
        let artifacts = create_artifacts(dir.path());
        std::fs::create_dir_all(artifacts.junit_dir.join("nested")).expect("junit dir");
        std::fs::create_dir_all(&artifacts.allure_results_dir).expect("allure dir");
        for path in [
            &artifacts.config_json,
            &artifacts.junit_xml,
            &artifacts.junit_dir.join("nested").join("second.xml"),
            &artifacts.runner_log,
            &artifacts.platform_log,
        ] {
            std::fs::write(path, b"fixture").expect("artifact");
        }

        let collected = collect_run_artifacts(&artifacts);
        let inventory: Vec<_> = collected
            .items
            .iter()
            .map(|item| {
                (
                    item.kind.clone(),
                    item.role.as_deref().expect("role"),
                    item.path.clone(),
                )
            })
            .collect();

        assert_eq!(
            inventory,
            vec![
                (
                    ArtifactKind::AllureResults,
                    ARTIFACT_ROLE_ALLURE_RESULTS,
                    artifacts.allure_results_dir.clone(),
                ),
                (
                    ArtifactKind::Config,
                    ARTIFACT_ROLE_CONFIG,
                    artifacts.config_json.clone(),
                ),
                (
                    ArtifactKind::JunitXml,
                    ARTIFACT_ROLE_JUNIT_XML,
                    artifacts.junit_dir.join("nested").join("second.xml"),
                ),
                (
                    ArtifactKind::JunitXml,
                    ARTIFACT_ROLE_JUNIT_XML,
                    artifacts.junit_xml.clone(),
                ),
                (
                    ArtifactKind::PlatformLog,
                    ARTIFACT_ROLE_PLATFORM_LOG,
                    artifacts.platform_log.clone(),
                ),
                (
                    ArtifactKind::RunDirectory,
                    "run_dir",
                    artifacts.run_dir.clone(),
                ),
                (
                    ArtifactKind::RunnerLog,
                    ARTIFACT_ROLE_RUNNER_LOG,
                    artifacts.runner_log.clone(),
                ),
            ]
        );
        assert!(collected.items.iter().all(|artifact| {
            serde_json::to_value(&artifact.kind).expect("artifact kind")
                != serde_json::json!("sentinel")
        }));
    }

    #[test]
    fn collect_run_artifacts_includes_nested_optional_diagnostics_in_stable_order() {
        let dir = tempdir().expect("tempdir");
        let artifacts = create_artifacts(dir.path());
        std::fs::create_dir_all(artifacts.error_details_dir.join("nested")).expect("error details");
        std::fs::create_dir_all(&artifacts.screenshots_dir).expect("screenshots");
        let later_error = artifacts.error_details_dir.join("z.txt");
        let earlier_error = artifacts.error_details_dir.join("nested/a.txt");
        let screenshot = artifacts.screenshots_dir.join("failure.png");
        std::fs::write(&later_error, "later").expect("later error");
        std::fs::write(&earlier_error, "earlier").expect("earlier error");
        std::fs::write(&screenshot, "png").expect("screenshot");

        let collected = collect_run_artifacts(&artifacts);
        let diagnostics = collected
            .items
            .iter()
            .filter(|artifact| {
                matches!(
                    artifact.kind,
                    ArtifactKind::ErrorDetails | ArtifactKind::Screenshot
                )
            })
            .map(|artifact| {
                (
                    artifact.kind.clone(),
                    artifact.role.clone().expect("role"),
                    artifact.path.clone(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            diagnostics,
            vec![
                (
                    ArtifactKind::ErrorDetails,
                    ARTIFACT_ROLE_ERROR_DETAILS.to_owned(),
                    earlier_error,
                ),
                (
                    ArtifactKind::ErrorDetails,
                    ARTIFACT_ROLE_ERROR_DETAILS.to_owned(),
                    later_error,
                ),
                (
                    ArtifactKind::Screenshot,
                    ARTIFACT_ROLE_SCREENSHOT.to_owned(),
                    screenshot,
                ),
            ]
        );
    }

    #[test]
    fn collect_run_artifacts_bounds_optional_diagnostics_across_categories() {
        // Break caught: independently budgeting categories or serializing every discovered
        // diagnostic file exceeds the shared inventory limit and loses the category fallback.
        const OPTIONAL_DIAGNOSTIC_FILE_LIMIT: usize = 100;

        let dir = tempdir().expect("tempdir");
        let artifacts = create_artifacts(dir.path());
        std::fs::create_dir_all(&artifacts.error_details_dir).expect("error details");
        std::fs::create_dir_all(&artifacts.screenshots_dir).expect("screenshots");
        for index in 0..=OPTIONAL_DIAGNOSTIC_FILE_LIMIT {
            std::fs::write(
                artifacts.error_details_dir.join(format!("{index:03}.txt")),
                "detail",
            )
            .expect("error detail");
        }
        let screenshot = artifacts.screenshots_dir.join("failure.png");
        std::fs::write(&screenshot, "png").expect("screenshot");

        let collected = collect_run_artifacts(&artifacts);
        let diagnostic_files = collected
            .items
            .iter()
            .filter(|artifact| {
                matches!(
                    artifact.kind,
                    ArtifactKind::ErrorDetails | ArtifactKind::Screenshot
                ) && artifact.path.is_file()
            })
            .collect::<Vec<_>>();

        assert_eq!(diagnostic_files.len(), OPTIONAL_DIAGNOSTIC_FILE_LIMIT);
        assert!(collected.items.iter().any(|artifact| {
            artifact.kind == ArtifactKind::ErrorDetails
                && artifact.path == artifacts.error_details_dir
                && artifact.role.as_deref() == Some(ARTIFACT_ROLE_ERROR_DETAILS)
        }));
        assert!(collected.items.iter().any(|artifact| {
            artifact.kind == ArtifactKind::Screenshot
                && artifact.path == artifacts.screenshots_dir
                && artifact.role.as_deref() == Some(ARTIFACT_ROLE_SCREENSHOT)
        }));
        assert!(collected
            .items
            .iter()
            .any(|artifact| artifact.path == artifacts.error_details_dir.join("000.txt")));
        assert!(!collected
            .items
            .iter()
            .any(|artifact| artifact.path == artifacts.error_details_dir.join("100.txt")));
        assert!(!collected
            .items
            .iter()
            .any(|artifact| artifact.path == screenshot));
    }

    #[cfg(unix)]
    #[test]
    fn collect_run_artifacts_skips_symlinked_optional_diagnostics() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("tempdir");
        let artifacts = create_artifacts(dir.path());
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::write(outside.join("secret.txt"), "secret").expect("outside file");
        std::fs::create_dir_all(&artifacts.error_details_dir).expect("error details");
        std::fs::create_dir_all(&artifacts.screenshots_dir).expect("screenshots");
        symlink(&outside, artifacts.error_details_dir.join("external")).expect("dir symlink");
        symlink(
            outside.join("secret.txt"),
            artifacts.screenshots_dir.join("external.png"),
        )
        .expect("file symlink");

        let collected = collect_run_artifacts(&artifacts);

        assert!(collected
            .get_all_by_role(ARTIFACT_ROLE_ERROR_DETAILS)
            .next()
            .is_none());
        assert!(collected
            .get_all_by_role(ARTIFACT_ROLE_SCREENSHOT)
            .next()
            .is_none());
    }

    #[test]
    fn discovers_sorted_nested_junit_reports_and_aggregates_every_report() {
        // Break caught: returning the first report or filesystem iteration order would lose cases.
        let dir = tempdir().expect("tempdir");
        let junit_dir = dir.path().join("junit");
        let nested = junit_dir.join("a");
        std::fs::create_dir_all(&nested).expect("nested junit dir");

        let later_path = junit_dir.join("z-report.xml");
        std::fs::write(
            &later_path,
            r#"<testsuite name="later"><testcase name="failed"><failure/></testcase><testcase name="errored"><error/></testcase></testsuite>"#,
        )
        .expect("later report");
        let earlier_path = nested.join("a-report.xml");
        std::fs::write(
            &earlier_path,
            r#"<testsuite name="earlier"><testcase name="passed"/></testsuite>"#,
        )
        .expect("earlier report");

        let reports = discover_junit_reports(&junit_dir).expect("discover reports");

        assert_eq!(reports, vec![earlier_path, later_path]);
        let parsed = parse_junit_reports(&reports);
        assert!(parsed.errors.is_empty());
        assert_eq!(
            parsed.payload.expect("aggregate report").summary,
            TestSummary {
                total: 3,
                passed: 1,
                failed: 1,
                skipped: 0,
                errors: 1,
            }
        );
    }

    #[test]
    fn rejects_aggregate_when_any_junit_report_is_malformed_with_its_path() {
        // Break caught: accepting the first valid report silently hides invalid native output.
        let dir = tempdir().expect("tempdir");
        let junit_dir = dir.path().join("junit");
        std::fs::create_dir_all(&junit_dir).expect("junit dir");
        let valid = junit_dir.join("a-valid.xml");
        std::fs::write(
            &valid,
            r#"<testsuite name="suite"><testcase name="passed"/></testsuite>"#,
        )
        .expect("valid report");
        let malformed = junit_dir.join("b-malformed.xml");
        std::fs::write(&malformed, "<testsuite>").expect("malformed report");
        let another_malformed = junit_dir.join("c-malformed.xml");
        std::fs::write(&another_malformed, "<testsuite><testcase>")
            .expect("another malformed report");

        let parsed = parse_junit_reports(&[valid, malformed.clone(), another_malformed.clone()]);

        assert!(parsed.payload.is_none());
        assert_eq!(parsed.errors.len(), 2);
        for path in [malformed, another_malformed] {
            assert!(parsed.errors.iter().any(|error| {
                error.code == TestErrorKind::JunitMalformed.code()
                    && error
                        .details
                        .iter()
                        .any(|detail| detail.contains(&path.display().to_string()))
            }));
        }
    }

    #[test]
    fn validates_missing_and_empty_allure_results() {
        // Break caught: treating pre-created or missing Allure directories as valid native output.
        let dir = tempdir().expect("tempdir");
        let missing = dir.path().join("missing-allure-results");
        let empty = dir.path().join("empty-allure-results");
        std::fs::create_dir_all(empty.join("nested")).expect("empty allure dir");

        assert_eq!(
            validate_allure_results(&missing).expect_err("missing").kind,
            TestErrorKind::AllureNotProduced
        );
        assert_eq!(
            validate_allure_results(&empty).expect_err("empty").kind,
            TestErrorKind::AllureEmpty
        );
    }

    #[test]
    fn accepts_allure_results_with_a_nested_regular_file() {
        let dir = tempdir().expect("tempdir");
        let allure = dir.path().join("allure-results");
        std::fs::create_dir_all(allure.join("nested")).expect("allure dir");
        std::fs::write(allure.join("nested/result.json"), "{}").expect("allure result");

        assert_eq!(validate_allure_results(&allure), Ok(()));
    }

    #[cfg(unix)]
    #[test]
    fn discovery_and_allure_validation_ignore_symlinked_outputs() {
        use std::os::unix::fs::symlink;

        // Break caught: following symlinks can escape the run directory or make empty output valid.
        let dir = tempdir().expect("tempdir");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).expect("outside dir");
        std::fs::write(
            outside.join("report.xml"),
            "<testsuite name=\"s\"><testcase name=\"p\"/></testsuite>",
        )
        .expect("outside junit");
        std::fs::write(outside.join("result.json"), "{}").expect("outside allure");

        let junit_dir = dir.path().join("junit");
        let allure_dir = dir.path().join("allure-results");
        std::fs::create_dir_all(&junit_dir).expect("junit dir");
        std::fs::create_dir_all(&allure_dir).expect("allure dir");
        symlink(&outside, junit_dir.join("external")).expect("junit symlink");
        symlink(
            outside.join("result.json"),
            allure_dir.join("external.json"),
        )
        .expect("allure symlink");

        assert!(discover_junit_reports(&junit_dir)
            .expect("discover reports")
            .is_empty());
        assert_eq!(
            validate_allure_results(&allure_dir)
                .expect_err("symlinks do not count")
                .kind,
            TestErrorKind::AllureEmpty
        );
    }

    #[cfg(unix)]
    #[test]
    fn allure_traversal_io_failure_preserves_path_and_os_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().expect("tempdir");
        let allure = dir.path().join("allure-results");
        let unreadable = allure.join("unreadable");
        std::fs::create_dir_all(&unreadable).expect("allure dir");
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000))
            .expect("restrict");

        let failure = validate_allure_results(&allure).expect_err("traversal failure");
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o700))
            .expect("restore");

        assert_eq!(failure.kind, TestErrorKind::TestSetupFailed);
        assert!(failure
            .details
            .iter()
            .any(|detail| detail.contains(&unreadable.display().to_string())));
        assert!(failure
            .details
            .iter()
            .any(|detail| detail.contains("Permission denied")));
    }

    #[test]
    fn classifies_native_reports_before_process_exit_status() {
        // Break caught: nonzero process exits masking report-proven test failures.
        let cases = [
            (
                TestSummary {
                    total: 1,
                    passed: 0,
                    failed: 1,
                    skipped: 0,
                    errors: 0,
                },
                1,
                Some(TestErrorKind::TestFailures),
                ExecutionStatus::Failed,
            ),
            (
                TestSummary {
                    total: 1,
                    passed: 0,
                    failed: 0,
                    skipped: 0,
                    errors: 1,
                },
                2,
                Some(TestErrorKind::TestFailures),
                ExecutionStatus::Failed,
            ),
            (
                TestSummary {
                    total: 1,
                    passed: 1,
                    failed: 0,
                    skipped: 0,
                    errors: 0,
                },
                1,
                Some(TestErrorKind::EnterpriseExitedNonZero),
                ExecutionStatus::Failed,
            ),
            (
                TestSummary {
                    total: 1,
                    passed: 1,
                    failed: 0,
                    skipped: 0,
                    errors: 0,
                },
                0,
                None,
                ExecutionStatus::Succeeded,
            ),
        ];

        for (summary, exit_code, expected, expected_status) in cases {
            let is_success = expected.is_none();
            assert_eq!(classify_test_completion(&summary, exit_code), expected);
            assert_eq!(test_execution_status(expected, is_success), expected_status);
        }
    }

    #[test]
    fn dropping_run_artifacts_removes_sentinel_but_keeps_run_directory() {
        let dir = tempdir().expect("tempdir");
        let config = config(dir.path());
        let (run_dir, sentinel) = {
            let artifacts = create_run_artifacts(&config, "yaxunit").expect("artifacts");
            assert!(artifacts.sentinel.is_file());
            (artifacts.run_dir.clone(), artifacts.sentinel.clone())
        };

        assert!(run_dir.is_dir());
        assert!(!sentinel.exists());
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

    #[cfg(windows)]
    #[test]
    fn windows_atomic_replace_supports_extended_length_paths() {
        let dir = tempdir().expect("tempdir");
        let mut long_dir = dir.path().to_path_buf();
        while long_dir.as_os_str().len() <= 300 {
            long_dir.push("long-path-segment");
        }
        std::fs::create_dir_all(&long_dir).expect("long directory");
        let source = long_dir.join("source.tmp");
        let destination = long_dir.join("runner.log");
        std::fs::write(&source, "replacement").expect("source");
        std::fs::write(&destination, "old").expect("destination");

        replace_file(&source, &destination).expect("atomic replace");

        assert_eq!(
            std::fs::read_to_string(&destination).expect("destination"),
            "replacement"
        );
        assert!(!source.exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_materialize_vanessa_runner_log_rejects_reparse_source() {
        use std::os::windows::fs::symlink_file;

        let dir = tempdir().expect("tempdir");
        let artifacts = create_artifacts(dir.path());
        std::fs::create_dir_all(&artifacts.run_dir).expect("run dir");
        let outside = dir.path().join("outside.log");
        std::fs::write(&outside, "outside").expect("outside");
        symlink_file(&outside, &artifacts.platform_log).expect("source symlink");

        let warning = materialize_vanessa_runner_log(&artifacts).expect_err("warning");
        assert!(warning.contains("symlink"));
        assert!(!artifacts.runner_log.exists());

        let error = open_file_no_follow(&artifacts.platform_log).expect_err("reparse point");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn materialize_vanessa_runner_log_rejects_symlink_source() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("tempdir");
        let artifacts = create_artifacts(dir.path());
        std::fs::create_dir_all(&artifacts.run_dir).expect("run dir");
        let outside = dir.path().join("outside.log");
        std::fs::write(&outside, "outside").expect("outside");
        symlink(&outside, &artifacts.platform_log).expect("source symlink");

        let warning = materialize_vanessa_runner_log(&artifacts).expect_err("warning");

        assert!(warning.contains("symlink"));
        assert_eq!(
            std::fs::read_to_string(&outside).expect("outside"),
            "outside"
        );
        assert!(!artifacts.runner_log.exists());
    }

    #[cfg(unix)]
    #[test]
    fn materialize_vanessa_runner_log_rejects_symlink_destination() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("tempdir");
        let artifacts = create_artifacts(dir.path());
        std::fs::create_dir_all(&artifacts.run_dir).expect("run dir");
        std::fs::write(&artifacts.platform_log, "platform").expect("platform");
        let outside = dir.path().join("outside.log");
        std::fs::write(&outside, "outside").expect("outside");
        symlink(&outside, &artifacts.runner_log).expect("destination symlink");

        let warning = materialize_vanessa_runner_log(&artifacts).expect_err("warning");

        assert!(warning.contains("symlink"));
        assert_eq!(
            std::fs::read_to_string(&outside).expect("outside"),
            "outside"
        );
    }

    #[test]
    fn vanessa_junit_parse_failure_inventories_materialized_runner_log() {
        let dir = tempdir().expect("tempdir");
        let config = config(dir.path());
        let artifacts = create_artifacts(dir.path());
        std::fs::create_dir_all(&artifacts.run_dir).expect("run dir");
        std::fs::create_dir_all(&artifacts.junit_dir).expect("junit dir");
        std::fs::write(&artifacts.platform_log, b"enterprise /Out").expect("platform log");

        materialize_vanessa_runner_log(&artifacts).expect("materialize log");
        let junit_parse = parse_junit_report(&artifacts);
        assert!(junit_parse.payload.is_none());
        assert_eq!(
            junit_parse.errors[0].code,
            TestErrorKind::JunitNotProduced.code()
        );
        assert!(junit_parse.errors[0]
            .details
            .iter()
            .any(|detail| detail.contains(&artifacts.junit_dir.display().to_string())));

        let retained = retain_run_artifacts(&config, &artifacts).expect("retain artifacts");
        assert_eq!(
            retained.get_by_role(ARTIFACT_ROLE_RUNNER_LOG),
            Some(artifacts.runner_log.as_path())
        );
        assert!(retained.get_by_role(ARTIFACT_ROLE_JUNIT_XML).is_none());
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
            build_policy: crate::use_cases::request::TestBuildPolicy::BuildFirst,
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

        let context = ExecutionContext::cli(CommandName::Test);
        let result = super::run_tests(&context, &config, &args);
        assert!(result.is_err());
        let error = result.err().expect("error");
        assert!(error.error.to_string().contains("unsafe path characters"));
        assert!(!dir.path().join("temp").exists());
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
            build_policy: crate::use_cases::request::TestBuildPolicy::BuildFirst,
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
        };

        let failure = run_tests(&context, &config, &args).expect_err("cancelled");
        let payload = failure.payload.expect("payload");

        assert_eq!(payload.execution.status, ExecutionStatus::Cancelled);
        assert_eq!(payload.execution.interruptions.len(), 1);
        assert!(payload.execution.errors.is_empty());
    }

    fn create_artifacts(root: &std::path::Path) -> RunArtifacts {
        let run_dir = root.join("run");
        let junit_dir = run_dir.join("junit");
        RunArtifacts {
            run_dir: run_dir.clone(),
            config_json: run_dir.join("config.json"),
            junit_xml: junit_dir.join("report.xml"),
            junit_dir: root.join("run/junit"),
            allure_results_dir: run_dir.join("allure-results"),
            error_details_dir: run_dir.join("error-details"),
            screenshots_dir: run_dir.join("screenshots"),
            runner_log: run_dir.join("yax.log"),
            platform_log: run_dir.join("platform.log"),
            sentinel: run_dir.join("run.inprogress"),
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
