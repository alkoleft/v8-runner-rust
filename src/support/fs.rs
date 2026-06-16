use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const TOOL_NAME: &str = "v8-runner";

pub fn is_known_tool_name(tool: &str) -> bool {
    tool == TOOL_NAME
}

#[cfg(test)]
thread_local! {
    static TEST_LOCK_WRITE_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> =
        std::cell::RefCell::new(None);
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvisoryLockMetadata {
    pub tool: String,
    pub pid: u32,
    pub owner_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct AdvisoryLockGuard {
    #[allow(dead_code)]
    file: Option<File>,
    path: PathBuf,
    metadata: AdvisoryLockMetadata,
}

impl Drop for AdvisoryLockGuard {
    fn drop(&mut self) {
        self.file.take();
        if lock_file_owned_by(&self.path, &self.metadata.owner_id) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[derive(Debug)]
pub struct ReplaceDirOutcome {
    pub cleanup_warning: Option<String>,
}

#[derive(Debug)]
pub struct ReplaceFileOutcome {
    pub cleanup_warning: Option<String>,
}

pub fn acquire_advisory_lock(path: &Path) -> std::io::Result<AdvisoryLockGuard> {
    loop {
        match try_acquire_advisory_lock(path) {
            Ok(guard) => return Ok(guard),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error),
        }
    }
}

pub fn try_acquire_advisory_lock(path: &Path) -> std::io::Result<AdvisoryLockGuard> {
    try_acquire_advisory_lock_impl(path)
}

fn try_acquire_advisory_lock_impl(path: &Path) -> std::io::Result<AdvisoryLockGuard> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }

    let metadata = AdvisoryLockMetadata {
        tool: TOOL_NAME.to_owned(),
        pid: std::process::id(),
        owner_id: Uuid::new_v4().to_string(),
        created_at: Utc::now(),
    };
    let encoded = serde_json::to_vec_pretty(&metadata)
        .map_err(|error| std::io::Error::new(ErrorKind::InvalidData, error))?;
    loop {
        match OpenOptions::new().create_new(true).write(true).open(path) {
            Ok(mut file) => {
                #[cfg(test)]
                TEST_LOCK_WRITE_HOOK.with(|cell| {
                    if let Some(hook) = cell.borrow().as_ref() {
                        hook();
                    }
                });

                let write_result = file.write_all(&encoded).and_then(|()| file.sync_all());
                if let Err(error) = write_result {
                    let _ = std::fs::remove_file(path);
                    return Err(error);
                }
                return Ok(AdvisoryLockGuard {
                    file: Some(file),
                    path: path.to_path_buf(),
                    metadata,
                });
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                if stale_advisory_lock_can_be_removed(path) {
                    match std::fs::remove_file(path) {
                        Ok(()) => continue,
                        Err(remove_error) if remove_error.kind() == ErrorKind::NotFound => continue,
                        Err(remove_error) => return Err(remove_error),
                    }
                }
                return Err(std::io::Error::new(
                    ErrorKind::WouldBlock,
                    format!("lock is already held: {}", path.display()),
                ));
            }
            Err(error) => return Err(error),
        }
    }
}

pub fn advisory_lock_owner_id(guard: &AdvisoryLockGuard) -> &str {
    &guard.metadata.owner_id
}

pub fn read_advisory_lock_metadata(path: &Path) -> std::io::Result<AdvisoryLockMetadata> {
    let raw = std::fs::read(path)?;
    serde_json::from_slice(&raw).map_err(|error| std::io::Error::new(ErrorKind::InvalidData, error))
}

fn stale_advisory_lock_can_be_removed(path: &Path) -> bool {
    match read_advisory_lock_metadata(path) {
        Ok(metadata) => !lock_holder_is_live(metadata.pid),
        Err(error) if error.kind() == ErrorKind::NotFound => true,
        Err(error) if error.kind() == ErrorKind::InvalidData => std::fs::metadata(path)
            .map(|metadata| metadata.len() > 0)
            .unwrap_or(true),
        Err(_) => false,
    }
}

#[cfg(test)]
fn try_acquire_advisory_lock_with_hook<F>(
    path: &Path,
    publish_hook: F,
) -> std::io::Result<AdvisoryLockGuard>
where
    F: Fn() + 'static,
{
    TEST_LOCK_WRITE_HOOK.with(|cell| {
        *cell.borrow_mut() = Some(Box::new(publish_hook));
    });
    let result = try_acquire_advisory_lock(path);
    TEST_LOCK_WRITE_HOOK.with(|cell| {
        *cell.borrow_mut() = None;
    });
    result
}

#[cfg(unix)]
fn lock_holder_is_live(pid: u32) -> bool {
    let rc = unsafe { libc::kill(pid as i32, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn lock_holder_is_live(pid: u32) -> bool {
    type Bool = i32;
    type Dword = u32;
    type Handle = *mut std::ffi::c_void;

    const PROCESS_QUERY_LIMITED_INFORMATION: Dword = 0x1000;
    const ERROR_INVALID_PARAMETER: Dword = 87;
    const ERROR_ACCESS_DENIED: Dword = 5;
    const STILL_ACTIVE: Dword = 259;

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(desired_access: Dword, inherit_handle: Bool, process_id: Dword) -> Handle;
        fn GetExitCodeProcess(process: Handle, exit_code: *mut Dword) -> Bool;
        fn CloseHandle(handle: Handle) -> Bool;
        fn GetLastError() -> Dword;
    }

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return match GetLastError() {
                ERROR_INVALID_PARAMETER => false,
                ERROR_ACCESS_DENIED => true,
                _ => true,
            };
        }

        let mut exit_code = 0;
        let ok = GetExitCodeProcess(handle, &mut exit_code);
        let _ = CloseHandle(handle);
        ok != 0 && exit_code == STILL_ACTIVE
    }
}

#[cfg(not(any(unix, windows)))]
fn lock_holder_is_live(_pid: u32) -> bool {
    true
}

pub fn best_effort_fsync_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    unsafe {
        let dir = File::open(path)?;
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
        let _ = path;
        Ok(())
    }
}

pub fn publish_file_atomically(temp_path: &Path, destination_path: &Path) -> std::io::Result<()> {
    publish_file_atomically_impl(
        temp_path,
        destination_path,
        &|from, to| std::fs::rename(from, to),
        &|path| remove_path_if_exists(path),
    )
}

fn publish_file_atomically_impl(
    temp_path: &Path,
    destination_path: &Path,
    rename: &dyn for<'a, 'b> Fn(&'a Path, &'b Path) -> std::io::Result<()>,
    cleanup: &dyn Fn(&Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    if !destination_path.exists() {
        return rename(temp_path, destination_path);
    }

    let parent = destination_path.parent().ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "destination path has no parent: {}",
                destination_path.display()
            ),
        )
    })?;
    let backup_path = parent.join(format!(
        ".{}.backup-{}",
        destination_path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "artifact".to_owned()),
        Uuid::new_v4()
    ));

    rename(destination_path, &backup_path)?;
    let publish_result = rename(temp_path, destination_path);
    match publish_result {
        Ok(()) => {
            let _ = cleanup(&backup_path);
            Ok(())
        }
        Err(error) => {
            let rollback_result = rename(&backup_path, destination_path);
            match rollback_result {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(std::io::Error::new(
                    error.kind(),
                    format!(
                        "failed to publish '{}' atomically: {error}; rollback failed: {rollback_error}",
                        destination_path.display()
                    ),
                )),
            }
        }
    }
}

fn lock_file_owned_by(path: &Path, owner_id: &str) -> bool {
    read_advisory_lock_metadata(path)
        .map(|metadata| metadata.owner_id == owner_id)
        .unwrap_or(false)
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
        tool: TOOL_NAME.to_owned(),
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
    backup_prefix: &str,
) -> std::io::Result<ReplaceDirOutcome> {
    let parent = target_dir.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("target path has no parent: {}", target_dir.display()),
        )
    })?;
    let backup_dir = parent.join(format!("{backup_prefix}-{run_id}"));
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

pub fn replace_file_atomically(
    staging_file: &Path,
    target_file: &Path,
    run_id: &str,
    target_identity: &str,
) -> std::io::Result<ReplaceFileOutcome> {
    let parent = target_file.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("target path has no parent: {}", target_file.display()),
        )
    })?;
    let backup_name = target_file
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "artifact".to_owned());
    let backup_file = parent.join(format!(".{backup_name}.backup-{run_id}"));
    let stage_metadata_path = metadata_sidecar_path(staging_file);
    let backup_metadata_path = metadata_sidecar_path(&backup_file);

    if !target_file.exists() {
        publish_file_atomically(staging_file, target_file)?;
        let fsync_result = best_effort_fsync_dir(parent);
        let _ = remove_path_if_exists(&stage_metadata_path);
        fsync_result?;
        return Ok(ReplaceFileOutcome {
            cleanup_warning: None,
        });
    }

    std::fs::rename(target_file, &backup_file)?;
    if let Err(error) = best_effort_fsync_dir(parent) {
        let rollback_result =
            std::fs::rename(&backup_file, target_file).and_then(|()| best_effort_fsync_dir(parent));
        return Err(with_rollback_context(
            error,
            rollback_result.err(),
            "failed to fsync parent after moving target file to backup",
        ));
    }

    if let Err(error) = write_temp_dir_metadata(
        &backup_file,
        TempDirKind::Backup,
        run_id,
        target_file,
        target_identity,
    ) {
        let rollback_result =
            std::fs::rename(&backup_file, target_file).and_then(|()| best_effort_fsync_dir(parent));
        return Err(with_rollback_context(
            error,
            rollback_result.err(),
            "failed to write backup file metadata",
        ));
    }

    if let Err(error) = publish_file_atomically(staging_file, target_file) {
        let rollback_result = publish_file_atomically(&backup_file, target_file)
            .and_then(|()| best_effort_fsync_dir(parent));
        return Err(with_rollback_context(
            error,
            rollback_result.err(),
            "failed to publish staged artifact file",
        ));
    }

    if let Err(error) = best_effort_fsync_dir(parent) {
        let rollback_result = std::fs::rename(target_file, staging_file)
            .and_then(|()| publish_file_atomically(&backup_file, target_file))
            .and_then(|()| best_effort_fsync_dir(parent));
        return Err(with_rollback_context(
            error,
            rollback_result.err(),
            "failed to fsync parent after publishing staged artifact file",
        ));
    }

    let _ = remove_path_if_exists(&stage_metadata_path);

    let mut warnings = Vec::new();
    if let Err(error) = remove_path_if_exists(&backup_file) {
        warnings.push(format!(
            "failed to remove backup file '{}': {error}",
            backup_file.display()
        ));
    } else if let Err(error) = remove_path_if_exists(&backup_metadata_path) {
        warnings.push(format!(
            "failed to remove backup metadata '{}': {error}",
            backup_metadata_path.display()
        ));
    }

    Ok(ReplaceFileOutcome {
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

#[cfg(test)]
mod tests {
    use super::{
        acquire_advisory_lock, advisory_lock_owner_id, publish_file_atomically,
        publish_file_atomically_impl, read_advisory_lock_metadata, remove_path_if_exists,
        try_acquire_advisory_lock, try_acquire_advisory_lock_with_hook, AdvisoryLockMetadata,
        TOOL_NAME,
    };
    use std::fs;
    use std::io::ErrorKind;
    use std::path::Path;
    use std::sync::mpsc;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::thread;
    use std::time::Duration;
    use tempfile::tempdir;

    #[test]
    fn try_acquire_advisory_lock_reports_busy() {
        let dir = tempdir().expect("tempdir");
        let lock_path = dir.path().join("busy.lock");
        let _guard = acquire_advisory_lock(&lock_path).expect("lock");

        let error = try_acquire_advisory_lock(&lock_path).expect_err("busy");

        assert_eq!(error.kind(), ErrorKind::WouldBlock);
    }

    #[test]
    fn advisory_lock_writes_owner_metadata() {
        let dir = tempdir().expect("tempdir");
        let lock_path = dir.path().join("owner.lock");
        let guard = acquire_advisory_lock(&lock_path).expect("lock");

        let metadata = read_advisory_lock_metadata(&lock_path).expect("metadata");

        assert_eq!(metadata.pid, std::process::id());
        assert_eq!(metadata.owner_id, advisory_lock_owner_id(&guard));
    }

    #[test]
    fn advisory_lock_serializes_blocking_waiters() {
        let dir = tempdir().expect("tempdir");
        let lock_path = dir.path().join("serialized.lock");
        let guard = acquire_advisory_lock(&lock_path).expect("lock");
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let lock_path_clone = lock_path.clone();

        let handle = thread::spawn(move || {
            started_tx.send(()).expect("send started");
            let _guard = acquire_advisory_lock(&lock_path_clone).expect("second lock");
            done_tx.send(()).expect("send done");
        });

        started_rx.recv().expect("started");
        assert!(done_rx.recv_timeout(Duration::from_millis(100)).is_err());
        drop(guard);
        done_rx.recv_timeout(Duration::from_secs(1)).expect("done");
        handle.join().expect("join");
    }

    #[test]
    fn publish_file_atomically_replaces_existing_destination() {
        let dir = tempdir().expect("tempdir");
        let temp = dir.path().join("temp.json");
        let destination = dir.path().join("dest.json");
        fs::write(&temp, "new").expect("temp");
        fs::write(&destination, "old").expect("dest");

        publish_file_atomically(&temp, &destination).expect("publish");

        assert_eq!(fs::read_to_string(&destination).expect("dest"), "new");
        assert!(!temp.exists());
    }

    #[test]
    fn publish_file_atomically_restores_backup_when_publish_fails() {
        let dir = tempdir().expect("tempdir");
        let temp = dir.path().join("temp.json");
        let destination = dir.path().join("dest.json");
        fs::write(&temp, "new").expect("temp");
        fs::write(&destination, "old").expect("dest");

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = calls.clone();
        let temp_path = temp.clone();
        let destination_path = destination.clone();
        let rename = move |from: &Path, to: &Path| {
            let count = calls_clone.fetch_add(1, Ordering::SeqCst);
            if count == 1 && from == temp_path.as_path() && to == destination_path.as_path() {
                return Err(std::io::Error::new(
                    ErrorKind::PermissionDenied,
                    "simulated failure",
                ));
            }
            fs::rename(from, to)
        };

        let error = publish_file_atomically_impl(&temp, &destination, &rename, &|path| {
            remove_path_if_exists(path)
        })
        .expect_err("publish");

        assert_eq!(error.kind(), ErrorKind::PermissionDenied);
        assert_eq!(fs::read_to_string(&destination).expect("dest"), "old");
        assert!(temp.exists());
    }

    #[test]
    fn stale_corrupt_lock_file_can_be_recovered() {
        let dir = tempdir().expect("tempdir");
        let lock_path = dir.path().join("corrupt.lock");
        let original = b"{not valid json".to_vec();
        fs::write(&lock_path, &original).expect("lock");

        let guard = try_acquire_advisory_lock(&lock_path).expect("recovered lock");

        assert_eq!(advisory_lock_owner_id(&guard).len(), 36);
        assert_ne!(fs::read(&lock_path).expect("lock"), original);
    }

    #[test]
    fn live_advisory_lock_metadata_remains_busy() {
        let dir = tempdir().expect("tempdir");
        let lock_path = dir.path().join("live.lock");
        let metadata = AdvisoryLockMetadata {
            tool: TOOL_NAME.to_owned(),
            pid: std::process::id(),
            owner_id: "live-owner".to_owned(),
            created_at: chrono::Utc::now(),
        };
        fs::write(
            &lock_path,
            serde_json::to_vec_pretty(&metadata).expect("metadata"),
        )
        .expect("live lock");

        let error = try_acquire_advisory_lock(&lock_path).expect_err("busy lock");

        assert_eq!(error.kind(), ErrorKind::WouldBlock);
        assert_eq!(
            read_advisory_lock_metadata(&lock_path)
                .expect("metadata")
                .owner_id,
            "live-owner"
        );
    }

    #[test]
    fn concurrent_acquisition_cannot_steal_lock_during_initial_publish() {
        let dir = tempdir().expect("tempdir");
        let lock_path = dir.path().join("publish.lock");
        let (hook_ready_tx, hook_ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let lock_path_clone = lock_path.clone();

        let handle = thread::spawn(move || {
            let hook = move || {
                hook_ready_tx.send(()).expect("signal hook");
                release_rx.recv().expect("release hook");
            };
            try_acquire_advisory_lock_with_hook(&lock_path_clone, hook).expect("first lock");
        });

        hook_ready_rx.recv().expect("hook reached");
        let contender = thread::spawn({
            let lock_path = lock_path.clone();
            move || try_acquire_advisory_lock(&lock_path)
        });

        thread::sleep(Duration::from_millis(100));
        release_tx.send(()).expect("release hook");

        handle.join().expect("join first");

        let second_result = contender.join().expect("join second");
        assert!(matches!(second_result, Err(error) if error.kind() == ErrorKind::WouldBlock));
    }

    #[test]
    fn publish_file_atomically_ignores_backup_cleanup_failure() {
        let dir = tempdir().expect("tempdir");
        let temp = dir.path().join("temp.json");
        let destination = dir.path().join("dest.json");
        let backup_path = dir.path().join(format!(
            ".{}.backup-test",
            destination
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_else(|| "artifact".to_owned())
        ));
        fs::write(&temp, "new").expect("temp");
        fs::write(&destination, "old").expect("dest");
        fs::write(&backup_path, "stale backup").expect("backup");

        let cleanup = |_path: &Path| {
            Err(std::io::Error::new(
                ErrorKind::PermissionDenied,
                "cleanup failed",
            ))
        };
        let result = publish_file_atomically_impl(
            &temp,
            &destination,
            &|from, to| fs::rename(from, to),
            &cleanup,
        );

        assert!(result.is_ok());
        assert_eq!(fs::read_to_string(&destination).expect("dest"), "new");
    }

    #[cfg(windows)]
    #[test]
    fn windows_reports_missing_process_as_not_live() {
        assert!(!super::lock_holder_is_live(u32::MAX));
    }
}
