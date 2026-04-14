use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::domain::artifact::{
    ArtifactKind, ArtifactRef, ArtifactSet, ARTIFACT_ROLE_CONFIG, ARTIFACT_ROLE_PLATFORM_LOG,
    ARTIFACT_ROLE_REPORT, ARTIFACT_ROLE_RUN_DIR, ARTIFACT_ROLE_RUNNER_LOG,
    ARTIFACT_ROLE_SENTINEL,
};
use crate::domain::execution::{
    ExecutionError, ExecutionMetrics, ExecutionOutcome, ExecutionStatus, StepResult,
};

pub const TEST_ERROR_CODE_BUILD_FAILED: &str = "build_failed";
pub const TEST_ERROR_CODE_ENTERPRISE_SPAWN_FAILED: &str = "enterprise_spawn_failed";
pub const TEST_ERROR_CODE_ENTERPRISE_TIMED_OUT: &str = "enterprise_timed_out";
pub const TEST_ERROR_CODE_ENTERPRISE_EXITED_NON_ZERO: &str = "enterprise_exited_non_zero";
pub const TEST_ERROR_CODE_TEST_FAILURES: &str = "test_failures";
pub const TEST_ERROR_CODE_JUNIT_NOT_PRODUCED: &str = "junit_not_produced";
pub const TEST_ERROR_CODE_JUNIT_EMPTY: &str = "junit_empty";
pub const TEST_ERROR_CODE_JUNIT_MALFORMED: &str = "junit_malformed";
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
    EnterpriseSpawnFailed,
    EnterpriseTimedOut,
    EnterpriseExitedNonZero,
    TestFailures,
    JunitNotProduced,
    JunitEmpty,
    JunitMalformed,
}

impl TestErrorKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::BuildFailed => TEST_ERROR_CODE_BUILD_FAILED,
            Self::EnterpriseSpawnFailed => TEST_ERROR_CODE_ENTERPRISE_SPAWN_FAILED,
            Self::EnterpriseTimedOut => TEST_ERROR_CODE_ENTERPRISE_TIMED_OUT,
            Self::EnterpriseExitedNonZero => TEST_ERROR_CODE_ENTERPRISE_EXITED_NON_ZERO,
            Self::TestFailures => TEST_ERROR_CODE_TEST_FAILURES,
            Self::JunitNotProduced => TEST_ERROR_CODE_JUNIT_NOT_PRODUCED,
            Self::JunitEmpty => TEST_ERROR_CODE_JUNIT_EMPTY,
            Self::JunitMalformed => TEST_ERROR_CODE_JUNIT_MALFORMED,
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            TEST_ERROR_CODE_BUILD_FAILED => Self::BuildFailed,
            TEST_ERROR_CODE_ENTERPRISE_SPAWN_FAILED => Self::EnterpriseSpawnFailed,
            TEST_ERROR_CODE_ENTERPRISE_TIMED_OUT => Self::EnterpriseTimedOut,
            TEST_ERROR_CODE_ENTERPRISE_EXITED_NON_ZERO => Self::EnterpriseExitedNonZero,
            TEST_ERROR_CODE_TEST_FAILURES => Self::TestFailures,
            TEST_ERROR_CODE_JUNIT_NOT_PRODUCED => Self::JunitNotProduced,
            TEST_ERROR_CODE_JUNIT_EMPTY => Self::JunitEmpty,
            TEST_ERROR_CODE_JUNIT_MALFORMED => Self::JunitMalformed,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetainedPaths {
    pub run_dir: PathBuf,
    pub config_json: PathBuf,
    pub junit_xml: PathBuf,
    pub yaxunit_log: PathBuf,
    pub platform_log: PathBuf,
    pub sentinel: PathBuf,
}

impl RetainedPaths {
    pub fn into_artifact_set(self) -> ArtifactSet {
        let mut set = ArtifactSet::with_root(self.run_dir.clone());
        set.push(
            ArtifactRef::new(ArtifactKind::RunDirectory, self.run_dir)
                .with_role(ARTIFACT_ROLE_RUN_DIR),
        );
        set.push(ArtifactRef::new(ArtifactKind::Config, self.config_json).with_role(ARTIFACT_ROLE_CONFIG));
        set.push(ArtifactRef::new(ArtifactKind::Report, self.junit_xml).with_role(ARTIFACT_ROLE_REPORT));
        set.push(
            ArtifactRef::new(ArtifactKind::RunnerLog, self.yaxunit_log)
                .with_role(ARTIFACT_ROLE_RUNNER_LOG),
        );
        set.push(
            ArtifactRef::new(ArtifactKind::PlatformLog, self.platform_log)
                .with_role(ARTIFACT_ROLE_PLATFORM_LOG),
        );
        set.push(
            ArtifactRef::new(ArtifactKind::Sentinel, self.sentinel).with_role(ARTIFACT_ROLE_SENTINEL),
        );
        set
    }

    pub fn from_artifact_set(set: &ArtifactSet) -> Option<Self> {
        Some(Self {
            run_dir: set.get_by_role(ARTIFACT_ROLE_RUN_DIR)?.to_path_buf(),
            config_json: set.get_by_role(ARTIFACT_ROLE_CONFIG)?.to_path_buf(),
            junit_xml: set.get_by_role(ARTIFACT_ROLE_REPORT)?.to_path_buf(),
            yaxunit_log: set.get_by_role(ARTIFACT_ROLE_RUNNER_LOG)?.to_path_buf(),
            platform_log: set.get_by_role(ARTIFACT_ROLE_PLATFORM_LOG)?.to_path_buf(),
            sentinel: set.get_by_role(ARTIFACT_ROLE_SENTINEL)?.to_path_buf(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TestRunResult {
    pub ok: bool,
    pub target: TestTarget,
    pub mode: TestOutputMode,
    pub error_kind: Option<TestErrorKind>,
    pub diagnostics: Vec<String>,
    pub retained_paths: Option<RetainedPaths>,
    pub report: Option<TestReport>,
    #[serde(skip)]
    pub warnings: Vec<String>,
    #[serde(skip)]
    pub steps: Vec<StepResult>,
    #[serde(skip)]
    pub duration_ms: u64,
    #[serde(skip)]
    pub outcome: ExecutionOutcome<TestReport>,
}

impl TestRunResult {
    pub fn from_outcome(
        outcome: ExecutionOutcome<TestReport>,
        target: TestTarget,
        mode: TestOutputMode,
        warnings: Vec<String>,
        steps: Vec<StepResult>,
        duration_ms: u64,
    ) -> Self {
        let metrics = outcome.metrics.clone();
        let mut report = outcome.payload.clone();
        if let (Some(report), Some(metrics)) = (report.as_mut(), metrics.as_ref()) {
            report.summary = TestSummary::from(metrics.clone());
        }

        Self {
            ok: outcome.is_ok(),
            target,
            mode,
            error_kind: outcome
                .errors
                .first()
                .and_then(|error| TestErrorKind::from_code(&error.code)),
            diagnostics: outcome.diagnostics.clone(),
            retained_paths: outcome.artifacts.as_ref().and_then(RetainedPaths::from_artifact_set),
            report,
            warnings,
            steps,
            duration_ms,
            outcome,
        }
    }

    #[cfg(test)]
    pub fn to_outcome(&self) -> ExecutionOutcome<TestReport> {
        self.outcome.clone()
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
    match (ok, kind) {
        (true, _) => ExecutionStatus::Succeeded,
        (_, Some(TestErrorKind::EnterpriseTimedOut)) => ExecutionStatus::TimedOut,
        (_, Some(TestErrorKind::JunitMalformed | TestErrorKind::JunitEmpty | TestErrorKind::JunitNotProduced)) => {
            ExecutionStatus::InvalidOutput
        }
        _ => ExecutionStatus::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        test_execution_error, RetainedPaths, TestErrorKind, TestOutputMode, TestReport,
        TestRunResult, TestSummary, TestTarget,
    };
    use crate::domain::execution::{ExecutionMetrics, ExecutionOutcome, ExecutionStatus};
    use std::path::PathBuf;

    #[test]
    fn retained_paths_roundtrip_to_artifact_set() {
        let retained = RetainedPaths {
            run_dir: PathBuf::from("/tmp/run"),
            config_json: PathBuf::from("/tmp/config.json"),
            junit_xml: PathBuf::from("/tmp/report.xml"),
            yaxunit_log: PathBuf::from("/tmp/yaxunit.log"),
            platform_log: PathBuf::from("/tmp/platform.log"),
            sentinel: PathBuf::from("/tmp/sentinel"),
        };

        let set = retained.clone().into_artifact_set();

        assert_eq!(RetainedPaths::from_artifact_set(&set), Some(retained));
    }

    #[test]
    fn wrapper_restores_legacy_fields_from_outcome() {
        let retained = RetainedPaths {
            run_dir: PathBuf::from("/tmp/run"),
            config_json: PathBuf::from("/tmp/config.json"),
            junit_xml: PathBuf::from("/tmp/report.xml"),
            yaxunit_log: PathBuf::from("/tmp/yaxunit.log"),
            platform_log: PathBuf::from("/tmp/platform.log"),
            sentinel: PathBuf::from("/tmp/sentinel"),
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

        assert!(!result.ok);
        assert_eq!(result.error_kind, Some(TestErrorKind::TestFailures));
        assert_eq!(result.diagnostics, vec!["diag"]);
        assert_eq!(result.retained_paths, Some(retained));
        assert_eq!(result.report.as_ref().expect("report").summary.total, 3);
        assert_eq!(result.to_outcome(), outcome);
    }

    #[test]
    fn serde_shape_keeps_legacy_fields_only() {
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
        assert!(value.get("ok").is_some());
        assert!(value.get("report").is_some());
        assert!(value.get("warnings").is_none());
        assert!(value.get("steps").is_none());
        assert!(value.get("duration_ms").is_none());
        assert!(value.get("outcome").is_none());
    }
}
