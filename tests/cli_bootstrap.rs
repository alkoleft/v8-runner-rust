#![cfg(unix)]

use assert_cmd::prelude::*;
use serde_json::Value;
use std::fs;
use tempfile::tempdir;

#[test]
fn missing_config_in_text_mode_returns_validation_error_on_stderr() {
    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
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
    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            "/definitely/missing/v8project.yaml",
            "--output",
            "json",
            "build",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));

    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "error");
    assert_eq!(payload["duration_ms"], 0);
    assert_eq!(
        payload["data"]["message"],
        "config file not found: /definitely/missing/v8project.yaml"
    );
}

#[test]
fn default_config_path_uses_v8project_yaml_from_current_dir() {
    let dir = tempdir().expect("tempdir");
    let config_path = dir.path().join("v8project.yaml");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    fs::write(
        &config_path,
        format!(
            "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: .\n",
            base_path.display(),
            work_path.display()
        ),
    )
    .expect("config");

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .current_dir(dir.path())
        .args(["--output", "json", "build"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "build");
}

#[test]
fn mcp_rejects_clean_before_execution_flag() {
    let dir = tempdir().expect("tempdir");
    let config_path = dir.path().join("v8project.yaml");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    fs::write(
        &config_path,
        format!(
            "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: .\n",
            base_path.display(),
            work_path.display()
        ),
    )
    .expect("config");

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
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
