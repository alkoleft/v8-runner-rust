#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::prelude::*;
use serde_json::Value;
use tempfile::tempdir;

fn make_executable(path: &Path) {
    let mut perms = fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod");
}

fn write_script(path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, "#!/bin/sh\nexit 0\n").expect("write");
    make_executable(path);
}

fn write_config(path: &Path, base_path: &Path, work_path: &Path, platform_path: &Path) {
    fs::write(
        path,
        format!(
            "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    purpose: CONFIGURATION\n    path: .\ntools:\n  platform:\n    path: '{}'\n",
            base_path.display(),
            work_path.display(),
            platform_path.display(),
        ),
    )
    .expect("config");
}

fn setup_project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let install_dir = dir.path().join("platform");
    let config_path = dir.path().join("application.yaml");

    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    write_script(&install_dir.join("bin").join("1cv8"));
    write_script(&install_dir.join("bin").join("1cv8c"));
    write_config(&config_path, &base_path, &work_path, &install_dir);

    (dir, config_path, install_dir, work_path)
}

#[test]
fn launch_json_returns_pid_and_selected_binary() {
    let (_dir, config_path, install_dir, _work_path) = setup_project();
    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "launch",
            "--mode",
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

    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--clean-before-execution",
            "launch",
            "--mode",
            "designer",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Launched designer via"));
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
fn launch_thick_uses_v8_binary() {
    let (_dir, config_path, install_dir, _work_path) = setup_project();
    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "launch",
            "--mode",
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
