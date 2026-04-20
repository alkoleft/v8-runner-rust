#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::prelude::*;
use tempfile::tempdir;

fn make_executable(path: &Path) {
    let mut perms = fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod");
}

fn write_script(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write");
    make_executable(path);
}

fn setup_extensions_project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let config_path = dir.path().join("v8project.yaml");
    let ibcmd_path = dir.path().join("ibcmd");
    let calls_log = dir.path().join("ibcmd.calls.log");

    fs::create_dir_all(base_path.join("configuration")).expect("configuration dir");
    fs::create_dir_all(base_path.join("exts").join("client-mcp")).expect("client_mcp dir");
    fs::create_dir_all(base_path.join("tests")).expect("tests dir");
    fs::create_dir_all(&work_path).expect("work");
    fs::write(
        base_path.join("exts").join("client-mcp").join(".project"),
        "<projectDescription><name>client_mcp</name></projectDescription>",
    )
    .expect("client_mcp project");
    fs::write(
        base_path.join("tests").join(".project"),
        "<projectDescription><name>tests</name></projectDescription>",
    )
    .expect("tests project");
    write_script(
        &ibcmd_path,
        &format!("printf '%s\\n' \"$*\" >> '{}'\nexit 0", calls_log.display()),
    );

    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: EDT\nbuilder: DESIGNER\nconnection: 'File={}'\nsource-set:\n  - name: configuration\n    type: CONFIGURATION\n    path: configuration\n  - name: client_mcp\n    type: EXTENSION\n    path: exts/client-mcp\n  - name: tests\n    type: EXTENSION\n    path: tests\ntools:\n  platform:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        dir.path().join("ib").display(),
        ibcmd_path.display(),
    );
    fs::write(&config_path, config).expect("config");

    (dir, config_path, calls_log, ibcmd_path)
}

#[test]
fn extensions_command_updates_all_extension_properties() {
    let (_dir, config_path, calls_log, _ibcmd_path) = setup_extensions_project();

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "extensions",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("client_mcp: disable_safety"));
    assert!(stdout.contains("tests: disable_safety"));
    assert!(stdout.contains("Extension properties updated successfully"));

    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("extension update"));
    assert!(calls.contains("--name=client_mcp"));
    assert!(calls.contains("--name=tests"));
    assert!(calls.contains("--safe-mode=no"));
    assert!(calls.contains("--unsafe-action-protection=no"));
}

#[test]
fn extensions_command_filters_by_requested_source_set_names() {
    let (_dir, config_path, calls_log, _ibcmd_path) = setup_extensions_project();

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "extensions",
            "--name",
            "client_mcp",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("--name=client_mcp"));
    assert!(!calls.contains("--name=tests"));
}

#[test]
fn extensions_command_json_failure_reports_operation_target_and_exit_code() {
    let (_dir, config_path, _calls_log, ibcmd_path) = setup_extensions_project();
    write_script(&ibcmd_path, "echo 'cannot update extension' >&2\nexit 17");

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "extensions",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(4));
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["data"]["steps"][0]["ok"], false);
    assert!(payload["data"]["steps"][0]["message"]
        .as_str()
        .expect("message")
        .contains("extension update failed for extension 'client_mcp' with exit code 17"));
    assert!(payload["data"]["steps"][0]["message"]
        .as_str()
        .expect("message")
        .contains("stderr: cannot update extension"));
}
