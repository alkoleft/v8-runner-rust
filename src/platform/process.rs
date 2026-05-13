use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

const EXECUTABLE_BUSY_MAX_RETRIES: usize = 5;
const EXECUTABLE_BUSY_RETRY_DELAY: Duration = Duration::from_millis(10);

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
    /// Optional grace period used by `spawn()` to detect immediate startup failures.
    pub startup_probe: Option<Duration>,
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
    /// Command-boundary interruption observed while the child was running.
    pub interruption: Option<ProcessInterruption>,
}

/// Result of a detached `spawn()` invocation.
#[derive(Debug, Clone)]
pub struct SpawnResult {
    /// Operating system process identifier.
    pub pid: u32,
    /// Binary that was used to start the process.
    pub binary: PathBuf,
}

/// Safety class applied by the process runner when interruption arrives mid-flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessInterruptionSafety {
    Interruptible,
    GracefulThenKill,
    CriticalNonAbortable,
}

/// Normalized interruption reason shared across timeout and cancellation paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessInterruptionReason {
    Cancelled,
    TimedOut,
}

/// How the runner handled the interruption after it arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessInterruptionAction {
    Deferred,
}

/// Metadata preserved when the runner observes interruption during process execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessInterruption {
    pub reason: ProcessInterruptionReason,
    pub action: ProcessInterruptionAction,
}

/// Shared execution policy passed from transport-neutral command context into the runner.
#[derive(Debug, Clone)]
pub struct ProcessExecutionPolicy {
    pub timeout: Option<Duration>,
    pub cancellation: CancellationToken,
    pub safety: ProcessInterruptionSafety,
    pub graceful_shutdown_timeout: Duration,
}

impl Default for ProcessExecutionPolicy {
    fn default() -> Self {
        Self {
            timeout: None,
            cancellation: CancellationToken::new(),
            safety: ProcessInterruptionSafety::Interruptible,
            graceful_shutdown_timeout: Duration::from_millis(250),
        }
    }
}

impl ProcessExecutionPolicy {
    pub fn new(
        timeout: Option<Duration>,
        cancellation: CancellationToken,
        safety: ProcessInterruptionSafety,
    ) -> Self {
        Self {
            timeout,
            cancellation,
            safety,
            ..Self::default()
        }
    }
}

/// Runner-level process execution failures.
#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("failed to spawn process '{cmd}': {source}")]
    SpawnFailed { cmd: String, source: std::io::Error },

    #[error("failed to observe process startup '{cmd}': {source}")]
    StartupCheckFailed { cmd: String, source: std::io::Error },

    #[error("process exited before startup completed '{cmd}' (exit {exit_code})")]
    ExitedEarly { cmd: String, exit_code: i32 },

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

    #[error("process cancelled '{cmd}' before reaching a safe completion point")]
    Cancelled { cmd: String },

    #[error("process timed out '{cmd}' after {timeout_ms}ms")]
    TimedOut { cmd: String, timeout_ms: u64 },
}

/// Boundary for synchronous and detached process execution.
pub trait ProcessRunner {
    /// Execute a process and wait for completion, capturing stdout/stderr.
    fn run(&self, request: &ProcessRequest) -> Result<ProcessResult, ProcessError>;

    /// Execute a process with a hard timeout, terminating the process group if needed.
    fn run_with_timeout(
        &self,
        request: &ProcessRequest,
        timeout: Duration,
    ) -> Result<ProcessResult, ProcessError>;

    /// Execute a process using the shared command-boundary execution policy.
    fn run_with_policy(
        &self,
        request: &ProcessRequest,
        policy: &ProcessExecutionPolicy,
    ) -> Result<ProcessResult, ProcessError> {
        match policy.timeout {
            Some(timeout) => self.run_with_timeout(request, timeout),
            None => self.run(request),
        }
    }

    /// Start a process in fire-and-forget mode without waiting for completion.
    fn spawn(&self, request: &ProcessRequest) -> Result<SpawnResult, ProcessError>;
}

/// Standard subprocess runner backed by `std::process::Command`.
pub struct ProcessExecutor;

impl ProcessRunner for ProcessExecutor {
    fn run(&self, request: &ProcessRequest) -> Result<ProcessResult, ProcessError> {
        self.run_internal(request, &ProcessExecutionPolicy::default())
    }

    fn run_with_timeout(
        &self,
        request: &ProcessRequest,
        timeout: Duration,
    ) -> Result<ProcessResult, ProcessError> {
        self.run_internal(
            request,
            &ProcessExecutionPolicy::new(
                Some(timeout),
                CancellationToken::new(),
                ProcessInterruptionSafety::Interruptible,
            ),
        )
    }

    fn run_with_policy(
        &self,
        request: &ProcessRequest,
        policy: &ProcessExecutionPolicy,
    ) -> Result<ProcessResult, ProcessError> {
        self.run_internal(request, policy)
    }

    fn spawn(&self, request: &ProcessRequest) -> Result<SpawnResult, ProcessError> {
        let rendered_command = render_command(request);
        debug!(command = rendered_command.as_str(), "spawning process");
        let child = spawn_command(request, ProcessIoMode::Detached, &rendered_command)?;
        let pid = child.id();
        let mut child = child;

        if let Some(startup_probe) = request.startup_probe {
            std::thread::sleep(startup_probe);
            if let Some(status) =
                child
                    .try_wait()
                    .map_err(|source| ProcessError::StartupCheckFailed {
                        cmd: rendered_command.clone(),
                        source,
                    })?
            {
                warn!(
                    command = rendered_command.as_str(),
                    exit_code = status.code().unwrap_or(-1),
                    "process exited during startup probe"
                );
                return Err(ProcessError::ExitedEarly {
                    cmd: rendered_command,
                    exit_code: status.code().unwrap_or(-1),
                });
            }
        }

        debug!(command = rendered_command.as_str(), pid, "process started");
        Ok(SpawnResult {
            pid,
            binary: request.program.clone(),
        })
    }
}

impl ProcessExecutor {
    fn run_internal(
        &self,
        request: &ProcessRequest,
        policy: &ProcessExecutionPolicy,
    ) -> Result<ProcessResult, ProcessError> {
        let rendered_command = render_command(request);
        debug!(
            command = rendered_command.as_str(),
            timeout_ms = policy.timeout.map(|value| value.as_millis() as u64),
            safety = ?policy.safety,
            "running process"
        );
        if policy.cancellation.is_cancelled() {
            return Err(ProcessError::Cancelled {
                cmd: rendered_command,
            });
        }
        if policy.timeout.is_some_and(|timeout| timeout.is_zero()) {
            return Err(ProcessError::TimedOut {
                cmd: rendered_command,
                timeout_ms: 0,
            });
        }
        let child = spawn_command(request, ProcessIoMode::Captured, &rendered_command)?;
        let output = wait_for_output(child, &rendered_command, policy)?;
        debug!(
            command = rendered_command.as_str(),
            exit_code = output.status.code().unwrap_or(-1),
            stdout_bytes = output.stdout.len(),
            stderr_bytes = output.stderr.len(),
            "process finished"
        );

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
            interruption: output.interruption,
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum ProcessIoMode {
    Detached,
    Captured,
}

fn spawn_command(
    request: &ProcessRequest,
    io_mode: ProcessIoMode,
    rendered_command: &str,
) -> Result<std::process::Child, ProcessError> {
    for attempt in 0..=EXECUTABLE_BUSY_MAX_RETRIES {
        let mut cmd = build_command(request, io_mode);
        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(source) if is_executable_busy(&source) && attempt < EXECUTABLE_BUSY_MAX_RETRIES => {
                warn!(
                    command = rendered_command,
                    attempt = attempt + 1,
                    max_retries = EXECUTABLE_BUSY_MAX_RETRIES,
                    delay_ms = EXECUTABLE_BUSY_RETRY_DELAY.as_millis() as u64,
                    "spawn hit executable-busy race, retrying"
                );
                std::thread::sleep(EXECUTABLE_BUSY_RETRY_DELAY);
            }
            Err(source) => {
                return Err(ProcessError::SpawnFailed {
                    cmd: rendered_command.to_owned(),
                    source,
                });
            }
        }
    }

    unreachable!("spawn loop must return on success or final error");
}

fn build_command(request: &ProcessRequest, io_mode: ProcessIoMode) -> Command {
    let mut cmd = Command::new(&request.program);
    cmd.args(&request.args);
    if let Some(workdir) = &request.workdir {
        cmd.current_dir(workdir);
    }
    cmd.stdin(Stdio::null());
    match io_mode {
        ProcessIoMode::Detached => {
            cmd.stdout(Stdio::null());
            cmd.stderr(Stdio::null());
        }
        ProcessIoMode::Captured => {
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                unsafe {
                    cmd.pre_exec(|| {
                        if libc::setpgid(0, 0) != 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
            }
        }
    }
    cmd
}

fn is_executable_busy(error: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        matches!(error.raw_os_error(), Some(libc::ETXTBSY))
            || error.kind() == std::io::ErrorKind::ExecutableFileBusy
    }

    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

fn wait_for_output(
    mut child: std::process::Child,
    rendered_command: &str,
    policy: &ProcessExecutionPolicy,
) -> Result<ObservedOutput, ProcessError> {
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProcessError::StartupCheckFailed {
            cmd: rendered_command.to_owned(),
            source: std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stdout pipe missing"),
        })?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| ProcessError::StartupCheckFailed {
            cmd: rendered_command.to_owned(),
            source: std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stderr pipe missing"),
        })?;
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let start = std::time::Instant::now();
    let mut observed_interruption: Option<ProcessInterruptionReason> = None;
    loop {
        if let Some(status) =
            child
                .try_wait()
                .map_err(|source| ProcessError::StartupCheckFailed {
                    cmd: rendered_command.to_owned(),
                    source,
                })?
        {
            let stdout = stdout_reader.join().unwrap_or_default();
            let stderr = stderr_reader.join().unwrap_or_default();
            return match observed_interruption {
                Some(ProcessInterruptionReason::Cancelled)
                    if policy.safety != ProcessInterruptionSafety::CriticalNonAbortable =>
                {
                    Err(ProcessError::Cancelled {
                        cmd: rendered_command.to_owned(),
                    })
                }
                Some(ProcessInterruptionReason::TimedOut)
                    if policy.safety != ProcessInterruptionSafety::CriticalNonAbortable =>
                {
                    Err(ProcessError::TimedOut {
                        cmd: rendered_command.to_owned(),
                        timeout_ms: policy.timeout.unwrap_or_default().as_millis() as u64,
                    })
                }
                Some(reason) => Ok(ObservedOutput {
                    status,
                    stdout,
                    stderr,
                    interruption: Some(ProcessInterruption {
                        reason,
                        action: ProcessInterruptionAction::Deferred,
                    }),
                }),
                None => Ok(ObservedOutput {
                    status,
                    stdout,
                    stderr,
                    interruption: None,
                }),
            };
        }

        if observed_interruption.is_none() {
            if policy.cancellation.is_cancelled() {
                observed_interruption = Some(ProcessInterruptionReason::Cancelled);
                if let Some(error) = interrupt_child(
                    &mut child,
                    rendered_command,
                    policy,
                    ProcessInterruptionReason::Cancelled,
                )? {
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(error);
                }
            } else if let Some(limit) = policy.timeout {
                if start.elapsed() >= limit {
                    observed_interruption = Some(ProcessInterruptionReason::TimedOut);
                    if let Some(error) = interrupt_child(
                        &mut child,
                        rendered_command,
                        policy,
                        ProcessInterruptionReason::TimedOut,
                    )? {
                        let _ = stdout_reader.join();
                        let _ = stderr_reader.join();
                        return Err(error);
                    }
                }
            }
        }

        std::thread::sleep(Duration::from_millis(10));
    }
}

struct ObservedOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    interruption: Option<ProcessInterruption>,
}

fn interrupt_child(
    child: &mut std::process::Child,
    rendered_command: &str,
    policy: &ProcessExecutionPolicy,
    reason: ProcessInterruptionReason,
) -> Result<Option<ProcessError>, ProcessError> {
    match policy.safety {
        ProcessInterruptionSafety::CriticalNonAbortable => {
            warn!(
                command = rendered_command,
                reason = ?reason,
                "interruption requested during critical process phase; waiting for terminal outcome"
            );
            Ok(None)
        }
        ProcessInterruptionSafety::Interruptible => {
            terminate_child_group(child);
            let _ = child.wait();
            Ok(Some(process_error_from_reason(
                rendered_command,
                policy.timeout,
                reason,
            )))
        }
        ProcessInterruptionSafety::GracefulThenKill => {
            terminate_child_group_gracefully(child, policy.graceful_shutdown_timeout);
            let _ = child.wait();
            Ok(Some(process_error_from_reason(
                rendered_command,
                policy.timeout,
                reason,
            )))
        }
    }
}

fn process_error_from_reason(
    rendered_command: &str,
    timeout: Option<Duration>,
    reason: ProcessInterruptionReason,
) -> ProcessError {
    match reason {
        ProcessInterruptionReason::Cancelled => ProcessError::Cancelled {
            cmd: rendered_command.to_owned(),
        },
        ProcessInterruptionReason::TimedOut => ProcessError::TimedOut {
            cmd: rendered_command.to_owned(),
            timeout_ms: timeout.unwrap_or_default().as_millis() as u64,
        },
    }
}

fn terminate_child_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        let pgid = -(child.id() as i32);
        let _ = libc::kill(pgid, libc::SIGKILL);
    }

    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

fn terminate_child_group_gracefully(child: &mut std::process::Child, timeout: Duration) {
    #[cfg(unix)]
    unsafe {
        let pgid = -(child.id() as i32);
        let _ = libc::kill(pgid, libc::SIGTERM);
    }

    #[cfg(not(unix))]
    {
        let _ = child.kill();
        return;
    }

    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => break,
        }
    }
    terminate_child_group(child);
}

fn render_command(request: &ProcessRequest) -> String {
    let mut parts = Vec::with_capacity(request.args.len() + 1);
    parts.push(request.program.display().to_string());
    let mut skip_next = false;
    for arg in &request.args {
        if skip_next {
            parts.push("***".to_owned());
            skip_next = false;
        } else if is_sensitive_flag(arg) {
            parts.push(arg.clone());
            skip_next = true;
        } else if let Some((key, _)) = split_sensitive_assignment(arg) {
            parts.push(format!("{key}=***"));
        } else {
            parts.push(arg.clone());
        }
    }
    parts.join(" ")
}

fn is_sensitive_flag(arg: &str) -> bool {
    const FLAGS: &[&str] = &[
        "/P",
        "-P",
        "--password",
        "--database-password",
        "--db-pwd",
        "--target-database-password",
        "--target-db-pwd",
        // `/UC` carries the infobase unlock code (TASK-124) — treat it as a secret.
        "/UC",
        "-UC",
    ];

    FLAGS.iter().any(|flag| arg.eq_ignore_ascii_case(flag))
}

fn split_sensitive_assignment(arg: &str) -> Option<(&str, &str)> {
    const FLAGS: &[&str] = &[
        "/P",
        "-P",
        "--password",
        "--database-password",
        "--db-pwd",
        "--target-database-password",
        "--target-db-pwd",
        // `/UC` carries the infobase unlock code (TASK-124) — treat it as a secret.
        "/UC",
        "-UC",
    ];

    let (key, value) = arg.split_once('=')?;
    if FLAGS.iter().any(|flag| key.eq_ignore_ascii_case(flag)) {
        Some((key, value))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        render_command, ProcessError, ProcessExecutionPolicy, ProcessExecutor,
        ProcessInterruptionAction, ProcessInterruptionReason, ProcessInterruptionSafety,
        ProcessRequest, ProcessRunner,
    };
    use std::fs;
    use std::path::Path;
    use std::thread;
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }

    #[cfg(unix)]
    fn write_script(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create dirs");
        }
        let staged = path.with_extension("tmp");
        fs::write(&staged, format!("#!/bin/sh\n{body}\n")).expect("write script");
        make_executable(&staged);
        fs::rename(&staged, path).expect("rename script");
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
                startup_probe: None,
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

    #[test]
    fn render_command_masks_unlock_code_flag() {
        let rendered = render_command(&ProcessRequest {
            program: Path::new("/tmp/1cv8").to_path_buf(),
            args: vec![
                "DESIGNER".to_owned(),
                "/UC".to_owned(),
                "seal-42".to_owned(),
                "/UpdateDBCfg".to_owned(),
            ],
            workdir: None,
            stdout_log_path: None,
            stderr_log_path: None,
            startup_probe: None,
        });

        assert!(rendered.contains("/UC ***"));
        assert!(!rendered.contains("seal-42"));
    }

    #[test]
    fn render_command_masks_ibcmd_password_flags() {
        let rendered = render_command(&ProcessRequest {
            program: Path::new("/tmp/ibcmd").to_path_buf(),
            args: vec![
                "--user".to_owned(),
                "admin".to_owned(),
                "/p".to_owned(),
                "secret".to_owned(),
                "--DATABASE-password=pg-secret".to_owned(),
                "-p=legacy-secret".to_owned(),
                "--target-db-pwd".to_owned(),
                "target-secret".to_owned(),
            ],
            workdir: None,
            stdout_log_path: None,
            stderr_log_path: None,
            startup_probe: None,
        });

        assert!(rendered.contains("/p ***"));
        assert!(rendered.contains("--DATABASE-password=***"));
        assert!(rendered.contains("-p=***"));
        assert!(rendered.contains("--target-db-pwd ***"));
        assert!(!rendered.contains("secret"));
        assert!(!rendered.contains("pg-secret"));
        assert!(!rendered.contains("legacy-secret"));
        assert!(!rendered.contains("target-secret"));
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
                startup_probe: None,
            })
            .expect("spawn");

        assert!(result.pid > 0);
        assert_eq!(result.binary, script);
    }

    #[cfg(unix)]
    #[test]
    fn spawn_detects_immediate_exit_when_probe_is_requested() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("exit.sh");
        write_script(&script, "exit 7");

        let runner = ProcessExecutor;
        let err = runner
            .spawn(&ProcessRequest {
                program: script,
                args: vec![],
                workdir: None,
                stdout_log_path: None,
                stderr_log_path: None,
                startup_probe: Some(Duration::from_millis(50)),
            })
            .expect_err("expected early exit");

        assert!(matches!(
            err,
            ProcessError::ExitedEarly { exit_code: 7, .. }
        ));
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
                startup_probe: None,
            })
            .expect_err("expected log write failure");

        assert!(matches!(err, ProcessError::StdoutLogIo { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn run_with_timeout_returns_timeout_error() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("sleep.sh");
        write_script(&script, "sleep 2");

        let runner = ProcessExecutor;
        let err = runner
            .run_with_timeout(
                &ProcessRequest {
                    program: script,
                    args: vec![],
                    workdir: None,
                    stdout_log_path: None,
                    stderr_log_path: None,
                    startup_probe: None,
                },
                Duration::from_millis(100),
            )
            .expect_err("expected timeout");

        assert!(matches!(err, ProcessError::TimedOut { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn run_with_policy_cancels_interruptible_process() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("sleep.sh");
        write_script(&script, "sleep 2");
        let cancellation = CancellationToken::new();
        let cancellation_clone = cancellation.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            cancellation_clone.cancel();
        });

        let runner = ProcessExecutor;
        let err = runner
            .run_with_policy(
                &ProcessRequest {
                    program: script,
                    args: vec![],
                    workdir: None,
                    stdout_log_path: None,
                    stderr_log_path: None,
                    startup_probe: None,
                },
                &ProcessExecutionPolicy::new(
                    None,
                    cancellation,
                    ProcessInterruptionSafety::Interruptible,
                ),
            )
            .expect_err("expected cancellation");

        assert!(matches!(err, ProcessError::Cancelled { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn run_with_policy_defers_timeout_for_critical_process() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("sleep.sh");
        write_script(&script, "sleep 0.1\nprintf 'done\\n'");

        let runner = ProcessExecutor;
        let result = runner
            .run_with_policy(
                &ProcessRequest {
                    program: script,
                    args: vec![],
                    workdir: None,
                    stdout_log_path: None,
                    stderr_log_path: None,
                    startup_probe: None,
                },
                &ProcessExecutionPolicy::new(
                    Some(Duration::from_millis(10)),
                    CancellationToken::new(),
                    ProcessInterruptionSafety::CriticalNonAbortable,
                ),
            )
            .expect("critical process must reach terminal success");

        assert_eq!(result.exit_code, 0);
        assert_eq!(
            result.interruption,
            Some(super::ProcessInterruption {
                reason: ProcessInterruptionReason::TimedOut,
                action: ProcessInterruptionAction::Deferred,
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_handles_large_stdout_without_deadlock() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("large.sh");
        write_script(
            &script,
            "i=0\nwhile [ \"$i\" -lt 20000 ]; do\n  printf 'line%05d\\n' \"$i\"\n  i=$((i+1))\ndone\nexit 0",
        );

        let runner = ProcessExecutor;
        let result = runner
            .run(&ProcessRequest {
                program: script,
                args: vec![],
                workdir: None,
                stdout_log_path: None,
                stderr_log_path: None,
                startup_probe: None,
            })
            .expect("run");

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("line19999"));
    }
}
