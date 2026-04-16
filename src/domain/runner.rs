use serde::{Deserialize, Serialize};

use crate::domain::execution::ExecutionTimeouts;

/// Shared launch options reused by direct launch and runner-like scenarios.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LaunchOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub c: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execute: Option<String>,
    #[serde(default)]
    pub use_privileged_mode: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal_out: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub raw_args: Vec<String>,
}

impl LaunchOptions {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Shared client/utility mode for runner-like execution requests.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LaunchClientModeRequest {
    Designer,
    Thin,
    Thick,
    Ordinary,
}

/// Extensible runner identity for test/package scenarios.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunnerKind {
    YaXUnit,
    Vanessa,
    Cf,
    Cfe,
    Epf,
    #[serde(alias = "epr")]
    Erf,
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
    /// Requested client/utility mode for the enterprise platform launcher.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_mode: Option<LaunchClientModeRequest>,
    /// Runtime currently consumes `total_ms` in the YaXUnit test flow; the rest
    /// of the budget is reserved for future runner integrations.
    #[serde(default)]
    pub timeouts: ExecutionTimeouts,
    /// Contract-level retention policy for future runner/package flows.
    /// The current test flow still decides retention in use-case code.
    #[serde(default)]
    pub policy: ExecutionPolicy,
    /// Shared launch surface for enterprise/designer execution scenarios.
    #[serde(default, skip_serializing_if = "LaunchOptions::is_empty")]
    pub launch: LaunchOptions,
}
