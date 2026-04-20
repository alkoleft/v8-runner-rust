use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::platform::connection::V8Connection;
use crate::platform::process::{ProcessError, ProcessRequest, ProcessRunner};
use crate::platform::result::PlatformCommandResult;

#[derive(Debug, Error)]
pub enum IbcmdError {
    #[error("ibcmd requires file-based infobase, got server connection")]
    ServerConnectionNotSupported,

    #[error("failed to execute ibcmd process: {0}")]
    Spawn(ProcessError),
}

#[derive(Debug, Clone)]
pub struct IbcmdConnection {
    database_path: PathBuf,
    user: Option<String>,
    password: Option<String>,
}

impl IbcmdConnection {
    pub fn from_v8_connection(conn: &V8Connection) -> Result<Self, IbcmdError> {
        let Some(database_path) = conn.file_path() else {
            return Err(IbcmdError::ServerConnectionNotSupported);
        };

        Ok(Self {
            database_path: PathBuf::from(database_path),
            user: conn.user.clone(),
            password: conn.password.clone(),
        })
    }

    pub fn args(&self) -> Vec<String> {
        let mut args = vec![format!("--database-path={}", self.database_path.display())];
        if let Some(user) = &self.user {
            args.push(format!("--user={user}"));
        }
        if let Some(password) = &self.password {
            if !password.is_empty() {
                args.push(format!("--password={password}"));
            }
        }
        args
    }
}

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

/// Low-level DSL for invoking `ibcmd`.
pub struct IbcmdDsl<'a> {
    binary: PathBuf,
    connection: IbcmdConnection,
    runner: &'a dyn ProcessRunner,
}

impl<'a> IbcmdDsl<'a> {
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

    pub fn config_import_full(
        &self,
        source_dir: &Path,
        extension: Option<&str>,
    ) -> Result<PlatformCommandResult, IbcmdError> {
        let mut args = vec!["config".to_owned(), "import".to_owned()];
        args.extend(self.base_args());
        if let Some(extension) = extension {
            args.push(format!("--extension={extension}"));
        }
        args.push(source_dir.display().to_string());
        self.run(&args)
    }

    pub fn infobase_create(&self) -> Result<PlatformCommandResult, IbcmdError> {
        let mut args = vec!["infobase".to_owned(), "create".to_owned()];
        args.extend(self.base_args());
        self.run(&args)
    }

    pub fn infobase_extension_update_properties(
        &self,
        name: &str,
        safe_mode: bool,
        unsafe_action_protection: bool,
    ) -> Result<PlatformCommandResult, IbcmdError> {
        let mut args = vec!["extension".to_owned(), "update".to_owned()];
        args.extend(self.base_args());
        args.push(format!("--name={name}"));
        args.push(format!(
            "--safe-mode={}",
            if safe_mode { "yes" } else { "no" }
        ));
        args.push(format!(
            "--unsafe-action-protection={}",
            if unsafe_action_protection {
                "yes"
            } else {
                "no"
            }
        ));
        self.run(&args)
    }

    pub fn config_import_partial(
        &self,
        base_dir: &Path,
        files: &[PathBuf],
        extension: Option<&str>,
    ) -> Result<PlatformCommandResult, IbcmdError> {
        let mut args = vec!["config".to_owned(), "import".to_owned(), "files".to_owned()];
        args.extend(self.base_args());
        if let Some(extension) = extension {
            args.push(format!("--extension={extension}"));
        }
        args.push("--partial".to_owned());
        args.push(format!("--base-dir={}", base_dir.display()));
        args.extend(files.iter().map(|path| path.display().to_string()));
        self.run(&args)
    }

    pub fn config_apply(
        &self,
        extension: Option<&str>,
        dynamic: DynamicUpdateMode,
    ) -> Result<PlatformCommandResult, IbcmdError> {
        let mut args = vec!["config".to_owned(), "apply".to_owned()];
        args.extend(self.base_args());
        if let Some(extension) = extension {
            args.push(format!("--extension={extension}"));
        }
        args.push(format!("--dynamic={}", dynamic.as_str()));
        self.run(&args)
    }

    pub fn config_export_full(
        &self,
        target_dir: &Path,
        extension: Option<&str>,
    ) -> Result<PlatformCommandResult, IbcmdError> {
        let mut args = vec!["config".to_owned(), "export".to_owned()];
        args.extend(self.base_args());
        if let Some(extension) = extension {
            args.push(format!("--extension={extension}"));
        }
        args.push("--force".to_owned());
        args.push(target_dir.display().to_string());
        self.run(&args)
    }

    pub fn config_export_incremental(
        &self,
        target_dir: &Path,
        extension: Option<&str>,
    ) -> Result<PlatformCommandResult, IbcmdError> {
        let mut args = vec!["config".to_owned(), "export".to_owned()];
        args.extend(self.base_args());
        if let Some(extension) = extension {
            args.push(format!("--extension={extension}"));
        }
        args.push("--sync".to_owned());
        args.push(target_dir.display().to_string());
        self.run(&args)
    }

    fn base_args(&self) -> Vec<String> {
        self.connection.args()
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

#[cfg(test)]
mod tests {
    use super::{DynamicUpdateMode, IbcmdConnection, IbcmdDsl, IbcmdError};
    use crate::platform::connection::V8Connection;
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

    #[test]
    fn ibcmd_connection_from_file_path() {
        let conn = V8Connection::from_connection_string("File=/tmp/ib");
        let ibcmd = IbcmdConnection::from_v8_connection(&conn).expect("connection");

        assert_eq!(ibcmd.args(), vec!["--database-path=/tmp/ib"]);
    }

    #[test]
    fn ibcmd_connection_from_server_fails() {
        let conn = V8Connection::from_connection_string("Srvr=demo;Ref=test");

        let err = IbcmdConnection::from_v8_connection(&conn).expect_err("expected error");

        assert!(matches!(err, IbcmdError::ServerConnectionNotSupported));
    }

    #[test]
    fn ibcmd_connection_includes_auth_args() {
        let mut conn = V8Connection::from_connection_string("File=/tmp/ib");
        conn.user = Some("admin".to_owned());
        conn.password = Some("secret".to_owned());

        let ibcmd = IbcmdConnection::from_v8_connection(&conn).expect("connection");

        assert_eq!(
            ibcmd.args(),
            vec![
                "--database-path=/tmp/ib",
                "--user=admin",
                "--password=secret"
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
        let conn =
            IbcmdConnection::from_v8_connection(&V8Connection::from_connection_string("File=/ib"))
                .expect("connection");
        let dsl = IbcmdDsl::new(script, conn, &runner as &dyn ProcessRunner);

        dsl.config_import_full(dir.path(), Some("Ext"))
            .expect("import");

        let args = fs::read_to_string(args_log).expect("args");
        assert!(args.contains("config"));
        assert!(args.contains("import"));
        assert!(args.contains("--database-path=/ib"));
        assert!(args.contains("--extension=Ext"));
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
        let conn =
            IbcmdConnection::from_v8_connection(&V8Connection::from_connection_string("File=/ib"))
                .expect("connection");
        let dsl = IbcmdDsl::new(script, conn, &runner as &dyn ProcessRunner);
        let files = vec![PathBuf::from("Catalogs/Items.xml")];

        dsl.config_import_partial(dir.path(), &files, None)
            .expect("import");

        let args = fs::read_to_string(args_log).expect("args");
        assert!(args.contains("import"));
        assert!(args.contains("files"));
        assert!(args.contains("--partial"));
        assert!(args.contains("--base-dir="));
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
        let conn =
            IbcmdConnection::from_v8_connection(&V8Connection::from_connection_string("File=/ib"))
                .expect("connection");
        let dsl = IbcmdDsl::new(script, conn, &runner as &dyn ProcessRunner);

        dsl.config_apply(None, DynamicUpdateMode::Auto)
            .expect("apply");

        let args = fs::read_to_string(args_log).expect("args");
        assert!(args.contains("apply"));
        assert!(args.contains("--dynamic=auto"));
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
        let conn =
            IbcmdConnection::from_v8_connection(&V8Connection::from_connection_string("File=/ib"))
                .expect("connection");
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
            let conn = IbcmdConnection::from_v8_connection(&V8Connection::from_connection_string(
                "File=/ib",
            ))
            .expect("connection");
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
        let conn =
            IbcmdConnection::from_v8_connection(&V8Connection::from_connection_string("File=/ib"))
                .expect("connection");
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
        let conn =
            IbcmdConnection::from_v8_connection(&V8Connection::from_connection_string("File=/ib"))
                .expect("connection");
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
        let conn =
            IbcmdConnection::from_v8_connection(&V8Connection::from_connection_string("File=/ib"))
                .expect("connection");
        let dsl = IbcmdDsl::new(script, conn, &runner as &dyn ProcessRunner);

        dsl.infobase_create().expect("create");

        let args = fs::read_to_string(args_log).expect("args");
        assert!(args.contains("infobase"));
        assert!(args.contains("create"));
        assert!(args.contains("--database-path=/ib"));
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
        let conn =
            IbcmdConnection::from_v8_connection(&V8Connection::from_connection_string("File=/ib"))
                .expect("connection");
        let dsl = IbcmdDsl::new(script, conn, &runner as &dyn ProcessRunner);

        dsl.infobase_extension_update_properties("client_mcp", false, false)
            .expect("update");

        let args = fs::read_to_string(args_log).expect("args");
        assert!(args.contains("extension"));
        assert!(args.contains("update"));
        assert!(args.contains("--name=client_mcp"));
        assert!(args.contains("--safe-mode=no"));
        assert!(args.contains("--unsafe-action-protection=no"));
    }
}
