use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::config::model::InfobaseConfig;
use crate::platform::connection::V8Connection;
use crate::platform::process::{ProcessError, ProcessRequest, ProcessRunner};
use crate::platform::result::PlatformCommandResult;

#[derive(Debug, Error)]
pub enum IbcmdError {
    #[error("server-based IBCMD connection requires infobase.dbms.{0}")]
    MissingServerDbmsField(&'static str),

    #[error("failed to execute ibcmd process: {0}")]
    Spawn(ProcessError),
}

/// Connection contract passed to `ibcmd infobase ...` commands.
#[derive(Debug, Clone)]
pub enum IbcmdConnection {
    File {
        database_path: PathBuf,
        user: Option<String>,
        password: Option<String>,
    },
    Server {
        dbms_kind: String,
        database_server: String,
        database_name: String,
        user: Option<String>,
        password: Option<String>,
        database_user: Option<String>,
        database_password: Option<String>,
    },
}

impl IbcmdConnection {
    /// Maps the public `infobase` config contract into `ibcmd` arguments.
    pub fn from_infobase(infobase: &InfobaseConfig) -> Result<Self, IbcmdError> {
        let conn = V8Connection::from_connection_string(&infobase.connection);
        let Some(database_path) = conn.file_path() else {
            let Some(dbms) = infobase.dbms.as_ref() else {
                return Err(IbcmdError::MissingServerDbmsField("kind"));
            };

            return Ok(Self::Server {
                dbms_kind: required_dbms_field("kind", dbms.kind.as_deref())?,
                database_server: required_dbms_field("server", dbms.server.as_deref())?,
                database_name: required_dbms_field("name", dbms.name.as_deref())?,
                user: infobase.user.clone(),
                password: infobase.password.clone(),
                database_user: dbms.user.clone(),
                database_password: dbms.password.clone(),
            });
        };

        Ok(Self::File {
            database_path: PathBuf::from(database_path),
            user: infobase.user.clone(),
            password: infobase.password.clone(),
        })
    }

    #[cfg(test)]
    fn args(&self) -> Vec<String> {
        let mut args = self.infobase_args();
        args.extend(self.auth_args());
        args.extend(self.dbms_auth_args());
        args
    }

    fn infobase_args(&self) -> Vec<String> {
        match self {
            Self::File { database_path, .. } => {
                let mut args = Vec::new();
                // 8.3.20 accepts only this short alias and expects it before nested commands.
                push_option_value(&mut args, "--db-path", database_path.display().to_string());
                args
            }
            Self::Server {
                dbms_kind,
                database_server,
                database_name,
                ..
            } => {
                let mut args = Vec::new();
                push_option_value(&mut args, "--dbms", dbms_kind);
                push_option_value(&mut args, "--database-server", database_server);
                push_option_value(&mut args, "--database-name", database_name);
                args
            }
        }
    }

    fn auth_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        let (user, password) = match self {
            Self::File { user, password, .. } | Self::Server { user, password, .. } => {
                (user, password)
            }
        };
        if let Some(user) = user {
            push_option_value(&mut args, "--user", user);
        }
        if let Some(password) = password {
            if !password.is_empty() {
                push_option_value(&mut args, "--password", password);
            }
        }
        args
    }

    fn dbms_auth_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Self::Server {
            database_user,
            database_password,
            ..
        } = self
        {
            if let Some(user) = database_user {
                if !user.trim().is_empty() {
                    push_option_value(&mut args, "--database-user", user);
                }
            }
            if let Some(password) = database_password {
                if !password.is_empty() {
                    push_option_value(&mut args, "--database-password", password);
                }
            }
        }
        args
    }
}

/// Dynamic apply mode supported by `ibcmd config apply`.
#[derive(Debug, Clone, Copy)]
pub enum DynamicUpdateMode {
    Auto,
}

impl DynamicUpdateMode {
    fn as_str(self) -> &'static str {
        match self {
            DynamicUpdateMode::Auto => "auto",
        }
    }
}

/// Result status returned by `ibcmd infobase create`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IbcmdInfobaseCreateStatus {
    Created,
    AlreadyExists,
    Failed,
}

/// Normalized outcome for infobase creation with the raw platform payload preserved.
#[derive(Debug)]
pub struct IbcmdInfobaseCreateOutcome {
    pub status: IbcmdInfobaseCreateStatus,
    pub result: PlatformCommandResult,
}

/// Low-level DSL for invoking `ibcmd`.
pub struct IbcmdDsl<'a> {
    binary: PathBuf,
    connection: IbcmdConnection,
    runner: &'a dyn ProcessRunner,
}

impl<'a> IbcmdDsl<'a> {
    /// Creates a new DSL bound to a resolved `ibcmd` binary and target infobase.
    pub fn new(
        binary: PathBuf,
        connection: IbcmdConnection,
        runner: &'a dyn ProcessRunner,
    ) -> Self {
        Self {
            binary,
            connection,
            runner,
        }
    }

    /// Imports a full configuration or extension snapshot into the target infobase.
    pub fn config_import_full(
        &self,
        source_dir: &Path,
        extension: Option<&str>,
    ) -> Result<PlatformCommandResult, IbcmdError> {
        let mut args = self.authenticated_infobase_args(&["config", "import"]);
        if let Some(extension) = extension {
            push_option_value(&mut args, "--extension", extension);
        }
        args.push(source_dir.display().to_string());
        self.run(&args)
    }

    /// Ensures the infobase exists and normalizes benign "already exists" outcomes.
    pub fn ensure_infobase_create(&self) -> Result<IbcmdInfobaseCreateOutcome, IbcmdError> {
        let args = self.create_infobase_args();
        let result = self.run(&args)?;
        let status = if result.process.exit_code == 0 {
            IbcmdInfobaseCreateStatus::Created
        } else if is_benign_already_exists(
            &result.process.stdout,
            &result.process.stderr,
        ) {
            IbcmdInfobaseCreateStatus::AlreadyExists
        } else {
            IbcmdInfobaseCreateStatus::Failed
        };

        Ok(IbcmdInfobaseCreateOutcome { status, result })
    }

    /// Updates extension security properties in the target infobase.
    pub fn infobase_extension_update_properties(
        &self,
        name: &str,
        safe_mode: bool,
        unsafe_action_protection: bool,
    ) -> Result<PlatformCommandResult, IbcmdError> {
        let mut args = self.authenticated_infobase_args(&["config", "extension", "update"]);
        push_option_value(&mut args, "--name", name);
        push_option_value(
            &mut args,
            "--safe-mode",
            if safe_mode { "yes" } else { "no" },
        );
        push_option_value(
            &mut args,
            "--unsafe-action-protection",
            if unsafe_action_protection {
                "yes"
            } else {
                "no"
            },
        );
        self.run(&args)
    }

    /// Imports a partial file list into the target infobase.
    pub fn config_import_partial(
        &self,
        base_dir: &Path,
        files: &[PathBuf],
        extension: Option<&str>,
    ) -> Result<PlatformCommandResult, IbcmdError> {
        let mut args = self.authenticated_infobase_args(&["config", "import", "files"]);
        if let Some(extension) = extension {
            push_option_value(&mut args, "--extension", extension);
        }
        args.push("--partial".to_owned());
        push_option_value(&mut args, "--base-dir", base_dir.display().to_string());
        args.extend(files.iter().map(|path| path.display().to_string()));
        self.run(&args)
    }

    /// Applies imported configuration changes to the infobase.
    pub fn config_apply(
        &self,
        extension: Option<&str>,
        dynamic: DynamicUpdateMode,
    ) -> Result<PlatformCommandResult, IbcmdError> {
        let mut args = self.authenticated_infobase_args(&["config", "apply"]);
        if let Some(extension) = extension {
            push_option_value(&mut args, "--extension", extension);
        }
        args.push("--force".to_owned());
        push_option_value(&mut args, "--dynamic", dynamic.as_str());
        self.run(&args)
    }

    /// Exports a full configuration or extension snapshot from the infobase.
    pub fn config_export_full(
        &self,
        target_dir: &Path,
        extension: Option<&str>,
    ) -> Result<PlatformCommandResult, IbcmdError> {
        let mut args = self.authenticated_infobase_args(&["config", "export"]);
        if let Some(extension) = extension {
            push_option_value(&mut args, "--extension", extension);
        }
        args.push("--force".to_owned());
        args.push(target_dir.display().to_string());
        self.run(&args)
    }

    /// Exports changes in sync mode relative to an existing target directory.
    pub fn config_export_incremental(
        &self,
        target_dir: &Path,
        extension: Option<&str>,
    ) -> Result<PlatformCommandResult, IbcmdError> {
        let mut args = self.authenticated_infobase_args(&["config", "export"]);
        if let Some(extension) = extension {
            push_option_value(&mut args, "--extension", extension);
        }
        args.push("--sync".to_owned());
        args.push(target_dir.display().to_string());
        self.run(&args)
    }

    fn base_args(&self) -> Vec<String> {
        self.connection.infobase_args()
    }

    fn infobase_args(&self, command: &[&str]) -> Vec<String> {
        let mut args = vec!["infobase".to_owned()];
        args.extend(self.base_args());
        args.extend(command.iter().map(|part| (*part).to_owned()));
        args
    }

    fn authenticated_infobase_args(&self, command: &[&str]) -> Vec<String> {
        let mut args = self.infobase_args(command);
        args.extend(self.connection.auth_args());
        args.extend(self.connection.dbms_auth_args());
        args
    }

    fn create_infobase_args(&self) -> Vec<String> {
        let mut args = self.infobase_args(&["create"]);
        if matches!(self.connection, IbcmdConnection::Server { .. }) {
            args.push("--create-database".to_owned());
        }
        args.extend(self.connection.auth_args());
        args.extend(self.connection.dbms_auth_args());
        args
    }

    fn run(&self, args: &[String]) -> Result<PlatformCommandResult, IbcmdError> {
        let process = self
            .runner
            .run(&ProcessRequest {
                program: self.binary.clone(),
                args: args.to_vec(),
                workdir: None,
                stdout_log_path: None,
                stderr_log_path: None,
                startup_probe: None,
            })
            .map_err(IbcmdError::Spawn)?;

        Ok(PlatformCommandResult {
            process,
            platform_log_path: None,
            platform_log: None,
            platform_log_read_error: None,
        })
    }
}

fn push_option_value(args: &mut Vec<String>, key: &str, value: impl ToString) {
    args.push(key.to_owned());
    args.push(value.to_string());
}

fn required_dbms_field(field: &'static str, value: Option<&str>) -> Result<String, IbcmdError> {
    match value.map(str::trim) {
        Some(value) if !value.is_empty() => Ok(value.to_owned()),
        _ => Err(IbcmdError::MissingServerDbmsField(field)),
    }
}

fn is_benign_already_exists(stdout: &str, stderr: &str) -> bool {
    let combined = format!("{stdout}\n{stderr}").to_lowercase();
    if combined.trim().is_empty() {
        return false;
    }

    const FATAL_PATTERNS: &[&str] = &[
        "access denied",
        "authentication",
        "permission denied",
        "timeout",
        "connection refused",
        "network",
        "ошибка авторизации",
        "доступ запрещен",
        "доступ запрещён",
        "недостаточно прав",
        "не удалось подключ",
        "таймаут",
    ];
    if FATAL_PATTERNS.iter().any(|pattern| combined.contains(pattern)) {
        return false;
    }

    const BENIGN_PATTERNS: &[&str] = &["already exists", "уже существует"];
    combined
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .any(|line| BENIGN_PATTERNS.iter().any(|pattern| line.contains(pattern)))
}

#[cfg(test)]
mod tests {
    use super::{
        is_benign_already_exists, DynamicUpdateMode, IbcmdConnection, IbcmdDsl,
        IbcmdInfobaseCreateStatus,
    };
    use crate::config::model::{InfobaseConfig, InfobaseDbmsConfig};
    use crate::platform::process::{ProcessExecutor, ProcessRunner};
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
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
        let staged = path.with_extension("tmp");
        let mut file = fs::File::create(&staged).expect("create script");
        file.write_all(format!("#!/bin/sh\n{body}\n").as_bytes())
            .expect("write script");
        file.sync_all().expect("sync script");
        drop(file);
        make_executable(&staged);
        fs::rename(&staged, path).expect("rename script");
    }

    fn file_connection(path: &str) -> IbcmdConnection {
        IbcmdConnection::from_infobase(&InfobaseConfig::file(path)).expect("connection")
    }

    #[test]
    fn ibcmd_connection_from_file_path() {
        let ibcmd = file_connection("File=/tmp/ib");

        assert_eq!(ibcmd.args(), vec!["--db-path", "/tmp/ib"]);
    }

    #[test]
    fn ibcmd_connection_from_server_uses_dbms_contract() {
        let ibcmd = IbcmdConnection::from_infobase(&InfobaseConfig::server(
            "Srvr=demo;Ref=test",
            InfobaseDbmsConfig::new("PostgreSQL", "localhost", "demo")
                .with_credentials(Some("postgres".to_owned()), Some("secret".to_owned())),
        ))
        .expect("connection");

        assert_eq!(
            ibcmd.args(),
            vec![
                "--dbms",
                "PostgreSQL",
                "--database-server",
                "localhost",
                "--database-name",
                "demo",
                "--database-user",
                "postgres",
                "--database-password",
                "secret"
            ]
        );
    }

    #[test]
    fn ibcmd_connection_includes_auth_args() {
        let ibcmd = IbcmdConnection::from_infobase(
            &InfobaseConfig::file("File=/tmp/ib")
                .with_credentials(Some("admin".to_owned()), Some("secret".to_owned())),
        )
        .expect("connection");

        assert_eq!(
            ibcmd.args(),
            vec![
                "--db-path",
                "/tmp/ib",
                "--user",
                "admin",
                "--password",
                "secret"
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn config_import_full_builds_expected_args() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("ibcmd");
        let args_log = dir.path().join("args.log");
        write_script(
            &script,
            &format!("printf '%s\\n' \"$@\" > \"{}\"\nexit 0", args_log.display()),
        );
        let runner = ProcessExecutor;
        let conn = file_connection("File=/ib");
        let dsl = IbcmdDsl::new(script, conn, &runner as &dyn ProcessRunner);

        dsl.config_import_full(dir.path(), Some("Ext"))
            .expect("import");

        let args = fs::read_to_string(args_log).expect("args");
        assert!(args.contains("config"));
        assert!(args.contains("import"));
        assert!(args.contains("infobase\n--db-path\n/ib\nconfig\nimport"));
        assert!(args.contains("--extension\nExt"));
    }

    #[cfg(unix)]
    #[test]
    fn config_import_partial_builds_expected_args() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("ibcmd");
        let args_log = dir.path().join("args.log");
        write_script(
            &script,
            &format!("printf '%s\\n' \"$@\" > \"{}\"\nexit 0", args_log.display()),
        );
        let runner = ProcessExecutor;
        let conn = file_connection("File=/ib");
        let dsl = IbcmdDsl::new(script, conn, &runner as &dyn ProcessRunner);
        let files = vec![PathBuf::from("Catalogs/Items.xml")];

        dsl.config_import_partial(dir.path(), &files, None)
            .expect("import");

        let args = fs::read_to_string(args_log).expect("args");
        assert!(args.contains("import"));
        assert!(args.contains("files"));
        assert!(args.contains("--partial"));
        assert!(args.contains("--base-dir\n"));
        assert!(args.contains("Catalogs/Items.xml"));
    }

    #[cfg(unix)]
    #[test]
    fn config_apply_builds_expected_args() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("ibcmd");
        let args_log = dir.path().join("args.log");
        write_script(
            &script,
            &format!("printf '%s\\n' \"$@\" > \"{}\"\nexit 0", args_log.display()),
        );
        let runner = ProcessExecutor;
        let conn = file_connection("File=/ib");
        let dsl = IbcmdDsl::new(script, conn, &runner as &dyn ProcessRunner);

        dsl.config_apply(None, DynamicUpdateMode::Auto)
            .expect("apply");

        let args = fs::read_to_string(args_log).expect("args");
        assert!(args.contains("apply"));
        assert!(args.contains("--force"));
        assert!(args.contains("--dynamic\nauto"));
    }

    #[cfg(unix)]
    #[test]
    fn config_export_full_builds_expected_args() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("ibcmd");
        let args_log = dir.path().join("args.log");
        write_script(
            &script,
            &format!("printf '%s\\n' \"$@\" > \"{}\"\nexit 0", args_log.display()),
        );
        let runner = ProcessExecutor;
        let conn = file_connection("File=/ib");
        let dsl = IbcmdDsl::new(script, conn, &runner as &dyn ProcessRunner);

        dsl.config_export_full(dir.path(), None).expect("export");

        let args = fs::read_to_string(args_log).expect("args");
        assert!(args.contains("export"));
        assert!(args.contains("--force"));
    }

    #[cfg(unix)]
    #[test]
    fn config_export_full_repeatedly_executes_fresh_script_without_etxtbsy() {
        for _ in 0..32 {
            let dir = tempdir().expect("tempdir");
            let script = dir.path().join("ibcmd");
            let args_log = dir.path().join("args.log");
            write_script(
                &script,
                &format!("printf '%s\\n' \"$@\" > \"{}\"\nexit 0", args_log.display()),
            );
            let runner = ProcessExecutor;
            let conn = file_connection("File=/ib");
            let dsl = IbcmdDsl::new(script, conn, &runner as &dyn ProcessRunner);

            dsl.config_export_full(dir.path(), None).expect("export");

            let args = fs::read_to_string(args_log).expect("args");
            assert!(args.contains("export"));
            assert!(args.contains("--force"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn config_export_incremental_builds_expected_args() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("ibcmd");
        let args_log = dir.path().join("args.log");
        write_script(
            &script,
            &format!("printf '%s\\n' \"$@\" > \"{}\"\nexit 0", args_log.display()),
        );
        let runner = ProcessExecutor;
        let conn = file_connection("File=/ib");
        let dsl = IbcmdDsl::new(script, conn, &runner as &dyn ProcessRunner);

        dsl.config_export_incremental(dir.path(), None)
            .expect("export");

        let args = fs::read_to_string(args_log).expect("args");
        assert!(args.contains("export"));
        assert!(args.contains("--sync"));
    }

    #[cfg(unix)]
    #[test]
    fn run_returns_stdout_stderr_without_platform_log() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("ibcmd");
        write_script(&script, "echo out; echo err 1>&2; exit 7");
        let runner = ProcessExecutor;
        let conn = file_connection("File=/ib");
        let dsl = IbcmdDsl::new(script, conn, &runner as &dyn ProcessRunner);

        let result = dsl
            .config_apply(None, DynamicUpdateMode::Auto)
            .expect("apply");

        assert_eq!(result.process.exit_code, 7);
        assert_eq!(result.process.stdout.trim(), "out");
        assert_eq!(result.process.stderr.trim(), "err");
        assert!(result.platform_log_path.is_none());
        assert!(result.platform_log.is_none());
        assert!(result.platform_log_read_error.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn infobase_create_builds_expected_args() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("ibcmd");
        let args_log = dir.path().join("args.log");
        write_script(
            &script,
            &format!("printf '%s\\n' \"$@\" > \"{}\"\nexit 0", args_log.display()),
        );
        let runner = ProcessExecutor;
        let conn = file_connection("File=/ib");
        let dsl = IbcmdDsl::new(script, conn, &runner as &dyn ProcessRunner);

        let outcome = dsl.ensure_infobase_create().expect("create");

        let args = fs::read_to_string(args_log).expect("args");
        assert_eq!(outcome.status, IbcmdInfobaseCreateStatus::Created);
        assert!(args.contains("infobase"));
        assert!(args.contains("create"));
        assert!(args.contains("infobase\n--db-path\n/ib\ncreate"));
    }

    #[cfg(unix)]
    #[test]
    fn server_infobase_create_adds_create_database_and_normalizes_already_exists() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("ibcmd");
        let args_log = dir.path().join("args.log");
        write_script(
            &script,
            &format!(
                "printf '%s\\n' \"$@\" > \"{}\"\nprintf 'already exists\\n' >&2\nexit 17",
                args_log.display()
            ),
        );
        let runner = ProcessExecutor;
        let conn = IbcmdConnection::from_infobase(&InfobaseConfig::server(
            "Srvr=demo;Ref=test",
            InfobaseDbmsConfig::new("PostgreSQL", "localhost", "demo")
                .with_credentials(Some("postgres".to_owned()), Some("secret".to_owned())),
        ))
        .expect("connection");
        let dsl = IbcmdDsl::new(script, conn, &runner as &dyn ProcessRunner);

        let outcome = dsl.ensure_infobase_create().expect("ensure");

        assert_eq!(outcome.status, IbcmdInfobaseCreateStatus::AlreadyExists);
        let args = fs::read_to_string(args_log).expect("args");
        assert!(args.contains("--create-database"));
        assert!(args.contains("--dbms\nPostgreSQL"));
        assert!(args.contains("--database-server\nlocalhost"));
        assert!(args.contains("--database-name\ndemo"));
        assert!(args.contains("--database-user\npostgres"));
        assert!(args.contains("--database-password\nsecret"));
    }

    #[test]
    fn already_exists_detection_handles_uppercase_russian_messages() {
        assert!(is_benign_already_exists("", "УЖЕ СУЩЕСТВУЕТ"));
    }

    #[test]
    fn already_exists_detection_keeps_uppercase_russian_auth_failures_fatal() {
        assert!(!is_benign_already_exists("", "ОШИБКА АВТОРИЗАЦИИ: УЖЕ СУЩЕСТВУЕТ"));
    }

    #[cfg(unix)]
    #[test]
    fn infobase_extension_update_properties_builds_expected_args() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("ibcmd");
        let args_log = dir.path().join("args.log");
        write_script(
            &script,
            &format!("printf '%s\\n' \"$@\" > \"{}\"\nexit 0", args_log.display()),
        );
        let runner = ProcessExecutor;
        let conn = file_connection("File=/ib");
        let dsl = IbcmdDsl::new(script, conn, &runner as &dyn ProcessRunner);

        dsl.infobase_extension_update_properties("client_mcp", false, false)
            .expect("update");

        let args = fs::read_to_string(args_log).expect("args");
        assert!(args.contains("extension"));
        assert!(args.contains("update"));
        assert!(args.contains("infobase\n--db-path\n/ib\nconfig\nextension\nupdate"));
        assert!(args.contains("--name\nclient_mcp"));
        assert!(args.contains("--safe-mode\nno"));
        assert!(args.contains("--unsafe-action-protection\nno"));
    }
}
