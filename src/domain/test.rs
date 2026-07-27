use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[cfg(test)]
use crate::domain::artifact::{ArtifactKind, ArtifactRef};
use crate::domain::artifact::{
    ArtifactSet, ARTIFACT_ROLE_ALLURE_RESULTS, ARTIFACT_ROLE_CONFIG, ARTIFACT_ROLE_JUNIT_XML,
    ARTIFACT_ROLE_PLATFORM_LOG, ARTIFACT_ROLE_REPORT, ARTIFACT_ROLE_RUNNER_LOG,
    ARTIFACT_ROLE_RUN_DIR,
};
use crate::domain::execution::{
    ExecutionError, ExecutionMetrics, ExecutionOutcome, ExecutionStatus, StepResult,
};

pub const TEST_ERROR_CODE_BUILD_FAILED: &str = "build_failed";
pub const TEST_ERROR_CODE_INFOBASE_UNAVAILABLE: &str = "infobase_unavailable";
pub const TEST_ERROR_CODE_TEST_SETUP_FAILED: &str = "test_setup_failed";
pub const TEST_ERROR_CODE_ENTERPRISE_SPAWN_FAILED: &str = "enterprise_spawn_failed";
pub const TEST_ERROR_CODE_ENTERPRISE_STARTUP_CHECK_FAILED: &str = "enterprise_startup_check_failed";
pub const TEST_ERROR_CODE_ENTERPRISE_EXITED_EARLY: &str = "enterprise_exited_early";
pub const TEST_ERROR_CODE_ENTERPRISE_STDOUT_LOG_IO: &str = "enterprise_stdout_log_io";
pub const TEST_ERROR_CODE_ENTERPRISE_STDERR_LOG_IO: &str = "enterprise_stderr_log_io";
pub const TEST_ERROR_CODE_ENTERPRISE_TIMED_OUT: &str = "enterprise_timed_out";
pub const TEST_ERROR_CODE_ENTERPRISE_EXITED_NON_ZERO: &str = "enterprise_exited_non_zero";
pub const TEST_ERROR_CODE_TEST_FAILURES: &str = "test_failures";
pub const TEST_ERROR_CODE_JUNIT_NOT_PRODUCED: &str = "junit_not_produced";
pub const TEST_ERROR_CODE_JUNIT_EMPTY: &str = "junit_empty";
pub const TEST_ERROR_CODE_JUNIT_MALFORMED: &str = "junit_malformed";
pub const TEST_ERROR_CODE_ALLURE_NOT_PRODUCED: &str = "allure_not_produced";
pub const TEST_ERROR_CODE_ALLURE_EMPTY: &str = "allure_empty";
pub const TEST_RUNNER_ID: &str = "yaxunit";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TestTarget {
    All,
    Module { name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TestOutputMode {
    Compact,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TestErrorKind {
    BuildFailed,
    InfobaseUnavailable,
    TestSetupFailed,
    EnterpriseSpawnFailed,
    EnterpriseStartupCheckFailed,
    EnterpriseExitedEarly,
    EnterpriseStdoutLogIo,
    EnterpriseStderrLogIo,
    EnterpriseTimedOut,
    EnterpriseExitedNonZero,
    TestFailures,
    JunitNotProduced,
    JunitEmpty,
    JunitMalformed,
    AllureNotProduced,
    AllureEmpty,
}

impl TestErrorKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::BuildFailed => TEST_ERROR_CODE_BUILD_FAILED,
            Self::InfobaseUnavailable => TEST_ERROR_CODE_INFOBASE_UNAVAILABLE,
            Self::TestSetupFailed => TEST_ERROR_CODE_TEST_SETUP_FAILED,
            Self::EnterpriseSpawnFailed => TEST_ERROR_CODE_ENTERPRISE_SPAWN_FAILED,
            Self::EnterpriseStartupCheckFailed => TEST_ERROR_CODE_ENTERPRISE_STARTUP_CHECK_FAILED,
            Self::EnterpriseExitedEarly => TEST_ERROR_CODE_ENTERPRISE_EXITED_EARLY,
            Self::EnterpriseStdoutLogIo => TEST_ERROR_CODE_ENTERPRISE_STDOUT_LOG_IO,
            Self::EnterpriseStderrLogIo => TEST_ERROR_CODE_ENTERPRISE_STDERR_LOG_IO,
            Self::EnterpriseTimedOut => TEST_ERROR_CODE_ENTERPRISE_TIMED_OUT,
            Self::EnterpriseExitedNonZero => TEST_ERROR_CODE_ENTERPRISE_EXITED_NON_ZERO,
            Self::TestFailures => TEST_ERROR_CODE_TEST_FAILURES,
            Self::JunitNotProduced => TEST_ERROR_CODE_JUNIT_NOT_PRODUCED,
            Self::JunitEmpty => TEST_ERROR_CODE_JUNIT_EMPTY,
            Self::JunitMalformed => TEST_ERROR_CODE_JUNIT_MALFORMED,
            Self::AllureNotProduced => TEST_ERROR_CODE_ALLURE_NOT_PRODUCED,
            Self::AllureEmpty => TEST_ERROR_CODE_ALLURE_EMPTY,
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            TEST_ERROR_CODE_BUILD_FAILED => Self::BuildFailed,
            TEST_ERROR_CODE_INFOBASE_UNAVAILABLE => Self::InfobaseUnavailable,
            TEST_ERROR_CODE_TEST_SETUP_FAILED => Self::TestSetupFailed,
            TEST_ERROR_CODE_ENTERPRISE_SPAWN_FAILED => Self::EnterpriseSpawnFailed,
            TEST_ERROR_CODE_ENTERPRISE_STARTUP_CHECK_FAILED => Self::EnterpriseStartupCheckFailed,
            TEST_ERROR_CODE_ENTERPRISE_EXITED_EARLY => Self::EnterpriseExitedEarly,
            TEST_ERROR_CODE_ENTERPRISE_STDOUT_LOG_IO => Self::EnterpriseStdoutLogIo,
            TEST_ERROR_CODE_ENTERPRISE_STDERR_LOG_IO => Self::EnterpriseStderrLogIo,
            TEST_ERROR_CODE_ENTERPRISE_TIMED_OUT => Self::EnterpriseTimedOut,
            TEST_ERROR_CODE_ENTERPRISE_EXITED_NON_ZERO => Self::EnterpriseExitedNonZero,
            TEST_ERROR_CODE_TEST_FAILURES => Self::TestFailures,
            TEST_ERROR_CODE_JUNIT_NOT_PRODUCED => Self::JunitNotProduced,
            TEST_ERROR_CODE_JUNIT_EMPTY => Self::JunitEmpty,
            TEST_ERROR_CODE_JUNIT_MALFORMED => Self::JunitMalformed,
            TEST_ERROR_CODE_ALLURE_NOT_PRODUCED => Self::AllureNotProduced,
            TEST_ERROR_CODE_ALLURE_EMPTY => Self::AllureEmpty,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetainedPaths {
    pub run_dir: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_json: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub junit_xml: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allure_results: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yaxunit_log: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_log: Option<PathBuf>,
}

impl RetainedPaths {
    #[cfg(test)]
    pub fn into_artifact_set(self) -> ArtifactSet {
        let mut set = ArtifactSet::with_root(self.run_dir.clone());
        set.push(
            ArtifactRef::new(ArtifactKind::RunDirectory, self.run_dir)
                .with_role(ARTIFACT_ROLE_RUN_DIR),
        );
        if let Some(config_json) = self.config_json {
            set.push(
                ArtifactRef::new(ArtifactKind::Config, config_json).with_role(ARTIFACT_ROLE_CONFIG),
            );
        }
        if let Some(junit_xml) = self.junit_xml {
            set.push(
                ArtifactRef::new(ArtifactKind::JunitXml, junit_xml)
                    .with_role(ARTIFACT_ROLE_JUNIT_XML),
            );
        }
        if let Some(allure_results) = self.allure_results {
            set.push(
                ArtifactRef::new(ArtifactKind::AllureResults, allure_results)
                    .with_role(ARTIFACT_ROLE_ALLURE_RESULTS),
            );
        }
        if let Some(yaxunit_log) = self.yaxunit_log {
            set.push(
                ArtifactRef::new(ArtifactKind::RunnerLog, yaxunit_log)
                    .with_role(ARTIFACT_ROLE_RUNNER_LOG),
            );
        }
        if let Some(platform_log) = self.platform_log {
            set.push(
                ArtifactRef::new(ArtifactKind::PlatformLog, platform_log)
                    .with_role(ARTIFACT_ROLE_PLATFORM_LOG),
            );
        }
        set
    }

    pub fn from_artifact_set(set: &ArtifactSet) -> Option<Self> {
        Some(Self {
            run_dir: set.get_by_role(ARTIFACT_ROLE_RUN_DIR)?.to_path_buf(),
            config_json: set.get_by_role(ARTIFACT_ROLE_CONFIG).map(Path::to_path_buf),
            junit_xml: set
                .get_all_by_role(ARTIFACT_ROLE_JUNIT_XML)
                .min()
                .or_else(|| set.get_all_by_role(ARTIFACT_ROLE_REPORT).min())
                .map(Path::to_path_buf),
            allure_results: set
                .get_by_role(ARTIFACT_ROLE_ALLURE_RESULTS)
                .map(Path::to_path_buf),
            yaxunit_log: set
                .get_by_role(ARTIFACT_ROLE_RUNNER_LOG)
                .map(Path::to_path_buf),
            platform_log: set
                .get_by_role(ARTIFACT_ROLE_PLATFORM_LOG)
                .map(Path::to_path_buf),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TestRunResult {
    pub target: TestTarget,
    pub mode: TestOutputMode,
    #[serde(skip)]
    pub warnings: Vec<String>,
    #[serde(skip)]
    pub steps: Vec<StepResult>,
    #[serde(skip)]
    pub duration_ms: u64,
    pub execution: ExecutionOutcome<TestReport>,
}

impl TestRunResult {
    pub fn from_outcome(
        mut outcome: ExecutionOutcome<TestReport>,
        target: TestTarget,
        mode: TestOutputMode,
        warnings: Vec<String>,
        steps: Vec<StepResult>,
        duration_ms: u64,
    ) -> Self {
        let metrics = outcome.metrics.clone();
        if let (Some(report), Some(metrics)) = (outcome.payload.as_mut(), metrics.as_ref()) {
            report.summary = TestSummary::from(metrics.clone());
        }

        Self {
            target,
            mode,
            warnings,
            steps,
            duration_ms,
            execution: outcome,
        }
    }

    #[cfg(test)]
    pub fn to_outcome(&self) -> ExecutionOutcome<TestReport> {
        self.execution.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TestReport {
    pub summary: TestSummary,
    pub suites: Vec<TestSuite>,
    pub extracted_errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TestSummary {
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub errors: u32,
}

impl From<TestSummary> for ExecutionMetrics {
    fn from(value: TestSummary) -> Self {
        Self {
            total: value.total,
            passed: value.passed,
            failed: value.failed,
            skipped: value.skipped,
            errors: value.errors,
            extra: Default::default(),
        }
    }
}

impl From<&TestSummary> for ExecutionMetrics {
    fn from(value: &TestSummary) -> Self {
        value.clone().into()
    }
}

impl From<ExecutionMetrics> for TestSummary {
    fn from(value: ExecutionMetrics) -> Self {
        Self {
            total: value.total,
            passed: value.passed,
            failed: value.failed,
            skipped: value.skipped,
            errors: value.errors,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TestSuite {
    pub name: String,
    pub cases: Vec<TestCase>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TestCase {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_name: Option<String>,
    pub status: TestStatus,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack_trace: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TestStatus {
    Passed,
    Failed,
    Skipped,
    Error,
}

impl Default for TestStatus {
    fn default() -> Self {
        Self::Passed
    }
}

pub fn test_execution_error(kind: TestErrorKind, message: impl Into<String>) -> ExecutionError {
    ExecutionError::new(kind.code(), message)
}

pub fn test_execution_status(kind: Option<TestErrorKind>, ok: bool) -> ExecutionStatus {
    if ok {
        return ExecutionStatus::Succeeded;
    }

    match kind {
        Some(TestErrorKind::EnterpriseTimedOut) => ExecutionStatus::TimedOut,
        Some(
            TestErrorKind::JunitMalformed
            | TestErrorKind::JunitEmpty
            | TestErrorKind::JunitNotProduced
            | TestErrorKind::AllureNotProduced
            | TestErrorKind::AllureEmpty,
        ) => ExecutionStatus::InvalidOutput,
        Some(
            TestErrorKind::BuildFailed
            | TestErrorKind::InfobaseUnavailable
            | TestErrorKind::TestSetupFailed
            | TestErrorKind::EnterpriseSpawnFailed
            | TestErrorKind::EnterpriseStartupCheckFailed
            | TestErrorKind::EnterpriseExitedEarly
            | TestErrorKind::EnterpriseStdoutLogIo
            | TestErrorKind::EnterpriseStderrLogIo
            | TestErrorKind::EnterpriseExitedNonZero
            | TestErrorKind::TestFailures,
        )
        | None => ExecutionStatus::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        test_execution_error, RetainedPaths, TestErrorKind, TestOutputMode, TestReport,
        TestRunResult, TestSummary, TestTarget,
    };
    use crate::domain::artifact::{
        ArtifactKind, ArtifactRef, ArtifactSet, ARTIFACT_ROLE_JUNIT_XML, ARTIFACT_ROLE_REPORT,
        ARTIFACT_ROLE_RUN_DIR,
    };
    use crate::domain::execution::{ExecutionMetrics, ExecutionOutcome, ExecutionStatus};
    use std::path::PathBuf;

    #[test]
    fn retained_paths_roundtrip_to_artifact_set() {
        let retained = RetainedPaths {
            run_dir: PathBuf::from("/tmp/run"),
            config_json: Some(PathBuf::from("/tmp/config.json")),
            junit_xml: Some(PathBuf::from("/tmp/z-report.xml")),
            allure_results: Some(PathBuf::from("/tmp/allure-results")),
            yaxunit_log: Some(PathBuf::from("/tmp/yaxunit.log")),
            platform_log: Some(PathBuf::from("/tmp/platform.log")),
        };

        let first_junit = PathBuf::from("/tmp/a-report.xml");
        let mut expected = retained.clone();
        expected.junit_xml = Some(first_junit.clone());
        let mut set = retained.into_artifact_set();
        set.push(
            ArtifactRef::new(ArtifactKind::JunitXml, first_junit)
                .with_role(ARTIFACT_ROLE_JUNIT_XML),
        );

        assert_eq!(RetainedPaths::from_artifact_set(&set), Some(expected));
    }

    #[test]
    fn retained_paths_falls_back_to_legacy_report_role() {
        let mut set = ArtifactSet::with_root("/tmp/run");
        set.push(
            ArtifactRef::new(ArtifactKind::RunDirectory, "/tmp/run")
                .with_role(ARTIFACT_ROLE_RUN_DIR),
        );
        set.push(
            ArtifactRef::new(ArtifactKind::Report, "/tmp/legacy-report.xml")
                .with_role(ARTIFACT_ROLE_REPORT),
        );

        let projected = RetainedPaths::from_artifact_set(&set).expect("retained paths");

        assert_eq!(
            projected.junit_xml,
            Some(PathBuf::from("/tmp/legacy-report.xml"))
        );
    }

    #[test]
    fn retained_paths_prefers_modern_junit_role_over_legacy_report() {
        let mut set = ArtifactSet::with_root("/tmp/run");
        set.push(
            ArtifactRef::new(ArtifactKind::RunDirectory, "/tmp/run")
                .with_role(ARTIFACT_ROLE_RUN_DIR),
        );
        set.push(
            ArtifactRef::new(ArtifactKind::Report, "/tmp/a-legacy-report.xml")
                .with_role(ARTIFACT_ROLE_REPORT),
        );
        set.push(
            ArtifactRef::new(ArtifactKind::JunitXml, "/tmp/z-modern-report.xml")
                .with_role(ARTIFACT_ROLE_JUNIT_XML),
        );

        let projected = RetainedPaths::from_artifact_set(&set).expect("retained paths");

        assert_eq!(
            projected.junit_xml,
            Some(PathBuf::from("/tmp/z-modern-report.xml"))
        );
    }

    #[test]
    fn retained_paths_omit_allure_artifact_until_runner_produces_it() {
        let retained = RetainedPaths {
            run_dir: PathBuf::from("/tmp/run"),
            config_json: Some(PathBuf::from("/tmp/config.json")),
            junit_xml: Some(PathBuf::from("/tmp/report.xml")),
            allure_results: None,
            yaxunit_log: Some(PathBuf::from("/tmp/yaxunit.log")),
            platform_log: Some(PathBuf::from("/tmp/platform.log")),
        };

        let set = retained.clone().into_artifact_set();

        assert_eq!(
            set.get_by_role(crate::domain::artifact::ARTIFACT_ROLE_ALLURE_RESULTS),
            None
        );
        assert_eq!(RetainedPaths::from_artifact_set(&set), Some(retained));
    }

    #[test]
    fn retained_paths_do_not_require_or_serialize_internal_sentinel() {
        let retained = RetainedPaths {
            run_dir: PathBuf::from("/tmp/run"),
            config_json: Some(PathBuf::from("/tmp/config.json")),
            junit_xml: Some(PathBuf::from("/tmp/report.xml")),
            allure_results: Some(PathBuf::from("/tmp/allure-results")),
            yaxunit_log: Some(PathBuf::from("/tmp/yaxunit.log")),
            platform_log: Some(PathBuf::from("/tmp/platform.log")),
        };
        let set = retained.into_artifact_set();

        let projected = RetainedPaths::from_artifact_set(&set).expect("retained paths");
        let json = serde_json::to_value(projected).expect("json");

        assert!(json.get("sentinel").is_none());
    }

    #[test]
    fn test_error_kind_codes_roundtrip_for_setup_and_process_failures() {
        let kinds = [
            TestErrorKind::InfobaseUnavailable,
            TestErrorKind::TestSetupFailed,
            TestErrorKind::EnterpriseSpawnFailed,
            TestErrorKind::EnterpriseStartupCheckFailed,
            TestErrorKind::EnterpriseExitedEarly,
            TestErrorKind::EnterpriseStdoutLogIo,
            TestErrorKind::EnterpriseStderrLogIo,
            TestErrorKind::EnterpriseTimedOut,
        ];

        for kind in kinds {
            let code = kind.clone().code();
            assert_eq!(TestErrorKind::from_code(code), Some(kind));
        }
    }

    #[test]
    fn test_error_status_maps_new_process_failures_explicitly() {
        for kind in [
            TestErrorKind::InfobaseUnavailable,
            TestErrorKind::TestSetupFailed,
            TestErrorKind::EnterpriseSpawnFailed,
            TestErrorKind::EnterpriseStartupCheckFailed,
            TestErrorKind::EnterpriseExitedEarly,
            TestErrorKind::EnterpriseStdoutLogIo,
            TestErrorKind::EnterpriseStderrLogIo,
        ] {
            assert_eq!(
                super::test_execution_status(Some(kind), false),
                ExecutionStatus::Failed
            );
        }
    }

    #[test]
    fn allure_output_errors_are_invalid_output() {
        assert_eq!(
            super::test_execution_status(Some(TestErrorKind::AllureNotProduced), false),
            ExecutionStatus::InvalidOutput
        );
        assert_eq!(
            super::test_execution_status(Some(TestErrorKind::AllureEmpty), false),
            ExecutionStatus::InvalidOutput
        );
    }

    #[test]
    fn test_run_result_derives_read_model_from_outcome() {
        let retained = RetainedPaths {
            run_dir: PathBuf::from("/tmp/run"),
            config_json: Some(PathBuf::from("/tmp/config.json")),
            junit_xml: Some(PathBuf::from("/tmp/report.xml")),
            allure_results: Some(PathBuf::from("/tmp/allure-results")),
            yaxunit_log: Some(PathBuf::from("/tmp/yaxunit.log")),
            platform_log: Some(PathBuf::from("/tmp/platform.log")),
        };
        let outcome = ExecutionOutcome::new(ExecutionStatus::Failed)
            .with_diagnostics(vec!["diag".to_owned()])
            .with_errors(vec![test_execution_error(
                TestErrorKind::TestFailures,
                "tests failed",
            )])
            .with_artifacts(retained.clone().into_artifact_set())
            .with_metrics(ExecutionMetrics {
                total: 3,
                passed: 2,
                failed: 1,
                skipped: 0,
                errors: 0,
                extra: Default::default(),
            })
            .with_payload(TestReport {
                summary: TestSummary {
                    total: 0,
                    passed: 0,
                    failed: 0,
                    skipped: 0,
                    errors: 0,
                },
                suites: vec![],
                extracted_errors: vec![],
            });

        let result = TestRunResult::from_outcome(
            outcome.clone(),
            TestTarget::All,
            TestOutputMode::Compact,
            vec!["warn".to_owned()],
            vec![],
            42,
        );

        assert!(!result.execution.is_ok());
        assert_eq!(
            result
                .execution
                .errors
                .first()
                .and_then(|error| TestErrorKind::from_code(&error.code)),
            Some(TestErrorKind::TestFailures)
        );
        assert_eq!(result.execution.diagnostics, vec!["diag"]);
        assert_eq!(
            result
                .execution
                .artifacts
                .as_ref()
                .and_then(RetainedPaths::from_artifact_set),
            Some(retained)
        );
        assert_eq!(
            result
                .execution
                .payload
                .as_ref()
                .expect("report")
                .summary
                .total,
            3
        );
        let mut expected = outcome;
        if let Some(report) = expected.payload.as_mut() {
            report.summary.total = 3;
            report.summary.passed = 2;
            report.summary.failed = 1;
        }
        assert_eq!(result.to_outcome(), expected);
    }

    #[test]
    fn serde_shape_keeps_canonical_execution_only() {
        let result = TestRunResult::from_outcome(
            ExecutionOutcome::new(ExecutionStatus::Succeeded).with_payload(TestReport {
                summary: TestSummary {
                    total: 1,
                    passed: 1,
                    failed: 0,
                    skipped: 0,
                    errors: 0,
                },
                suites: vec![],
                extracted_errors: vec![],
            }),
            TestTarget::All,
            TestOutputMode::Full,
            vec!["warn".to_owned()],
            vec![],
            10,
        );

        let value = serde_json::to_value(result).expect("json");
        assert!(value.get("ok").is_none());
        assert!(value.get("report").is_none());
        assert!(value.get("error_kind").is_none());
        assert!(value.get("diagnostics").is_none());
        assert!(value.get("retained_paths").is_none());
        assert!(value.get("warnings").is_none());
        assert!(value.get("steps").is_none());
        assert!(value.get("duration_ms").is_none());
        assert!(value.get("execution").is_some());
        assert!(value.get("outcome").is_none());
    }
}
