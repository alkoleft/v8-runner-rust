use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::config::model::{AppConfig, VanessaProfileConfig};
use crate::domain::launch::{LaunchMode, LaunchResult};
use crate::domain::runner::LaunchOptions;
use crate::platform::enterprise::{
    build_launch_args, normalize_launch_payload_path, LaunchClientMode,
};
use crate::platform::locator::UtilityType;
use crate::platform::process::ProcessRequest;
use crate::platform::utilities::PlatformUtilities;
use crate::support::error::AppError;
use crate::support::path::is_safe_path_segment;
use crate::use_cases::context::{ExecutionContext, ExecutionInterruption};
use crate::use_cases::progress::log_live_stage;
use crate::use_cases::request::{
    ClientMcpAddonRequest, ClientMcpMode, ClientMcpOptionsRequest, EnterpriseLaunchTarget,
    LaunchRequest as LaunchArgs, LaunchTargetRequest,
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
    let mut utilities = PlatformUtilities::from_config(config);
    let location = utilities
        .locate(utility)
        .map_err(|error| UseCaseFailure::without_payload(AppError::from(error)))?;
    let process_args = build_launch_args(
        client_mode,
        &config.v8_connection(),
        &config.tools.enterprise.additional_launch_keys,
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
        let (epf_path, params_path) = prepare_vanessa_mcp_launch(config)?;
        let params_payload_path = normalize_launch_payload_path(&params_path);
        if params_payload_path.contains(';') {
            return Err(AppError::Validation(
                "generated Vanessa params path for launch mcp must not contain ';' because the /C payload is semicolon-delimited".to_owned(),
            ));
        }
        launch.execute = Some(normalize_launch_payload_path(&epf_path));
        payload.push_str(&format!(
            ";StartFeaturePlayer;VAParams={params_payload_path}"
        ));
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

fn prepare_vanessa_mcp_launch(config: &AppConfig) -> Result<(PathBuf, PathBuf), AppError> {
    let va = &config.tests.va;
    let epf_path =
        config.tools.va.epf_path.clone().ok_or_else(|| {
            AppError::Validation("tools.va.epf_path is not configured".to_owned())
        })?;
    let params_path = va
        .params_path
        .as_ref()
        .ok_or_else(|| AppError::Validation("tests.va.params_path is not configured".to_owned()))?;
    let profile_name = va
        .profile
        .as_deref()
        .ok_or_else(|| AppError::Validation("tests.va.profile is not configured".to_owned()))?;
    if !is_safe_path_segment(profile_name) {
        return Err(AppError::Validation(format!(
            "tests.va.profile contains unsafe path characters: {profile_name}"
        )));
    }
    let profile = va.profiles.get(profile_name).ok_or_else(|| {
        AppError::Validation(format!(
            "unknown Vanessa Automation profile '{profile_name}'"
        ))
    })?;

    let run_dir = config
        .work_path
        .join("temp")
        .join("client-mcp")
        .join("va")
        .join(format!(
            "{}-{}-{}",
            chrono::Utc::now().timestamp_millis(),
            std::process::id(),
            Uuid::new_v4().simple()
        ));
    fs::create_dir_all(&run_dir).map_err(|error| {
        AppError::Runtime(format!(
            "failed to create Vanessa params directory: {error}"
        ))
    })?;
    set_dir_permissions(&run_dir).map_err(|error| {
        AppError::Runtime(format!("failed to chmod Vanessa params directory: {error}"))
    })?;
    let runtime_params_path = run_dir.join("va-params.json");
    let base = fs::read_to_string(params_path).map_err(|error| {
        AppError::Runtime(format!("failed to read Vanessa params template: {error}"))
    })?;
    let mut payload: Value = serde_json::from_str(&base).map_err(|error| {
        AppError::Runtime(format!("failed to parse Vanessa params JSON: {error}"))
    })?;
    let object = payload
        .as_object_mut()
        .ok_or_else(|| AppError::Runtime("Vanessa params JSON must be an object".to_owned()))?;
    apply_vanessa_mcp_overlay(object, profile, va.fail_fast);
    let payload = serde_json::to_vec_pretty(&payload).map_err(|error| {
        AppError::Runtime(format!("failed to serialize Vanessa params JSON: {error}"))
    })?;
    write_private_file(&runtime_params_path, &payload)
        .map_err(|error| AppError::Runtime(format!("failed to write Vanessa params: {error}")))?;
    Ok((epf_path, runtime_params_path))
}

fn apply_vanessa_mcp_overlay(
    object: &mut Map<String, Value>,
    profile: &VanessaProfileConfig,
    fail_fast: bool,
) {
    object.insert("stoponerror".to_owned(), Value::Bool(fail_fast));
    if let Some(feature_path) = profile.feature_path.as_ref() {
        object.insert(
            "featurepath".to_owned(),
            Value::String(feature_path.display().to_string()),
        );
    }
    insert_string_array_if_non_empty(object, "FeaturesToRun", &profile.features_to_run);
    insert_string_array_if_non_empty(object, "filtertags", &profile.filter_tags);
    insert_string_array_if_non_empty(object, "ignoretags", &profile.ignore_tags);
    insert_string_array_if_non_empty(object, "scenariofilter", &profile.scenario_filter);
}

fn insert_string_array_if_non_empty(object: &mut Map<String, Value>, key: &str, values: &[String]) {
    if values.is_empty() {
        object.remove(key);
        return;
    }
    object.insert(
        key.to_owned(),
        Value::Array(values.iter().cloned().map(Value::String).collect()),
    );
}

fn write_private_file(path: &Path, payload: &[u8]) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(payload)?;
    set_file_permissions(path)?;
    Ok(())
}

fn set_dir_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

fn set_file_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::execute;
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, EnterpriseToolConfig, PlatformToolConfig,
        SourceFormat, SourceSetConfig, SourceSetPurpose, TestsConfig, ToolsConfig,
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
