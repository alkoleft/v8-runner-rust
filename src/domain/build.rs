use crate::domain::sync_receipt::SyncReceipt;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildResult {
    pub ok: bool,
    pub steps: Vec<BuildStep>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildStep {
    pub source_set: String,
    pub mode: BuildMode,
    pub ok: bool,
    pub message: Option<String>,
    pub duration_ms: u64,
    #[serde(default)]
    pub receipt: SyncReceipt,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BuildMode {
    EdtExport,
    Full,
    Partial { file_count: usize },
    Skipped,
}
