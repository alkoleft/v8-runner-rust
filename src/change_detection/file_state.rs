use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

/// Recorded state of a single file at scan time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileState {
    /// Absolute path to the file.
    pub path: PathBuf,
    /// Last-modified timestamp (seconds since UNIX epoch).
    pub mtime: u64,
    /// SHA-256 hex digest of file contents.
    pub hash: String,
}

impl FileState {
    pub fn new(path: PathBuf, mtime: u64, hash: String) -> Self {
        Self { path, mtime, hash }
    }
}

/// Convert a `SystemTime` to seconds since UNIX epoch, returning 0 on error.
pub fn mtime_secs(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
