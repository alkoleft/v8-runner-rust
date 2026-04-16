use std::path::{Path, PathBuf};
use std::time::Duration;

use thiserror::Error;

use crate::platform::connection::V8Connection;
use crate::platform::process::{ProcessError, ProcessRequest, ProcessRunner};
use crate::platform::result::PlatformCommandResult;

#[derive(Debug, Error)]
pub enum EnterpriseError {
    #[error("failed to execute enterprise process: {0}")]
    Spawn(ProcessError),
}

pub enum EnterpriseScenario<'a> {
    YaXUnit {
        config_path: &'a Path,
    },
    Vanessa {
        epf_path: &'a Path,
        params_path: &'a Path,
    },
}

pub struct EnterpriseDsl<'a> {
    binary: PathBuf,
    connection: V8Connection,
    additional_launch_keys: Vec<String>,
    runner: &'a dyn ProcessRunner,
    log_file: PathBuf,
    timeout: Duration,
}

impl<'a> EnterpriseDsl<'a> {
    pub fn new(
        binary: PathBuf,
        connection: V8Connection,
        additional_launch_keys: Vec<String>,
        runner: &'a dyn ProcessRunner,
        log_file: PathBuf,
        timeout: Duration,
    ) -> Self {
        Self {
            binary,
            connection,
            additional_launch_keys,
            runner,
            log_file,
            timeout,
        }
    }

    pub fn run_scenario(
        &self,
        scenario: EnterpriseScenario<'_>,
    ) -> Result<PlatformCommandResult, EnterpriseError> {
        let args = self.build_args(scenario);
        let process = self
            .runner
            .run_with_timeout(
                &ProcessRequest {
                    program: self.binary.clone(),
                    args,
                    workdir: None,
                    stdout_log_path: None,
                    stderr_log_path: None,
                    startup_probe: None,
                },
                self.timeout,
            )
            .map_err(EnterpriseError::Spawn)?;

        let (platform_log_path, platform_log, platform_log_read_error) =
            match std::fs::read_to_string(&self.log_file) {
                Ok(contents) => (Some(self.log_file.clone()), Some(contents), None),
                Err(error) => (
                    Some(self.log_file.clone()),
                    None,
                    Some(format!(
                        "failed to read enterprise /Out log '{}': {error}",
                        self.log_file.display()
                    )),
                ),
            };

        Ok(PlatformCommandResult {
            process,
            platform_log_path,
            platform_log,
            platform_log_read_error,
        })
    }

    fn build_args(&self, scenario: EnterpriseScenario<'_>) -> Vec<String> {
        let mut args = vec!["ENTERPRISE".to_owned()];
        args.extend(self.connection.args());
        match scenario {
            EnterpriseScenario::YaXUnit { config_path } => {
                args.extend(self.additional_launch_keys.clone());
                args.push("/C".to_owned());
                args.push(format!(
                    "RunUnitTests={}",
                    normalize_c_payload_path(config_path)
                ));
            }
            EnterpriseScenario::Vanessa {
                epf_path,
                params_path,
            } => {
                args.push("/Execute".to_owned());
                args.push(normalize_c_payload_path(epf_path));
                args.extend(self.additional_launch_keys.clone());
                args.push("/C".to_owned());
                args.push(format!(
                    "StartFeaturePlayer;VAParams={}",
                    normalize_c_payload_path(params_path)
                ));
            }
        }
        args.push("/Out".to_owned());
        args.push(self.log_file.display().to_string());
        args
    }
}

fn normalize_c_payload_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::{normalize_c_payload_path, EnterpriseDsl, EnterpriseScenario};
    use crate::platform::connection::V8Connection;
    use crate::platform::process::{ProcessExecutor, ProcessRunner};
    use std::path::Path;
    use std::time::Duration;
    use tempfile::tempdir;

    #[test]
    fn c_payload_uses_forward_slashes() {
        let normalized = normalize_c_payload_path(Path::new("C:\\tmp\\path with space\\cfg.json"));
        assert_eq!(normalized, "C:/tmp/path with space/cfg.json");
    }

    #[test]
    fn builds_expected_run_unit_tests_arguments() {
        let dir = tempdir().expect("tempdir");
        let runner = ProcessExecutor;
        let dsl = EnterpriseDsl::new(
            dir.path().join("1cv8c"),
            V8Connection::from_connection_string("File=/tmp/ib"),
            vec!["/TESTMANAGER".to_owned()],
            &runner as &dyn ProcessRunner,
            dir.path().join("platform.log"),
            Duration::from_secs(5),
        );

        let args = dsl.build_args(EnterpriseScenario::YaXUnit {
            config_path: Path::new("/tmp/path with space/тест config.json"),
        });

        assert_eq!(args[0], "ENTERPRISE");
        assert!(args.iter().any(|arg| arg == "/TESTMANAGER"));
        assert!(args.iter().any(|arg| arg == "/C"));
        assert!(args
            .iter()
            .any(|arg| arg == "RunUnitTests=/tmp/path with space/тест config.json"));
    }

    #[test]
    fn builds_expected_vanessa_arguments() {
        let dir = tempdir().expect("tempdir");
        let runner = ProcessExecutor;
        let dsl = EnterpriseDsl::new(
            dir.path().join("1cv8c"),
            V8Connection::from_connection_string("File=/tmp/ib"),
            vec!["/TESTMANAGER".to_owned()],
            &runner as &dyn ProcessRunner,
            dir.path().join("platform.log"),
            Duration::from_secs(5),
        );

        let args = dsl.build_args(EnterpriseScenario::Vanessa {
            epf_path: Path::new("/tmp/va/vanessa automation.epf"),
            params_path: Path::new("/tmp/va/va-params.json"),
        });

        assert_eq!(args[0], "ENTERPRISE");
        assert!(args.iter().any(|arg| arg == "/Execute"));
        assert!(args
            .iter()
            .any(|arg| arg == "/tmp/va/vanessa automation.epf"));
        assert!(args.iter().any(|arg| arg == "/TESTMANAGER"));
        assert!(args.iter().any(|arg| arg == "/C"));
        assert!(args
            .iter()
            .any(|arg| arg == "StartFeaturePlayer;VAParams=/tmp/va/va-params.json"));
    }
}
