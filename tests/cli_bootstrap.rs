#![cfg(unix)]

use assert_cmd::prelude::*;
use serde_json::Value;
use std::fs;
use tempfile::tempdir;

#[test]
fn missing_config_in_text_mode_returns_validation_error_on_stderr() {
    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args(["--config", "/definitely/missing/application.yaml", "build"])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("config file not found"));
}

#[test]
fn missing_config_in_json_mode_keeps_error_envelope_shape() {
    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            "/definitely/missing/application.yaml",
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
        "config file not found: /definitely/missing/application.yaml"
    );
}

#[test]
fn mcp_rejects_clean_before_execution_flag() {
    let dir = tempdir().expect("tempdir");
    let config_path = dir.path().join("application.yaml");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    fs::write(
        &config_path,
        format!(
            "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    purpose: CONFIGURATION\n    path: .\n",
            base_path.display(),
            work_path.display()
        ),
    )
    .expect("config");

    let output = std::process::Command::cargo_bin("v8-test-runner")
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
