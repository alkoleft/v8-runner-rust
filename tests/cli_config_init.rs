#![cfg(unix)]

use assert_cmd::prelude::*;
use serde_json::Value;
use std::fs;
use tempfile::tempdir;

#[test]
fn config_init_creates_yaml_with_detected_designer_sources() {
    let dir = tempdir().expect("tempdir");
    let main = dir.path().join("src").join("configuration");
    let ext = dir.path().join("extensions").join("sales");
    fs::create_dir_all(&main).expect("main");
    fs::create_dir_all(&ext).expect("ext");
    fs::write(main.join("Configuration.xml"), "<Configuration/>").expect("main xml");
    fs::write(
        ext.join("Configuration.xml"),
        "<ConfigurationExtensionPurpose>Customization</ConfigurationExtensionPurpose>",
    )
    .expect("ext xml");

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .current_dir(dir.path())
        .args(["config", "init"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let config = fs::read_to_string(dir.path().join("v8project.yaml")).expect("config");
    assert!(config.contains("format: DESIGNER"));
    assert!(config.contains("workPath: 'build'"));
    assert!(config.contains("infobase:"));
    assert!(config.contains("  connection: 'File=build/ib'"));
    assert!(config.contains("path: 'src/configuration'"));
    assert!(config.contains("type: EXTENSION"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Config written"));
}

#[test]
fn config_init_uses_json_envelope_and_config_path_override() {
    let dir = tempdir().expect("tempdir");
    fs::write(dir.path().join("Configuration.xml"), "<Configuration/>").expect("xml");
    let config_path = dir.path().join("custom.yaml");

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .current_dir(dir.path())
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "config",
            "init",
            "--connection",
            "File=/tmp/test-ib",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    assert!(config_path.exists());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "config init");
    assert_eq!(payload["data"]["source_sets"][0]["path"], ".");
    assert_eq!(payload["data"]["source_sets"][0]["type"], "CONFIGURATION");
    let config = fs::read_to_string(config_path).expect("config");
    assert!(config.contains("infobase:"));
    assert!(config.contains("  connection: 'File=/tmp/test-ib'"));
}

#[test]
fn config_init_refuses_to_overwrite_without_force() {
    let dir = tempdir().expect("tempdir");
    fs::write(dir.path().join("v8project.yaml"), "existing").expect("existing");

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .current_dir(dir.path())
        .args(["config", "init"])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("already exists"));
}
