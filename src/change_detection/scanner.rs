use std::path::{Path, PathBuf};
use sha2::{Digest, Sha256};
use thiserror::Error;
use walkdir::WalkDir;

use crate::change_detection::file_state::{mtime_secs, FileState};

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("failed to walk directory '{path}': {source}")]
    Walk { path: PathBuf, source: walkdir::Error },

    #[error("failed to read file '{path}': {source}")]
    Read { path: PathBuf, source: std::io::Error },

    #[error("failed to read metadata for '{path}': {source}")]
    Meta { path: PathBuf, source: std::io::Error },
}

/// Directory/file names that are always excluded from scanning.
const IGNORED_DIRS: &[&str] = &[
    ".git", ".gradle", "build", "target", "temp", "tmp", ".yaxunit",
];
const IGNORED_FILES: &[&str] = &["ConfigDumpInfo.xml"];

/// Recursively scan `root`, returning a `FileState` for every non-ignored file.
///
/// On any I/O error the function returns `Err` — callers should fall back to
/// "all changed" semantics per the spec.
pub fn scan(root: &Path) -> Result<Vec<FileState>, ScanError> {
    let mut results = Vec::new();

    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|e| ScanError::Walk {
            path: root.to_path_buf(),
            source: e,
        })?;

        let path = entry.path();

        // Skip ignored directories (prune by checking each component).
        if entry.file_type().is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if IGNORED_DIRS.contains(&name) {
                    continue;
                }
            }
            continue; // directories themselves are not file states
        }

        if !entry.file_type().is_file() {
            continue;
        }

        // Skip ignored file names.
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if IGNORED_FILES.contains(&name) {
                continue;
            }
        }

        let meta = std::fs::metadata(path).map_err(|e| ScanError::Meta {
            path: path.to_path_buf(),
            source: e,
        })?;

        let mtime = mtime_secs(meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH));

        let hash = hash_file(path)?;

        results.push(FileState::new(path.to_path_buf(), mtime, hash));
    }

    Ok(results)
}

/// Compute SHA-256 hex digest of a file's contents.
pub fn hash_file(path: &Path) -> Result<String, ScanError> {
    let data = std::fs::read(path).map_err(|e| ScanError::Read {
        path: path.to_path_buf(),
        source: e,
    })?;
    let digest = Sha256::digest(&data);
    Ok(format!("{:x}", digest))
}
