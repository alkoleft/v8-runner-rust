use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::output::json::StepResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestTarget {
    All,
    Module { name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestOutputMode {
    Compact,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetainedPaths {
    pub run_dir: PathBuf,
    pub config_json: PathBuf,
    pub junit_xml: PathBuf,
    pub yaxunit_log: PathBuf,
    pub platform_log: PathBuf,
    pub sentinel: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestReport {
    pub summary: TestSummary,
    pub suites: Vec<TestSuite>,
    pub extracted_errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSummary {
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub errors: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSuite {
    pub name: String,
    pub cases: Vec<TestCase>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl TestRunResult {
    pub fn has_failures(&self) -> bool {
        self.report.as_ref().is_some_and(|report| {
            report.summary.failed > 0 || report.summary.errors > 0
        })
    }
}
