use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const ARTIFACT_ROLE_RUN_DIR: &str = "run_dir";
pub const ARTIFACT_ROLE_CONFIG: &str = "config";
pub const ARTIFACT_ROLE_REPORT: &str = "report";
pub const ARTIFACT_ROLE_RUNNER_LOG: &str = "runner_log";
pub const ARTIFACT_ROLE_PLATFORM_LOG: &str = "platform_log";
pub const ARTIFACT_ROLE_SENTINEL: &str = "sentinel";

/// Stable artifact classification for runner/package outputs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    RunDirectory,
    Config,
    Report,
    RunnerLog,
    PlatformLog,
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
}
