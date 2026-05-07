#![cfg(unix)]

mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::Value;
use support::{temp_workspace, v8_runner_command, write_shell_script_atomically};

fn write_script(path: &Path) {
    write_shell_script_atomically(path, "sleep 1");
}

fn write_logging_script(path: &Path, args_log: &Path) {
    write_shell_script_atomically(
        path,
        &format!("printf '%s\n' \"$@\" > '{}'\nsleep 1", args_log.display()),
    );
}

fn write_config(
    path: &Path,
    base_path: &Path,
    work_path: &Path,
    platform_path: &Path,
    platform_version: Option<&str>,
) {
    let mut config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: .\ntools:\n  platform:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        platform_path.display(),
    );
    if let Some(platform_version) = platform_version {
        config.push_str(&format!("    version: '{}'\n", platform_version));
    }

    fs::write(path, config).expect("config");
}

fn setup_project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let install_dir = dir.path().join("platform");
    let config_path = dir.path().join("v8project.yaml");

    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    write_script(&install_dir.join("bin").join("1cv8"));
    write_script(&install_dir.join("bin").join("1cv8c"));
    write_config(&config_path, &base_path, &work_path, &install_dir, None);

    (dir, config_path, install_dir, work_path)
}

fn setup_versioned_project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let root_path = dir.path().join("platform-root");
    let version = root_path.join("8.3.25.1234");
    let config_path = dir.path().join("v8project.yaml");

    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    write_script(&version.join("bin").join("1cv8"));
    write_script(&version.join("bin").join("1cv8c"));
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &root_path,
        Some("8.3.25.1234"),
    );

    (dir, config_path, version, work_path)
}

fn setup_mcp_va_project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    setup_mcp_va_project_with_work_name("work")
}

fn setup_mcp_va_project_with_work_name(
    work_name: &str,
) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    setup_mcp_va_project_with_options(work_name, &[])
}

fn setup_mcp_va_project_with_options(
    work_name: &str,
    additional_launch_keys: &[&str],
) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join(work_name);
    let install_dir = dir.path().join("platform");
    let config_path = dir.path().join("v8project.yaml");
    let args_log = install_dir.join("mcp-va.args.log");
    let va_epf = dir.path().join("va").join("vanessa-automation.epf");
    let va_params = dir.path().join("cfg").join("va-base.json");
    let features_dir = dir.path().join("features").join("smoke");

    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    fs::create_dir_all(va_epf.parent().expect("va dir")).expect("va dir");
    fs::create_dir_all(va_params.parent().expect("cfg dir")).expect("cfg dir");
    fs::create_dir_all(&features_dir).expect("features");
    fs::write(&va_epf, "epf").expect("epf");
    fs::write(&va_params, "{\n  \"existing\": true\n}\n").expect("params");
    fs::write(features_dir.join("login.feature"), "Feature: Login\n").expect("feature");
    write_script(&install_dir.join("bin").join("1cv8c"));
    write_logging_script(&install_dir.join("bin").join("1cv8"), &args_log);

    let additional_launch_keys_block = if additional_launch_keys.is_empty() {
        String::new()
    } else {
        format!(
            "  enterprise:\n    additional-launch-keys:\n{}",
            additional_launch_keys
                .iter()
                .map(|key| format!("      - '{}'\n", key))
                .collect::<String>()
        )
    };
    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\ntests:\n  va:\n    params_path: '{}'\n    profile: smoke\n    profiles:\n      smoke:\n        feature_path: '{}'\n        features_to_run:\n          - login\n        filter_tags:\n          - '@smoke'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: .\ntools:\n  client_mcp:\n    port: 9874\n  va:\n    epf_path: '{}'\n  platform:\n    path: '{}'\n{}",
        base_path.display(),
        work_path.display(),
        va_params.display(),
        features_dir.display(),
        va_epf.display(),
        install_dir.display(),
        additional_launch_keys_block,
    );
    fs::write(&config_path, config).expect("config");

    (dir, config_path, install_dir, args_log)
}

#[test]
fn launch_json_returns_pid_and_selected_binary() {
    let (_dir, config_path, install_dir, _work_path) = setup_project();
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "launch",
            "thin",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    let data = &payload["data"];
    assert_eq!(payload["ok"], true);
    assert_eq!(data["mode"], "thin");
    assert_eq!(
        data["binary"].as_str().expect("binary"),
        install_dir.join("bin").join("1cv8c").to_string_lossy()
    );
    assert!(data["pid"].as_u64().expect("pid") > 0);
}

#[test]
fn launch_text_includes_binary_pid_and_cleans_platform_logs() {
    let (_dir, config_path, install_dir, work_path) = setup_project();
    let logs_dir = work_path.join("logs").join("platform");
    fs::create_dir_all(&logs_dir).expect("logs dir");
    let stale_log = logs_dir.join("stale.log");
    fs::write(&stale_log, "old log").expect("stale log");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "--clean-before-execution",
            "launch",
            "designer",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Launch completed successfully"));
    assert!(stdout.contains("mode: конфигуратор"));
    assert!(stdout.contains("[status] Launched конфигуратор via"));
    assert!(stdout.contains(
        install_dir
            .join("bin")
            .join("1cv8")
            .to_string_lossy()
            .as_ref()
    ));
    assert!(stdout.contains("pid"));
    assert!(!stale_log.exists());
}

#[test]
fn launch_designer_accepts_positional_mode() {
    let (_dir, config_path, install_dir, _work_path) = setup_project();
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "launch",
            "designer",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["data"]["mode"], "designer");
    assert_eq!(
        payload["data"]["binary"].as_str().expect("binary"),
        install_dir.join("bin").join("1cv8").to_string_lossy()
    );
}

#[test]
fn launch_thick_uses_v8_binary() {
    let (_dir, config_path, install_dir, _work_path) = setup_project();
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "launch",
            "thick",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(
        payload["data"]["binary"].as_str().expect("binary"),
        install_dir.join("bin").join("1cv8").to_string_lossy()
    );
}

#[test]
fn launch_uses_versioned_root_hint() {
    let (_dir, config_path, version_dir, _work_path) = setup_versioned_project();
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "launch",
            "thin",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(
        payload["data"]["binary"].as_str().expect("binary"),
        version_dir.join("bin").join("1cv8c").to_string_lossy()
    );
}

#[test]
fn launch_fails_when_process_exits_during_startup_probe() {
    let (_dir, config_path, install_dir, _work_path) = setup_project();
    let thin = install_dir.join("bin").join("1cv8c");
    write_shell_script_atomically(&thin, "exit 9");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "launch",
            "thin",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&output.stderr).contains("exited before startup completed"));
}

#[test]
fn launch_json_failure_returns_error_envelope_and_exit_code() {
    let (_dir, config_path, install_dir, _work_path) = setup_project();
    let thin = install_dir.join("bin").join("1cv8c");
    write_shell_script_atomically(&thin, "exit 9");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "launch",
            "thin",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(4));
    assert!(output.stderr.is_empty());
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "launch");
    assert_eq!(payload["error"]["code"], "platform_failure");
    assert_eq!(payload["error"]["kind"], "platform");
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("exited before startup completed"));
}

#[test]
fn launch_ordinary_supports_typed_keys_and_filters_reserved_raw_duplicates() {
    let (_dir, config_path, install_dir, _work_path) = setup_project();
    let args_log = install_dir.join("ordinary.args.log");
    write_logging_script(&install_dir.join("bin").join("1cv8"), &args_log);

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "launch",
            "ordinary",
            "--c",
            "DoWork",
            "--execute",
            "/tmp/tool.epf",
            "--use-privileged-mode",
            "--output",
            "/tmp/user.out.log",
            "--raw-key",
            "/RunModeOrdinaryApplication",
            "--raw-key",
            "/Out",
            "--raw-key",
            "/tmp/ignored.out.log",
            "--raw-key",
            "/WA-",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let args = fs::read_to_string(args_log).expect("args log");
    assert!(args.contains("ENTERPRISE"));
    assert!(args.contains("/DisableStartupDialogs"));
    assert_eq!(args.matches("/RunModeOrdinaryApplication").count(), 1);
    assert!(args.contains("/UsePrivilegedMode"));
    assert!(args.contains("/Execute"));
    assert!(args.contains("/tmp/tool.epf"));
    assert!(args.contains("/C\"DoWork\""));
    assert!(args.contains("DoWork"));
    assert!(args.contains("/WA-"));
    assert!(args.contains("/tmp/user.out.log"));
    assert!(!args.contains("/tmp/ignored.out.log"));
}

#[test]
fn launch_mcp_va_builds_payload_from_configured_port_and_ordinary_mode() {
    let (_dir, config_path, install_dir, args_log) = setup_mcp_va_project();
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "launch",
            "mcp",
            "va",
            "--mode",
            "ordinary",
            "--mcp-config",
            "/tmp/mcp conf.json",
            "--mcp-transport",
            "legacy",
            "--raw-key",
            "/WA-",
        ])
        .output()
        .expect("run command");

    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["data"]["mode"], "mcp");
    assert_eq!(payload["data"]["transport"], "legacy");
    assert_eq!(payload["data"]["mcp_port"], 9874);
    assert_eq!(
        payload["data"]["binary"].as_str().expect("binary"),
        install_dir.join("bin").join("1cv8").to_string_lossy()
    );

    let args = fs::read_to_string(args_log).expect("args log");
    assert!(args.contains("ENTERPRISE"));
    assert!(args.contains("/DisableStartupDialogs"));
    assert!(args.contains("/RunModeOrdinaryApplication"));
    assert!(args.contains("/Execute"));
    assert!(args.contains("vanessa-automation.epf"));
    assert!(args.contains("/C\"runMcp=/tmp/mcp conf.json;mcpPort=9874;VAParams="));
    assert!(!args.contains("StartFeaturePlayer"));
    assert!(args.contains("/TESTMANAGER"));
    assert!(args.contains("/WA-"));
    let params_arg = args
        .lines()
        .find(|line| line.contains("VAParams="))
        .expect("VAParams argument");
    let params_path = params_arg
        .split("VAParams=")
        .nth(1)
        .expect("VAParams path")
        .trim_end_matches('"');
    let params = fs::read_to_string(params_path).expect("runtime params");
    let params_json: Value = serde_json::from_str(&params).expect("runtime params JSON");
    assert_eq!(params_json["existing"], true);
    assert!(params_json["WorkspaceRoot"]
        .as_str()
        .expect("WorkspaceRoot")
        .contains("/project"));
    assert_eq!(params_json["ОстановкаПриВозникновенииОшибки"], false);
    assert_eq!(params_json["СписокФичДляВыполнения"][0], "login");
    assert_eq!(params_json["СписокТеговОтбор"][0], "smoke");
    assert_eq!(
        params_json["ДелатьЛогВыполненияСценариевВТекстовыйФайл"],
        true
    );
    assert_eq!(params_json["ВыводитьВЛогВыполнениеШагов"], true);
    assert_eq!(params_json["ПодробныйЛогВыполненияСценариев"], 1);
    assert_eq!(params_json["ВыгружатьСтатусВыполненияСценариевВФайл"], true);
    assert!(
        params_json["ПутьКФайлуДляВыгрузкиСтатусаВыполненияСценариев"]
            .as_str()
            .expect("status path")
            .ends_with("/va-status.log")
    );
    assert!(params_json["ИмяФайлаЛогВыполненияСценариев"]
        .as_str()
        .expect("text log path")
        .ends_with("/va-text.log"));
    assert_eq!(
        fs::metadata(params_path)
            .expect("params metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let params_dir = Path::new(params_path).parent().expect("params dir");
    assert_eq!(
        fs::metadata(params_dir)
            .expect("params dir metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
fn launch_mcp_va_does_not_duplicate_explicit_testmanager_raw_key() {
    let (_dir, config_path, _install_dir, args_log) =
        setup_mcp_va_project_with_options("work", &["/TESTMANAGER"]);
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "launch",
            "mcp",
            "va",
            "--mode",
            "ordinary",
            "--raw-key",
            "/TESTMANAGER",
        ])
        .output()
        .expect("run command");

    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let args = fs::read_to_string(args_log).expect("args log");
    let test_manager_count = args
        .split_whitespace()
        .filter(|arg| arg.eq_ignore_ascii_case("/TESTMANAGER"))
        .count();
    assert_eq!(test_manager_count, 1);
}

#[test]
fn launch_mcp_va_adds_testmanager_when_raw_value_matches_name() {
    let (_dir, config_path, _install_dir, args_log) = setup_mcp_va_project();
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "launch",
            "mcp",
            "va",
            "--mode",
            "ordinary",
            "--raw-key",
            "/VAUser",
            "--raw-key",
            "TESTMANAGER",
        ])
        .output()
        .expect("run command");

    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let args = fs::read_to_string(args_log).expect("args log");
    assert!(args.contains("/VAUser"));
    assert!(args.contains("TESTMANAGER"));
    assert!(args
        .split_whitespace()
        .any(|arg| arg.eq_ignore_ascii_case("/TESTMANAGER")));
}

#[test]
fn launch_mcp_rejects_user_managed_c_payload() {
    let (_dir, config_path, _install_dir, _work_path) = setup_project();
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "launch",
            "mcp",
            "--c",
            "runMcp",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("launch mcp manages /C internally"));
}

#[test]
fn launch_mcp_rejects_user_managed_execute_payload() {
    let (_dir, config_path, _install_dir, _work_path) = setup_project();
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "launch",
            "mcp",
            "--execute",
            "tool.epf",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("launch mcp manages /C internally"));
}

#[test]
fn launch_mcp_rejects_reserved_raw_payloads() {
    let (_dir, config_path, _install_dir, _work_path) = setup_project();
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "launch",
            "mcp",
            "--raw-key",
            "/C\"runOther\"",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not support raw /C"));
}

#[test]
fn launch_mcp_rejects_semicolon_in_mcp_config_path() {
    let (_dir, config_path, _install_dir, _work_path) = setup_project();
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "launch",
            "mcp",
            "--mcp-config",
            "/tmp/conf;mcpPort=1.json",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("must not contain ';'"));
}

#[test]
fn launch_mcp_rejects_zero_mcp_port() {
    let (_dir, config_path, _install_dir, _work_path) = setup_project();
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "launch",
            "mcp",
            "--mcp-port",
            "0",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("--mcp-port must be greater than or equal to 1"));
}

#[test]
fn launch_mcp_va_rejects_semicolon_in_generated_params_path() {
    let (_dir, config_path, _install_dir, _args_log) =
        setup_mcp_va_project_with_work_name("work;bad");
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "launch",
            "mcp",
            "va",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("generated Vanessa params path for launch mcp must not contain ';'"));
}

#[test]
fn launch_non_mcp_rejects_mcp_options() {
    let (_dir, config_path, _install_dir, _work_path) = setup_project();
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "launch",
            "thin",
            "--mcp-port",
            "9876",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains(
        "--mcp-config, --mcp-port, --mode, and MCP_SCENARIO are supported only for `launch mcp`"
    ));
}

// ----------------------------------------------------------------------------
// MCP WS-mode (mcpMode=ws) integration tests
// ----------------------------------------------------------------------------

fn setup_mcp_project_with_logging_thin() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let install_dir = dir.path().join("platform");
    let config_path = dir.path().join("v8project.yaml");
    let args_log = install_dir.join("mcp.args.log");

    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    write_logging_script(&install_dir.join("bin").join("1cv8c"), &args_log);
    write_script(&install_dir.join("bin").join("1cv8"));
    write_config(&config_path, &base_path, &work_path, &install_dir, None);

    (dir, config_path, args_log)
}

#[test]
fn launch_mcp_legacy_transport_emits_runmcp_payload_and_legacy_envelope() {
    let (_dir, config_path, args_log) = setup_mcp_project_with_logging_thin();
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "launch",
            "mcp",
            "--mcp-transport",
            "legacy",
            "--mcp-port",
            "9999",
        ])
        .output()
        .expect("run command");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["data"]["transport"], "legacy");
    assert_eq!(payload["data"]["mcp_port"], 9999);
    assert!(payload["data"]["client_uid"].is_null());

    let args = fs::read_to_string(args_log).expect("args log");
    assert!(args.contains("/C\"runMcp;mcpPort=9999\""));
    assert!(!args.contains("mcpMode=ws"));
}

#[test]
fn launch_mcp_ws_transport_with_listener_emits_ws_payload_and_ws_envelope() {
    let (_dir, config_path, args_log) = setup_mcp_project_with_logging_thin();
    // Spawn an ephemeral listener; the manager_url points at it so the probe
    // succeeds and v8-runner picks the WS branch.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let manager_url = format!(
        "ws://127.0.0.1:{}/sessions",
        listener.local_addr().expect("addr").port()
    );
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "launch",
            "mcp",
            "--mcp-transport",
            "ws",
            "--manager-url",
            &manager_url,
            "--mcp-log-level",
            "debug",
            "--mcp-ws-timeout-ms",
            "2500",
            "--client-uid",
            "00000000-0000-0000-0000-000000000abc",
            "--corr-id",
            "vr-deadbeef",
        ])
        .output()
        .expect("run command");
    drop(listener);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["data"]["transport"], "ws");
    assert_eq!(
        payload["data"]["client_uid"],
        "00000000-0000-0000-0000-000000000abc"
    );
    assert_eq!(payload["data"]["kind"], "v8_runner_client");
    assert_eq!(payload["data"]["manager_url"], manager_url);
    assert_eq!(payload["data"]["corr_id"], "vr-deadbeef");

    let args = fs::read_to_string(args_log).expect("args log");
    assert!(args.contains("mcpMode=ws"));
    assert!(args.contains("client_uid=00000000-0000-0000-0000-000000000abc"));
    assert!(args.contains("kind=v8_runner_client"));
    assert!(args.contains(&format!("manager_url={manager_url}")));
    assert!(args.contains("corr_id=vr-deadbeef"));
    assert!(args.contains("mcp_log_level=debug"));
    assert!(args.contains("mcp_ws_timeout_ms=2500"));
    assert!(!args.contains("runMcp"));
}

#[test]
fn launch_mcp_ws_required_fails_when_manager_unreachable() {
    let (_dir, config_path, _args_log) = setup_mcp_project_with_logging_thin();
    // Bind & immediately drop to grab a guaranteed-free port.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    let manager_url = format!("ws://127.0.0.1:{port}/sessions");
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "launch",
            "mcp",
            "--mcp-transport",
            "ws",
            "--manager-url",
            &manager_url,
        ])
        .output()
        .expect("run command");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stderr}\n{stdout}");
    assert!(
        combined.contains("session-manager unreachable") || combined.contains("unreachable"),
        "expected 'unreachable' diagnostic, got stderr={stderr}, stdout={stdout}"
    );
}

#[test]
fn launch_mcp_auto_falls_back_to_legacy_when_manager_unreachable() {
    let (_dir, config_path, args_log) = setup_mcp_project_with_logging_thin();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    let manager_url = format!("ws://127.0.0.1:{port}/sessions");
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "launch",
            "mcp",
            "--mcp-transport",
            "auto",
            "--manager-url",
            &manager_url,
        ])
        .output()
        .expect("run command");
    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["data"]["transport"], "legacy");
    let args = fs::read_to_string(args_log).expect("args log");
    assert!(args.contains("/C\"runMcp\""));
    assert!(!args.contains("mcpMode=ws"));
}

#[test]
fn launch_mcp_rejects_invalid_manager_url() {
    let (_dir, config_path, _args_log) = setup_mcp_project_with_logging_thin();
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "launch",
            "mcp",
            "--manager-url",
            "ws://bare-host-no-port/sessions",
        ])
        .output()
        .expect("run command");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--manager-url"));
}

#[test]
fn launch_mcp_rejects_zero_ws_timeout() {
    let (_dir, config_path, _args_log) = setup_mcp_project_with_logging_thin();
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "launch",
            "mcp",
            "--mcp-ws-timeout-ms",
            "0",
        ])
        .output()
        .expect("run command");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("--mcp-ws-timeout-ms must be greater than or equal to 1"));
}
