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
}

/// Supported application launch modes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LaunchMode {
    Designer,
    Thin,
    Thick,
    Ordinary,
}
