use std::path::{Path, PathBuf};
use std::time::Duration;

use thiserror::Error;

use crate::platform::process::{ProcessError, ProcessRequest, ProcessRunner};
use crate::platform::result::PlatformCommandResult;

#[derive(Debug, Error)]
pub enum EdtError {
    #[error("failed to prepare edt workspace '{path}': {source}")]
    PrepareWorkspace {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to execute edt process: {0}")]
    Spawn(ProcessError),
}

/// Low-level DSL for invoking `1cedtcli`.
pub struct EdtDsl<'a> {
    binary: PathBuf,
    workspace: PathBuf,
    runner: &'a dyn ProcessRunner,
    timeout: Option<Duration>,
}

impl<'a> EdtDsl<'a> {
    /// Create a new EDT DSL bound to one executable path and runner.
    pub fn new(binary: PathBuf, workspace: PathBuf, runner: &'a dyn ProcessRunner) -> Self {
        Self {
            binary,
            workspace,
            runner,
            timeout: None,
        }
    }

    /// Overrides the timeout used for one-shot EDT process execution.
    pub const fn with_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }

    /// `-command export --project <source> --configuration-files <target>`
    pub fn export_project(
        &self,
        source: &Path,
        target: &Path,
    ) -> Result<PlatformCommandResult, EdtError> {
        let args = process_arguments(&self.workspace, &export_command_arguments(source, target));
        self.run(&args, None)
    }

    /// `-command validate --file <out_log> --project-list <source>`
    pub fn validate_project(
        &self,
        source: &Path,
        out_log: &Path,
    ) -> Result<PlatformCommandResult, EdtError> {
        let args = process_arguments(
            &self.workspace,
            &validate_command_arguments(source, out_log),
        );
        self.run(&args, Some(out_log))
    }

    fn run(
        &self,
        args: &[String],
        out_log: Option<&Path>,
    ) -> Result<PlatformCommandResult, EdtError> {
        std::fs::create_dir_all(&self.workspace).map_err(|source| EdtError::PrepareWorkspace {
            path: self.workspace.clone(),
            source,
        })?;

        if let Some(path) = out_log {
            let _ = std::fs::remove_file(path);
        }

        let request = ProcessRequest {
            program: self.binary.clone(),
            args: args.to_vec(),
            workdir: None,
            stdout_log_path: None,
            stderr_log_path: None,
            startup_probe: None,
        };
        let process = match self.timeout {
            Some(timeout) => self.runner.run_with_timeout(&request, timeout),
            None => self.runner.run(&request),
        }
        .map_err(EdtError::Spawn)?;

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
}

/// Renders an interactive `validate` command for the shared EDT session.
pub(crate) fn render_interactive_validate_command(source: &Path, out_log: &Path) -> String {
    render_interactive_command(&validate_command_arguments(source, out_log))
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

fn export_command_arguments(source: &Path, target: &Path) -> Vec<String> {
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

fn render_interactive_command(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|argument| quote_interactive_argument(argument))
        .collect::<Vec<_>>()
        .join(" ")
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
        render_interactive_validate_command, EdtDsl,
    };
    use crate::platform::process::{
        ProcessError, ProcessExecutor, ProcessRequest, ProcessResult, ProcessRunner, SpawnResult,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
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
        fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        make_executable(path);
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
            .export_project(Path::new("/tmp/project"), Path::new("/tmp/out"))
            .expect("export project");

        assert_eq!(result.process.exit_code, 0);
        assert!(result.platform_log.is_none());
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
            .export_project(Path::new("/tmp/project"), Path::new("/tmp/out"))
            .expect("export project");

        assert_eq!(result.process.exit_code, 0);
        assert!(workspace.is_dir());
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
                stdout_log_path: None,
                stderr_log_path: None,
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

        assert_eq!(
            *runner.timeout.lock().expect("timeout lock"),
            Some(Duration::from_secs(7))
        );
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
}
