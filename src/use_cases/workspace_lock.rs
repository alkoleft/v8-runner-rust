use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::config::model::AppConfig;
use crate::support::error::AppError;
use crate::support::fs::{try_acquire_advisory_lock, AdvisoryLockGuard};

const WORKSPACE_LOCK_FILE_NAME: &str = ".v8-test-runner.workspace.lock";
const WORKSPACE_LOCK_SIDECAR_FILE_NAME: &str = ".v8-test-runner.workspace.lock.json";

#[derive(Debug)]
pub(crate) struct WorkspaceLockGuard {
    _lock: AdvisoryLockGuard,
    sidecar_path: PathBuf,
}

impl Drop for WorkspaceLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.sidecar_path);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceLockMetadata {
    pid: u32,
    #[serde(default)]
    holder_start_ticks: Option<u64>,
    command: String,
    started_at: DateTime<Utc>,
    canonical_work_path: PathBuf,
}

pub(crate) fn acquire_workspace_lock(
    config: &AppConfig,
    command_name: &str,
) -> Result<WorkspaceLockGuard, AppError> {
    let canonical_work_path =
        nearest_existing_canonical_path(&config.work_path).map_err(|error| {
            AppError::Runtime(format!(
                "failed to canonicalize workPath '{}': {error}",
                config.work_path.display()
            ))
        })?;
    let lock_path = workspace_lock_path(&canonical_work_path);
    let sidecar_path = workspace_lock_sidecar_path(&canonical_work_path);

    let lock = try_acquire_advisory_lock(&lock_path).map_err(|error| match error.kind() {
        ErrorKind::WouldBlock => AppError::Runtime(render_busy_message(
            command_name,
            &canonical_work_path,
            &sidecar_path,
        )),
        _ => AppError::Runtime(format!(
            "failed to acquire {command_name} workspace lock '{}': {error}",
            lock_path.display()
        )),
    })?;

    cleanup_sidecar_temp_files(&canonical_work_path);

    if let Err(error) = write_lock_metadata(&sidecar_path, command_name, &canonical_work_path) {
        let _ = std::fs::remove_file(&sidecar_path);
        warn!(
            command = command_name,
            sidecar_path = %sidecar_path.display(),
            error = %error,
            "failed to write workspace lock metadata; continuing without sidecar"
        );
    }

    Ok(WorkspaceLockGuard {
        _lock: lock,
        sidecar_path,
    })
}

pub(crate) fn workspace_lock_path(work_path: &Path) -> PathBuf {
    work_path.join(WORKSPACE_LOCK_FILE_NAME)
}

fn workspace_lock_sidecar_path(work_path: &Path) -> PathBuf {
    work_path.join(WORKSPACE_LOCK_SIDECAR_FILE_NAME)
}

fn write_lock_metadata(
    sidecar_path: &Path,
    command_name: &str,
    canonical_work_path: &Path,
) -> Result<(), AppError> {
    let metadata = WorkspaceLockMetadata {
        pid: std::process::id(),
        holder_start_ticks: current_process_start_ticks(),
        command: command_name.to_owned(),
        started_at: Utc::now(),
        canonical_work_path: canonical_work_path.to_path_buf(),
    };
    let encoded = serde_json::to_vec_pretty(&metadata).map_err(|error| {
        AppError::Runtime(format!("failed to encode workspace lock metadata: {error}"))
    })?;
    let temp_path = sidecar_path.with_extension(format!(
        "{}.tmp.{}",
        sidecar_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("json"),
        std::process::id()
    ));
    std::fs::write(&temp_path, encoded).map_err(|error| {
        AppError::Runtime(format!(
            "failed to write temporary workspace lock metadata '{}': {error}",
            temp_path.display()
        ))
    })?;
    std::fs::rename(&temp_path, sidecar_path).map_err(|error| {
        let _ = std::fs::remove_file(&temp_path);
        AppError::Runtime(format!(
            "failed to publish workspace lock metadata '{}': {error}",
            sidecar_path.display()
        ))
    })
}

fn render_busy_message(
    command_name: &str,
    canonical_work_path: &Path,
    sidecar_path: &Path,
) -> String {
    match read_lock_metadata(sidecar_path).ok().filter(metadata_holder_is_live) {
        Some(metadata) => format!(
            "cannot start {command_name}: workspace '{}' is already locked by '{}' (pid {}, started at {})",
            canonical_work_path.display(),
            metadata.command,
            metadata.pid,
            metadata.started_at.to_rfc3339(),
        ),
        None => format!(
            "cannot start {command_name}: workspace '{}' is already in use by another command",
            canonical_work_path.display()
        ),
    }
}

fn read_lock_metadata(path: &Path) -> std::io::Result<WorkspaceLockMetadata> {
    let raw = std::fs::read(path)?;
    serde_json::from_slice(&raw)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn metadata_holder_is_live(metadata: &WorkspaceLockMetadata) -> bool {
    #[cfg(unix)]
    {
        let rc = unsafe { libc::kill(metadata.pid as i32, 0) };
        if rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) {
            return match metadata.holder_start_ticks {
                Some(expected) => process_start_ticks(metadata.pid) == Some(expected),
                None => false,
            };
        }
        return false;
    }

    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
    }
}

fn cleanup_sidecar_temp_files(work_path: &Path) {
    let prefix = format!("{WORKSPACE_LOCK_SIDECAR_FILE_NAME}.tmp.");
    let Ok(entries) = std::fs::read_dir(work_path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let matches_prefix = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(&prefix));
        if matches_prefix {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(target_os = "linux")]
fn current_process_start_ticks() -> Option<u64> {
    process_start_ticks(std::process::id())
}

#[cfg(not(target_os = "linux"))]
fn current_process_start_ticks() -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
fn process_start_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, rest) = stat.rsplit_once(") ")?;
    rest.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
fn process_start_ticks(pid: u32) -> Option<u64> {
    let _ = pid;
    None
}

fn nearest_existing_canonical_path(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    let mut existing = absolute.as_path();
    while !existing.exists() {
        existing = existing.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no existing ancestor for path '{}'", path.display()),
            )
        })?;
    }

    let existing_canonical = std::fs::canonicalize(existing)?;
    if existing == absolute {
        return Ok(existing_canonical);
    }

    let suffix = absolute
        .strip_prefix(existing)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    Ok(existing_canonical.join(suffix))
}

#[cfg(test)]
mod tests {
    use super::{
        acquire_workspace_lock, workspace_lock_path, WorkspaceLockGuard,
        WORKSPACE_LOCK_SIDECAR_FILE_NAME,
    };
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, SourceFormat, SourceSetConfig, SourceSetPurpose,
        TestsConfig, ToolsConfig,
    };
    use crate::support::fs::acquire_advisory_lock;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    fn sample_config(work_path: &Path) -> AppConfig {
        AppConfig {
            base_path: work_path.join("base"),
            work_path: work_path.to_path_buf(),
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            connection: "File=/tmp/ib".to_owned(),
            credentials: Default::default(),
            source_sets: vec![SourceSetConfig {
                name: "main".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: PathBuf::from("main"),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig::default(),
            mcp: Default::default(),
            tests: TestsConfig::default(),
        }
    }

    fn hold_lock(config: &AppConfig, command_name: &str) -> WorkspaceLockGuard {
        acquire_workspace_lock(config, command_name).expect("workspace lock")
    }

    #[cfg(unix)]
    #[test]
    fn conflicts_use_canonical_workspace_path_and_sidecar_metadata() {
        let dir = tempdir().expect("tempdir");
        let real_work = dir.path().join("real-work");
        fs::create_dir_all(&real_work).expect("work dir");
        let link_work = dir.path().join("work-link");
        std::os::unix::fs::symlink(&real_work, &link_work).expect("symlink");

        let first = sample_config(&real_work);
        let second = sample_config(&link_work);
        let _guard = hold_lock(&first, "build");

        let error = acquire_workspace_lock(&second, "test").expect_err("busy workspace");
        let message = error.to_string();

        assert!(message.contains(
            &std::fs::canonicalize(&real_work)
                .expect("canonical")
                .display()
                .to_string()
        ));
        assert!(message.contains("'build'"));
        assert!(message.contains("pid"));
        assert!(message.contains("started at"));
    }

    #[cfg(unix)]
    #[test]
    fn stale_sidecar_metadata_falls_back_to_generic_busy_message() {
        let dir = tempdir().expect("tempdir");
        let work = dir.path().join("work");
        fs::create_dir_all(&work).expect("work dir");
        let config = sample_config(&work);
        let canonical_work = std::fs::canonicalize(&work).expect("canonical");
        let lock_path = workspace_lock_path(&canonical_work);
        let _guard = acquire_advisory_lock(&lock_path).expect("workspace lock");
        let sidecar_path = canonical_work.join(WORKSPACE_LOCK_SIDECAR_FILE_NAME);
        fs::write(
            &sidecar_path,
            r#"{"pid":999999,"command":"build","started_at":"2026-01-01T00:00:00Z","canonical_work_path":"/tmp/stale"}"#,
        )
        .expect("sidecar");

        let error = acquire_workspace_lock(&config, "test").expect_err("busy workspace");
        let message = error.to_string();

        assert!(message.contains("already in use by another command"));
        assert!(!message.contains("999999"));
        assert!(!message.contains("'build'"));
    }

    #[test]
    fn next_lock_acquisition_cleans_stale_sidecar_temp_files() {
        let dir = tempdir().expect("tempdir");
        let work = dir.path().join("work");
        fs::create_dir_all(&work).expect("work dir");
        let config = sample_config(&work);
        let canonical_work = std::fs::canonicalize(&work).expect("canonical");
        let stale_temp =
            canonical_work.join(format!("{WORKSPACE_LOCK_SIDECAR_FILE_NAME}.tmp.stale"));
        fs::write(&stale_temp, b"stale").expect("stale temp");

        let _guard = hold_lock(&config, "build");

        assert!(!stale_temp.exists());
    }

    #[test]
    fn drop_removes_sidecar_file() {
        let dir = tempdir().expect("tempdir");
        let work = dir.path().join("work");
        fs::create_dir_all(&work).expect("work dir");
        let config = sample_config(&work);
        let canonical_work = std::fs::canonicalize(&work).expect("canonical");
        let sidecar = canonical_work.join(".v8-test-runner.workspace.lock.json");

        let guard = hold_lock(&config, "build");
        assert!(workspace_lock_path(&canonical_work).exists());
        assert!(sidecar.exists());
        drop(guard);

        assert!(!sidecar.exists());
    }
}
