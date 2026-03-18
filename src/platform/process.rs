use std::path::PathBuf;
use std::process::{Command, Stdio};

use thiserror::Error;

/// Request for launching an external utility.
#[derive(Debug, Clone)]
pub struct ProcessRequest {
    /// Absolute path to the executable to run.
    pub program: PathBuf,
    /// Command-line arguments passed to the executable.
    pub args: Vec<String>,
    /// Optional working directory for the child process.
    pub workdir: Option<PathBuf>,
    /// Optional path where runner-captured stdout is mirrored.
    pub stdout_log_path: Option<PathBuf>,
    /// Optional path where runner-captured stderr is mirrored.
    pub stderr_log_path: Option<PathBuf>,
}

/// Result of a completed `run()` invocation.
#[derive(Debug, Clone)]
pub struct ProcessResult {
    /// Child exit code.
    pub exit_code: i32,
    /// Captured stdout as UTF-8 (lossy-decoded).
    pub stdout: String,
    /// Captured stderr as UTF-8 (lossy-decoded).
    pub stderr: String,
    /// Path where stdout was mirrored by the runner, if requested.
    pub stdout_log_path: Option<PathBuf>,
    /// Path where stderr was mirrored by the runner, if requested.
    pub stderr_log_path: Option<PathBuf>,
}

/// Result of a detached `spawn()` invocation.
#[derive(Debug, Clone)]
pub struct SpawnResult {
    /// Operating system process identifier.
    pub pid: u32,
    /// Binary that was used to start the process.
    pub binary: PathBuf,
}

/// Runner-level process execution failures.
#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("failed to spawn process '{cmd}': {source}")]
    SpawnFailed { cmd: String, source: std::io::Error },

    #[error("failed to write stdout log '{path}': {source}")]
    StdoutLogIo {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to write stderr log '{path}': {source}")]
    StderrLogIo {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Boundary for synchronous and detached process execution.
pub trait ProcessRunner {
    /// Execute a process and wait for completion, capturing stdout/stderr.
    fn run(&self, request: &ProcessRequest) -> Result<ProcessResult, ProcessError>;

    /// Start a process in fire-and-forget mode without waiting for completion.
    fn spawn(&self, request: &ProcessRequest) -> Result<SpawnResult, ProcessError>;
}

/// Standard subprocess runner backed by `std::process::Command`.
pub struct ProcessExecutor;

impl ProcessRunner for ProcessExecutor {
    fn run(&self, request: &ProcessRequest) -> Result<ProcessResult, ProcessError> {
        let mut cmd = Command::new(&request.program);
        cmd.args(&request.args);
        if let Some(workdir) = &request.workdir {
            cmd.current_dir(workdir);
        }

        let output = cmd.output().map_err(|source| ProcessError::SpawnFailed {
            cmd: render_command(request),
            source,
        })?;

        if let Some(path) = &request.stdout_log_path {
            std::fs::write(path, &output.stdout).map_err(|source| ProcessError::StdoutLogIo {
                path: path.clone(),
                source,
            })?;
        }

        if let Some(path) = &request.stderr_log_path {
            std::fs::write(path, &output.stderr).map_err(|source| ProcessError::StderrLogIo {
                path: path.clone(),
                source,
            })?;
        }

        Ok(ProcessResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            stdout_log_path: request.stdout_log_path.clone(),
            stderr_log_path: request.stderr_log_path.clone(),
        })
    }

    fn spawn(&self, request: &ProcessRequest) -> Result<SpawnResult, ProcessError> {
        let mut cmd = Command::new(&request.program);
        cmd.args(&request.args);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        if let Some(workdir) = &request.workdir {
            cmd.current_dir(workdir);
        }

        let child = cmd.spawn().map_err(|source| ProcessError::SpawnFailed {
            cmd: render_command(request),
            source,
        })?;

        Ok(SpawnResult {
            pid: child.id(),
            binary: request.program.clone(),
        })
    }
}

fn render_command(request: &ProcessRequest) -> String {
    let mut parts = Vec::with_capacity(request.args.len() + 1);
    parts.push(request.program.display().to_string());
    parts.extend(request.args.iter().cloned());
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::{ProcessError, ProcessExecutor, ProcessRequest, ProcessRunner};
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }

    #[cfg(unix)]
    fn write_script(path: &Path, body: &str) {
        fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        make_executable(path);
    }

    #[cfg(unix)]
    #[test]
    fn run_captures_output_and_mirrors_logs() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("echo.sh");
        let stdout_log = dir.path().join("stdout.log");
        let stderr_log = dir.path().join("stderr.log");
        write_script(&script, "echo hello\nprintf 'oops\\n' >&2\nexit 3");

        let runner = ProcessExecutor;
        let result = runner
            .run(&ProcessRequest {
                program: script,
                args: vec![],
                workdir: None,
                stdout_log_path: Some(stdout_log.clone()),
                stderr_log_path: Some(stderr_log.clone()),
            })
            .expect("run");

        assert_eq!(result.exit_code, 3);
        assert_eq!(result.stdout.trim(), "hello");
        assert_eq!(result.stderr.trim(), "oops");
        assert_eq!(
            fs::read_to_string(stdout_log).expect("stdout log").trim(),
            "hello"
        );
        assert_eq!(
            fs::read_to_string(stderr_log).expect("stderr log").trim(),
            "oops"
        );
    }

    #[cfg(unix)]
    #[test]
    fn spawn_returns_pid_and_binary_without_waiting() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("sleep.sh");
        write_script(&script, "sleep 0.1");

        let runner = ProcessExecutor;
        let result = runner
            .spawn(&ProcessRequest {
                program: script.clone(),
                args: vec![],
                workdir: None,
                stdout_log_path: None,
                stderr_log_path: None,
            })
            .expect("spawn");

        assert!(result.pid > 0);
        assert_eq!(result.binary, script);
    }

    #[cfg(unix)]
    #[test]
    fn run_surfaces_stdout_log_write_failures_separately() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("echo.sh");
        write_script(&script, "echo hello");

        let runner = ProcessExecutor;
        let err = runner
            .run(&ProcessRequest {
                program: script,
                args: vec![],
                workdir: None,
                stdout_log_path: Some(dir.path().join("missing").join("stdout.log")),
                stderr_log_path: None,
            })
            .expect_err("expected log write failure");

        assert!(matches!(err, ProcessError::StdoutLogIo { .. }));
    }
}
