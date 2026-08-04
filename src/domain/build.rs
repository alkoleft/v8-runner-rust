use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildResult {
    pub ok: bool,
    pub steps: Vec<BuildStep>,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cdfi_recovery: Option<Box<CdfiRecoverySummary>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildStep {
    pub source_set: String,
    pub mode: BuildMode,
    pub ok: bool,
    pub message: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BuildMode {
    EdtExport,
    Full,
    Partial { file_count: usize },
    Skipped,
}

/// Diagnostics emitted by a Designer build's CDFI recovery guard.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CdfiRecoverySummary {
    pub tracked_path: std::path::PathBuf,
    pub original_existed: bool,
    pub changed_entry_count: Option<usize>,
    pub action: CdfiRecoveryAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_path: Option<std::path::PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_warning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

/// The outcome of attempting CDFI recovery after a failed Designer build.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CdfiRecoveryAction {
    NotNeeded,
    #[serde(alias = "restored_original")]
    Restored,
    RemovedCreatedFile,
    #[serde(alias = "restore_failed")]
    Failed,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::{BuildResult, CdfiRecoveryAction, CdfiRecoverySummary};

    #[test]
    fn build_result_serializes_retained_cdfi_recovery_failure() {
        let result = BuildResult {
            ok: false,
            steps: vec![],
            duration_ms: 42,
            cdfi_recovery: Some(Box::new(CdfiRecoverySummary {
                tracked_path: PathBuf::from("/src/ConfigDumpInfo.xml"),
                original_existed: true,
                changed_entry_count: Some(1),
                action: CdfiRecoveryAction::Failed,
                snapshot_path: Some(PathBuf::from("/work/cdfi-recovery-42/ConfigDumpInfo.xml")),
                cleanup_warning: None,
                failure: Some("permission denied while restoring CDFI".to_owned()),
            })),
        };

        assert_eq!(
            serde_json::to_value(result).expect("serialize build result"),
            json!({
                "ok": false,
                "steps": [],
                "duration_ms": 42,
                "cdfi_recovery": {
                    "tracked_path": "/src/ConfigDumpInfo.xml",
                    "original_existed": true,
                    "changed_entry_count": 1,
                    "action": "failed",
                    "snapshot_path": "/work/cdfi-recovery-42/ConfigDumpInfo.xml",
                    "failure": "permission denied while restoring CDFI",
                },
            })
        );
    }

    #[test]
    fn build_result_serializes_successful_designer_recovery_summary() {
        let result = BuildResult {
            ok: true,
            steps: vec![],
            duration_ms: 7,
            cdfi_recovery: Some(Box::new(CdfiRecoverySummary {
                tracked_path: PathBuf::from("/src/ConfigDumpInfo.xml"),
                original_existed: false,
                changed_entry_count: Some(1),
                action: CdfiRecoveryAction::NotNeeded,
                snapshot_path: None,
                cleanup_warning: Some(
                    "failed to remove CDFI recovery snapshot after successful Designer build"
                        .to_owned(),
                ),
                failure: None,
            })),
        };

        assert_eq!(
            serde_json::to_value(result).expect("serialize build result"),
            json!({
                "ok": true,
                "steps": [],
                "duration_ms": 7,
                "cdfi_recovery": {
                    "tracked_path": "/src/ConfigDumpInfo.xml",
                    "original_existed": false,
                    "changed_entry_count": 1,
                    "action": "not_needed",
                    "cleanup_warning": "failed to remove CDFI recovery snapshot after successful Designer build",
                },
            })
        );
    }
}
