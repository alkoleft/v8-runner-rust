use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Create a directory and all missing parents.
pub fn ensure_dir(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
    }
    Ok(())
}

/// Remove all files and directories directly under `dir`.
pub fn clean_dir(dir: &Path) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
    }

    Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TempDirKind {
    Stage,
    Backup,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TempDirMetadata {
    pub tool: String,
    pub kind: TempDirKind,
    pub run_id: String,
    pub target_path: PathBuf,
    pub target_identity: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct AdvisoryLockGuard {
    file: File,
}

impl Drop for AdvisoryLockGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        // Best-effort unlock; close-on-drop also releases the lock.
        unsafe {
            libc::flock(std::os::fd::AsRawFd::as_raw_fd(&self.file), libc::LOCK_UN);
        }
    }
}

#[derive(Debug)]
pub struct ReplaceDirOutcome {
    pub cleanup_warning: Option<String>,
}

pub fn acquire_advisory_lock(path: &Path) -> std::io::Result<AdvisoryLockGuard> {
    acquire_advisory_lock_with_mode(path, false)
}

pub fn try_acquire_advisory_lock(path: &Path) -> std::io::Result<AdvisoryLockGuard> {
    acquire_advisory_lock_with_mode(path, true)
}

fn acquire_advisory_lock_with_mode(
    path: &Path,
    nonblocking: bool,
) -> std::io::Result<AdvisoryLockGuard> {
    #[cfg(not(unix))]
    {
        let _ = (path, nonblocking);
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "advisory locks are supported only on unix-like platforms",
        ));
    }

    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }

    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)?;

    #[cfg(unix)]
    unsafe {
        let mode = if nonblocking {
            libc::LOCK_EX | libc::LOCK_NB
        } else {
            libc::LOCK_EX
        };
        if libc::flock(std::os::fd::AsRawFd::as_raw_fd(&file), mode) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }

    Ok(AdvisoryLockGuard { file })
}

pub fn best_effort_fsync_dir(path: &Path) -> std::io::Result<()> {
    let dir = File::open(path)?;

    #[cfg(unix)]
    unsafe {
        let rc = libc::fsync(std::os::fd::AsRawFd::as_raw_fd(&dir));
        if rc == 0 {
            return Ok(());
        }

        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EINVAL) {
            return Ok(());
        }
        return Err(error);
    }

    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

pub fn metadata_sidecar_path(dir: &Path) -> PathBuf {
    let file_name = dir
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "temp-dir".to_owned());
    dir.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{file_name}.meta.json"))
}

pub fn write_temp_dir_metadata(
    dir: &Path,
    kind: TempDirKind,
    run_id: &str,
    target_path: &Path,
    target_identity: &str,
) -> std::io::Result<()> {
    let metadata = TempDirMetadata {
        tool: "v8-test-runner".to_owned(),
        kind,
        run_id: run_id.to_owned(),
        target_path: target_path.to_path_buf(),
        target_identity: target_identity.to_owned(),
        created_at: Utc::now(),
    };

    std::fs::write(
        metadata_sidecar_path(dir),
        serde_json::to_vec_pretty(&metadata)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?,
    )
}

pub fn read_temp_dir_metadata(dir: &Path) -> std::io::Result<TempDirMetadata> {
    let raw = std::fs::read(metadata_sidecar_path(dir))?;
    serde_json::from_slice(&raw)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

pub fn remove_path_if_exists(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }

    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

pub fn replace_dir_atomically(
    staging_dir: &Path,
    target_dir: &Path,
    run_id: &str,
    target_identity: &str,
) -> std::io::Result<ReplaceDirOutcome> {
    let parent = target_dir.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("target path has no parent: {}", target_dir.display()),
        )
    })?;
    let backup_dir = parent.join(format!(".dump-backup-{run_id}"));
    let stage_metadata_path = metadata_sidecar_path(staging_dir);
    let backup_metadata_path = metadata_sidecar_path(&backup_dir);

    if !target_dir.exists() {
        std::fs::rename(staging_dir, target_dir)?;
        let fsync_result = best_effort_fsync_dir(parent);
        let _ = remove_path_if_exists(&stage_metadata_path);
        fsync_result?;
        return Ok(ReplaceDirOutcome {
            cleanup_warning: None,
        });
    }

    std::fs::rename(target_dir, &backup_dir)?;
    if let Err(error) = best_effort_fsync_dir(parent) {
        let rollback_result =
            std::fs::rename(&backup_dir, target_dir).and_then(|()| best_effort_fsync_dir(parent));
        return Err(with_rollback_context(
            error,
            rollback_result.err(),
            "failed to fsync parent after moving target to backup",
        ));
    }

    if let Err(error) = write_temp_dir_metadata(
        &backup_dir,
        TempDirKind::Backup,
        run_id,
        target_dir,
        target_identity,
    ) {
        let rollback_result =
            std::fs::rename(&backup_dir, target_dir).and_then(|()| best_effort_fsync_dir(parent));
        return Err(with_rollback_context(
            error,
            rollback_result.err(),
            "failed to write backup metadata",
        ));
    }

    if let Err(error) = std::fs::rename(staging_dir, target_dir) {
        let rollback_result =
            std::fs::rename(&backup_dir, target_dir).and_then(|()| best_effort_fsync_dir(parent));
        return Err(with_rollback_context(
            error,
            rollback_result.err(),
            "failed to publish staged dump",
        ));
    }

    if let Err(error) = best_effort_fsync_dir(parent) {
        let rollback_result = std::fs::rename(target_dir, staging_dir)
            .and_then(|()| std::fs::rename(&backup_dir, target_dir))
            .and_then(|()| best_effort_fsync_dir(parent));
        return Err(with_rollback_context(
            error,
            rollback_result.err(),
            "failed to fsync parent after publishing staged dump",
        ));
    }

    let _ = remove_path_if_exists(&stage_metadata_path);

    let mut warnings = Vec::new();
    if let Err(error) = remove_path_if_exists(&backup_dir) {
        warnings.push(format!(
            "failed to remove backup dir '{}': {error}",
            backup_dir.display()
        ));
    } else if let Err(error) = remove_path_if_exists(&backup_metadata_path) {
        warnings.push(format!(
            "failed to remove backup metadata '{}': {error}",
            backup_metadata_path.display()
        ));
    }

    Ok(ReplaceDirOutcome {
        cleanup_warning: if warnings.is_empty() {
            None
        } else {
            Some(warnings.join("; "))
        },
    })
}

fn with_rollback_context(
    error: std::io::Error,
    rollback_error: Option<std::io::Error>,
    context: &str,
) -> std::io::Error {
    match rollback_error {
        Some(rollback_error) => std::io::Error::new(
            error.kind(),
            format!("{context}: {error}; rollback failed: {rollback_error}"),
        ),
        None => std::io::Error::new(error.kind(), format!("{context}: {error}")),
    }
}
