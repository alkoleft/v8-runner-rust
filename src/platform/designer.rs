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

    /// `CREATEINFOBASE <connection-string>`
    pub fn create_infobase(&self) -> Result<PlatformCommandResult, DesignerError> {
        let mut args = vec!["CREATEINFOBASE".to_owned()];
        let connection = self.connection.create_infobase_arg().ok_or_else(|| {
            DesignerError::UtilityNotFound("file-based connection is required".to_owned())
        })?;
        args.push(connection);
        self.run(&args)
    }

    /// `/DumpConfigToFiles <dir> [-Extension <name>] -updateConfigDumpInfo`
    pub fn dump_config_to_files(
        &self,
        target_dir: &Path,
        extension: Option<&str>,
    ) -> Result<PlatformCommandResult, DesignerError> {
        let mut args = self.base_args();
        args.push("/DumpConfigToFiles".to_owned());
        args.push(target_dir.display().to_string());
        args.push("-updateConfigDumpInfo".to_owned());
        if let Some(extension) = extension {
            args.push("-Extension".to_owned());
            args.push(extension.to_owned());
        }
        self.run(&args)
    }

    /// `/DumpCfg <file> [-Extension <name>]`
    pub fn dump_cfg(
        &self,
        target_file: &Path,
        extension: Option<&str>,
    ) -> Result<PlatformCommandResult, DesignerError> {
        let mut args = self.base_args();
        args.push("/DumpCfg".to_owned());
        args.push(target_file.display().to_string());
        if let Some(extension) = extension {
            args.push("-Extension".to_owned());
            args.push(extension.to_owned());
        }
        self.run(&args)
    }

    /// `/DumpExternalDataProcessorOrReportToFiles <root-xml> <binary>`
    pub fn dump_external_data_processor_or_report_to_files(
        &self,
        binary_file: &Path,
        root_xml_path: &Path,
    ) -> Result<PlatformCommandResult, DesignerError> {
        let mut args = self.base_args();
        args.push("/DumpExternalDataProcessorOrReportToFiles".to_owned());
        args.push(root_xml_path.display().to_string());
        args.push(binary_file.display().to_string());
        self.run(&args)
    }

    /// `/LoadExternalDataProcessorOrReportFromFiles <root-xml> <binary>`
    pub fn load_external_data_processor_or_report_from_files(
        &self,
        root_xml_path: &Path,
        binary_file: &Path,
    ) -> Result<PlatformCommandResult, DesignerError> {
        let mut args = self.base_args();
        args.push("/LoadExternalDataProcessorOrReportFromFiles".to_owned());
        args.push(root_xml_path.display().to_string());
        args.push(binary_file.display().to_string());
        self.run(&args)
    }

    /// `/DumpConfigToFiles <dir> -partial -listFile <list_file> -updateConfigDumpInfo`
    /// `[-Extension <name>]`
    pub fn dump_config_to_files_partial(
        &self,
        target_dir: &Path,
        list_file: &Path,
        extension: Option<&str>,
    ) -> Result<PlatformCommandResult, DesignerError> {
        let mut args = self.base_args();
        args.push("/DumpConfigToFiles".to_owned());
        args.push(target_dir.display().to_string());
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
        if let Some(log_file) = &self.log_file {
            let _ = std::fs::remove_file(log_file);
        }
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
            .map_err(DesignerError::Spawn)?;

        let (platform_log_path, platform_log, platform_log_read_error) =
            if let Some(path) = &self.log_file {
                match std::fs::read_to_string(path) {
                    Ok(contents) => (Some(path.clone()), Some(contents), None),
                    Err(error) => (
                        Some(path.clone()),
                        None,
                        Some(format!(
                            "failed to read designer /Out log '{}': {error}",
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
        assert!(result.platform_log_read_error.is_none());
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
        assert!(result.platform_log_read_error.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn preserves_process_result_when_platform_log_is_unreadable() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        let log_path = dir.path().join("missing").join("designer.log");
        write_script(&script, "exit 101");
        let runner = ProcessExecutor;
        let dsl = DesignerDsl::new(
            script,
            V8Connection::from_connection_string("File=/tmp/ib"),
            &runner as &dyn ProcessRunner,
            Some(log_path.clone()),
        );

        let result = dsl.check_modules(&["-Server"]).expect("check modules");

        assert_eq!(result.process.exit_code, 101);
        assert_eq!(result.platform_log_path, Some(log_path));
        assert!(result.platform_log.is_none());
        assert!(result
            .platform_log_read_error
            .as_deref()
            .expect("read error")
            .contains("failed to read designer /Out log"));
    }

    #[cfg(unix)]
    #[test]
    fn dump_config_to_files_requests_config_dump_info_update() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        let args_log = dir.path().join("args.log");
        write_script(
            &script,
            &format!("printf '%s\n' \"$@\" > \"{}\"\nexit 0", args_log.display()),
        );
        let runner = ProcessExecutor;
        let dsl = DesignerDsl::new(
            script,
            V8Connection::from_connection_string("File=/tmp/ib"),
            &runner as &dyn ProcessRunner,
            None,
        );

        dsl.dump_config_to_files(dir.path().join("out").as_path(), None)
            .expect("dump config");

        let args = fs::read_to_string(args_log).expect("args log");
        assert!(args.contains("/DumpConfigToFiles"));
        assert!(args.contains("-updateConfigDumpInfo"));
    }

    #[cfg(unix)]
    #[test]
    fn dump_config_to_files_partial_passes_partial_list_args() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        let args_log = dir.path().join("args.log");
        write_script(
            &script,
            &format!("printf '%s\n' \"$@\" > \"{}\"\nexit 0", args_log.display()),
        );
        let runner = ProcessExecutor;
        let dsl = DesignerDsl::new(
            script,
            V8Connection::from_connection_string("File=/tmp/ib"),
            &runner as &dyn ProcessRunner,
            None,
        );

        dsl.dump_config_to_files_partial(
            dir.path().join("out").as_path(),
            dir.path().join("objects.txt").as_path(),
            Some("ExtName"),
        )
        .expect("dump config partial");

        let args = fs::read_to_string(args_log).expect("args log");
        assert!(args.contains("/DumpConfigToFiles"));
        assert!(args.contains("-partial"));
        assert!(args.contains("-listFile"));
        assert!(args.contains("objects.txt"));
        assert!(args.contains("-updateConfigDumpInfo"));
        assert!(args.contains("-Extension"));
        assert!(args.contains("ExtName"));
    }

    #[cfg(unix)]
    #[test]
    fn create_infobase_builds_expected_args() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        let args_log = dir.path().join("args.log");
        write_script(
            &script,
            &format!("printf '%s\n' \"$@\" > \"{}\"\nexit 0", args_log.display()),
        );
        let runner = ProcessExecutor;
        let dsl = DesignerDsl::new(
            script,
            V8Connection::from_connection_string("File=/tmp/my ib"),
            &runner as &dyn ProcessRunner,
            None,
        );

        dsl.create_infobase().expect("create infobase");

        let args = fs::read_to_string(args_log).expect("args log");
        assert!(args.contains("CREATEINFOBASE"));
        assert!(args.contains("File='/tmp/my ib'"));
    }

    #[cfg(unix)]
    #[test]
    fn dump_cfg_passes_dumpcfg_extension_and_out_arguments() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        let args_log = dir.path().join("args.log");
        let log_path = dir.path().join("designer.log");
        let target = dir.path().join("release.cfe");
        write_script(
            &script,
            &format!(
                "printf '%s\\n' \"$*\" > \"{}\"\nprev=''\nfor arg in \"$@\"; do if [ \"$prev\" = '/Out' ]; then printf 'designer log' > \"$arg\"; fi; prev=\"$arg\"; done\nexit 0",
                args_log.display()
            ),
        );
        let runner = ProcessExecutor;
        let dsl = DesignerDsl::new(
            script,
            V8Connection::from_connection_string("File=/tmp/ib"),
            &runner as &dyn ProcessRunner,
            Some(log_path.clone()),
        );

        let result = dsl.dump_cfg(&target, Some("SalesAddon")).expect("dump cfg");

        let args = fs::read_to_string(args_log).expect("args log");
        assert!(args.contains("/DumpCfg"));
        assert!(args.contains(&target.display().to_string()));
        assert!(args.contains("-Extension SalesAddon"));
        assert!(args.contains(&format!("/Out {}", log_path.display())));
        assert_eq!(result.platform_log_path, Some(log_path));
    }

    #[cfg(unix)]
    #[test]
    fn external_dump_and_load_commands_shape_arguments() {
        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("1cv8");
        let args_log = dir.path().join("args.log");
        write_script(
            &script,
            &format!("printf '%s\\n' \"$@\" > \"{}\"\nexit 0", args_log.display()),
        );
        let runner = ProcessExecutor;
        let dsl = DesignerDsl::new(
            script,
            V8Connection::from_connection_string("File=/tmp/ib"),
            &runner as &dyn ProcessRunner,
            None,
        );

        dsl.dump_external_data_processor_or_report_to_files(
            &dir.path().join("artifact.epf"),
            &dir.path().join("dump/Artifact.xml"),
        )
        .expect("external dump");
        let args = fs::read_to_string(&args_log)
            .expect("args log")
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let dump_pos = args
            .iter()
            .position(|line| line == "/DumpExternalDataProcessorOrReportToFiles")
            .expect("dump arg");
        assert_eq!(args[dump_pos + 1], dir.path().join("dump/Artifact.xml").display().to_string());
        assert_eq!(args[dump_pos + 2], dir.path().join("artifact.epf").display().to_string());

        dsl.load_external_data_processor_or_report_from_files(
            &dir.path().join("dump/Artifact.xml"),
            &dir.path().join("artifact.epf"),
        )
        .expect("external load");
        let args = fs::read_to_string(&args_log)
            .expect("args log")
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let load_pos = args
            .iter()
            .position(|line| line == "/LoadExternalDataProcessorOrReportFromFiles")
            .expect("load arg");
        assert_eq!(args[load_pos + 1], dir.path().join("dump/Artifact.xml").display().to_string());
        assert_eq!(args[load_pos + 2], dir.path().join("artifact.epf").display().to_string());
    }
}
