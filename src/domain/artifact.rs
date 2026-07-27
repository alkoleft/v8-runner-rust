use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const ARTIFACT_ROLE_RUN_DIR: &str = "run_dir";
pub const ARTIFACT_ROLE_CONFIG: &str = "config";
#[allow(dead_code)]
pub const ARTIFACT_ROLE_REPORT: &str = "report";
pub const ARTIFACT_ROLE_JUNIT_XML: &str = "junit_xml";
pub const ARTIFACT_ROLE_ALLURE_RESULTS: &str = "allure_results";
#[allow(dead_code)]
pub const ARTIFACT_ROLE_ERROR_DETAILS: &str = "error_details";
#[allow(dead_code)]
pub const ARTIFACT_ROLE_SCREENSHOT: &str = "screenshot";
pub const ARTIFACT_ROLE_RUNNER_LOG: &str = "runner_log";
pub const ARTIFACT_ROLE_PLATFORM_LOG: &str = "platform_log";
pub const ARTIFACT_ROLE_PACKAGE_FILE: &str = "package_file";
pub const ARTIFACT_ROLE_STAGE_FILE: &str = "stage_file";
#[allow(dead_code)]
#[deprecated(note = "sentinels are internal cleanup markers and are no longer emitted")]
pub const ARTIFACT_ROLE_SENTINEL: &str = "sentinel";

/// Stable artifact classification for runner/package outputs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    RunDirectory,
    Config,
    Package,
    Report,
    JunitXml,
    AllureResults,
    ErrorDetails,
    Screenshot,
    RunnerLog,
    PlatformLog,
    #[deprecated(note = "sentinels are internal cleanup markers and are no longer emitted")]
    Sentinel,
    Other(String),
}

/// Reference to a single retained artifact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactRef {
    pub kind: ArtifactKind,
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl ArtifactRef {
    pub fn new(kind: ArtifactKind, path: impl Into<PathBuf>) -> Self {
        Self {
            kind,
            path: path.into(),
            role: None,
            label: None,
        }
    }

    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.role = Some(role.into());
        self
    }
}

/// A retained artifact collection for a single execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ArtifactSet {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<ArtifactRef>,
}

impl ArtifactSet {
    pub fn with_root(root_dir: impl Into<PathBuf>) -> Self {
        Self {
            root_dir: Some(root_dir.into()),
            items: Vec::new(),
        }
    }

    pub fn push(&mut self, artifact: ArtifactRef) {
        self.items.push(artifact);
    }

    pub fn get_by_role(&self, role: &str) -> Option<&Path> {
        self.items
            .iter()
            .find(|item| item.role.as_deref() == Some(role))
            .map(|item| item.path.as_path())
    }

    pub fn get_all_by_role<'a>(&'a self, role: &'a str) -> impl Iterator<Item = &'a Path> + 'a {
        self.items
            .iter()
            .filter(move |item| item.role.as_deref() == Some(role))
            .map(|item| item.path.as_path())
    }
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::{
        ArtifactKind, ARTIFACT_ROLE_ALLURE_RESULTS, ARTIFACT_ROLE_ERROR_DETAILS,
        ARTIFACT_ROLE_JUNIT_XML, ARTIFACT_ROLE_SCREENSHOT, ARTIFACT_ROLE_SENTINEL,
    };

    #[test]
    fn serializes_typed_test_artifact_kinds_and_roles() {
        assert_eq!(
            serde_json::to_value(ArtifactKind::JunitXml).unwrap(),
            serde_json::json!("junit_xml")
        );
        assert_eq!(
            serde_json::to_value(ArtifactKind::AllureResults).unwrap(),
            serde_json::json!("allure_results")
        );
        assert_eq!(ARTIFACT_ROLE_JUNIT_XML, "junit_xml");
        assert_eq!(ARTIFACT_ROLE_ALLURE_RESULTS, "allure_results");
        assert_eq!(ARTIFACT_ROLE_ERROR_DETAILS, "error_details");
        assert_eq!(ARTIFACT_ROLE_SCREENSHOT, "screenshot");
    }

    #[test]
    fn deserializes_legacy_sentinel_artifact_kind() {
        assert_eq!(
            serde_json::from_value::<ArtifactKind>(serde_json::json!("sentinel")).unwrap(),
            ArtifactKind::Sentinel
        );
        assert_eq!(ARTIFACT_ROLE_SENTINEL, "sentinel");
    }
}
