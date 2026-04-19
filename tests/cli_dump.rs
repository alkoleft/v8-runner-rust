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

fn write_script(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write");
    make_executable(path);
}

fn write_ibcmd_script(path: &Path, calls_log: &Path, fail_pattern: Option<&str>) {
    let pattern_branch = fail_pattern
        .map(|pattern| {
            format!(
                "if printf '%s' \"$args\" | grep -F -q -- '{}'; then exit 17; fi",
                pattern
            )
        })
        .unwrap_or_default();
    let body = format!(
        "args=\"$*\"\nprintf '%s\\n' \"$args\" >> \"{}\"\n{}\nmkdir -p \"$(printf '%s' \"$args\" | awk '{{print $NF}}')\"\nexit 0",
        calls_log.display(),
        pattern_branch
    );
    write_script(path, &body);
}

fn write_config(path: &Path, base_path: &Path, work_path: &Path, platform_path: &Path) {
    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: IBCMD\nconnection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    purpose: CONFIGURATION\n    path: main\ntools:\n  platform:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        platform_path.display(),
    );

    fs::write(path, config).expect("config");
}

fn setup_project() -> (
    tempfile::TempDir,
    PathBuf,
    PathBuf,
    PathBuf,
    PathBuf,
    PathBuf,
) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let config_path = dir.path().join("v8project.yaml");
    let binary_path = dir.path().join("ibcmd");
    let calls_log = dir.path().join("calls.log");

    fs::create_dir_all(base_path.join("main")).expect("main");
    fs::create_dir_all(&work_path).expect("work");
    fs::write(base_path.join("main").join("old.txt"), "old").expect("old");

    write_ibcmd_script(&binary_path, &calls_log, None);
    write_config(&config_path, &base_path, &work_path, &binary_path);

    (
        dir,
        config_path,
        binary_path,
        work_path,
        base_path,
        calls_log,
    )
}

#[test]
fn dump_ibcmd_full_json_success() {
    let (_dir, config_path, _binary_path, _work_path, base_path, calls_log) = setup_project();

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "dump",
            "--mode",
            "full",
            "--source-set",
            "main",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("--force"));
    assert!(base_path.join("main").exists());
}

#[test]
fn dump_ibcmd_incremental_json_success() {
    let (_dir, config_path, _binary_path, _work_path, base_path, calls_log) = setup_project();
    fs::remove_dir_all(base_path.join("main")).expect("remove target");

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "dump",
            "--mode",
            "incremental",
            "--source-set",
            "main",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("--sync"));
    assert!(calls.contains(base_path.join("main").display().to_string().as_str()));
}

#[test]
fn dump_ibcmd_partial_json_success_uses_degraded_fallback() {
    let (_dir, config_path, _binary_path, _work_path, _base_path, calls_log) = setup_project();

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "dump",
            "--mode",
            "partial",
            "--source-set",
            "main",
            "--object",
            "Catalog.Items",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    let data = &payload["data"];
    assert_eq!(payload["ok"], true);
    assert_eq!(data["mode"], "PARTIAL");
    assert!(data["message"]
        .as_str()
        .expect("message")
        .contains("IBCMD does not support object-scoped partial dump"));
    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("--sync"));
}

#[test]
fn dump_ibcmd_partial_failure_keeps_partial_mode_and_warning() {
    let (_dir, config_path, binary_path, _work_path, _base_path, calls_log) = setup_project();
    write_ibcmd_script(&binary_path, &calls_log, Some("--sync"));

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "dump",
            "--mode",
            "partial",
            "--source-set",
            "main",
            "--object",
            "Catalog.Items",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(4));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    let data = &payload["data"];
    assert_eq!(payload["ok"], false);
    assert_eq!(data["mode"], "PARTIAL");
    assert!(data["message"]
        .as_str()
        .expect("message")
        .contains("IBCMD does not support object-scoped partial dump"));
    assert!(data["message"]
        .as_str()
        .expect("message")
        .contains("dump failed for source-set 'main' with exit code 17"));
}
