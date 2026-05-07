use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Structured result of a `launch` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchResult {
    /// `true` when the process was spawned successfully.
    pub ok: bool,
    /// Requested launch mode.
    pub mode: LaunchMode,
    /// OS process identifier if the launcher exposed one.
    pub pid: Option<u32>,
    /// Selected binary path used to spawn the process.
    pub binary: PathBuf,
    /// Human-readable launch summary.
    pub message: Option<String>,
    /// MCP transport selected for this launch (`ws` or `legacy`).
    /// Present only for `launch mcp` and `test` flows.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub transport: Option<String>,
    /// Per-launch UUID announced to the session-manager
    /// (`mcpMode=ws` only).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub client_uid: Option<String>,
    /// Manager-side client kind announced to the session-manager
    /// (`mcpMode=ws` only).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kind: Option<String>,
    /// Session-manager WS endpoint used for this launch (`mcpMode=ws` only).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub manager_url: Option<String>,
    /// Correlation id for trace correlation in manager logs
    /// (`mcpMode=ws` only).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub corr_id: Option<String>,
    /// Local HTTP MCP port (`legacy` transport only).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mcp_port: Option<u16>,
}

/// Supported application launch modes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LaunchMode {
    Designer,
    Thin,
    Thick,
    Ordinary,
    Mcp,
}
