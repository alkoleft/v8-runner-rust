use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;
use tracing::warn;

use crate::platform::process::{
    ProcessExecutionPolicy, ProcessInterruption, ProcessInterruptionAction,
    ProcessInterruptionReason, ProcessInterruptionSafety,
};

const DEFAULT_PROMPT: &[u8] = b"1C:EDT>";
const EXECUTABLE_BUSY_MAX_RETRIES: usize = 5;
const EXECUTABLE_BUSY_RETRY_DELAY: Duration = Duration::from_millis(10);
const IO_POLL_INTERVAL: Duration = Duration::from_millis(10);
const PROMPT_DRAIN_GRACE: Duration = Duration::from_millis(20);
const STREAM_BUFFER_SIZE: usize = 1024;

/// Request describing how to start an interactive child process.
#[derive(Debug, Clone)]
pub struct InteractiveProcessRequest {
    /// Executable path.
    pub program: PathBuf,
    /// Command-line arguments passed to the executable.
    pub args: Vec<String>,
    /// Optional working directory for the child process.
    pub workdir: Option<PathBuf>,
    /// Prompt token that delimits command completion.
    pub prompt: Vec<u8>,
}

impl InteractiveProcessRequest {
    /// Creates a request for the provided executable using the default EDT prompt.
    pub fn new(program: PathBuf) -> Self {
        Self {
            program,
            args: Vec::new(),
            workdir: None,
            prompt: DEFAULT_PROMPT.to_vec(),
        }
    }

    /// Replaces the argument vector.
    pub fn with_args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }
}

/// Output captured for one prompt-delimited interactive command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractiveCommandOutput {
    /// Captured stdout without the trailing prompt token.
    pub stdout: String,
    /// Captured stderr without the trailing prompt token.
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InteractiveCommandExecution {
    pub output: InteractiveCommandOutput,
    pub interruption: Option<ProcessInterruption>,
}

/// Stream that emitted a prompt or transport event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractiveStream {
    /// Child stdout.
    Stdout,
    /// Child stderr.
    Stderr,
}

impl InteractiveStream {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

/// Result of shutting the interactive process down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownOutcome {
    /// The child exited after stdin was closed.
    Graceful { exit_code: i32 },
    /// The child ignored the graceful shutdown window and was killed.
    ForcedKill,
}

/// Failures surfaced by the interactive process executor.
#[derive(Debug, Error)]
pub enum InteractiveProcessError {
    #[error("failed to spawn interactive process '{cmd}': {source}")]
    SpawnFailed { cmd: String, source: std::io::Error },

    #[error("interactive process '{cmd}' is missing stdin")]
    MissingStdin { cmd: String },

    #[error("interactive process '{cmd}' is missing stdout")]
    MissingStdout { cmd: String },

    #[error("interactive process '{cmd}' is missing stderr")]
    MissingStderr { cmd: String },

    #[error("interactive process startup timed out after {timeout_ms}ms while waiting for prompt")]
    StartupTimeout {
        timeout_ms: u64,
        stdout: String,
        stderr: String,
    },

    #[error(
        "interactive command '{command}' timed out after {timeout_ms}ms while waiting for prompt"
    )]
    CommandTimeout {
        command: String,
        timeout_ms: u64,
        stdout: String,
        stderr: String,
    },

    #[error(
        "interactive command '{command}' was cancelled before reaching a safe completion point"
    )]
    CommandCancelled {
        command: String,
        stdout: String,
        stderr: String,
    },

    #[error("interactive process exited before the next prompt (exit {exit_code})")]
    ProcessExited {
        exit_code: i32,
        stdout: String,
        stderr: String,
    },

    #[error("interactive process is no longer usable after a previous failure")]
    Poisoned,

    #[error("interactive process has already been terminated")]
    Terminated,

    #[error("failed to write interactive command '{command}': {source}")]
    StdinWriteFailed {
        command: String,
        source: std::io::Error,
    },

    #[error("failed to flush interactive command '{command}': {source}")]
    StdinFlushFailed {
        command: String,
        source: std::io::Error,
    },

    #[error("failed to read interactive {stream}: {message}")]
    StreamReadFailed {
        stream: &'static str,
        message: String,
    },

    #[error("failed to observe interactive process shutdown: {source}")]
    WaitFailed { source: std::io::Error },

    #[error("failed to terminate interactive process: {source}")]
    KillFailed { source: std::io::Error },
}

/// Low-level prompt-delimited interactive process executor.
#[derive(Debug)]
pub struct InteractiveProcessExecutor {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    events: Receiver<ReaderEvent>,
    stdout_pending: Vec<u8>,
    stderr_pending: Vec<u8>,
    prompt: Vec<u8>,
    poisoned: bool,
    terminated: bool,
}

impl InteractiveProcessExecutor {
    /// Starts a new interactive process and waits until its prompt becomes ready.
    pub fn spawn(
        request: InteractiveProcessRequest,
        startup_timeout: Duration,
    ) -> Result<Self, InteractiveProcessError> {
        let rendered_command = render_command(&request);
        let mut child = spawn_command(&request, &rendered_command)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| InteractiveProcessError::MissingStdin {
                cmd: rendered_command.clone(),
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| InteractiveProcessError::MissingStdout {
                cmd: rendered_command.clone(),
            })?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| InteractiveProcessError::MissingStderr {
                cmd: rendered_command,
            })?;

        let (sender, receiver) = mpsc::channel();
        spawn_reader(stdout, InteractiveStream::Stdout, sender.clone());
        spawn_reader(stderr, InteractiveStream::Stderr, sender);

        let mut executor = Self {
            child: Some(child),
            stdin: Some(stdin),
            events: receiver,
            stdout_pending: Vec::new(),
            stderr_pending: Vec::new(),
            prompt: request.prompt,
            poisoned: false,
            terminated: false,
        };

        if executor.prompt.is_empty() {
            executor.prompt = DEFAULT_PROMPT.to_vec();
        }

        if let Err(error) = executor.wait_for_prompt(WaitMode::Startup, startup_timeout) {
            let _ = executor.kill_internal();
            return Err(error);
        }

        Ok(executor)
    }

    /// Returns the child process identifier while the executor is live.
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Runs one interactive command and waits for the next prompt.
    pub fn execute(
        &mut self,
        command: &str,
        timeout: Duration,
    ) -> Result<InteractiveCommandOutput, InteractiveProcessError> {
        if self.poisoned {
            return Err(InteractiveProcessError::Poisoned);
        }
        if self.terminated || self.child.is_none() {
            return Err(InteractiveProcessError::Terminated);
        }

        let stdin = self
            .stdin
            .as_mut()
            .ok_or(InteractiveProcessError::Terminated)?;
        stdin.write_all(command.as_bytes()).map_err(|source| {
            InteractiveProcessError::StdinWriteFailed {
                command: command.to_owned(),
                source,
            }
        })?;
        stdin
            .write_all(b"\n")
            .map_err(|source| InteractiveProcessError::StdinWriteFailed {
                command: command.to_owned(),
                source,
            })?;
        stdin
            .flush()
            .map_err(|source| InteractiveProcessError::StdinFlushFailed {
                command: command.to_owned(),
                source,
            })?;

        self.wait_for_prompt(
            WaitMode::Command {
                command: command.to_owned(),
            },
            timeout,
        )
    }

    pub(crate) fn execute_with_policy(
        &mut self,
        command: &str,
        timeout: Duration,
        policy: &ProcessExecutionPolicy,
    ) -> Result<InteractiveCommandExecution, InteractiveProcessError> {
        if self.poisoned {
            return Err(InteractiveProcessError::Poisoned);
        }
        if self.terminated || self.child.is_none() {
            return Err(InteractiveProcessError::Terminated);
        }
        if policy.cancellation.is_cancelled() {
            return Err(InteractiveProcessError::CommandCancelled {
                command: command.to_owned(),
                stdout: String::new(),
                stderr: String::new(),
            });
        }
        if timeout.is_zero() {
            return Err(InteractiveProcessError::CommandTimeout {
                command: command.to_owned(),
                timeout_ms: 0,
                stdout: String::new(),
                stderr: String::new(),
            });
        }

        let stdin = self
            .stdin
            .as_mut()
            .ok_or(InteractiveProcessError::Terminated)?;
        stdin.write_all(command.as_bytes()).map_err(|source| {
            InteractiveProcessError::StdinWriteFailed {
                command: command.to_owned(),
                source,
            }
        })?;
        stdin
            .write_all(b"\n")
            .map_err(|source| InteractiveProcessError::StdinWriteFailed {
                command: command.to_owned(),
                source,
            })?;
        stdin
            .flush()
            .map_err(|source| InteractiveProcessError::StdinFlushFailed {
                command: command.to_owned(),
                source,
            })?;

        self.wait_for_prompt_with_policy(
            WaitMode::Command {
                command: command.to_owned(),
            },
            timeout,
            policy,
        )
    }

    /// Closes stdin and waits for the child to exit. Escalates to a forced kill on timeout.
    pub fn shutdown(
        &mut self,
        timeout: Duration,
    ) -> Result<ShutdownOutcome, InteractiveProcessError> {
        if self.terminated || self.child.is_none() {
            return Err(InteractiveProcessError::Terminated);
        }

        let _ = self.stdin.take();
        let deadline = Instant::now() + timeout;
        loop {
            let child = self
                .child
                .as_mut()
                .ok_or(InteractiveProcessError::Terminated)?;
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.child = None;
                    self.terminated = true;
                    self.poisoned = false;
                    return Ok(ShutdownOutcome::Graceful {
                        exit_code: status.code().unwrap_or(-1),
                    });
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        self.kill_internal()?;
                        return Ok(ShutdownOutcome::ForcedKill);
                    }
                    thread::sleep(IO_POLL_INTERVAL);
                }
                Err(source) => return Err(InteractiveProcessError::WaitFailed { source }),
            }
        }
    }

    /// Forces the entire child process group to terminate.
    pub fn kill(&mut self) -> Result<(), InteractiveProcessError> {
        if self.terminated || self.child.is_none() {
            return Ok(());
        }
        self.kill_internal()
    }

    fn wait_for_prompt(
        &mut self,
        mode: WaitMode,
        timeout: Duration,
    ) -> Result<InteractiveCommandOutput, InteractiveProcessError> {
        let deadline = Instant::now() + timeout;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        loop {
            let prompt_seen =
                if consume_until_prompt(&mut self.stdout_pending, &mut stdout, &self.prompt)
                    .is_some()
                {
                    true
                } else {
                    consume_until_prompt(&mut self.stderr_pending, &mut stderr, &self.prompt)
                        .is_some()
                };
            if prompt_seen {
                return self.finish_prompt_wait(&mut stdout, &mut stderr);
            }

            flush_completed_bytes(&mut self.stdout_pending, &mut stdout, self.prompt.len());
            flush_completed_bytes(&mut self.stderr_pending, &mut stderr, self.prompt.len());

            if Instant::now() >= deadline {
                flush_pending(&mut self.stdout_pending, &mut stdout);
                flush_pending(&mut self.stderr_pending, &mut stderr);
                let error = match &mode {
                    WaitMode::Startup => InteractiveProcessError::StartupTimeout {
                        timeout_ms: timeout.as_millis() as u64,
                        stdout: String::from_utf8_lossy(&stdout).into_owned(),
                        stderr: String::from_utf8_lossy(&stderr).into_owned(),
                    },
                    WaitMode::Command { command } => InteractiveProcessError::CommandTimeout {
                        command: command.clone(),
                        timeout_ms: timeout.as_millis() as u64,
                        stdout: String::from_utf8_lossy(&stdout).into_owned(),
                        stderr: String::from_utf8_lossy(&stderr).into_owned(),
                    },
                };
                self.kill_internal()?;
                self.poisoned = true;
                return Err(error);
            }

            if let Some(status) = self.try_wait_child()? {
                if let Err(error) = self.drain_ready_events(&mut stdout, &mut stderr) {
                    let _ = self.kill_internal();
                    self.poisoned = true;
                    return Err(error);
                }
                let prompt_seen =
                    if consume_until_prompt(&mut self.stdout_pending, &mut stdout, &self.prompt)
                        .is_some()
                    {
                        true
                    } else {
                        consume_until_prompt(&mut self.stderr_pending, &mut stderr, &self.prompt)
                            .is_some()
                    };
                if prompt_seen {
                    self.child = None;
                    self.stdin = None;
                    self.terminated = true;
                    self.poisoned = true;
                    flush_pending(&mut self.stdout_pending, &mut stdout);
                    flush_pending(&mut self.stderr_pending, &mut stderr);
                    return Err(InteractiveProcessError::ProcessExited {
                        exit_code: status.code().unwrap_or(-1),
                        stdout: String::from_utf8_lossy(&stdout).into_owned(),
                        stderr: String::from_utf8_lossy(&stderr).into_owned(),
                    });
                }

                flush_pending(&mut self.stdout_pending, &mut stdout);
                flush_pending(&mut self.stderr_pending, &mut stderr);
                self.child = None;
                self.stdin = None;
                self.terminated = true;
                self.poisoned = true;
                return Err(InteractiveProcessError::ProcessExited {
                    exit_code: status.code().unwrap_or(-1),
                    stdout: String::from_utf8_lossy(&stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr).into_owned(),
                });
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            let wait = remaining.min(IO_POLL_INTERVAL);
            match self.events.recv_timeout(wait) {
                Ok(event) => {
                    if let Err(error) = self.apply_event(event, &mut stdout, &mut stderr) {
                        let _ = self.kill_internal();
                        self.poisoned = true;
                        return Err(error);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    flush_pending(&mut self.stdout_pending, &mut stdout);
                    flush_pending(&mut self.stderr_pending, &mut stderr);
                    self.poisoned = true;
                    let _ = self.kill_internal();
                    return Err(InteractiveProcessError::ProcessExited {
                        exit_code: -1,
                        stdout: String::from_utf8_lossy(&stdout).into_owned(),
                        stderr: String::from_utf8_lossy(&stderr).into_owned(),
                    });
                }
            }
        }
    }

    fn wait_for_prompt_with_policy(
        &mut self,
        mode: WaitMode,
        timeout: Duration,
        policy: &ProcessExecutionPolicy,
    ) -> Result<InteractiveCommandExecution, InteractiveProcessError> {
        let deadline = Instant::now() + timeout;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut observed_interruption = None;

        loop {
            let prompt_seen =
                if consume_until_prompt(&mut self.stdout_pending, &mut stdout, &self.prompt)
                    .is_some()
                {
                    true
                } else {
                    consume_until_prompt(&mut self.stderr_pending, &mut stderr, &self.prompt)
                        .is_some()
                };
            if prompt_seen {
                let output = self.finish_prompt_wait(&mut stdout, &mut stderr)?;
                return Ok(InteractiveCommandExecution {
                    output,
                    interruption: observed_interruption.map(|reason| ProcessInterruption {
                        reason,
                        action: ProcessInterruptionAction::Deferred,
                    }),
                });
            }

            flush_completed_bytes(&mut self.stdout_pending, &mut stdout, self.prompt.len());
            flush_completed_bytes(&mut self.stderr_pending, &mut stderr, self.prompt.len());

            if let Some(status) = self.try_wait_child()? {
                if let Err(error) = self.drain_ready_events(&mut stdout, &mut stderr) {
                    let _ = self.kill_internal();
                    self.poisoned = true;
                    return Err(error);
                }
                let prompt_seen =
                    if consume_until_prompt(&mut self.stdout_pending, &mut stdout, &self.prompt)
                        .is_some()
                    {
                        true
                    } else {
                        consume_until_prompt(&mut self.stderr_pending, &mut stderr, &self.prompt)
                            .is_some()
                    };
                if prompt_seen {
                    self.child = None;
                    self.stdin = None;
                    self.terminated = true;
                    self.poisoned = true;
                    flush_pending(&mut self.stdout_pending, &mut stdout);
                    flush_pending(&mut self.stderr_pending, &mut stderr);
                    return Err(InteractiveProcessError::ProcessExited {
                        exit_code: status.code().unwrap_or(-1),
                        stdout: String::from_utf8_lossy(&stdout).into_owned(),
                        stderr: String::from_utf8_lossy(&stderr).into_owned(),
                    });
                }

                flush_pending(&mut self.stdout_pending, &mut stdout);
                flush_pending(&mut self.stderr_pending, &mut stderr);
                self.child = None;
                self.stdin = None;
                self.terminated = true;
                self.poisoned = true;
                return Err(InteractiveProcessError::ProcessExited {
                    exit_code: status.code().unwrap_or(-1),
                    stdout: String::from_utf8_lossy(&stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr).into_owned(),
                });
            }

            if observed_interruption.is_none() {
                if policy.cancellation.is_cancelled() {
                    observed_interruption = Some(ProcessInterruptionReason::Cancelled);
                    if let Some(error) = self.interrupt_with_policy(
                        &mode,
                        timeout,
                        policy,
                        ProcessInterruptionReason::Cancelled,
                        &mut stdout,
                        &mut stderr,
                    )? {
                        return Err(error);
                    }
                } else if Instant::now() >= deadline {
                    observed_interruption = Some(ProcessInterruptionReason::TimedOut);
                    if let Some(error) = self.interrupt_with_policy(
                        &mode,
                        timeout,
                        policy,
                        ProcessInterruptionReason::TimedOut,
                        &mut stdout,
                        &mut stderr,
                    )? {
                        return Err(error);
                    }
                }
            }

            let wait = if observed_interruption.is_some() {
                IO_POLL_INTERVAL
            } else {
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(IO_POLL_INTERVAL)
            };
            match self.events.recv_timeout(wait) {
                Ok(event) => {
                    if let Err(error) = self.apply_event(event, &mut stdout, &mut stderr) {
                        let _ = self.kill_internal();
                        self.poisoned = true;
                        return Err(error);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    flush_pending(&mut self.stdout_pending, &mut stdout);
                    flush_pending(&mut self.stderr_pending, &mut stderr);
                    self.poisoned = true;
                    let _ = self.kill_internal();
                    return Err(InteractiveProcessError::ProcessExited {
                        exit_code: -1,
                        stdout: String::from_utf8_lossy(&stdout).into_owned(),
                        stderr: String::from_utf8_lossy(&stderr).into_owned(),
                    });
                }
            }
        }
    }

    fn apply_event(
        &mut self,
        event: ReaderEvent,
        _stdout: &mut Vec<u8>,
        _stderr: &mut Vec<u8>,
    ) -> Result<(), InteractiveProcessError> {
        match event {
            ReaderEvent::Chunk(InteractiveStream::Stdout, bytes) => {
                self.stdout_pending.extend(bytes);
                Ok(())
            }
            ReaderEvent::Chunk(InteractiveStream::Stderr, bytes) => {
                self.stderr_pending.extend(bytes);
                Ok(())
            }
            ReaderEvent::Closed => Ok(()),
            ReaderEvent::ReadError(stream, message) => {
                self.poisoned = true;
                Err(InteractiveProcessError::StreamReadFailed {
                    stream: stream.as_str(),
                    message,
                })
            }
        }
    }

    fn drain_ready_events(
        &mut self,
        stdout: &mut Vec<u8>,
        stderr: &mut Vec<u8>,
    ) -> Result<(), InteractiveProcessError> {
        while let Ok(event) = self.events.try_recv() {
            self.apply_event(event, stdout, stderr)?;
        }
        Ok(())
    }

    fn try_wait_child(
        &mut self,
    ) -> Result<Option<std::process::ExitStatus>, InteractiveProcessError> {
        match self.child.as_mut() {
            Some(child) => child
                .try_wait()
                .map_err(|source| InteractiveProcessError::WaitFailed { source }),
            None => Ok(Some(exit_status_unavailable())),
        }
    }

    fn kill_internal(&mut self) -> Result<(), InteractiveProcessError> {
        if let Some(child) = self.child.as_mut() {
            kill_process_group(child)
                .map_err(|source| InteractiveProcessError::KillFailed { source })?;
            child
                .wait()
                .map_err(|source| InteractiveProcessError::WaitFailed { source })?;
        }
        self.child = None;
        self.stdin = None;
        self.terminated = true;
        Ok(())
    }

    fn finish_prompt_wait(
        &mut self,
        stdout: &mut Vec<u8>,
        stderr: &mut Vec<u8>,
    ) -> Result<InteractiveCommandOutput, InteractiveProcessError> {
        let deadline = Instant::now() + PROMPT_DRAIN_GRACE;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }

            match self.events.recv_timeout(remaining.min(IO_POLL_INTERVAL)) {
                Ok(event) => self.apply_event(event, stdout, stderr)?,
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        flush_pending(&mut self.stdout_pending, stdout);
        flush_pending(&mut self.stderr_pending, stderr);

        if let Some(status) = self.try_wait_child()? {
            self.child = None;
            self.stdin = None;
            self.terminated = true;
            self.poisoned = true;
            return Err(InteractiveProcessError::ProcessExited {
                exit_code: status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(stdout).into_owned(),
                stderr: String::from_utf8_lossy(stderr).into_owned(),
            });
        }

        Ok(InteractiveCommandOutput {
            stdout: String::from_utf8_lossy(stdout).into_owned(),
            stderr: String::from_utf8_lossy(stderr).into_owned(),
        })
    }

    fn interrupt_with_policy(
        &mut self,
        mode: &WaitMode,
        timeout: Duration,
        policy: &ProcessExecutionPolicy,
        reason: ProcessInterruptionReason,
        stdout: &mut Vec<u8>,
        stderr: &mut Vec<u8>,
    ) -> Result<Option<InteractiveProcessError>, InteractiveProcessError> {
        let command = match mode {
            WaitMode::Startup => {
                return Ok(None);
            }
            WaitMode::Command { command } => command,
        };

        match policy.safety {
            ProcessInterruptionSafety::CriticalNonAbortable => {
                warn!(
                    command,
                    reason = ?reason,
                    "interruption requested during critical interactive phase; waiting for prompt"
                );
                Ok(None)
            }
            ProcessInterruptionSafety::Interruptible => {
                flush_pending(&mut self.stdout_pending, stdout);
                flush_pending(&mut self.stderr_pending, stderr);
                self.kill_internal()?;
                self.poisoned = true;
                Ok(Some(interactive_error_from_reason(
                    command, timeout, reason, stdout, stderr,
                )))
            }
            ProcessInterruptionSafety::GracefulThenKill => {
                flush_pending(&mut self.stdout_pending, stdout);
                flush_pending(&mut self.stderr_pending, stderr);
                match self.shutdown(policy.graceful_shutdown_timeout) {
                    Ok(_) | Err(InteractiveProcessError::Terminated) => {}
                    Err(error) => return Err(error),
                }
                self.poisoned = true;
                Ok(Some(interactive_error_from_reason(
                    command, timeout, reason, stdout, stderr,
                )))
            }
        }
    }
}

impl Drop for InteractiveProcessExecutor {
    fn drop(&mut self) {
        let _ = self.kill_internal();
    }
}

#[derive(Debug)]
enum WaitMode {
    Startup,
    Command { command: String },
}

fn interactive_error_from_reason(
    command: &str,
    timeout: Duration,
    reason: ProcessInterruptionReason,
    stdout: &[u8],
    stderr: &[u8],
) -> InteractiveProcessError {
    match reason {
        ProcessInterruptionReason::Cancelled => InteractiveProcessError::CommandCancelled {
            command: command.to_owned(),
            stdout: String::from_utf8_lossy(stdout).into_owned(),
            stderr: String::from_utf8_lossy(stderr).into_owned(),
        },
        ProcessInterruptionReason::TimedOut => InteractiveProcessError::CommandTimeout {
            command: command.to_owned(),
            timeout_ms: timeout.as_millis() as u64,
            stdout: String::from_utf8_lossy(stdout).into_owned(),
            stderr: String::from_utf8_lossy(stderr).into_owned(),
        },
    }
}

#[derive(Debug)]
enum ReaderEvent {
    Chunk(InteractiveStream, Vec<u8>),
    Closed,
    ReadError(InteractiveStream, String),
}

fn spawn_reader<T>(mut reader: T, stream: InteractiveStream, sender: Sender<ReaderEvent>)
where
    T: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = [0_u8; STREAM_BUFFER_SIZE];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    let _ = sender.send(ReaderEvent::Closed);
                    break;
                }
                Ok(read) => {
                    let _ = sender.send(ReaderEvent::Chunk(stream, buffer[..read].to_vec()));
                }
                Err(error) => {
                    let _ = sender.send(ReaderEvent::ReadError(stream, error.to_string()));
                    break;
                }
            }
        }
    });
}

fn consume_until_prompt(buffer: &mut Vec<u8>, output: &mut Vec<u8>, prompt: &[u8]) -> Option<()> {
    if let Some((index, consumed_len)) = find_prompt(buffer, prompt) {
        output.extend_from_slice(&buffer[..index]);
        let tail = buffer.split_off(index + consumed_len);
        buffer.clear();
        buffer.extend(tail);
        return Some(());
    }
    None
}

fn flush_completed_bytes(buffer: &mut Vec<u8>, output: &mut Vec<u8>, prompt_len: usize) {
    if prompt_len <= 1 || buffer.len() <= prompt_len.saturating_sub(1) {
        return;
    }

    let keep = prompt_len.saturating_sub(1);
    let flush_len = buffer.len() - keep;
    output.extend_from_slice(&buffer[..flush_len]);
    buffer.drain(..flush_len);
}

fn flush_pending(buffer: &mut Vec<u8>, output: &mut Vec<u8>) {
    output.extend_from_slice(buffer);
    buffer.clear();
}

fn find_prompt(buffer: &[u8], prompt: &[u8]) -> Option<(usize, usize)> {
    if prompt.is_empty() || buffer.len() < prompt.len() {
        return None;
    }

    for index in (0..=buffer.len() - prompt.len()).rev() {
        if &buffer[index..index + prompt.len()] != prompt {
            continue;
        }
        let suffix = &buffer[index + prompt.len()..];
        let has_boundary = index == 0 || matches!(buffer[index - 1], b'\n' | b'\r');
        if has_boundary
            && suffix
                .iter()
                .all(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        {
            return Some((index, prompt.len() + suffix.len()));
        }
    }

    None
}

fn render_command(request: &InteractiveProcessRequest) -> String {
    let mut parts = Vec::with_capacity(request.args.len() + 1);
    parts.push(request.program.display().to_string());
    parts.extend(request.args.iter().cloned());
    parts.join(" ")
}

fn spawn_command(
    request: &InteractiveProcessRequest,
    rendered_command: &str,
) -> Result<Child, InteractiveProcessError> {
    spawn_with_executable_busy_retry(rendered_command, || {
        let mut command = build_command(request);
        command.spawn()
    })
}

fn build_command(request: &InteractiveProcessRequest) -> Command {
    let mut command = Command::new(&request.program);
    command.args(&request.args);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    if let Some(workdir) = &request.workdir {
        command.current_dir(workdir);
    }
    configure_process_group(&mut command);
    command
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

fn spawn_with_executable_busy_retry<F>(
    rendered_command: &str,
    mut spawn: F,
) -> Result<Child, InteractiveProcessError>
where
    F: FnMut() -> Result<Child, std::io::Error>,
{
    for attempt in 0..=EXECUTABLE_BUSY_MAX_RETRIES {
        match spawn() {
            Ok(child) => return Ok(child),
            Err(source) if is_executable_busy(&source) && attempt < EXECUTABLE_BUSY_MAX_RETRIES => {
                warn!(
                    command = rendered_command,
                    attempt = attempt + 1,
                    max_retries = EXECUTABLE_BUSY_MAX_RETRIES,
                    delay_ms = EXECUTABLE_BUSY_RETRY_DELAY.as_millis() as u64,
                    "interactive spawn hit executable-busy race, retrying"
                );
                thread::sleep(EXECUTABLE_BUSY_RETRY_DELAY);
            }
            Err(source) => {
                return Err(InteractiveProcessError::SpawnFailed {
                    cmd: rendered_command.to_owned(),
                    source,
                });
            }
        }
    }

    unreachable!("interactive spawn loop must return on success or final error");
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn kill_process_group(child: &mut Child) -> std::io::Result<()> {
    unsafe {
        let pgid = -(child.id() as i32);
        if libc::kill(pgid, libc::SIGKILL) != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn kill_process_group(child: &mut Child) -> std::io::Result<()> {
    child.kill()
}

#[cfg(unix)]
fn exit_status_unavailable() -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;

    std::process::ExitStatus::from_raw(-1)
}

#[cfg(windows)]
fn exit_status_unavailable() -> std::process::ExitStatus {
    use std::os::windows::process::ExitStatusExt;

    std::process::ExitStatus::from_raw(1)
}

#[cfg(test)]
mod tests {
    use super::{
        spawn_with_executable_busy_retry, InteractiveCommandOutput, InteractiveProcessError,
        InteractiveProcessExecutor, InteractiveProcessRequest, ShutdownOutcome,
    };
    use std::fs;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;
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
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create dirs");
        }
        fs::write(path, format!("#!/bin/sh\nset -eu\n{body}\n")).expect("write script");
        make_executable(path);
    }

    #[cfg(unix)]
    fn repl_script(path: &Path, child_pid_path: &Path) {
        write_script(
            path,
            &format!(
                "prompt_suffix=\"${{PROMPT_SUFFIX:-}}\"\n\
                 prompt_stdout() {{ printf '1C:EDT>%s' \"$prompt_suffix\"; }}\n\
                 prompt_stderr() {{ printf '1C:EDT>%s' \"$prompt_suffix\" >&2; }}\n\
                 startup_stream=\"${{STARTUP_PROMPT_STREAM:-stdout}}\"\n\
                 case \"$startup_stream\" in\n\
                   stdout) prompt_stdout ;;\n\
                   stderr) prompt_stderr ;;\n\
                 esac\n\
                 while IFS= read -r line; do\n\
                   case \"$line\" in\n\
                     pid)\n\
                       printf '%s\\n' \"$$\"\n\
                       prompt_stdout\n\
                       ;;\n\
                     echo:*)\n\
                       printf '%s\\n' \"${{line#echo:}}\"\n\
                       prompt_stdout\n\
                       ;;\n\
                     prompt_newline)\n\
                       printf 'line-before-prompt\\n'\n\
                       printf '1C:EDT>\\n'\n\
                       ;;\n\
                     split)\n\
                       printf 'chunk-one\\n'\n\
                       printf '1C:'\n\
                       sleep 0.05\n\
                       printf 'EDT>'\n\
                       ;;\n\
                     stderr_prompt)\n\
                       printf 'warning\\n' >&2\n\
                       prompt_stderr\n\
                       ;;\n\
                     literal_prompt)\n\
                       printf 'payload 1C:EDT> text\\n'\n\
                       printf 'second line\\n'\n\
                       prompt_stdout\n\
                       ;;\n\
                     fake_prompt_tail)\n\
                       printf 'payload tail 1C:EDT>'\n\
                       sleep 2\n\
                       ;;\n\
                     hang)\n\
                       printf 'started\\n'\n\
                       sleep 2\n\
                       prompt_stdout\n\
                       ;;\n\
                     prompt_exit)\n\
                       printf 'before-exit\\n'\n\
                       prompt_stdout\n\
                       exit 0\n\
                       ;;\n\
                     drop_pipes)\n\
                       exec 1>&-\n\
                       exec 2>&-\n\
                       sleep 2\n\
                       ;;\n\
                     die)\n\
                       printf 'goodbye\\n'\n\
                       exit 7\n\
                       ;;\n\
                     spawn_child)\n\
                       sleep 30 &\n\
                       echo $! > '{}'\n\
                       printf 'spawned\\n'\n\
                       prompt_stdout\n\
                       ;;\n\
                     noisy)\n\
                       i=0\n\
                       while [ \"$i\" -lt 2000 ]; do\n\
                         printf 'stdout-%04d\\n' \"$i\"\n\
                         printf 'stderr-%04d\\n' \"$i\" >&2\n\
                         i=$((i+1))\n\
                       done\n\
                       prompt_stdout\n\
                       ;;\n\
                     *)\n\
                       printf 'unknown:%s\\n' \"$line\"\n\
                       prompt_stdout\n\
                       ;;\n\
                   esac\n\
                 done\n\
                 sleep 30\n",
                child_pid_path.display()
            ),
        );
    }

    #[cfg(unix)]
    fn startup_timeout_script(path: &Path) {
        write_script(path, "sleep 2\nprintf '1C:EDT>'\n");
    }

    #[cfg(unix)]
    fn exit_before_prompt_script(path: &Path) {
        write_script(path, "printf 'booting\\n'\nexit 9\n");
    }

    #[cfg(unix)]
    fn prompt_then_exit_startup_script(path: &Path) {
        write_script(path, "printf '1C:EDT>\\n'\nexit 0\n");
    }

    #[cfg(unix)]
    fn spawn_executor(script: &Path, startup_timeout: Duration) -> InteractiveProcessExecutor {
        InteractiveProcessExecutor::spawn(
            InteractiveProcessRequest::new(script.to_path_buf()),
            startup_timeout,
        )
        .expect("spawn executor")
    }

    #[cfg(unix)]
    fn spawn_executor_with_startup_stream(
        script: &Path,
        startup_timeout: Duration,
        stream: &str,
    ) -> InteractiveProcessExecutor {
        spawn_executor_with_wrapper(
            script,
            startup_timeout,
            &format!(
                "STARTUP_PROMPT_STREAM='{}' exec '{}'\n",
                stream,
                script.display()
            ),
        )
    }

    #[cfg(unix)]
    fn spawn_executor_with_prompt_suffix(
        script: &Path,
        startup_timeout: Duration,
        prompt_suffix: &str,
    ) -> InteractiveProcessExecutor {
        spawn_executor_with_wrapper(
            script,
            startup_timeout,
            &format!(
                "PROMPT_SUFFIX='{}' exec '{}'\n",
                prompt_suffix,
                script.display()
            ),
        )
    }

    #[cfg(unix)]
    fn spawn_executor_with_wrapper(
        script: &Path,
        startup_timeout: Duration,
        wrapper_body: &str,
    ) -> InteractiveProcessExecutor {
        let wrapper = script
            .parent()
            .expect("script parent")
            .join(format!("wrapper-{}.sh", std::process::id()));
        write_script(&wrapper, wrapper_body);

        InteractiveProcessExecutor::spawn(InteractiveProcessRequest::new(wrapper), startup_timeout)
            .expect("spawn executor")
    }

    #[cfg(unix)]
    fn run_command(
        executor: &mut InteractiveProcessExecutor,
        command: &str,
        timeout: Duration,
    ) -> InteractiveCommandOutput {
        executor.execute(command, timeout).expect("execute command")
    }

    #[cfg(unix)]
    #[test]
    fn interactive_spawn_retries_executable_busy_before_succeeding() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let child = spawn_with_executable_busy_retry("fake interactive command", {
            let attempts = attempts.clone();
            move || {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    return Err(std::io::Error::from_raw_os_error(libc::ETXTBSY));
                }

                let mut command = Command::new("/bin/sh");
                command.arg("-c").arg("printf '1C:EDT>'");
                command.stdin(Stdio::null());
                command.stdout(Stdio::piped());
                command.stderr(Stdio::piped());
                command.spawn()
            }
        })
        .expect("spawn with retry");

        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        let output = child.wait_with_output().expect("child output");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "1C:EDT>");
    }

    #[cfg(unix)]
    #[test]
    fn interactive_spawn_does_not_retry_non_executable_busy_errors() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let err = spawn_with_executable_busy_retry("fake interactive command", {
            let attempts = attempts.clone();
            move || {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "missing binary",
                ))
            }
        })
        .expect_err("spawn must fail");

        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert!(matches!(err, InteractiveProcessError::SpawnFailed { .. }));
    }

    #[cfg(unix)]
    fn is_process_alive(pid: u32) -> bool {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[cfg(unix)]
    #[test]
    fn startup_waits_for_prompt() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("repl.sh");
        repl_script(&script, &dir.path().join("child.pid"));

        let executor = spawn_executor(&script, Duration::from_millis(200));

        assert!(executor.pid().expect("pid") > 0);
    }

    #[cfg(unix)]
    #[test]
    fn startup_prompt_on_stderr_is_supported() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("repl.sh");
        repl_script(&script, &dir.path().join("child.pid"));

        let executor =
            spawn_executor_with_startup_stream(&script, Duration::from_millis(200), "stderr");

        assert!(executor.pid().expect("pid") > 0);
    }

    #[cfg(unix)]
    #[test]
    fn startup_prompt_with_trailing_space_is_supported() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("repl.sh");
        repl_script(&script, &dir.path().join("child.pid"));

        let executor = spawn_executor_with_prompt_suffix(&script, Duration::from_millis(200), " ");

        assert!(executor.pid().expect("pid") > 0);
    }

    #[cfg(unix)]
    #[test]
    fn startup_timeout_kills_process() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("slow.sh");
        startup_timeout_script(&script);

        let err = InteractiveProcessExecutor::spawn(
            InteractiveProcessRequest::new(script),
            Duration::from_millis(50),
        )
        .expect_err("startup must time out");

        assert!(matches!(
            err,
            InteractiveProcessError::StartupTimeout { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn startup_reports_process_exit_before_prompt() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("exit.sh");
        exit_before_prompt_script(&script);

        let err = InteractiveProcessExecutor::spawn(
            InteractiveProcessRequest::new(script),
            Duration::from_millis(200),
        )
        .expect_err("startup must fail");

        assert!(matches!(
            err,
            InteractiveProcessError::ProcessExited { exit_code: 9, .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn startup_prompt_then_exit_is_reported_as_process_exited() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("prompt-exit.sh");
        prompt_then_exit_startup_script(&script);

        let err = InteractiveProcessExecutor::spawn(
            InteractiveProcessRequest::new(script),
            Duration::from_millis(200),
        )
        .expect_err("startup prompt followed by exit must fail");

        assert!(matches!(
            err,
            InteractiveProcessError::ProcessExited { exit_code: 0, .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn repeated_commands_reuse_same_process() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("repl.sh");
        repl_script(&script, &dir.path().join("child.pid"));
        let mut executor = spawn_executor(&script, Duration::from_millis(200));

        let first = run_command(&mut executor, "pid", Duration::from_millis(200));
        let second = run_command(&mut executor, "pid", Duration::from_millis(200));

        assert_eq!(first.stdout.trim(), second.stdout.trim());
        assert_eq!(
            executor.pid().expect("pid").to_string(),
            first.stdout.trim()
        );
    }

    #[cfg(unix)]
    #[test]
    fn prompt_split_across_chunks_is_detected() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("repl.sh");
        repl_script(&script, &dir.path().join("child.pid"));
        let mut executor = spawn_executor(&script, Duration::from_millis(200));

        let output = run_command(&mut executor, "split", Duration::from_millis(500));

        assert_eq!(output.stdout, "chunk-one\n");
        assert!(output.stderr.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn prompt_followed_by_newline_is_supported() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("repl.sh");
        repl_script(&script, &dir.path().join("child.pid"));
        let mut executor = spawn_executor(&script, Duration::from_millis(200));

        let output = run_command(&mut executor, "prompt_newline", Duration::from_millis(200));

        assert_eq!(output.stdout, "line-before-prompt\n");
    }

    #[cfg(unix)]
    #[test]
    fn prompt_on_stderr_is_supported() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("repl.sh");
        let child_pid = dir.path().join("child.pid");
        repl_script(&script, &child_pid);

        let mut executor = InteractiveProcessExecutor::spawn(
            InteractiveProcessRequest::new(script),
            Duration::from_millis(200),
        )
        .expect("spawn executor");
        let output = run_command(&mut executor, "stderr_prompt", Duration::from_millis(200));

        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "warning\n");
    }

    #[cfg(unix)]
    #[test]
    fn literal_prompt_inside_payload_does_not_finish_early() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("repl.sh");
        repl_script(&script, &dir.path().join("child.pid"));
        let mut executor = spawn_executor(&script, Duration::from_millis(200));

        let output = run_command(&mut executor, "literal_prompt", Duration::from_millis(200));

        assert_eq!(output.stdout, "payload 1C:EDT> text\nsecond line\n");
    }

    #[cfg(unix)]
    #[test]
    fn command_timeout_kills_process_and_poison_fails_next_call() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("repl.sh");
        repl_script(&script, &dir.path().join("child.pid"));
        let mut executor = spawn_executor(&script, Duration::from_millis(200));

        let err = executor
            .execute("hang", Duration::from_millis(50))
            .expect_err("command must time out");

        assert!(matches!(
            err,
            InteractiveProcessError::CommandTimeout { .. }
        ));
        assert!(matches!(
            executor.execute("pid", Duration::from_millis(50)),
            Err(InteractiveProcessError::Poisoned | InteractiveProcessError::Terminated)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn process_exit_during_command_is_reported_immediately() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("repl.sh");
        repl_script(&script, &dir.path().join("child.pid"));
        let mut executor = spawn_executor(&script, Duration::from_millis(200));

        let err = executor
            .execute("die", Duration::from_secs(1))
            .expect_err("child must exit");

        assert!(matches!(
            err,
            InteractiveProcessError::ProcessExited { exit_code: 7, .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn prompt_then_immediate_exit_returns_process_exited() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("repl.sh");
        repl_script(&script, &dir.path().join("child.pid"));
        let mut executor = spawn_executor(&script, Duration::from_millis(200));

        let err = executor
            .execute("prompt_exit", Duration::from_millis(200))
            .expect_err("prompt followed by exit must fail");

        assert!(matches!(
            err,
            InteractiveProcessError::ProcessExited { exit_code: 0, .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn literal_prompt_at_payload_tail_does_not_finish_command() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("repl.sh");
        repl_script(&script, &dir.path().join("child.pid"));
        let mut executor = spawn_executor(&script, Duration::from_millis(200));

        let err = executor
            .execute("fake_prompt_tail", Duration::from_millis(50))
            .expect_err("payload tail must not count as prompt");

        assert!(matches!(
            err,
            InteractiveProcessError::CommandTimeout { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn command_prompt_with_trailing_space_is_supported() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("repl.sh");
        repl_script(&script, &dir.path().join("child.pid"));
        let mut executor =
            spawn_executor_with_prompt_suffix(&script, Duration::from_millis(200), " ");

        let output = run_command(&mut executor, "echo:ok", Duration::from_millis(200));

        assert_eq!(output.stdout, "ok\n");
        assert_eq!(output.stderr, "");
    }

    #[cfg(unix)]
    #[test]
    fn stdio_disconnect_kills_child_process() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("repl.sh");
        repl_script(&script, &dir.path().join("child.pid"));
        let mut executor = spawn_executor(&script, Duration::from_millis(200));
        let pid = executor.pid().expect("pid");

        let err = executor
            .execute("drop_pipes", Duration::from_millis(200))
            .expect_err("disconnect must fail");
        assert!(matches!(err, InteractiveProcessError::ProcessExited { .. }));

        thread::sleep(Duration::from_millis(50));
        assert!(!is_process_alive(pid));
    }

    #[cfg(unix)]
    #[test]
    fn graceful_shutdown_closes_process() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("repl.sh");
        repl_script(&script, &dir.path().join("child.pid"));
        let mut executor = spawn_executor(&script, Duration::from_millis(200));

        let outcome = executor
            .shutdown(Duration::from_millis(100))
            .expect("shutdown result");

        assert!(matches!(outcome, ShutdownOutcome::ForcedKill));
    }

    #[cfg(unix)]
    #[test]
    fn kill_terminates_background_child_processes() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("repl.sh");
        let child_pid_path = dir.path().join("child.pid");
        repl_script(&script, &child_pid_path);
        let mut executor = spawn_executor(&script, Duration::from_millis(200));

        let _ = run_command(&mut executor, "spawn_child", Duration::from_millis(200));
        let child_pid: u32 = fs::read_to_string(&child_pid_path)
            .expect("child pid")
            .trim()
            .parse()
            .expect("pid");
        assert!(is_process_alive(child_pid));

        executor.kill().expect("kill executor");
        thread::sleep(Duration::from_millis(50));

        assert!(!is_process_alive(child_pid));
    }

    #[cfg(unix)]
    #[test]
    fn large_output_does_not_deadlock() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("repl.sh");
        repl_script(&script, &dir.path().join("child.pid"));
        let mut executor = spawn_executor(&script, Duration::from_millis(200));

        let output = run_command(&mut executor, "noisy", Duration::from_secs(2));

        assert!(output.stdout.contains("stdout-1999"));
        assert!(output.stderr.contains("stderr-1999"));

        let next = run_command(&mut executor, "echo:after", Duration::from_millis(200));
        assert_eq!(next.stdout, "after\n");
        assert!(next.stderr.is_empty());
    }
}
