use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use thiserror::Error;
use walkdir::WalkDir;

use crate::change_detection::file_state::{mtime_nanos, MtimeError};

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("failed to walk directory '{path}': {source}")]
    Walk {
        path: PathBuf,
        source: walkdir::Error,
    },

    #[error("failed to read file '{path}': {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to read metadata for '{path}': {source}")]
    Meta {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to convert mtime for '{path}': {source}")]
    Mtime { path: PathBuf, source: MtimeError },

    #[error("failed to build path relative to scan root '{root}' for '{path}'")]
    RelativePath { root: PathBuf, path: PathBuf },
}

/// Directory/file names that are always excluded from scanning.
const IGNORED_DIRS: &[&str] = &[
    ".git", ".gradle", "build", "target", "temp", "tmp", ".yaxunit",
];
const IGNORED_FILES: &[&str] = &["ConfigDumpInfo.xml"];

/// Coarse filesystem mtime guard (2 seconds).
pub const COARSE_MARGIN_NS: u64 = 2_000_000_000;

/// One discovered source file (metadata only, no hash).
#[derive(Debug, Clone)]
pub struct SeenFile {
    pub rel_path: String,
    pub mtime_ns: u64,
}

/// One hashed candidate file.
#[derive(Debug, Clone)]
pub struct CandidateFile {
    pub path: PathBuf,
    pub rel_path: String,
    pub mtime_ns: u64,
    pub hash: String,
}

/// Full scanner output for one source-set root.
#[derive(Debug, Clone)]
pub struct ScanSnapshot {
    pub scan_started_at: u64,
    pub seen_files: Vec<SeenFile>,
    pub candidates: Vec<CandidateFile>,
}

/// Recursively scan `root` and return:
/// - all seen files with metadata
/// - only candidate files hashed by mtime/watermark rules
pub fn scan(
    root: &Path,
    watermark: Option<u64>,
    stored_keys: &HashSet<String>,
) -> Result<ScanSnapshot, ScanError> {
    let scan_started_at =
        mtime_nanos(std::time::SystemTime::now(), root).map_err(|source| ScanError::Mtime {
            path: root.to_path_buf(),
            source,
        })?;
    let mut seen_files = Vec::new();
    let mut candidates = Vec::new();

    let cutoff = watermark.map(|w| w.saturating_sub(COARSE_MARGIN_NS));
    tracing::debug!(
        root = %root.display(),
        watermark,
        stored_files = stored_keys.len(),
        "starting filesystem scan"
    );
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_ignored_dir(e))
    {
        let entry = entry.map_err(|e| ScanError::Walk {
            path: root.to_path_buf(),
            source: e,
        })?;

        let path = entry.path();

        if entry.file_type().is_dir() {
            continue;
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

        let mtime_ns = mtime_nanos(
            meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            path,
        )
        .map_err(|source| ScanError::Mtime {
            path: path.to_path_buf(),
            source,
        })?;
        let rel_path = rel_path(root, path)?;
        let seen = SeenFile {
            rel_path: rel_path.clone(),
            mtime_ns,
        };
        let is_new = !stored_keys.contains(&rel_path);
        let is_candidate = match cutoff {
            None => true,
            Some(cutoff) => is_new || mtime_ns >= cutoff,
        };
        if is_candidate {
            let hash = hash_file(path)?;
            candidates.push(CandidateFile {
                path: path.to_path_buf(),
                rel_path,
                mtime_ns,
                hash,
            });
        }
        seen_files.push(seen);
        if seen_files.len() % 1000 == 0 {
            tracing::debug!(
                root = %root.display(),
                seen_files = seen_files.len(),
                candidate_files = candidates.len(),
                current_file = %path.display(),
                "filesystem scan progress"
            );
        }
    }

    tracing::debug!(
        root = %root.display(),
        seen_files = seen_files.len(),
        candidate_files = candidates.len(),
        "finished filesystem scan"
    );
    Ok(ScanSnapshot {
        scan_started_at,
        seen_files,
        candidates,
    })
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

fn rel_path(root: &Path, path: &Path) -> Result<String, ScanError> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| ScanError::RelativePath {
            root: root.to_path_buf(),
            path: path.to_path_buf(),
        })?;
    Ok(rel.to_string_lossy().replace('\\', "/"))
}

fn is_ignored_dir(entry: &walkdir::DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }
    let Some(name) = entry.file_name().to_str() else {
        return false;
    };
    IGNORED_DIRS.contains(&name)
}
