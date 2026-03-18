use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::platform::connection::V8Connection;
use crate::platform::process::{ProcessError, ProcessRequest, ProcessRunner};
use crate::platform::result::PlatformCommandResult;

#[derive(Debug, Error)]
pub enum DesignerError {
    #[error("designer utility not found: {0}")]
    UtilityNotFound(String),

    #[error("failed to execute designer process: {0}")]
    Spawn(ProcessError),

    #[error("failed to read designer /Out log '{path}': {source}")]
    LogRead {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Low-level DSL for invoking `1cv8` in `DESIGNER` mode.
pub struct DesignerDsl<'a> {
    binary: PathBuf,
    connection: V8Connection,
    runner: &'a dyn ProcessRunner,
    /// Optional file where Designer should write its `/Out` log.
    log_file: Option<PathBuf>,
}

impl<'a> DesignerDsl<'a> {
    /// Create a new Designer DSL bound to one executable path and runner.
    pub fn new(
        binary: PathBuf,
        connection: V8Connection,
        runner: &'a dyn ProcessRunner,
        log_file: Option<PathBuf>,
    ) -> Self {
        Self {
            binary,
            connection,
            runner,
            log_file,
        }
    }

    /// `/LoadConfigFromFiles <dir> -updateConfigDumpInfo`
    pub fn load_config_from_files_full(
        &self,
        source_dir: &Path,
        extension: Option<&str>,
    ) -> Result<PlatformCommandResult, DesignerError> {
        let mut args = self.base_args();
        args.push("/LoadConfigFromFiles".to_owned());
        args.push(source_dir.display().to_string());
        args.push("-updateConfigDumpInfo".to_owned());
        if let Some(extension) = extension {
            args.push("-Extension".to_owned());
            args.push(extension.to_owned());
        }
        self.run(&args)
    }

    /// `/LoadConfigFromFiles <dir> -partial -listFile <list_file> -updateConfigDumpInfo`
    pub fn load_config_from_files_partial(
        &self,
        source_dir: &Path,
        list_file: &Path,
        extension: Option<&str>,
    ) -> Result<PlatformCommandResult, DesignerError> {
        let mut args = self.base_args();
        args.push("/LoadConfigFromFiles".to_owned());
        args.push(source_dir.display().to_string());
        args.push("-partial".to_owned());
        args.push("-listFile".to_owned());
        args.push(list_file.display().to_string());
        args.push("-updateConfigDumpInfo".to_owned());
        if let Some(extension) = extension {
            args.push("-Extension".to_owned());
            args.push(extension.to_owned());
        }
        self.run(&args)
    }

    /// `/UpdateDBCfg`
    pub fn update_db_cfg(
        &self,
        extension: Option<&str>,
    ) -> Result<PlatformCommandResult, DesignerError> {
        let mut args = self.base_args();
        args.push("/UpdateDBCfg".to_owned());
        if let Some(extension) = extension {
            args.push("-Extension".to_owned());
            args.push(extension.to_owned());
        }
        self.run(&args)
    }

    /// `/DumpConfigToFiles <dir> [-Extension <name>]`
    pub fn dump_config_to_files(
        &self,
        target_dir: &Path,
        extension: Option<&str>,
    ) -> Result<PlatformCommandResult, DesignerError> {
        let mut args = self.base_args();
        args.push("/DumpConfigToFiles".to_owned());
        args.push(target_dir.display().to_string());
        if let Some(extension) = extension {
            args.push("-Extension".to_owned());
            args.push(extension.to_owned());
        }
        self.run(&args)
    }

    /// `/CheckConfig [-ThinClient] [-Server] ...`
    pub fn check_config(&self, flags: &[&str]) -> Result<PlatformCommandResult, DesignerError> {
        let mut args = self.base_args();
        args.push("/CheckConfig".to_owned());
        args.extend(flags.iter().map(|flag| (*flag).to_owned()));
        self.run(&args)
    }

    /// `/CheckModules [-ThinClient] [-Server] ...`
    pub fn check_modules(&self, flags: &[&str]) -> Result<PlatformCommandResult, DesignerError> {
        let mut args = self.base_args();
        args.push("/CheckModules".to_owned());
        args.extend(flags.iter().map(|flag| (*flag).to_owned()));
        self.run(&args)
    }

    fn base_args(&self) -> Vec<String> {
        let mut args = vec![
            "DESIGNER".to_owned(),
            "/DisableStartupDialogs".to_owned(),
            "/DisableStartupMessages".to_owned(),
        ];
        args.extend(self.connection.args());
        if let Some(log_file) = &self.log_file {
            args.push("/Out".to_owned());
            args.push(log_file.display().to_string());
            args.push("-NoTruncate".to_owned());
        }
        args
    }

    fn run(&self, args: &[String]) -> Result<PlatformCommandResult, DesignerError> {
        let process = self
            .runner
            .run(&ProcessRequest {
                program: self.binary.clone(),
                args: args.to_vec(),
                workdir: None,
                stdout_log_path: None,
                stderr_log_path: None,
            })
            .map_err(DesignerError::Spawn)?;

        let (platform_log_path, platform_log) = if let Some(path) = &self.log_file {
            let contents =
                std::fs::read_to_string(path).map_err(|source| DesignerError::LogRead {
                    path: path.clone(),
                    source,
                })?;
            (Some(path.clone()), Some(contents))
        } else {
            (None, None)
        };

        Ok(PlatformCommandResult {
            process,
            platform_log_path,
            platform_log,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::DesignerDsl;
    use crate::platform::connection::V8Connection;
    use crate::platform::process::{ProcessExecutor, ProcessRunner};
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
    fn returns_none_platform_log_when_out_is_not_requested() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        write_script(&script, "exit 0");
        let runner = ProcessExecutor;
        let dsl = DesignerDsl::new(
            script,
            V8Connection::from_connection_string("File=/tmp/ib"),
            &runner as &dyn ProcessRunner,
            None,
        );

        let result = dsl.check_config(&[]).expect("check config");

        assert!(result.platform_log_path.is_none());
        assert!(result.platform_log.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn reads_platform_log_when_out_is_requested() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        let log_path = dir.path().join("designer.log");
        write_script(
            &script,
            "while [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"/Out\" ]; then\n    shift\n    printf 'designer log\\n' > \"$1\"\n    break\n  fi\n  shift\ndone\nexit 1",
        );
        let runner = ProcessExecutor;
        let dsl = DesignerDsl::new(
            script,
            V8Connection::from_connection_string("File=/tmp/ib"),
            &runner as &dyn ProcessRunner,
            Some(log_path.clone()),
        );

        let result = dsl.check_modules(&["-Server"]).expect("check modules");

        assert_eq!(result.process.exit_code, 1);
        assert_eq!(result.platform_log_path, Some(log_path));
        assert_eq!(result.platform_log.as_deref(), Some("designer log\n"));
    }
}
