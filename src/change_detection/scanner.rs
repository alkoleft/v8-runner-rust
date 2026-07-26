use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
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

    #[error("managed path relative to '{root}' is not valid UTF-8: '{path}'")]
    NonUtf8RelativePath { root: PathBuf, path: PathBuf },

    #[error(
        "managed path relative to '{root}' contains a non-portable backslash component: '{path}'"
    )]
    NonPortableBackslash { root: PathBuf, path: PathBuf },

    #[error("failed to inspect excluded source root '{path}': {source}")]
    ExcludedRoot {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("excluded source root must be absolute: '{0}'")]
    InvalidExcludedRoot(PathBuf),
}

/// Precomputed policy defining which filesystem entries belong to a managed source tree.
#[derive(Debug, Clone)]
pub struct SourceInventoryPolicy {
    active_excluded_roots: Vec<PathBuf>,
}

impl SourceInventoryPolicy {
    pub fn new(root: &Path, excluded_roots: &[PathBuf]) -> Result<Self, ScanError> {
        let canonical_root = std::fs::canonicalize(root).map_err(|source| ScanError::Meta {
            path: root.to_path_buf(),
            source,
        })?;
        let mut active = Vec::new();
        for excluded in excluded_roots {
            if !excluded.is_absolute() {
                return Err(ScanError::InvalidExcludedRoot(excluded.clone()));
            }
            if excluded != root && excluded.starts_with(root) {
                active.push(excluded.clone());
            }
            match excluded.try_exists() {
                Ok(true) => {
                    let canonical_excluded = std::fs::canonicalize(excluded).map_err(|source| {
                        ScanError::ExcludedRoot {
                            path: excluded.clone(),
                            source,
                        }
                    })?;
                    if canonical_excluded != canonical_root
                        && canonical_excluded.starts_with(&canonical_root)
                    {
                        if let Ok(suffix) = canonical_excluded.strip_prefix(&canonical_root) {
                            active.push(root.join(suffix));
                        }
                    }
                }
                Ok(false) => {}
                Err(source) => {
                    return Err(ScanError::ExcludedRoot {
                        path: excluded.clone(),
                        source,
                    })
                }
            }
        }
        active.sort();
        active.dedup();
        Ok(Self {
            active_excluded_roots: active,
        })
    }

    pub fn excludes(&self, path: &Path) -> bool {
        self.active_excluded_roots
            .iter()
            .any(|excluded| path.starts_with(excluded))
    }

    /// Whether a directory is part of the inventory traversal.
    pub fn should_descend(&self, path: &Path) -> bool {
        !self.excludes(path) && !has_ignored_name(path, IGNORED_DIRS)
    }

    /// Whether a regular file belongs to the managed inventory.
    /// This path-only predicate is reusable by staged/private inventory implementations.
    pub fn includes_file(&self, path: &Path) -> bool {
        !self.excludes(path) && !has_ignored_name(path, IGNORED_FILES)
    }
}

/// Directory/file names that are always excluded from scanning.
const IGNORED_DIRS: &[&str] = &[
    ".git", ".gradle", "build", "target", "temp", "tmp", ".yaxunit",
];
const IGNORED_FILES: &[&str] = &["ConfigDumpInfo.xml"];

/// Whether a normalized relative path belongs to the repository-private inventory policy.
pub(crate) fn is_always_ignored_relative_path(path: &Path) -> bool {
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return true;
        };
        let text = name.to_string_lossy();
        if components.peek().is_some()
            && IGNORED_DIRS
                .iter()
                .any(|ignored| text.eq_ignore_ascii_case(ignored))
        {
            return true;
        }
        if components.peek().is_none()
            && IGNORED_FILES
                .iter()
                .any(|ignored| text.eq_ignore_ascii_case(ignored))
        {
            return true;
        }
    }
    false
}

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
    excluded_roots: &[PathBuf],
) -> Result<ScanSnapshot, ScanError> {
    let policy = SourceInventoryPolicy::new(root, excluded_roots)?;
    let scan_started_at =
        mtime_nanos(std::time::SystemTime::now(), root).map_err(|source| ScanError::Mtime {
            path: root.to_path_buf(),
            source,
        })?;
    let mut seen_files = Vec::new();
    let mut candidates = Vec::new();

    let cutoff = watermark.map(|w| w.saturating_sub(COARSE_MARGIN_NS));
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !entry.file_type().is_dir() || policy.should_descend(entry.path()))
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
        if !policy.includes_file(path) {
            continue;
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
    }

    seen_files.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    candidates.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));

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

/// Convert a path below `root` to the collision-free portable inventory representation.
pub fn portable_relative_path(root: &Path, path: &Path) -> Result<String, ScanError> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| ScanError::RelativePath {
            root: root.to_path_buf(),
            path: path.to_path_buf(),
        })?;
    let mut normalized = Vec::new();
    for component in rel.components() {
        let Component::Normal(component) = component else {
            return Err(ScanError::RelativePath {
                root: root.to_path_buf(),
                path: path.to_path_buf(),
            });
        };
        let component = component
            .to_str()
            .ok_or_else(|| ScanError::NonUtf8RelativePath {
                root: root.to_path_buf(),
                path: path.to_path_buf(),
            })?;
        if component.contains('\\') {
            return Err(ScanError::NonPortableBackslash {
                root: root.to_path_buf(),
                path: path.to_path_buf(),
            });
        }
        normalized.push(component);
    }
    Ok(normalized.join("/"))
}

fn rel_path(root: &Path, path: &Path) -> Result<String, ScanError> {
    portable_relative_path(root, path)
}

fn has_ignored_name(path: &Path, ignored_names: &[&str]) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            ignored_names
                .iter()
                .any(|ignored| name.eq_ignore_ascii_case(ignored))
        })
}

#[cfg(test)]
mod tests {
    use super::{rel_path, scan, ScanError};
    use std::collections::HashSet;
    use tempfile::tempdir;

    #[test]
    fn excludes_work_root_and_config_dump_info_case_insensitively() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let work = root.join("work");
        std::fs::create_dir(&work).expect("work");
        std::fs::write(root.join("Configuration.xml"), "managed").expect("managed");
        std::fs::write(root.join("configdumpinfo.XML"), "dump metadata").expect("dump info");
        std::fs::write(work.join("runtime.redb"), "state").expect("state");

        let snapshot = scan(root, None, &HashSet::new(), &[work]).expect("scan");
        let paths = snapshot
            .seen_files
            .iter()
            .map(|file| file.rel_path.as_str())
            .collect::<Vec<_>>();

        assert_eq!(paths, vec!["Configuration.xml"]);
    }

    #[cfg(unix)]
    #[test]
    fn never_follows_symlinked_files_or_directories() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("src");
        let outside = dir.path().join("outside");
        std::fs::create_dir(&root).expect("root");
        std::fs::create_dir(&outside).expect("outside");
        std::fs::write(outside.join("secret.xml"), "secret").expect("secret");
        std::os::unix::fs::symlink(&outside, root.join("linked-dir")).expect("dir link");
        std::os::unix::fs::symlink(outside.join("secret.xml"), root.join("linked-file.xml"))
            .expect("file link");

        let snapshot = scan(&root, None, &HashSet::new(), &[]).expect("scan");

        assert!(snapshot.seen_files.is_empty());
    }

    #[test]
    fn ancestor_exclusion_does_not_blank_generated_tree_inside_work_path() {
        let dir = tempdir().expect("tempdir");
        let work = dir.path().join("work");
        let generated = work.join("designer/main");
        std::fs::create_dir_all(&generated).expect("generated");
        std::fs::write(generated.join("Configuration.xml"), "managed").expect("managed");

        let snapshot = scan(&generated, None, &HashSet::new(), &[work]).expect("scan");

        assert_eq!(snapshot.seen_files.len(), 1);
        assert_eq!(snapshot.seen_files[0].rel_path, "Configuration.xml");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_utf8_relative_paths_without_lossy_collisions() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let root = std::path::Path::new("/source");
        let path = root.join(OsString::from_vec(vec![b'a', 0xff]));

        let error = rel_path(root, &path).expect_err("non-utf8");
        assert!(matches!(error, ScanError::NonUtf8RelativePath { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_literal_backslash_in_unix_component() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a\\b.xml"), "managed").expect("file");

        let error = scan(dir.path(), None, &HashSet::new(), &[]).expect_err("backslash");
        assert!(matches!(error, ScanError::NonPortableBackslash { .. }));
    }
}
