use std::time::Duration;

use crate::config::model::AppConfig;
use crate::domain::launch::{LaunchMode, LaunchResult};
use crate::platform::enterprise::{build_launch_args, LaunchClientMode};
use crate::platform::locator::UtilityType;
use crate::platform::process::ProcessRequest;
use crate::platform::utilities::PlatformUtilities;
use crate::support::error::AppError;
use crate::use_cases::context::{ExecutionContext, ExecutionInterruption};
use crate::use_cases::request::{
    EnterpriseLaunchTarget, LaunchRequest as LaunchArgs, LaunchTargetRequest,
};
use crate::use_cases::result::{UseCaseFailure, UseCaseResult};
use tracing::debug;

const LAUNCH_STARTUP_PROBE: Duration = Duration::from_millis(250);

pub fn execute(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &LaunchArgs,
) -> UseCaseResult<LaunchResult> {
    debug!(
        command = context.command().as_str(),
        transport = ?context.transport(),
        target = ?args.target,
        "executing launch use case"
    );
    let (mode, utility, client_mode) = match args.target {
        LaunchTargetRequest::Designer => (
            LaunchMode::Designer,
            UtilityType::V8,
            LaunchClientMode::Designer,
        ),
        LaunchTargetRequest::Enterprise(EnterpriseLaunchTarget::ThinClient) => {
            (LaunchMode::Thin, UtilityType::V8C, LaunchClientMode::Thin)
        }
        LaunchTargetRequest::Enterprise(EnterpriseLaunchTarget::ThickClient) => {
            (LaunchMode::Thick, UtilityType::V8, LaunchClientMode::Thick)
        }
        LaunchTargetRequest::Enterprise(EnterpriseLaunchTarget::OrdinaryApplication) => (
            LaunchMode::Ordinary,
            UtilityType::V8,
            LaunchClientMode::Ordinary,
        ),
    };

    if let Some(interruption) = context.interruption() {
        return Err(UseCaseFailure::without_payload(AppError::Runtime(format!(
            "{} for command '{}'",
            interruption_message(interruption),
            context.command().as_str()
        ))));
    }

    let mut utilities = PlatformUtilities::from_config(config);
    let location = utilities
        .locate(utility)
        .map_err(|e| UseCaseFailure::without_payload(AppError::Platform(e.to_string())))?;

    let process_args = build_launch_args(
        client_mode,
        &config.v8_connection(),
        &config.tools.enterprise.additional_launch_keys,
        &args.launch,
    );

    debug!("[Запуск] Приложение: {}", mode_label(args.target));
    let spawned = utilities
        .runner_for(utility)
        .spawn(&ProcessRequest {
            program: location.path.clone(),
            args: process_args,
            workdir: None,
            stdout_log_path: None,
            stderr_log_path: None,
            startup_probe: Some(LAUNCH_STARTUP_PROBE),
        })
        .map_err(|e| UseCaseFailure::without_payload(AppError::Platform(e.to_string())))?;

    let result = LaunchResult {
        ok: true,
        mode,
        pid: Some(spawned.pid),
        binary: spawned.binary.clone(),
        message: Some(format!(
            "Launched {} via {} (pid {})",
            mode_label(args.target),
            spawned.binary.display(),
            spawned.pid
        )),
    };
    Ok(result)
}

fn interruption_message(interruption: ExecutionInterruption) -> &'static str {
    match interruption {
        ExecutionInterruption::Cancelled => {
            "execution cancelled before reaching a safe completion point"
        }
        ExecutionInterruption::TimedOut => {
            "execution timeout expired before reaching a safe completion point"
        }
    }
}

fn mode_label(target: LaunchTargetRequest) -> &'static str {
    match target {
        LaunchTargetRequest::Designer => "конфигуратор",
        LaunchTargetRequest::Enterprise(EnterpriseLaunchTarget::ThinClient) => "тонкий клиент",
        LaunchTargetRequest::Enterprise(EnterpriseLaunchTarget::ThickClient) => "толстый клиент",
        LaunchTargetRequest::Enterprise(EnterpriseLaunchTarget::OrdinaryApplication) => {
            "обычное приложение"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::execute;
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, EnterpriseToolConfig, PlatformToolConfig,
        SourceFormat, SourceSetConfig, SourceSetPurpose, TestsConfig, ToolsConfig,
    };
    use crate::use_cases::context::{CommandName, ExecutionContext};
    use crate::use_cases::request::{LaunchRequest, LaunchTargetRequest};
    use std::fs;
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
        fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        make_executable(path);
    }

    fn sample_config(base_path: &Path, work_path: &Path, platform_path: &Path) -> AppConfig {
        AppConfig {
            base_path: base_path.to_path_buf(),
            work_path: work_path.to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
            source_sets: vec![SourceSetConfig {
                name: "main".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: PathBuf::from("."),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig {
                platform: PlatformToolConfig {
                    path: Some(platform_path.to_path_buf()),
                    version: None,
                },
                enterprise: EnterpriseToolConfig::default(),
                edt_cli: Default::default(),
            },
            mcp: Default::default(),
            tests: TestsConfig::default(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn thin_launch_app_appends_enterprise_additional_keys() {
        let dir = tempdir().expect("tempdir");
        let args_log = dir.path().join("thin.args.log");
        let platform_dir = dir.path().join("platform");
        write_script(
            &platform_dir.join("bin").join("1cv8c"),
            &format!("printf '%s\n' \"$@\" > '{}'\nsleep 1", args_log.display()),
        );

        let mut config = sample_config(dir.path(), dir.path(), &platform_dir);
        config.tools.enterprise.additional_launch_keys = vec!["/TESTMANAGER".to_owned()];

        let result = execute(
            &ExecutionContext::cli(CommandName::Launch),
            &config,
            &LaunchRequest {
                target: LaunchTargetRequest::thin_client(),
                launch: Default::default(),
            },
        )
        .expect("launch succeeds");

        assert!(result.ok);
        let args = fs::read_to_string(args_log).expect("args log");
        assert!(args.contains("ENTERPRISE"));
        assert!(args.contains("/TESTMANAGER"));
    }

    #[cfg(unix)]
    #[test]
    fn designer_launch_app_does_not_append_enterprise_additional_keys() {
        let dir = tempdir().expect("tempdir");
        let args_log = dir.path().join("designer.args.log");
        let platform_dir = dir.path().join("platform");
        write_script(
            &platform_dir.join("bin").join("1cv8"),
            &format!("printf '%s\n' \"$@\" > '{}'\nsleep 1", args_log.display()),
        );

        let mut config = sample_config(dir.path(), dir.path(), &platform_dir);
        config.tools.enterprise.additional_launch_keys = vec!["/TESTMANAGER".to_owned()];

        let result = execute(
            &ExecutionContext::cli(CommandName::Launch),
            &config,
            &LaunchRequest {
                target: LaunchTargetRequest::designer(),
                launch: Default::default(),
            },
        )
        .expect("launch succeeds");

        assert!(result.ok);
        let args = fs::read_to_string(args_log).expect("args log");
        assert!(args.contains("DESIGNER"));
        assert!(args.contains("/DisableStartupDialogs"));
        assert!(!args.contains("/TESTMANAGER"));
    }

    #[cfg(unix)]
    #[test]
    fn ordinary_launch_app_uses_enterprise_binary_and_ordinary_mode_key() {
        let dir = tempdir().expect("tempdir");
        let args_log = dir.path().join("ordinary.args.log");
        let platform_dir = dir.path().join("platform");
        write_script(
            &platform_dir.join("bin").join("1cv8"),
            &format!("printf '%s\n' \"$@\" > '{}'\nsleep 1", args_log.display()),
        );

        let config = sample_config(dir.path(), dir.path(), &platform_dir);

        let result = execute(
            &ExecutionContext::cli(CommandName::Launch),
            &config,
            &LaunchRequest {
                target: LaunchTargetRequest::ordinary_application(),
                launch: Default::default(),
            },
        )
        .expect("launch succeeds");

        assert!(result.ok);
        let args = fs::read_to_string(args_log).expect("args log");
        assert!(args.contains("ENTERPRISE"));
        assert!(args.contains("/RunModeOrdinaryApplication"));
        assert!(args.contains("/DisableStartupDialogs"));
    }
}
