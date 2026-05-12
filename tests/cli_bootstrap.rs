#![cfg(unix)]

mod support;

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use support::{temp_workspace, v8_runner_command};

fn write_minimal_config(dir: &Path) -> PathBuf {
    let config_path = dir.join("v8project.yaml");
    let base_path = dir.join("project");
    let work_path = dir.join("work");
    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    fs::write(
        &config_path,
        format!(
            "workPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: project\n",
            work_path.display()
        ),
    )
    .expect("config");
    config_path
}

#[test]
fn missing_config_in_text_mode_returns_validation_error_on_stderr() {
    let output = v8_runner_command()
        .args(["--config", "/definitely/missing/v8project.yaml", "build"])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("config file not found"));
}

#[test]
fn missing_config_in_json_mode_keeps_error_envelope_shape() {
    let output = v8_runner_command()
        .args([
            "--config",
            "/definitely/missing/v8project.yaml",
            "--json-message",
            "build",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));

    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "build");
    assert_eq!(payload["duration_ms"], 0);
    assert_eq!(payload["error"]["code"], "invalid_argument");
    assert_eq!(payload["error"]["kind"], "validation");
    assert_eq!(
        payload["data"]["message"],
        "config file not found: /definitely/missing/v8project.yaml"
    );
}

#[test]
fn default_config_path_uses_v8project_yaml_from_current_dir() {
    let dir = temp_workspace();
    let _config_path = write_minimal_config(dir.path());

    let output = v8_runner_command()
        .current_dir(dir.path())
        .args(["--json-message", "build"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "build");
}

#[test]
fn default_config_path_applies_sibling_local_overlay() {
    let dir = temp_workspace();
    let _config_path = write_minimal_config(dir.path());
    let local_work_path = dir.path().join("local-work");
    fs::write(
        dir.path().join("v8project.local.yaml"),
        "workPath: local-work\n",
    )
    .expect("local overlay");

    let output = v8_runner_command()
        .current_dir(dir.path())
        .args(["--json-message", "build"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "build");
    assert!(local_work_path.exists());
}

#[test]
fn unsupported_main_config_shape_is_rejected_in_json_mode() {
    let dir = temp_workspace();
    let config_path = write_minimal_config(dir.path());
    let mut config = fs::read_to_string(&config_path).expect("config");
    config.push_str("tools:\n  platform:\n    typo: value\n");
    fs::write(&config_path, config).expect("config");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "build",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["command"], "build");
    assert_eq!(payload["error"]["code"], "invalid_argument");
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("config contains unsupported key or value"));
}

#[test]
fn unsupported_local_overlay_shape_is_rejected_in_json_mode() {
    let dir = temp_workspace();
    let config_path = write_minimal_config(dir.path());
    fs::write(
        dir.path().join("v8project.local.yaml"),
        "tools:\n  platform:\n    typo: value\n",
    )
    .expect("local overlay");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "build",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["command"], "build");
    assert_eq!(payload["error"]["code"], "invalid_argument");
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("local config overlay contains unsupported key or value"));
}

#[test]
fn action_logging_failure_in_json_mode_keeps_command_identity() {
    let dir = temp_workspace();
    let config_path = write_minimal_config(dir.path());
    let log_path = dir.path().join("action-log-as-dir");
    fs::create_dir_all(&log_path).expect("log dir");

    let output = v8_runner_command()
        .env("V8TR_ACTION_LOG_FILE", &log_path)
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "build",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(3));

    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "build");
    assert_eq!(payload["error"]["code"], "runtime_failure");
    assert_eq!(payload["error"]["kind"], "runtime");
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("failed to open action log file"));
}

#[test]
fn test_module_pre_dispatch_validation_in_json_mode_keeps_command_identity() {
    let dir = temp_workspace();
    let config_path = write_minimal_config(dir.path());

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "test",
            "yaxunit",
            "module",
            "   ",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));

    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "test");
    assert_eq!(payload["error"]["code"], "invalid_argument");
    assert_eq!(payload["error"]["kind"], "validation");
    assert_eq!(
        payload["data"]["message"],
        "test module requires a non-empty module name"
    );
}

#[test]
fn artifacts_pre_dispatch_validation_in_json_mode_keeps_command_identity() {
    let dir = temp_workspace();
    let config_path = write_minimal_config(dir.path());

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "make",
            "--output",
            &dir.path().join("out.cf").display().to_string(),
            "--source-set",
            "missing",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));

    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "make");
    assert_eq!(payload["error"]["code"], "invalid_argument");
    assert_eq!(payload["error"]["kind"], "validation");
    assert_eq!(payload["data"]["message"], "unknown source-set 'missing'");
}

#[test]
fn mcp_rejects_clean_before_execution_flag() {
    let dir = temp_workspace();
    let config_path = write_minimal_config(dir.path());

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--clean-before-execution",
            "mcp",
            "serve",
            "stdio",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("--clean-before-execution is not supported for MCP transports"));
}

#[test]
fn legacy_top_level_connection_is_rejected_in_json_mode() {
    let dir = temp_workspace();
    let config_path = dir.path().join("v8project.yaml");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    fs::write(
        &config_path,
        format!(
            "workPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: project\n",
            work_path.display()
        ),
    )
    .expect("config");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "build",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["command"], "build");
    assert_eq!(payload["error"]["code"], "invalid_argument");
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("legacy top-level key 'connection'"));
}

#[test]
fn legacy_top_level_credentials_is_rejected_in_json_mode() {
    let dir = temp_workspace();
    let config_path = dir.path().join("v8project.yaml");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    fs::write(
        &config_path,
        format!(
            "workPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\ncredentials:\n  user: Admin\n  password: secret\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: project\n",
            work_path.display()
        ),
    )
    .expect("config");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "build",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["command"], "build");
    assert_eq!(payload["error"]["code"], "invalid_argument");
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("legacy top-level key 'credentials'"));
}

#[test]
fn top_level_execution_timeout_seconds_is_rejected_in_json_mode() {
    let dir = temp_workspace();
    let config_path = dir.path().join("v8project.yaml");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    fs::write(
        &config_path,
        format!(
            "workPath: '{}'\nexecution_timeout_seconds: 300\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: project\n",
            work_path.display()
        ),
    )
    .expect("config");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "build",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["command"], "build");
    assert_eq!(payload["error"]["code"], "invalid_argument");
    let message = payload["data"]["message"].as_str().expect("message");
    assert!(message.contains("top-level key 'execution_timeout_seconds'"));
    assert!(message.contains("execution_timeout in milliseconds"));
}
