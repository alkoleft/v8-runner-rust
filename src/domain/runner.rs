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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_epf_wait: Option<ExternalEpfWaitOptions>,
}

/// Opt-in bounded observation settings for a direct external EPF launch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalEpfWaitOptions {
    pub timeout_ms: u64,
    pub stderr_output: String,
}

impl LaunchOptions {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

pub(crate) fn launch_key_alias_matches(raw: &str, key: &str) -> bool {
    let trimmed = raw.trim_start();
    if !trimmed.starts_with(['/', '-']) {
        return false;
    }
    let normalized = trimmed
        .trim_start_matches(['/', '-'])
        .trim_end()
        .to_ascii_lowercase();
    let expected = key
        .trim_start_matches(['/', '-'])
        .trim()
        .to_ascii_lowercase();
    if normalized == expected {
        return true;
    }
    let Some(rest) = normalized.strip_prefix(&expected) else {
        return false;
    };
    rest.chars()
        .next()
        .is_some_and(|ch| matches!(ch, '"' | '=' | ':' | ' ' | '\t'))
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
    AllureResults,
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

#[cfg(test)]
mod tests {
    use super::launch_key_alias_matches;

    #[test]
    fn launch_key_alias_matches_exact_and_attached_separator_forms() {
        for raw in [
            "/C",
            "-c",
            "/C\"payload\"",
            "/C payload",
            "/C=payload",
            "/C:payload",
        ] {
            assert!(launch_key_alias_matches(raw, "c"), "{raw}");
        }
    }

    #[test]
    fn launch_key_alias_does_not_match_longer_key_prefixes() {
        for raw in [
            "C=payload",
            "/ C payload",
            "/Config",
            "/Client",
            "/Certificate",
            "/ExecuteScript",
        ] {
            assert!(!launch_key_alias_matches(raw, "c"), "{raw}");
            assert!(!launch_key_alias_matches(raw, "execute"), "{raw}");
        }
    }
}
