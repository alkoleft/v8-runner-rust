use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("failed to spawn process '{cmd}': {source}")]
    SpawnFailed { cmd: String, source: std::io::Error },

    #[error("process '{cmd}' failed with exit code {code}")]
    NonZeroExit { cmd: String, code: i32 },

    #[error("I/O error writing log: {0}")]
    LogIo(#[from] std::io::Error),
}

/// Result of a completed process execution.
#[derive(Debug)]
pub struct ProcessResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    /// Path to the stdout log file, if log capture was requested.
    pub stdout_log: Option<PathBuf>,
    /// Path to the stderr log file, if log capture was requested.
    pub stderr_log: Option<PathBuf>,
}

impl ProcessResult {
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Options for process execution.
#[derive(Debug, Default)]
pub struct RunOptions<'a> {
    /// Working directory for the child process.
    pub workdir: Option<&'a Path>,
    /// If set, stdout is written to this file in addition to being captured.
    pub stdout_log: Option<&'a Path>,
    /// If set, stderr is written to this file in addition to being captured.
    pub stderr_log: Option<&'a Path>,
}

/// Executes external processes and captures their output.
pub struct ProcessExecutor;

impl ProcessExecutor {
    /// Run `program` with `args`, optionally writing stdout/stderr to log files.
    pub fn run(
        program: &Path,
        args: &[&str],
        opts: RunOptions<'_>,
    ) -> Result<ProcessResult, ProcessError> {
        let mut cmd = Command::new(program);
        cmd.args(args);
        if let Some(wd) = opts.workdir {
            cmd.current_dir(wd);
        }

        let output: Output = cmd.output().map_err(|e| ProcessError::SpawnFailed {
            cmd: program.display().to_string(),
            source: e,
        })?;

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

        let stdout_log = if let Some(path) = opts.stdout_log {
            std::fs::write(path, &output.stdout)?;
            Some(path.to_path_buf())
        } else {
            None
        };

        let stderr_log = if let Some(path) = opts.stderr_log {
            std::fs::write(path, &output.stderr)?;
            Some(path.to_path_buf())
        } else {
            None
        };

        Ok(ProcessResult {
            exit_code,
            stdout,
            stderr,
            stdout_log,
            stderr_log,
        })
    }

    /// Convenience: run without log capture.
    pub fn run_simple(
        program: &Path,
        args: &[&str],
        workdir: Option<&Path>,
    ) -> Result<ProcessResult, ProcessError> {
        Self::run(program, args, RunOptions { workdir, ..Default::default() })
    }
}
