use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, FileTimes};
use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::use_cases::dump_shadow::{visit_managed_files, DumpShadowError, ManagedFileAccess};
use sha2::{Digest, Sha256};
use thiserror::Error;

fn shadow_mtime_sentinel() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(86_400)
}

#[derive(Debug, Error)]
pub(crate) enum ShadowObservationError {
    #[error("failed to inspect private dump shadow: {0}")]
    Shadow(#[from] DumpShadowError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShadowFileState {
    len: u64,
    sha256: [u8; 32],
    identity: StableFileIdentity,
    mtime_ns: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StableFileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume: Option<u32>,
    #[cfg(windows)]
    index: Option<u64>,
}

/// Complete private-shadow observation captured before the platform command.
#[derive(Debug)]
pub(crate) struct ShadowObservation {
    files: BTreeMap<String, ShadowFileState>,
}

/// Effective platform scope used to interpret observable private-shadow writes.
#[derive(Debug, Clone, Copy)]
pub(crate) enum EffectiveWriteScope<'a> {
    Full { baseline: &'a BTreeSet<String> },
    Incremental,
}

/// Deterministic evidence of paths processed by the platform command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShadowWriteSet {
    paths: Vec<String>,
}

impl ShadowWriteSet {
    pub(crate) fn paths(&self) -> &[String] {
        &self.paths
    }

    #[cfg(test)]
    pub(crate) fn from_paths_for_test(paths: &[&str]) -> Self {
        Self {
            paths: paths.iter().map(|path| (*path).to_owned()).collect(),
        }
    }
}

impl ShadowObservation {
    pub(crate) fn normalize_and_capture(
        root: &Path,
        excluded_roots: &[PathBuf],
    ) -> Result<Self, ShadowObservationError> {
        let mut files = BTreeMap::new();
        visit_managed_files(
            root,
            excluded_roots,
            ManagedFileAccess::UpdateMetadata,
            |relative, file| {
                file.set_times(FileTimes::new().set_modified(shadow_mtime_sentinel()))?;
                files.insert(relative.to_owned(), capture_open_file(file)?);
                Ok(())
            },
        )?;
        Ok(Self { files })
    }

    pub(crate) fn observe_writes(
        self,
        root: &Path,
        excluded_roots: &[PathBuf],
        scope: EffectiveWriteScope<'_>,
    ) -> Result<ShadowWriteSet, ShadowObservationError> {
        let mut after = BTreeMap::new();
        visit_managed_files(
            root,
            excluded_roots,
            ManagedFileAccess::Read,
            |relative, file| {
                after.insert(relative.to_owned(), capture_open_file(file)?);
                Ok(())
            },
        )?;
        let mut paths = match scope {
            EffectiveWriteScope::Full { baseline } => after
                .keys()
                .cloned()
                .chain(
                    baseline
                        .iter()
                        .filter(|path| !after.contains_key(path.as_str()))
                        .cloned(),
                )
                .collect::<BTreeSet<_>>(),
            EffectiveWriteScope::Incremental => self
                .files
                .keys()
                .chain(after.keys())
                .filter(|path| self.files.get(*path) != after.get(*path))
                .cloned()
                .collect(),
        };
        Ok(ShadowWriteSet {
            paths: std::mem::take(&mut paths).into_iter().collect(),
        })
    }
}

fn capture_open_file(file: &mut File) -> io::Result<ShadowFileState> {
    file.rewind()?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed entry is not a regular file",
        ));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let modified = metadata
        .modified()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "mtime predates Unix epoch"))?
        .as_nanos();
    Ok(ShadowFileState {
        len: metadata.len(),
        sha256: hasher.finalize().into(),
        identity: stable_identity(&metadata),
        mtime_ns: modified,
    })
}

#[cfg(unix)]
fn stable_identity(metadata: &fs::Metadata) -> StableFileIdentity {
    use std::os::unix::fs::MetadataExt;
    StableFileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[cfg(windows)]
fn stable_identity(metadata: &fs::Metadata) -> StableFileIdentity {
    use std::os::windows::fs::MetadataExt;
    StableFileIdentity {
        volume: metadata.volume_serial_number(),
        index: metadata.file_index(),
    }
}

#[cfg(not(any(unix, windows)))]
fn stable_identity(_metadata: &fs::Metadata) -> StableFileIdentity {
    StableFileIdentity {}
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;

    use tempfile::tempdir;

    use super::{EffectiveWriteScope, ShadowObservation};

    #[test]
    fn unchanged_rewrite_is_observable_but_untouched_file_is_not() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("rewritten.txt"), b"same").expect("rewritten");
        fs::write(dir.path().join("untouched.txt"), b"same").expect("untouched");
        let before = ShadowObservation::normalize_and_capture(dir.path(), &[]).expect("before");

        fs::write(dir.path().join("rewritten.txt"), b"same").expect("rewrite");
        let writes = before
            .observe_writes(dir.path(), &[], EffectiveWriteScope::Incremental)
            .expect("writes");

        assert_eq!(writes.paths(), &["rewritten.txt"]);
    }

    #[test]
    fn full_scope_contains_complete_final_tree_and_baseline_deletions() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("kept.txt"), b"same").expect("kept");
        let before = ShadowObservation::normalize_and_capture(dir.path(), &[]).expect("before");
        let baseline = BTreeSet::from(["deleted.txt".to_owned(), "kept.txt".to_owned()]);

        let writes = before
            .observe_writes(
                dir.path(),
                &[],
                EffectiveWriteScope::Full {
                    baseline: &baseline,
                },
            )
            .expect("writes");

        assert_eq!(writes.paths(), &["deleted.txt", "kept.txt"]);
    }

    #[test]
    fn platform_owned_cdfi_is_excluded_from_write_evidence() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("ConfigDumpInfo.xml"), b"before").expect("cdfi");
        let before = ShadowObservation::normalize_and_capture(dir.path(), &[]).expect("before");
        fs::write(dir.path().join("ConfigDumpInfo.xml"), b"after").expect("cdfi rewrite");

        let writes = before
            .observe_writes(dir.path(), &[], EffectiveWriteScope::Incremental)
            .expect("writes");

        assert!(writes.paths().is_empty());
    }
}
