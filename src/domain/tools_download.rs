use std::path::PathBuf;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExtensionInstallMode {
    Sources,
    Artifacts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDownloadTarget {
    Yaxunit,
    VanessaAutomationSingle,
    ClientMcp,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolsDownloadResult {
    pub ok: bool,
    pub tool: String,
    pub mode: String,
    pub destinations: Vec<ToolDownloadDestination>,
    pub config_path: PathBuf,
    pub local_config_path: PathBuf,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDownloadDestination {
    pub tool: String,
    pub tag: String,
    pub source: String,
    pub path: PathBuf,
    pub config: String,
}
