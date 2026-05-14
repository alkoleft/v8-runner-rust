use std::path::Path;
use std::time::Duration;

use crate::config::model::AppConfig;
use crate::domain::launch::{LaunchMode, LaunchResult};
use crate::domain::runner::LaunchOptions;
use crate::platform::enterprise::{
    build_launch_args, normalize_launch_payload_path, LaunchClientMode,
};
use crate::platform::locator::UtilityType;
use crate::platform::process::ProcessRequest;
use crate::platform::utilities::PlatformUtilities;
use crate::support::error::AppError;
use crate::use_cases::context::{ExecutionContext, ExecutionInterruption};
use crate::use_cases::launch_keys::vanessa_enterprise_launch_keys;
use crate::use_cases::progress::log_live_stage;
use crate::use_cases::request::{
    ClientMcpAddonRequest, ClientMcpMode, ClientMcpOptionsRequest, EnterpriseLaunchTarget,
    LaunchRequest as LaunchArgs, LaunchTargetRequest,
};
use crate::use_cases::result::{UseCaseFailure, UseCaseResult};
use crate::use_cases::tool_extension;
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
        LaunchTargetRequest::Enterprise(EnterpriseLaunchTarget::ClientMcp { mode }) => {
            client_mcp_launch_shape(mode)
        }
    };

    if let Some(interruption) = context.interruption() {
        return Err(UseCaseFailure::without_payload(AppError::Runtime(format!(
            "{} for command '{}'",
            interruption_message(interruption),
            context.command().as_str()
        ))));
    }

    let launch = effective_launch_options(config, args)
        .map_err(|error| UseCaseFailure::without_payload(error))?;
    let additional_launch_keys = effective_enterprise_launch_keys(config, args, &launch);
    let mut utilities = PlatformUtilities::from_config(config);
    let location = utilities
        .locate(utility)
        .map_err(|error| UseCaseFailure::without_payload(AppError::from(error)))?;
    let process_args = build_launch_args(
        client_mode,
        &config.v8_connection(),
        &additional_launch_keys,
        &launch,
    );

    debug!("[Запуск] Приложение: {}", mode_label(args.target));
    log_live_stage("launch: start", "[Launch] starting client process");
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
        .map_err(|error| UseCaseFailure::without_payload(AppError::from(error)))?;

    let result = LaunchResult {
        ok: true,
        mode,
        pid: Some(spawned.pid),
        binary: spawned.binary.clone(),
        message: Some(launch_message(config, args, &spawned.binary, spawned.pid)),
    };
    Ok(result)
}

fn launch_message(config: &AppConfig, args: &LaunchArgs, binary: &Path, pid: u32) -> String {
    let mut message = format!(
        "Launched {} via {} (pid {})",
        mode_label(args.target),
        binary.display(),
        pid
    );
    if is_client_mcp_launch(args) {
        if let Some(hint) = tool_extension::client_mcp_build_hint(config) {
            message.push_str("; ");
            message.push_str(hint);
        }
    }
    message
}

fn is_client_mcp_launch(args: &LaunchArgs) -> bool {
    matches!(
        args.target,
        LaunchTargetRequest::Enterprise(EnterpriseLaunchTarget::ClientMcp { .. })
    )
}

fn effective_enterprise_launch_keys(
    config: &AppConfig,
    args: &LaunchArgs,
    launch: &LaunchOptions,
) -> Vec<String> {
    if is_client_mcp_va_launch(args) {
        return vanessa_enterprise_launch_keys(
            &config.tools.enterprise.additional_launch_keys,
            launch,
        );
    }
    config.tools.enterprise.additional_launch_keys.clone()
}

fn is_client_mcp_va_launch(args: &LaunchArgs) -> bool {
    args.client_mcp.as_ref().is_some_and(|client_mcp| {
        matches!(
            client_mcp.addon,
            Some(ClientMcpAddonRequest::VanessaAutomation)
        )
    })
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
        LaunchTargetRequest::Enterprise(EnterpriseLaunchTarget::ClientMcp { .. }) => {
            "клиентский MCP-сервер"
        }
    }
}

fn client_mcp_launch_shape(mode: ClientMcpMode) -> (LaunchMode, UtilityType, LaunchClientMode) {
    match mode {
        ClientMcpMode::Thin => (LaunchMode::Mcp, UtilityType::V8C, LaunchClientMode::Thin),
        ClientMcpMode::Thick => (LaunchMode::Mcp, UtilityType::V8, LaunchClientMode::Thick),
        ClientMcpMode::Ordinary => (LaunchMode::Mcp, UtilityType::V8, LaunchClientMode::Ordinary),
    }
}

fn effective_launch_options(
    config: &AppConfig,
    args: &LaunchArgs,
) -> Result<LaunchOptions, AppError> {
    let is_client_mcp = matches!(
        args.target,
        LaunchTargetRequest::Enterprise(EnterpriseLaunchTarget::ClientMcp { .. })
    );
    let Some(client_mcp) = args.client_mcp.as_ref() else {
        return if is_client_mcp {
            Err(AppError::Validation(
                "launch mcp requires client_mcp options".to_owned(),
            ))
        } else {
            Ok(args.launch.clone())
        };
    };
    if !is_client_mcp {
        return Err(AppError::Validation(
            "client_mcp options are supported only for launch mcp".to_owned(),
        ));
    }

    let mut launch = args.launch.clone();
    let mut payload = build_client_mcp_payload(client_mcp, config.tools.client_mcp.port);
    if matches!(
        client_mcp.addon,
        Some(ClientMcpAddonRequest::VanessaAutomation)
    ) {
        let va_launch = crate::use_cases::vanessa::prepare_client_mcp_launch(config)?;
        crate::use_cases::vanessa::apply_client_mcp_launch(&mut launch, &mut payload, &va_launch);
    }
    launch.c = Some(payload);
    Ok(launch)
}

fn build_client_mcp_payload(
    options: &ClientMcpOptionsRequest,
    configured_port: Option<u16>,
) -> String {
    let mut payload = match options.config_path.as_deref() {
        Some(path) => format!("runMcp={}", normalize_launch_payload_path(Path::new(path))),
        None => "runMcp".to_owned(),
    };
    if let Some(port) = options.port.or(configured_port) {
        payload.push_str(&format!(";mcpPort={port}"));
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::execute;
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, EnterpriseToolConfig, PlatformToolConfig,
        SourceFormat, SourceSetConfig, SourceSetPurpose, TestsConfig, ToolExtensionArtifactConfig,
        ToolExtensionConfig, ToolExtensionInput, ToolsConfig,
    };
    use crate::use_cases::context::{CommandName, ExecutionContext};
    use crate::use_cases::request::{
        ClientMcpMode, ClientMcpOptionsRequest, LaunchRequest, LaunchTargetRequest,
    };
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
                ..Default::default()
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
                client_mcp: None,
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
                client_mcp: None,
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
                client_mcp: None,
            },
        )
        .expect("launch succeeds");

        assert!(result.ok);
        let args = fs::read_to_string(args_log).expect("args log");
        assert!(args.contains("ENTERPRISE"));
        assert!(args.contains("/RunModeOrdinaryApplication"));
        assert!(args.contains("/DisableStartupDialogs"));
    }

    #[cfg(unix)]
    #[test]
    fn client_mcp_launch_does_not_prepare_configured_tool_extension() {
        let dir = tempdir().expect("tempdir");
        let args_log = dir.path().join("mcp.args.log");
        let platform_dir = dir.path().join("platform");
        write_script(
            &platform_dir.join("bin").join("1cv8c"),
            &format!("printf '%s\n' \"$@\" > '{}'\nsleep 1", args_log.display()),
        );

        let mut config = sample_config(dir.path(), dir.path(), &platform_dir);
        config.tools.client_mcp.port = Some(9874);
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Artifact(ToolExtensionArtifactConfig {
                path: dir.path().join("client_mcp.cfe"),
            }),
        });

        let result = execute(
            &ExecutionContext::cli(CommandName::Launch),
            &config,
            &LaunchRequest {
                target: LaunchTargetRequest::client_mcp_with_mode(ClientMcpMode::Thin),
                launch: Default::default(),
                client_mcp: Some(ClientMcpOptionsRequest::default()),
            },
        )
        .expect("launch succeeds");

        assert!(result.ok);
        assert!(result
            .message
            .as_deref()
            .expect("message")
            .contains("v8-runner build"));
        let args = fs::read_to_string(args_log).expect("args log");
        assert!(args.contains("ENTERPRISE"));
        assert!(args.contains("/C\nrunMcp;mcpPort=9874\n"));
        assert!(!args.contains("/LoadCfg"));
        assert!(!args.contains("-Extension"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_inconsistent_client_mcp_request_state_before_locating_platform() {
        let dir = tempdir().expect("tempdir");
        let platform_dir = dir.path().join("missing-platform");
        let config = sample_config(dir.path(), dir.path(), &platform_dir);

        let missing_options = execute(
            &ExecutionContext::cli(CommandName::Launch),
            &config,
            &LaunchRequest {
                target: LaunchTargetRequest::client_mcp_with_mode(ClientMcpMode::Thin),
                launch: Default::default(),
                client_mcp: None,
            },
        )
        .expect_err("client_mcp options are required");
        assert!(
            missing_options
                .error
                .to_string()
                .contains("launch mcp requires client_mcp options"),
            "{missing_options:?}"
        );

        let unexpected_options = execute(
            &ExecutionContext::cli(CommandName::Launch),
            &config,
            &LaunchRequest {
                target: LaunchTargetRequest::thin_client(),
                launch: Default::default(),
                client_mcp: Some(ClientMcpOptionsRequest::default()),
            },
        )
        .expect_err("client_mcp options are rejected for non-mcp launch");
        assert!(
            unexpected_options
                .error
                .to_string()
                .contains("client_mcp options are supported only for launch mcp"),
            "{unexpected_options:?}"
        );
    }
}
