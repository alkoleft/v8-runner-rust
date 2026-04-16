use std::path::{Path, PathBuf};
use std::time::Duration;

use thiserror::Error;

use crate::domain::runner::LaunchOptions;
use crate::domain::runner::LaunchClientModeRequest;
use crate::platform::connection::V8Connection;
use crate::platform::process::{ProcessError, ProcessRequest, ProcessRunner};
use crate::platform::result::PlatformCommandResult;

#[derive(Debug, Error)]
pub enum EnterpriseError {
    #[error("failed to execute enterprise process: {0}")]
    Spawn(ProcessError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchClientMode {
    Designer,
    Thin,
    Thick,
    Ordinary,
}

pub struct EnterpriseDsl<'a> {
    binary: PathBuf,
    connection: V8Connection,
    additional_launch_keys: Vec<String>,
    client_mode: LaunchClientMode,
    runner: &'a dyn ProcessRunner,
    log_file: PathBuf,
    timeout: Duration,
}

impl<'a> EnterpriseDsl<'a> {
    pub fn new(
        binary: PathBuf,
        connection: V8Connection,
        additional_launch_keys: Vec<String>,
        client_mode: LaunchClientMode,
        runner: &'a dyn ProcessRunner,
        log_file: PathBuf,
        timeout: Duration,
    ) -> Self {
        Self {
            binary,
            connection,
            additional_launch_keys,
            client_mode,
            runner,
            log_file,
            timeout,
        }
    }

    pub fn run_launch(
        &self,
        launch: &LaunchOptions,
    ) -> Result<PlatformCommandResult, EnterpriseError> {
        let args = self.build_args(launch);
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

    fn build_args(&self, launch: &LaunchOptions) -> Vec<String> {
        let mut launch = launch.clone();
        launch.internal_out = Some(self.log_file.display().to_string());
        build_launch_args(
            self.client_mode,
            &self.connection,
            &self.additional_launch_keys,
            &launch,
        )
    }
}

impl From<LaunchClientModeRequest> for LaunchClientMode {
    fn from(value: LaunchClientModeRequest) -> Self {
        match value {
            LaunchClientModeRequest::Designer => LaunchClientMode::Designer,
            LaunchClientModeRequest::Thin => LaunchClientMode::Thin,
            LaunchClientModeRequest::Thick => LaunchClientMode::Thick,
            LaunchClientModeRequest::Ordinary => LaunchClientMode::Ordinary,
        }
    }
}

pub fn build_launch_args(
    mode: LaunchClientMode,
    connection: &V8Connection,
    additional_launch_keys: &[String],
    launch: &LaunchOptions,
) -> Vec<String> {
    let mut args = vec![match mode {
        LaunchClientMode::Designer => "DESIGNER",
        LaunchClientMode::Thin | LaunchClientMode::Thick | LaunchClientMode::Ordinary => {
            "ENTERPRISE"
        }
    }
    .to_owned()];
    args.push("/DisableStartupDialogs".to_owned());
    args.extend(connection.args());
    if matches!(mode, LaunchClientMode::Ordinary) {
        args.push("/RunModeOrdinaryApplication".to_owned());
    }
    if launch.use_privileged_mode {
        args.push("/UsePrivilegedMode".to_owned());
    }
    if let Some(execute) = &launch.execute {
        args.push("/Execute".to_owned());
        args.push(execute.clone());
    }
    if let Some(c) = &launch.c {
        args.push("/C".to_owned());
        args.push(c.clone());
    }

    let mut extra_args = Vec::new();
    if !matches!(mode, LaunchClientMode::Designer) {
        extra_args.extend(filtered_raw_launch_args(additional_launch_keys));
    }
    extra_args.extend(filtered_raw_launch_args(&launch.raw_args));
    args.extend(extra_args);

    if let Some(out) = effective_out_path(launch) {
        args.push("/Out".to_owned());
        args.push(out.to_owned());
    }
    args
}

pub fn normalize_launch_payload_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn effective_out_path(launch: &LaunchOptions) -> Option<&str> {
    launch.internal_out.as_deref().or(launch.out.as_deref())
}

fn filtered_raw_launch_args(args: &[String]) -> Vec<String> {
    let mut filtered = Vec::new();
    let mut skip_value = false;
    for arg in args {
        if skip_value {
            skip_value = false;
            continue;
        }
        if let Some((reserved, consumes_value)) = reserved_launch_key(arg) {
            if reserved {
                skip_value = consumes_value;
                continue;
            }
        }
        filtered.push(arg.clone());
    }
    filtered
}

fn reserved_launch_key(arg: &str) -> Option<(bool, bool)> {
    if !arg.starts_with('/') && !arg.starts_with('-') {
        return None;
    }

    let normalized = arg
        .trim_start_matches(['/', '-'])
        .trim()
        .to_ascii_lowercase();
    let consumes_value = matches!(normalized.as_str(), "c" | "execute" | "out");
    let reserved = matches!(
        normalized.as_str(),
        "c" | "execute"
            | "useprivilegedmode"
            | "out"
            | "runmodeordinaryapplication"
            | "disablestartupdialogs"
    );
    Some((reserved, consumes_value))
}

#[cfg(test)]
mod tests {
    use super::{
        build_launch_args, normalize_launch_payload_path, EnterpriseDsl, LaunchClientMode,
    };
    use crate::domain::runner::LaunchOptions;
    use crate::platform::connection::V8Connection;
    use crate::platform::process::{ProcessExecutor, ProcessRunner};
    use std::path::Path;
    use std::time::Duration;
    use tempfile::tempdir;

    #[test]
    fn c_payload_uses_forward_slashes() {
        let normalized =
            normalize_launch_payload_path(Path::new("C:\\tmp\\path with space\\cfg.json"));
        assert_eq!(normalized, "C:/tmp/path with space/cfg.json");
    }

    #[test]
    fn builds_expected_run_unit_tests_arguments() {
        let args = build_launch_args(
            LaunchClientMode::Thin,
            &V8Connection::from_connection_string("File=/tmp/ib"),
            &["/TESTMANAGER".to_owned()],
            &LaunchOptions {
                c: Some("RunUnitTests=/tmp/path with space/тест config.json".to_owned()),
                internal_out: Some("/tmp/platform.log".to_owned()),
                ..LaunchOptions::default()
            },
        );

        assert_eq!(args[0], "ENTERPRISE");
        assert_eq!(args[1], "/DisableStartupDialogs");
        assert!(args.iter().any(|arg| arg == "/TESTMANAGER"));
        assert!(args.iter().any(|arg| arg == "/C"));
        assert!(args
            .iter()
            .any(|arg| arg == "RunUnitTests=/tmp/path with space/тест config.json"));
        assert!(args.iter().any(|arg| arg == "/Out"));
    }

    #[test]
    fn builds_expected_vanessa_arguments() {
        let args = build_launch_args(
            LaunchClientMode::Thin,
            &V8Connection::from_connection_string("File=/tmp/ib"),
            &["/TESTMANAGER".to_owned()],
            &LaunchOptions {
                execute: Some("/tmp/va/vanessa automation.epf".to_owned()),
                c: Some("StartFeaturePlayer;VAParams=/tmp/va/va-params.json".to_owned()),
                internal_out: Some("/tmp/platform.log".to_owned()),
                ..LaunchOptions::default()
            },
        );

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

    #[test]
    fn ordinary_mode_adds_run_mode_and_filters_reserved_raw_keys() {
        let args = build_launch_args(
            LaunchClientMode::Ordinary,
            &V8Connection::from_connection_string("File=/tmp/ib"),
            &[
                "/TESTMANAGER".to_owned(),
                "/DisableStartupDialogs".to_owned(),
            ],
            &LaunchOptions {
                use_privileged_mode: true,
                raw_args: vec![
                    "/RunModeOrdinaryApplication".to_owned(),
                    "/Out".to_owned(),
                    "user.log".to_owned(),
                    "/C".to_owned(),
                    "ignored".to_owned(),
                    "/WA-".to_owned(),
                ],
                out: Some("launch.log".to_owned()),
                ..LaunchOptions::default()
            },
        );

        assert!(args.iter().any(|arg| arg == "/RunModeOrdinaryApplication"));
        assert_eq!(
            args.iter()
                .filter(|arg| arg.as_str() == "/DisableStartupDialogs")
                .count(),
            1
        );
        assert!(args.iter().any(|arg| arg == "/UsePrivilegedMode"));
        assert!(args.iter().any(|arg| arg == "/WA-"));
        assert!(!args.iter().any(|arg| arg == "ignored"));
        assert!(args.ends_with(&["/Out".to_owned(), "launch.log".to_owned()]));
    }

    #[test]
    fn internal_out_has_priority_over_user_out() {
        let args = build_launch_args(
            LaunchClientMode::Thin,
            &V8Connection::from_connection_string("File=/tmp/ib"),
            &[],
            &LaunchOptions {
                out: Some("user.log".to_owned()),
                internal_out: Some("internal.log".to_owned()),
                ..LaunchOptions::default()
            },
        );

        assert!(args.ends_with(&["/Out".to_owned(), "internal.log".to_owned()]));
        assert!(!args.iter().any(|arg| arg == "user.log"));
    }

    #[test]
    fn enterprise_dsl_applies_internal_out_to_launch() {
        let dir = tempdir().expect("tempdir");
        let runner = ProcessExecutor;
        let dsl = EnterpriseDsl::new(
            dir.path().join("1cv8c"),
            V8Connection::from_connection_string("File=/tmp/ib"),
            vec!["/TESTMANAGER".to_owned()],
            LaunchClientMode::Thin,
            &runner as &dyn ProcessRunner,
            dir.path().join("platform.log"),
            Duration::from_secs(5),
        );

        let args = dsl.build_args(&LaunchOptions {
            c: Some("RunUnitTests=/tmp/test.json".to_owned()),
            out: Some("user.log".to_owned()),
            ..LaunchOptions::default()
        });

        assert!(args.ends_with(&[
            "/Out".to_owned(),
            dir.path().join("platform.log").display().to_string()
        ]));
        assert!(!args.iter().any(|arg| arg == "user.log"));
    }
}
