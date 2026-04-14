use serde::{Deserialize, Serialize};

use crate::domain::execution::ExecutionTimeouts;

/// Extensible runner identity for test/package scenarios.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunnerKind {
    YaXUnit,
    Vanessa,
    Cf,
    Cfe,
    Epf,
    Epr,
    Custom(String),
}

/// Declares primary output formats produced by a runner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunnerOutputFormat {
    JunitXml,
    PlainTextLog,
    Json,
    Binary,
    Directory,
    Custom(String),
}

/// Runner profile shared by transport-neutral execution requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunnerProfile {
    pub id: String,
    pub kind: RunnerKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_formats: Vec<RunnerOutputFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_hint: Option<String>,
}

/// Execution retention policy shared by runner-like requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ExecutionPolicy {
    pub retain_artifacts_on_failure: bool,
    pub retain_artifacts_on_success: bool,
}

/// Shared execution request block for runner-like flows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScenarioExecutionRequest {
    pub profile: RunnerProfile,
    /// Runtime currently consumes `total_ms` in the YaXUnit test flow; the rest
    /// of the budget is reserved for future runner integrations.
    #[serde(default)]
    pub timeouts: ExecutionTimeouts,
    /// Contract-level retention policy for future runner/package flows.
    /// The current test flow still decides retention in use-case code.
    #[serde(default)]
    pub policy: ExecutionPolicy,
}
