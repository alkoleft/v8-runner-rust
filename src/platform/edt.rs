use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use thiserror::Error;
use tracing::debug;

use crate::platform::edt_session::{EdtSessionError, EdtSessionManager, EdtSessionRequest};
use crate::platform::interactive::{
    InteractiveCommandExecution, InteractiveProcessError, InteractiveProcessExecutor,
    InteractiveProcessRequest,
};
use crate::platform::process::{
    ProcessError, ProcessExecutionPolicy, ProcessInterruptionReason, ProcessRequest, ProcessRunner,
};
use crate::platform::result::PlatformCommandResult;

const INTERACTIVE_EDT_ERROR_MARKER: &str = "Run '$exception printStackTrace' for error details";

#[derive(Debug, Error)]
pub enum EdtError {
    #[error("failed to prepare edt workspace '{path}': {source}")]
    PrepareWorkspace {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to execute edt process: {0}")]
    Spawn(ProcessError),

    #[error("failed to execute interactive edt process: {0}")]
    Interactive(InteractiveProcessError),

    #[error("failed to execute shared edt session: {0}")]
    SharedSession(EdtSessionError),
}

/// Low-level DSL for invoking `1cedtcli`.
pub struct EdtDsl<'a> {
    binary: PathBuf,
    workspace: PathBuf,
    backend: EdtBackend<'a>,
    timeout: Option<Duration>,
    execution_policy: ProcessExecutionPolicy,
    budget_started_at: Instant,
}

impl<'a> EdtDsl<'a> {
    /// Create a new EDT DSL bound to one executable path and runner.
    pub fn new(binary: PathBuf, workspace: PathBuf, runner: &'a dyn ProcessRunner) -> Self {
        Self {
            binary,
            workspace,
            backend: EdtBackend::OneShot { runner },
            timeout: None,
            execution_policy: ProcessExecutionPolicy::default(),
            budget_started_at: Instant::now(),
        }
    }

    /// Create a new interactive EDT DSL that keeps a long-lived `1cedtcli` process alive.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new_interactive(
        binary: PathBuf,
        workspace: PathBuf,
        startup_timeout: Duration,
        command_timeout: Duration,
    ) -> Result<Self, EdtError> {
        std::fs::create_dir_all(&workspace).map_err(|source| EdtError::PrepareWorkspace {
            path: workspace.clone(),
            source,
        })?;
        let request = InteractiveProcessRequest::new(binary.clone())
            .with_args(["-data".to_owned(), workspace.display().to_string()]);
        debug!(
            command = render_process_command(
                &binary,
                &["-data".to_owned(), workspace.display().to_string()]
            ),
            startup_timeout_ms = startup_timeout.as_millis() as u64,
            command_timeout_ms = command_timeout.as_millis() as u64,
            "starting interactive edt session"
        );
        let session = InteractiveProcessExecutor::spawn(request, startup_timeout)
            .map_err(EdtError::Interactive)?;
        debug!(
            binary = %binary.display(),
            workspace = %workspace.display(),
            pid = session.pid(),
            "interactive edt session started"
        );

        Ok(Self {
            binary,
            workspace,
            backend: EdtBackend::Interactive {
                session: RefCell::new(session),
                command_timeout,
                shutdown_timeout: command_timeout.min(Duration::from_secs(5)),
            },
            timeout: None,
            execution_policy: ProcessExecutionPolicy::default(),
            budget_started_at: Instant::now(),
        })
    }

    /// Create an EDT DSL over the shared interactive EDT actor.
    pub fn new_shared_session(
        binary: PathBuf,
        workspace: PathBuf,
        manager: Arc<EdtSessionManager>,
        startup_timeout: Duration,
        command_timeout: Duration,
    ) -> Result<Self, EdtError> {
        std::fs::create_dir_all(&workspace).map_err(|source| EdtError::PrepareWorkspace {
            path: workspace.clone(),
            source,
        })?;

        Ok(Self {
            binary,
            workspace,
            backend: EdtBackend::SharedSession {
                manager,
                startup_timeout,
                command_timeout,
            },
            timeout: None,
            execution_policy: ProcessExecutionPolicy::default(),
            budget_started_at: Instant::now(),
        })
    }

    /// Overrides the timeout cap used for EDT command execution.
    pub fn with_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self.budget_started_at = Instant::now();
        self
    }

    /// Overrides the shared execution policy for EDT command execution.
    pub fn with_execution_policy(mut self, execution_policy: ProcessExecutionPolicy) -> Self {
        self.execution_policy = execution_policy;
        self.budget_started_at = Instant::now();
        self
    }

    /// `-command export --project-name <project_name> --configuration-files <target>`
    pub fn export_project(
        &self,
        project_name: &str,
        target: &Path,
    ) -> Result<PlatformCommandResult, EdtError> {
        let command_arguments = export_command_arguments(project_name, target);
        self.run(
            &process_arguments(&self.workspace, &command_arguments),
            &render_interactive_export_command(project_name, target),
            None,
        )
    }

    /// `-command export --project <source> --configuration-files <target>`
    pub fn export_project_path(
        &self,
        source: &Path,
        target: &Path,
    ) -> Result<PlatformCommandResult, EdtError> {
        let command_arguments = export_project_path_command_arguments(source, target);
        self.run(
            &process_arguments(&self.workspace, &command_arguments),
            &render_interactive_export_project_path_command(source, target),
            None,
        )
    }

    /// `-command validate --file <out_log> --project-list <source>`
    pub fn validate_project(
        &self,
        source: &Path,
        out_log: &Path,
    ) -> Result<PlatformCommandResult, EdtError> {
        let command_arguments = validate_command_arguments(source, out_log);
        self.run(
            &process_arguments(&self.workspace, &command_arguments),
            &render_interactive_command(&command_arguments),
            Some(out_log),
        )
    }

    /// `-command import --project <source>`
    pub fn import_project(&self, source: &Path) -> Result<PlatformCommandResult, EdtError> {
        let command_arguments = import_command_arguments(source);
        self.run(
            &process_arguments(&self.workspace, &command_arguments),
            &render_interactive_import_command(source),
            None,
        )
    }

    /// `-command import --configuration-files <source> --project <target> [...]`
    pub fn import_configuration_files(
        &self,
        target_project: &Path,
        configuration_files: &Path,
        version: Option<&str>,
        base_project_name: Option<&str>,
        build: bool,
    ) -> Result<PlatformCommandResult, EdtError> {
        let command_arguments = import_configuration_files_command_arguments(
            target_project,
            configuration_files,
            version,
            base_project_name,
            build,
        );
        self.run(
            &process_arguments(&self.workspace, &command_arguments),
            &render_interactive_command(&command_arguments),
            None,
        )
    }

    fn run(
        &self,
        args: &[String],
        interactive_command: &str,
        out_log: Option<&Path>,
    ) -> Result<PlatformCommandResult, EdtError> {
        std::fs::create_dir_all(&self.workspace).map_err(|source| EdtError::PrepareWorkspace {
            path: self.workspace.clone(),
            source,
        })?;

        if let Some(path) = out_log {
            let _ = std::fs::remove_file(path);
        }

        let process = match &self.backend {
            EdtBackend::OneShot { runner } => {
                let rendered_command = render_process_command(&self.binary, args);
                debug!(
                    command = rendered_command.as_str(),
                    timeout_ms = self.timeout.map(|value| value.as_millis() as u64),
                    "running edt command"
                );
                let request = ProcessRequest {
                    program: self.binary.clone(),
                    args: args.to_vec(),
                    workdir: None,
                    stdout_log_path: None,
                    stderr_log_path: None,
                    startup_probe: None,
                };
                let mut execution_policy = self.execution_policy.clone();
                execution_policy.timeout = match (
                    self.remaining_budget_cap(self.timeout),
                    self.remaining_budget_cap(execution_policy.timeout),
                ) {
                    (Some(timeout), Some(cap)) => Some(timeout.min(cap)),
                    (Some(timeout), None) => Some(timeout),
                    (None, timeout) => timeout,
                };
                runner
                    .run_with_policy(&request, &execution_policy)
                    .map_err(EdtError::Spawn)?
            }
            EdtBackend::Interactive {
                session,
                command_timeout,
                ..
            } => {
                let effective_timeout = match (
                    self.remaining_budget_cap(self.timeout),
                    self.remaining_budget_cap(self.execution_policy.timeout),
                ) {
                    (Some(timeout), Some(cap)) => timeout.min(cap),
                    (Some(timeout), None) => timeout,
                    (None, Some(cap)) => (*command_timeout).min(cap),
                    (None, None) => *command_timeout,
                };
                let deadline = Instant::now() + effective_timeout;
                let mut execution_policy = self.execution_policy.clone();
                execution_policy.timeout = Some(effective_timeout);
                let mut session = session.borrow_mut();
                let change_dir_command = render_interactive_change_dir_command(&self.workspace);
                debug!(
                    workspace = %self.workspace.display(),
                    command = interactive_command,
                    timeout_ms = effective_timeout.as_millis() as u64,
                    "running interactive edt command"
                );
                let change_dir = session
                    .execute_with_policy(
                        &change_dir_command,
                        remaining_interactive_timeout(deadline),
                        &execution_policy,
                    )
                    .map_err(|error| {
                        map_interactive_command_error(
                            &self.binary,
                            &self.workspace,
                            interactive_command,
                            error,
                        )
                    })?;
                if let Some(interruption) = change_dir.interruption {
                    return Err(EdtError::Spawn(process_error_from_interruption(
                        &self.binary,
                        &self.workspace,
                        interactive_command,
                        interruption.reason,
                        effective_timeout,
                    )));
                }
                let execution = session
                    .execute_with_policy(
                        interactive_command,
                        remaining_interactive_timeout(deadline),
                        &execution_policy,
                    )
                    .map_err(|error| {
                        map_interactive_command_error(
                            &self.binary,
                            &self.workspace,
                            interactive_command,
                            error,
                        )
                    })?;
                let InteractiveCommandExecution {
                    output,
                    interruption,
                } = execution;
                debug!(
                    workspace = %self.workspace.display(),
                    command = interactive_command,
                    stdout = render_interactive_output_for_log(&output.stdout),
                    stderr = render_interactive_output_for_log(&output.stderr),
                    "interactive edt command finished"
                );
                let exit_code =
                    if output_indicates_interactive_command_error(&output.stdout, &output.stderr) {
                        1
                    } else {
                        0
                    };
                crate::platform::process::ProcessResult {
                    exit_code,
                    stdout: output.stdout,
                    stderr: output.stderr,
                    interruption,
                }
            }
            EdtBackend::SharedSession {
                manager,
                startup_timeout,
                command_timeout,
            } => {
                if !manager.has_live_session() {
                    manager
                        .execute_blocking(
                            EdtSessionRequest::new(
                                render_interactive_change_dir_command(&self.workspace),
                                Instant::now() + *startup_timeout,
                            )
                            .with_cancellation(self.execution_policy.cancellation.clone()),
                        )
                        .map_err(|error| {
                            map_shared_session_error(
                                &self.binary,
                                &self.workspace,
                                &render_interactive_change_dir_command(&self.workspace),
                                error,
                                *startup_timeout,
                            )
                        })?;
                }
                let effective_timeout = match (
                    self.remaining_budget_cap(self.timeout),
                    self.remaining_budget_cap(self.execution_policy.timeout),
                ) {
                    (Some(timeout), Some(cap)) => timeout.min(cap),
                    (Some(timeout), None) => timeout,
                    (None, Some(cap)) => (*command_timeout).min(cap),
                    (None, None) => *command_timeout,
                };
                debug!(
                    workspace = %self.workspace.display(),
                    command = interactive_command,
                    timeout_ms = effective_timeout.as_millis() as u64,
                    "running shared edt command"
                );
                let output = manager
                    .execute_blocking(
                        EdtSessionRequest::new(
                            interactive_command.to_owned(),
                            Instant::now() + effective_timeout,
                        )
                        .with_cancellation(self.execution_policy.cancellation.clone()),
                    )
                    .map_err(|error| {
                        map_shared_session_error(
                            &self.binary,
                            &self.workspace,
                            interactive_command,
                            error,
                            effective_timeout,
                        )
                    })?;
                debug!(
                    workspace = %self.workspace.display(),
                    command = interactive_command,
                    stdout = render_interactive_output_for_log(&output.stdout),
                    stderr = render_interactive_output_for_log(&output.stderr),
                    "shared edt command finished"
                );
                let exit_code =
                    if output_indicates_interactive_command_error(&output.stdout, &output.stderr) {
                        1
                    } else {
                        0
                    };
                crate::platform::process::ProcessResult {
                    exit_code,
                    stdout: output.stdout,
                    stderr: output.stderr,
                    interruption: None,
                }
            }
        };

        let (platform_log_path, platform_log, platform_log_read_error) = if let Some(path) = out_log
        {
            match std::fs::read_to_string(path) {
                Ok(contents) => (Some(path.to_path_buf()), Some(contents), None),
                Err(error) => (
                    Some(path.to_path_buf()),
                    None,
                    Some(format!(
                        "failed to read edt --file log '{}': {error}",
                        path.display()
                    )),
                ),
            }
        } else {
            (None, None, None)
        };

        Ok(PlatformCommandResult {
            process,
            platform_log_path,
            platform_log,
            platform_log_read_error,
        })
    }

    fn remaining_budget_cap(&self, timeout: Option<Duration>) -> Option<Duration> {
        timeout.map(|value| value.saturating_sub(self.budget_started_at.elapsed()))
    }
}

fn map_interactive_command_error(
    binary: &Path,
    workspace: &Path,
    command: &str,
    error: InteractiveProcessError,
) -> EdtError {
    match error {
        InteractiveProcessError::CommandCancelled { .. } => {
            EdtError::Spawn(ProcessError::Cancelled {
                cmd: render_interactive_session_command(binary, workspace, command),
            })
        }
        InteractiveProcessError::CommandTimeout { timeout_ms, .. } => {
            EdtError::Spawn(ProcessError::TimedOut {
                cmd: render_interactive_session_command(binary, workspace, command),
                timeout_ms,
            })
        }
        other @ (InteractiveProcessError::SpawnFailed { .. }
        | InteractiveProcessError::MissingStdin { .. }
        | InteractiveProcessError::MissingStdout { .. }
        | InteractiveProcessError::MissingStderr { .. }
        | InteractiveProcessError::StartupTimeout { .. }
        | InteractiveProcessError::ProcessExited { .. }
        | InteractiveProcessError::Poisoned
        | InteractiveProcessError::Terminated
        | InteractiveProcessError::StdinWriteFailed { .. }
        | InteractiveProcessError::StdinFlushFailed { .. }
        | InteractiveProcessError::StreamReadFailed { .. }
        | InteractiveProcessError::WaitFailed { .. }
        | InteractiveProcessError::KillFailed { .. }) => EdtError::Interactive(other),
    }
}

fn map_shared_session_error(
    binary: &Path,
    workspace: &Path,
    command: &str,
    error: EdtSessionError,
    timeout: Duration,
) -> EdtError {
    match error {
        EdtSessionError::QueuedCancelled | EdtSessionError::RunningCancelled => {
            EdtError::Spawn(ProcessError::Cancelled {
                cmd: render_interactive_session_command(binary, workspace, command),
            })
        }
        EdtSessionError::QueuedTimeout | EdtSessionError::RunningTimeout => {
            EdtError::Spawn(ProcessError::TimedOut {
                cmd: render_interactive_session_command(binary, workspace, command),
                timeout_ms: timeout.as_millis() as u64,
            })
        }
        other @ (EdtSessionError::QueueFull
        | EdtSessionError::StartupFailed { .. }
        | EdtSessionError::SessionFailed { .. }
        | EdtSessionError::DrainedByRestartOrShutdown { .. }
        | EdtSessionError::InternalFailure { .. }) => EdtError::SharedSession(other),
    }
}

fn process_error_from_interruption(
    binary: &Path,
    workspace: &Path,
    command: &str,
    reason: ProcessInterruptionReason,
    timeout: Duration,
) -> ProcessError {
    match reason {
        ProcessInterruptionReason::Cancelled => ProcessError::Cancelled {
            cmd: render_interactive_session_command(binary, workspace, command),
        },
        ProcessInterruptionReason::TimedOut => ProcessError::TimedOut {
            cmd: render_interactive_session_command(binary, workspace, command),
            timeout_ms: timeout.as_millis() as u64,
        },
    }
}

impl Drop for EdtDsl<'_> {
    fn drop(&mut self) {
        match &self.backend {
            EdtBackend::Interactive {
                session,
                shutdown_timeout,
                ..
            } => {
                debug!(
                    workspace = %self.workspace.display(),
                    timeout_ms = shutdown_timeout.as_millis() as u64,
                    "shutting down interactive edt session"
                );
                let _ = session.borrow_mut().shutdown(*shutdown_timeout);
            }
            EdtBackend::SharedSession { manager, .. } if Arc::strong_count(manager) == 1 => {
                debug!(
                    workspace = %self.workspace.display(),
                    "shutting down shared edt session manager owned by edt dsl"
                );
                let _ = manager.shutdown();
            }
            _ => {}
        }
    }
}

enum EdtBackend<'a> {
    OneShot {
        runner: &'a dyn ProcessRunner,
    },
    #[cfg_attr(not(test), allow(dead_code))]
    Interactive {
        session: RefCell<InteractiveProcessExecutor>,
        command_timeout: Duration,
        shutdown_timeout: Duration,
    },
    SharedSession {
        manager: Arc<EdtSessionManager>,
        startup_timeout: Duration,
        command_timeout: Duration,
    },
}

/// Renders an interactive `validate` command for the shared EDT session.
pub(crate) fn render_interactive_validate_command(source: &Path, out_log: &Path) -> String {
    render_interactive_command(&validate_command_arguments(source, out_log))
}

/// Renders an interactive `export` command.
pub(crate) fn render_interactive_export_command(project_name: &str, target: &Path) -> String {
    render_interactive_command(&export_command_arguments(project_name, target))
}

/// Renders an interactive `import` command.
pub(crate) fn render_interactive_import_command(source: &Path) -> String {
    render_interactive_command(&import_command_arguments(source))
}

/// Renders an interactive `cd <workspace>` reset command for the shared EDT session.
pub(crate) fn render_interactive_change_dir_command(path: &Path) -> String {
    render_interactive_command(&["cd".to_owned(), path.display().to_string()])
}

/// Renders an interactive `cd` probe command that prints the current workspace.
pub(crate) fn render_interactive_probe_workdir_command() -> String {
    "cd".to_owned()
}

fn process_arguments(workspace: &Path, command_arguments: &[String]) -> Vec<String> {
    let mut args = vec![
        "-data".to_owned(),
        workspace.display().to_string(),
        "-command".to_owned(),
    ];
    args.extend(command_arguments.iter().cloned());
    args
}

fn render_process_command(binary: &Path, args: &[String]) -> String {
    let mut parts = vec![binary.display().to_string()];
    parts.extend(args.iter().cloned());
    parts.join(" ")
}

fn render_interactive_session_command(binary: &Path, workspace: &Path, command: &str) -> String {
    format!(
        "{} -data {} [interactive: {}]",
        binary.display(),
        workspace.display(),
        interactive_command_name(command)
    )
}

fn interactive_command_name(command: &str) -> &str {
    command.split_whitespace().next().unwrap_or("<unknown>")
}

fn remaining_interactive_timeout(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

fn render_interactive_output_for_log(output: &str) -> String {
    const MAX_LEN: usize = 400;

    let trimmed = output.trim();
    if trimmed.is_empty() {
        return "<empty>".to_owned();
    }

    let mut rendered = trimmed.replace('\n', "\\n");
    if rendered.len() > MAX_LEN {
        rendered.truncate(MAX_LEN);
        rendered.push_str("...");
    }
    rendered
}

fn output_indicates_interactive_command_error(stdout: &str, stderr: &str) -> bool {
    stdout.contains(INTERACTIVE_EDT_ERROR_MARKER) || stderr.contains(INTERACTIVE_EDT_ERROR_MARKER)
}

fn export_command_arguments(project_name: &str, target: &Path) -> Vec<String> {
    vec![
        "export".to_owned(),
        "--project-name".to_owned(),
        project_name.to_owned(),
        "--configuration-files".to_owned(),
        target.display().to_string(),
    ]
}

fn export_project_path_command_arguments(source: &Path, target: &Path) -> Vec<String> {
    vec![
        "export".to_owned(),
        "--project".to_owned(),
        source.display().to_string(),
        "--configuration-files".to_owned(),
        target.display().to_string(),
    ]
}

fn validate_command_arguments(source: &Path, out_log: &Path) -> Vec<String> {
    vec![
        "validate".to_owned(),
        "--file".to_owned(),
        out_log.display().to_string(),
        "--project-list".to_owned(),
        source.display().to_string(),
    ]
}

fn import_command_arguments(source: &Path) -> Vec<String> {
    vec![
        "import".to_owned(),
        "--project".to_owned(),
        source.display().to_string(),
    ]
}

fn import_configuration_files_command_arguments(
    target_project: &Path,
    configuration_files: &Path,
    version: Option<&str>,
    base_project_name: Option<&str>,
    build: bool,
) -> Vec<String> {
    let mut arguments = vec![
        "import".to_owned(),
        "--configuration-files".to_owned(),
        configuration_files.display().to_string(),
        "--project".to_owned(),
        target_project.display().to_string(),
    ];
    if let Some(version) = version {
        arguments.push("--version".to_owned());
        arguments.push(version.to_owned());
    }
    if let Some(base_project_name) = base_project_name {
        arguments.push("--base-project-name".to_owned());
        arguments.push(base_project_name.to_owned());
    }
    if build {
        arguments.push("--build".to_owned());
        arguments.push("true".to_owned());
    }
    arguments
}

fn render_interactive_command(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|argument| quote_interactive_argument(argument))
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_interactive_export_project_path_command(source: &Path, target: &Path) -> String {
    render_interactive_command(&export_project_path_command_arguments(source, target))
}

fn quote_interactive_argument(argument: &str) -> String {
    if argument.is_empty()
        || argument
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '"' | '\\' | '\n' | '\r' | '\t'))
    {
        let escaped = argument.replace('\\', "\\\\").replace('"', "\\\"");
        return format!("\"{escaped}\"");
    }

    argument.to_owned()
}

#[cfg(test)]
mod tests {
    use super::{
        render_interactive_change_dir_command, render_interactive_probe_workdir_command,
        render_interactive_validate_command, EdtDsl, EdtError, INTERACTIVE_EDT_ERROR_MARKER,
    };
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, SourceFormat, SourceSetConfig, SourceSetPurpose,
        TestsConfig, ToolsConfig,
    };
    use crate::platform::edt_session::{EdtSessionHostOptions, EdtSessionManager};
    use crate::platform::process::{
        ProcessError, ProcessExecutionPolicy, ProcessExecutor, ProcessInterruptionAction,
        ProcessInterruptionReason, ProcessInterruptionSafety, ProcessRequest, ProcessResult,
        ProcessRunner, SpawnResult,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
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
        fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        make_executable(path);
    }

    #[cfg(unix)]
    fn write_shared_interactive_script(path: &Path, command_log: &Path) {
        let body = format!(
            "set -eu\n\
             prompt() {{ printf '1C:EDT>'; }}\n\
             current_dir=\"\"\n\
             prev=\"\"\n\
             for arg in \"$@\"; do\n\
               if [ \"$prev\" = \"-data\" ]; then current_dir=\"$arg\"; fi\n\
               prev=\"$arg\"\n\
             done\n\
             printf 'START\\n' >> '{}'\n\
             trap 'printf \"EXIT\\\\n\" >> \"{}\"' EXIT\n\
             prompt\n\
             while IFS= read -r line; do\n\
               printf '%s\\n' \"$line\" >> '{}'\n\
               eval \"set -- $line\"\n\
               cmd=\"${{1:-}}\"\n\
               if [ \"$#\" -gt 0 ]; then shift; fi\n\
               case \"$cmd\" in\n\
                 cd)\n\
                   if [ \"$#\" -eq 0 ]; then\n\
                     printf '%s\\n' \"$current_dir\"\n\
                   else\n\
                     current_dir=\"$1\"\n\
                   fi\n\
                   prompt\n\
                   ;;\n\
                 export|import|validate)\n\
                   prompt\n\
                   ;;\n\
                 *)\n\
                   prompt\n\
                   ;;\n\
               esac\n\
             done\n",
            command_log.display(),
            command_log.display(),
            command_log.display()
        );
        write_script(path, &body);
    }

    fn sample_shared_config(base_path: &Path, work_path: &Path, edt_cli_path: &Path) -> AppConfig {
        AppConfig {
            base_path: base_path.to_path_buf(),
            work_path: work_path.to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Edt,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
            source_sets: vec![SourceSetConfig {
                name: "main".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: PathBuf::from("main"),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig {
                platform: Default::default(),
                enterprise: Default::default(),
                edt_cli: crate::config::model::EdtCliConfig {
                    path: Some(edt_cli_path.to_path_buf()),
                    interactive_mode: true,
                    startup_timeout_ms: 500,
                    command_timeout_ms: 200,
                    ..Default::default()
                },
                ..Default::default()
            },
            mcp: Default::default(),
            tests: TestsConfig::default(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn export_project_passes_expected_arguments() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cedtcli");
        let args_log = dir.path().join("args.log");
        write_script(
            &script,
            &format!("printf '%s\n' \"$@\" > \"{}\"\nexit 0", args_log.display()),
        );

        let runner = ProcessExecutor;
        let dsl = EdtDsl::new(script, dir.path().join("ws"), &runner as &dyn ProcessRunner);

        let result = dsl
            .export_project("project", Path::new("/tmp/out"))
            .expect("export project");

        assert_eq!(result.process.exit_code, 0);
        assert!(result.platform_log.is_none());
        let args = fs::read_to_string(args_log).expect("args log");
        assert!(args.contains("-command"));
        assert!(args.contains("export"));
        assert!(args.contains("--project-name"));
        assert!(args.contains("project"));
        assert!(args.contains("--configuration-files"));
        assert!(args.contains("/tmp/out"));
    }

    #[cfg(unix)]
    #[test]
    fn export_project_path_passes_expected_arguments() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cedtcli");
        let args_log = dir.path().join("args.log");
        write_script(
            &script,
            &format!("printf '%s\n' \"$@\" > \"{}\"\nexit 0", args_log.display()),
        );

        let runner = ProcessExecutor;
        let dsl = EdtDsl::new(script, dir.path().join("ws"), &runner as &dyn ProcessRunner);

        let result = dsl
            .export_project_path(Path::new("/tmp/project"), Path::new("/tmp/out"))
            .expect("export project path");

        assert_eq!(result.process.exit_code, 0);
        let args = fs::read_to_string(args_log).expect("args log");
        assert!(args.contains("-command"));
        assert!(args.contains("export"));
        assert!(args.contains("--project"));
        assert!(args.contains("/tmp/project"));
        assert!(args.contains("--configuration-files"));
        assert!(args.contains("/tmp/out"));
    }

    #[cfg(unix)]
    #[test]
    fn validate_project_reads_out_log() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cedtcli");
        let args_log = dir.path().join("args.log");
        write_script(
            &script,
            &format!(
                "printf '%s\n' \"$@\" > \"{}\"\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"--file\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf 'line1\\nline2\\n' > \"$out\"; fi\nexit 2",
                args_log.display()
            ),
        );
        let out_log = dir.path().join("validate.log");

        let runner = ProcessExecutor;
        let dsl = EdtDsl::new(script, dir.path().join("ws"), &runner as &dyn ProcessRunner);
        let result = dsl
            .validate_project(Path::new("/tmp/project"), &out_log)
            .expect("validate project");

        assert_eq!(result.process.exit_code, 2);
        assert_eq!(result.platform_log_path.as_deref(), Some(out_log.as_path()));
        assert_eq!(result.platform_log.as_deref(), Some("line1\nline2\n"));
        assert!(result.platform_log_read_error.is_none());
        let args = fs::read_to_string(args_log).expect("args log");
        assert!(args.contains("-command"));
        assert!(args.contains("validate"));
        assert!(args.contains("--file"));
        assert!(args.contains("--project-list"));
    }

    #[cfg(unix)]
    #[test]
    fn validate_project_keeps_process_result_when_out_log_is_unreadable() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cedtcli");
        write_script(&script, "exit 1");
        let out_log = dir.path().join("missing").join("validate.log");

        let runner = ProcessExecutor;
        let dsl = EdtDsl::new(script, dir.path().join("ws"), &runner as &dyn ProcessRunner);
        let result = dsl
            .validate_project(Path::new("/tmp/project"), &out_log)
            .expect("validate project");

        assert_eq!(result.process.exit_code, 1);
        assert_eq!(result.platform_log_path.as_deref(), Some(out_log.as_path()));
        assert!(result.platform_log.is_none());
        assert!(result
            .platform_log_read_error
            .as_deref()
            .expect("log read error")
            .contains("failed to read edt --file log"));
    }

    #[cfg(unix)]
    #[test]
    fn export_project_creates_workspace_before_spawn() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cedtcli");
        write_script(
            &script,
            "ws=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"-data\" ]; then ws=\"$arg\"; fi\n  prev=\"$arg\"\ndone\n[ -d \"$ws\" ] || exit 11\nexit 0",
        );
        let workspace = dir.path().join("missing").join("ws");

        let runner = ProcessExecutor;
        let dsl = EdtDsl::new(script, workspace.clone(), &runner as &dyn ProcessRunner);
        let result = dsl
            .export_project("project", Path::new("/tmp/out"))
            .expect("export project");

        assert_eq!(result.process.exit_code, 0);
        assert!(workspace.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn import_project_passes_expected_arguments() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cedtcli");
        let args_log = dir.path().join("args.log");
        write_script(
            &script,
            &format!("printf '%s\n' \"$@\" > \"{}\"\nexit 0", args_log.display()),
        );

        let runner = ProcessExecutor;
        let dsl = EdtDsl::new(script, dir.path().join("ws"), &runner as &dyn ProcessRunner);

        let result = dsl
            .import_project(Path::new("/tmp/project"))
            .expect("import project");

        assert_eq!(result.process.exit_code, 0);
        let args = fs::read_to_string(args_log).expect("args log");
        assert!(args.contains("-command"));
        assert!(args.contains("import"));
        assert!(args.contains("--project"));
        assert!(args.contains("/tmp/project"));
    }

    #[cfg(unix)]
    #[test]
    fn import_configuration_files_passes_expected_arguments() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cedtcli");
        let args_log = dir.path().join("args.log");
        write_script(
            &script,
            &format!("printf '%s\n' \"$@\" > \"{}\"\nexit 0", args_log.display()),
        );

        let runner = ProcessExecutor;
        let dsl = EdtDsl::new(script, dir.path().join("ws"), &runner as &dyn ProcessRunner);

        let result = dsl
            .import_configuration_files(
                Path::new("/tmp/project"),
                Path::new("/tmp/xml"),
                Some("8.3.24"),
                Some("BaseProject"),
                true,
            )
            .expect("import configuration files");

        assert_eq!(result.process.exit_code, 0);
        let args = fs::read_to_string(args_log).expect("args log");
        assert!(args.contains("-command"));
        assert!(args.contains("import"));
        assert!(args.contains("--configuration-files"));
        assert!(args.contains("/tmp/xml"));
        assert!(args.contains("--project"));
        assert!(args.contains("/tmp/project"));
        assert!(args.contains("--version"));
        assert!(args.contains("8.3.24"));
        assert!(args.contains("--base-project-name"));
        assert!(args.contains("BaseProject"));
        assert!(args.contains("--build"));
        assert!(args.contains("true"));
    }

    #[derive(Default)]
    struct RecordingRunner {
        timeout: Arc<Mutex<Option<Duration>>>,
    }

    impl ProcessRunner for RecordingRunner {
        fn run(&self, _request: &ProcessRequest) -> Result<ProcessResult, ProcessError> {
            Ok(ProcessResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
                interruption: None,
            })
        }

        fn run_with_timeout(
            &self,
            request: &ProcessRequest,
            timeout: Duration,
        ) -> Result<ProcessResult, ProcessError> {
            *self.timeout.lock().expect("timeout lock") = Some(timeout);
            self.run(request)
        }

        fn spawn(&self, _request: &ProcessRequest) -> Result<SpawnResult, ProcessError> {
            panic!("spawn must not be called in EDT DSL tests")
        }
    }

    #[test]
    fn validate_project_uses_configured_timeout() {
        let runner = RecordingRunner::default();
        let dsl = EdtDsl::new(
            PathBuf::from("/tmp/1cedtcli"),
            PathBuf::from("/tmp/ws"),
            &runner as &dyn ProcessRunner,
        )
        .with_timeout(Some(Duration::from_secs(7)));

        let _ = dsl.validate_project(Path::new("/tmp/project"), Path::new("/tmp/log"));

        let timeout = runner
            .timeout
            .lock()
            .expect("timeout lock")
            .expect("recorded timeout");
        assert!(timeout <= Duration::from_secs(7));
        assert!(timeout > Duration::from_secs(0));
    }

    #[test]
    fn interactive_validate_command_quotes_paths_with_spaces_and_quotes() {
        let command = render_interactive_validate_command(
            Path::new("/tmp/My \"Project\""),
            Path::new("/tmp/log dir/validate.log"),
        );

        assert_eq!(
            command,
            "validate --file \"/tmp/log dir/validate.log\" --project-list \"/tmp/My \\\"Project\\\"\""
        );
    }

    #[test]
    fn interactive_cd_commands_render_expected_forms() {
        assert_eq!(
            render_interactive_change_dir_command(Path::new("/tmp/work dir")),
            "cd \"/tmp/work dir\""
        );
        assert_eq!(render_interactive_probe_workdir_command(), "cd");
    }

    #[cfg(unix)]
    #[test]
    fn interactive_dsl_routes_export_and_import_through_live_session() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cedtcli");
        let command_log = dir.path().join("commands.log");
        let body = format!(
            "set -eu\n\
             prompt() {{ printf '1C:EDT>'; }}\n\
             prompt\n\
             while IFS= read -r line; do\n\
               printf '%s\\n' \"$line\" >> '{}'\n\
               eval \"set -- $line\"\n\
               cmd=\"${{1:-}}\"\n\
               if [ \"$#\" -gt 0 ]; then shift; fi\n\
               case \"$cmd\" in\n\
                 cd)\n\
                   prompt\n\
                   ;;\n\
                 export)\n\
                   prompt\n\
                   ;;\n\
                 import)\n\
                   prompt\n\
                   ;;\n\
                 *)\n\
                   printf 'unknown:%s\\n' \"$line\"\n\
                   prompt\n\
                   ;;\n\
               esac\n\
             done\n",
            command_log.display()
        );
        write_script(&script, &body);

        let dsl = EdtDsl::new_interactive(
            script,
            dir.path().join("ws"),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect("interactive dsl");

        dsl.export_project("project", Path::new("/tmp/out"))
            .expect("export");
        dsl.import_project(Path::new("/tmp/project"))
            .expect("import");

        let commands = fs::read_to_string(command_log).expect("command log");
        assert!(commands.contains("cd "));
        assert!(commands.contains("export --project-name project --configuration-files /tmp/out"));
        assert!(commands.contains("import --project /tmp/project"));
        assert!(!commands.contains("-command"));
    }

    #[cfg(unix)]
    #[test]
    fn shared_session_drop_does_not_shutdown_other_manager_owner() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cedtcli");
        let command_log = dir.path().join("commands.log");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        fs::create_dir_all(base.join("main")).expect("source dir");
        write_shared_interactive_script(&script, &command_log);
        let config = sample_shared_config(&base, &work, &script);
        let manager = Arc::new(
            EdtSessionManager::for_config(&config, EdtSessionHostOptions::for_cli_command(&config))
                .expect("manager"),
        );
        let workspace = work.join("edt-workspace");

        let first = EdtDsl::new_shared_session(
            script.clone(),
            workspace.clone(),
            manager.clone(),
            Duration::from_millis(config.tools.edt_cli.startup_timeout_ms),
            Duration::from_millis(config.tools.edt_cli.command_timeout_ms),
        )
        .expect("first dsl");
        let second = EdtDsl::new_shared_session(
            script,
            workspace,
            manager,
            Duration::from_millis(config.tools.edt_cli.startup_timeout_ms),
            Duration::from_millis(config.tools.edt_cli.command_timeout_ms),
        )
        .expect("second dsl");

        first
            .import_project(Path::new("/tmp/first-project"))
            .expect("first import");
        drop(first);

        second
            .import_project(Path::new("/tmp/second-project"))
            .expect("second import");

        let commands = fs::read_to_string(command_log).expect("command log");
        assert_eq!(commands.matches("START").count(), 1);
        assert!(commands.contains("import --project /tmp/first-project"));
        assert!(commands.contains("import --project /tmp/second-project"));
    }

    #[cfg(unix)]
    #[test]
    fn interactive_dsl_treats_exception_marker_as_command_failure() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cedtcli");
        let body = format!(
            "set -eu\n\
             prompt() {{ printf '1C:EDT>'; }}\n\
             prompt\n\
             while IFS= read -r line; do\n\
               eval \"set -- $line\"\n\
               cmd=\"${{1:-}}\"\n\
               if [ \"$#\" -gt 0 ]; then shift; fi\n\
                 case \"$cmd\" in\n\
                 cd)\n\
                   prompt\n\
                   ;;\n\
                 export)\n\
                   cat <<'OUT'\n\
edtsh: Invalid project description.\n\
/workspace/exts/client-mcp overlaps the location of another project: 'ClientMcp'\n\
{}\n\
OUT\n\
                   prompt\n\
                   ;;\n\
                 *)\n\
                   prompt\n\
                   ;;\n\
               esac\n\
             done\n",
            INTERACTIVE_EDT_ERROR_MARKER
        );
        write_script(&script, &body);

        let dsl = EdtDsl::new_interactive(
            script,
            dir.path().join("ws"),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect("interactive dsl");

        let result = dsl
            .export_project("project", Path::new("/tmp/out"))
            .expect("export result");

        assert_eq!(result.process.exit_code, 1);
        assert!(result.process.stdout.contains(INTERACTIVE_EDT_ERROR_MARKER));
    }

    #[cfg(unix)]
    #[test]
    fn interactive_dsl_honors_execution_policy_timeout() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cedtcli");
        write_script(
            &script,
            "set -eu\n\
             prompt() { printf '1C:EDT>'; }\n\
             prompt\n\
             while IFS= read -r line; do\n\
               eval \"set -- $line\"\n\
               cmd=\"${1:-}\"\n\
               if [ \"$#\" -gt 0 ]; then shift; fi\n\
               case \"$cmd\" in\n\
                 cd)\n\
                   prompt\n\
                   ;;\n\
                 export)\n\
                   sleep 0.2\n\
                   prompt\n\
                   ;;\n\
                 *)\n\
                   prompt\n\
                   ;;\n\
               esac\n\
             done\n",
        );

        let dsl = EdtDsl::new_interactive(
            script,
            dir.path().join("ws"),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect("interactive dsl")
        .with_execution_policy(ProcessExecutionPolicy::new(
            Some(Duration::from_millis(50)),
            CancellationToken::new(),
            ProcessInterruptionSafety::GracefulThenKill,
        ));

        let error = dsl
            .export_project("project", Path::new("/tmp/out"))
            .expect_err("timeout error");

        assert!(matches!(
            error,
            EdtError::Spawn(ProcessError::TimedOut { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn interactive_dsl_timeout_budget_covers_prelude_and_business_command() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cedtcli");
        write_script(
            &script,
            "set -eu\n\
             prompt() { printf '1C:EDT>'; }\n\
             prompt\n\
             while IFS= read -r line; do\n\
               eval \"set -- $line\"\n\
               cmd=\"${1:-}\"\n\
               if [ \"$#\" -gt 0 ]; then shift; fi\n\
               case \"$cmd\" in\n\
                 cd)\n\
                   sleep 0.07\n\
                   prompt\n\
                   ;;\n\
                 export)\n\
                   sleep 0.07\n\
                   prompt\n\
                   ;;\n\
                 *)\n\
                   prompt\n\
                   ;;\n\
               esac\n\
             done\n",
        );

        let dsl = EdtDsl::new_interactive(
            script,
            dir.path().join("ws"),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect("interactive dsl")
        .with_execution_policy(ProcessExecutionPolicy::new(
            Some(Duration::from_millis(100)),
            CancellationToken::new(),
            ProcessInterruptionSafety::GracefulThenKill,
        ));

        let error = dsl
            .export_project("project", Path::new("/tmp/out"))
            .expect_err("timeout error");

        match error {
            EdtError::Spawn(ProcessError::TimedOut { cmd, .. }) => {
                assert!(cmd.contains("[interactive: export]"));
                assert!(!cmd.contains("[interactive: cd]"));
            }
            other => panic!("expected timeout error, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn interactive_dsl_reused_session_shares_timeout_budget_across_commands() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cedtcli");
        write_script(
            &script,
            "set -eu\n\
             prompt() { printf '1C:EDT>'; }\n\
             prompt\n\
             while IFS= read -r line; do\n\
               eval \"set -- $line\"\n\
               cmd=\"${1:-}\"\n\
               if [ \"$#\" -gt 0 ]; then shift; fi\n\
               case \"$cmd\" in\n\
                 cd)\n\
                   prompt\n\
                   ;;\n\
                 export)\n\
                   sleep 0.07\n\
                   prompt\n\
                   ;;\n\
                 *)\n\
                   prompt\n\
                   ;;\n\
               esac\n\
             done\n",
        );

        let dsl = EdtDsl::new_interactive(
            script,
            dir.path().join("ws"),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect("interactive dsl")
        .with_execution_policy(ProcessExecutionPolicy::new(
            Some(Duration::from_millis(120)),
            CancellationToken::new(),
            ProcessInterruptionSafety::GracefulThenKill,
        ));

        dsl.export_project("first", Path::new("/tmp/out1"))
            .expect("first export");
        let error = dsl
            .export_project("second", Path::new("/tmp/out2"))
            .expect_err("second export must exhaust remaining budget");

        assert!(matches!(
            error,
            EdtError::Spawn(ProcessError::TimedOut { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn interactive_dsl_honors_execution_policy_cancellation() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cedtcli");
        write_script(
            &script,
            "set -eu\n\
             prompt() { printf '1C:EDT>'; }\n\
             prompt\n\
             while IFS= read -r line; do\n\
               eval \"set -- $line\"\n\
               cmd=\"${1:-}\"\n\
               if [ \"$#\" -gt 0 ]; then shift; fi\n\
               case \"$cmd\" in\n\
                 cd)\n\
                   prompt\n\
                   ;;\n\
                 export)\n\
                   sleep 0.2\n\
                   prompt\n\
                   ;;\n\
                 *)\n\
                   prompt\n\
                   ;;\n\
               esac\n\
             done\n",
        );

        let cancellation = CancellationToken::new();
        let delayed_cancel = cancellation.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            delayed_cancel.cancel();
        });

        let dsl = EdtDsl::new_interactive(
            script,
            dir.path().join("ws"),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect("interactive dsl")
        .with_execution_policy(ProcessExecutionPolicy::new(
            Some(Duration::from_secs(1)),
            cancellation,
            ProcessInterruptionSafety::GracefulThenKill,
        ));

        let error = dsl
            .export_project("project", Path::new("/tmp/out"))
            .expect_err("cancelled error");

        assert!(matches!(
            error,
            EdtError::Spawn(ProcessError::Cancelled { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn interactive_dsl_preserves_deferred_interruption_for_critical_phase() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cedtcli");
        write_script(
            &script,
            "set -eu\n\
             prompt() { printf '1C:EDT>'; }\n\
             prompt\n\
             while IFS= read -r line; do\n\
               eval \"set -- $line\"\n\
               cmd=\"${1:-}\"\n\
               if [ \"$#\" -gt 0 ]; then shift; fi\n\
               case \"$cmd\" in\n\
                 cd)\n\
                   prompt\n\
                   ;;\n\
                 export)\n\
                   sleep 0.1\n\
                   prompt\n\
                   ;;\n\
                 *)\n\
                   prompt\n\
                   ;;\n\
               esac\n\
             done\n",
        );

        let dsl = EdtDsl::new_interactive(
            script,
            dir.path().join("ws"),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect("interactive dsl")
        .with_execution_policy(ProcessExecutionPolicy::new(
            Some(Duration::from_millis(50)),
            CancellationToken::new(),
            ProcessInterruptionSafety::CriticalNonAbortable,
        ));

        let result = dsl
            .export_project("project", Path::new("/tmp/out"))
            .expect("deferred interruption result");

        assert_eq!(result.process.exit_code, 0);
        assert_eq!(
            result.process.interruption,
            Some(crate::platform::process::ProcessInterruption {
                reason: ProcessInterruptionReason::TimedOut,
                action: ProcessInterruptionAction::Deferred,
            })
        );
    }
}
