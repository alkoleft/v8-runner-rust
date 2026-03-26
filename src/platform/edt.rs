use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Duration;

use thiserror::Error;
use tracing::debug;

use crate::platform::interactive::{
    InteractiveProcessError, InteractiveProcessExecutor, InteractiveProcessRequest,
};
use crate::platform::process::{ProcessError, ProcessRequest, ProcessRunner};
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
}

/// Low-level DSL for invoking `1cedtcli`.
pub struct EdtDsl<'a> {
    binary: PathBuf,
    workspace: PathBuf,
    backend: EdtBackend<'a>,
    timeout: Option<Duration>,
}

impl<'a> EdtDsl<'a> {
    /// Create a new EDT DSL bound to one executable path and runner.
    pub fn new(binary: PathBuf, workspace: PathBuf, runner: &'a dyn ProcessRunner) -> Self {
        Self {
            binary,
            workspace,
            backend: EdtBackend::OneShot { runner },
            timeout: None,
        }
    }

    /// Create a new interactive EDT DSL that keeps a long-lived `1cedtcli` process alive.
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
        })
    }

    /// Overrides the timeout used for one-shot EDT process execution.
    pub const fn with_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }

    /// `-command export --project-name <source_name> --configuration-files <target>`
    pub fn export_project(
        &self,
        source: &Path,
        target: &Path,
    ) -> Result<PlatformCommandResult, EdtError> {
        let command_arguments = export_command_arguments(source, target);
        self.run(
            &process_arguments(&self.workspace, &command_arguments),
            &render_interactive_export_command(source, target),
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
                match self.timeout {
                    Some(timeout) => runner.run_with_timeout(&request, timeout),
                    None => runner.run(&request),
                }
                .map_err(EdtError::Spawn)?
            }
            EdtBackend::Interactive {
                session,
                command_timeout,
                ..
            } => {
                let effective_timeout = self.timeout.unwrap_or(*command_timeout);
                let mut session = session.borrow_mut();
                debug!(
                    workspace = %self.workspace.display(),
                    command = interactive_command,
                    timeout_ms = effective_timeout.as_millis() as u64,
                    "running interactive edt command"
                );
                session
                    .execute(
                        &render_interactive_change_dir_command(&self.workspace),
                        effective_timeout,
                    )
                    .map_err(EdtError::Interactive)?;
                let output = session
                    .execute(interactive_command, effective_timeout)
                    .map_err(EdtError::Interactive)?;
                debug!(
                    workspace = %self.workspace.display(),
                    command = interactive_command,
                    stdout = render_interactive_output_for_log(&output.stdout),
                    stderr = render_interactive_output_for_log(&output.stderr),
                    "interactive edt command finished"
                );
                let exit_code = if output_indicates_interactive_command_error(&output.stdout, &output.stderr) {
                    1
                } else {
                    0
                };
                crate::platform::process::ProcessResult {
                    exit_code,
                    stdout: output.stdout,
                    stderr: output.stderr,
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
}

impl Drop for EdtDsl<'_> {
    fn drop(&mut self) {
        if let EdtBackend::Interactive {
            session,
            shutdown_timeout,
            ..
        } = &self.backend
        {
            debug!(
                workspace = %self.workspace.display(),
                timeout_ms = shutdown_timeout.as_millis() as u64,
                "shutting down interactive edt session"
            );
            let _ = session.borrow_mut().shutdown(*shutdown_timeout);
        }
    }
}

enum EdtBackend<'a> {
    OneShot {
        runner: &'a dyn ProcessRunner,
    },
    Interactive {
        session: RefCell<InteractiveProcessExecutor>,
        command_timeout: Duration,
        shutdown_timeout: Duration,
    },
}

/// Renders an interactive `validate` command for the shared EDT session.
pub(crate) fn render_interactive_validate_command(source: &Path, out_log: &Path) -> String {
    render_interactive_command(&validate_command_arguments(source, out_log))
}

/// Renders an interactive `export` command.
pub(crate) fn render_interactive_export_command(source: &Path, target: &Path) -> String {
    render_interactive_command(&export_command_arguments(source, target))
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

fn export_command_arguments(source: &Path, target: &Path) -> Vec<String> {
    let project_name = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_else(|| source.as_os_str().to_str().unwrap_or_default());
    vec![
        "export".to_owned(),
        "--project-name".to_owned(),
        project_name.to_owned(),
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
        render_interactive_validate_command, EdtDsl, INTERACTIVE_EDT_ERROR_MARKER,
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
        assert!(args.contains("--project-name"));
        assert!(args.contains("project"));
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

        dsl.export_project(Path::new("/tmp/project"), Path::new("/tmp/out"))
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
            .export_project(Path::new("/tmp/project"), Path::new("/tmp/out"))
            .expect("export result");

        assert_eq!(result.process.exit_code, 1);
        assert!(result.process.stdout.contains(INTERACTIVE_EDT_ERROR_MARKER));
    }
}
