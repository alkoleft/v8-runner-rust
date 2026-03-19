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

fn write_build_script(path: &Path, fail_pattern: Option<&str>) {
    let pattern_branch = fail_pattern
        .map(|pattern| {
            format!(
                "if printf '%s' \"$args\" | grep -F -q -- '{}'; then exit 17; fi",
                pattern
            )
        })
        .unwrap_or_default();
    let body = format!(
        "args=\"$*\"\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf 'designer log for %s\\n' \"$args\" > \"$out\"; fi\n{}\nexit 0",
        pattern_branch
    );
    write_script(path, &body);
}

fn write_config(path: &Path, base_path: &Path, work_path: &Path, platform_path: &Path) {
    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: 'File=/tmp/ib'\nbuild:\n  partialLoadThreshold: 20\nsource-set:\n  - name: main\n    purpose: CONFIGURATION\n    path: main\n  - name: ext\n    purpose: EXTENSION\n    path: ext\ntools:\n  platform:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        platform_path.display(),
    );

    fs::write(path, config).expect("config");
}

fn setup_project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let config_path = dir.path().join("application.yaml");
    let binary_path = dir.path().join("1cv8");

    fs::create_dir_all(base_path.join("main").join("Catalogs.Items")).expect("main");
    fs::create_dir_all(base_path.join("ext").join("CommonModules")).expect("ext");
    fs::create_dir_all(&work_path).expect("work");
    fs::write(
        base_path
            .join("main")
            .join("Catalogs.Items")
            .join("ObjectModule.bsl"),
        "procedure Test() endprocedure",
    )
    .expect("main bsl");
    fs::write(
        base_path
            .join("main")
            .join("Catalogs.Items")
            .join("ObjectModule.xml"),
        "<MetaDataObject />",
    )
    .expect("main xml");
    fs::write(
        base_path
            .join("ext")
            .join("CommonModules")
            .join("Module.bsl"),
        "procedure Test() endprocedure",
    )
    .expect("ext bsl");

    write_build_script(&binary_path, None);
    write_config(&config_path, &base_path, &work_path, &binary_path);

    (dir, config_path, binary_path, work_path)
}

#[test]
fn build_json_failure_returns_step_payload() {
    let (_dir, config_path, binary_path, _work_path) = setup_project();
    write_build_script(&binary_path, Some("/UpdateDBCfg -Extension ext"));

    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "build",
            "--full-rebuild",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(4));

    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "build");
    assert_eq!(payload["data"]["ok"], false);
    assert_eq!(payload["data"]["steps"][0]["source_set"], "main");
    assert_eq!(payload["data"]["steps"][0]["ok"], true);
    assert_eq!(payload["data"]["steps"][1]["source_set"], "ext");
    assert_eq!(payload["data"]["steps"][1]["ok"], false);
    assert!(payload["data"]["steps"][1]["message"]
        .as_str()
        .expect("message")
        .contains("exit code 17"));
}

#[test]
fn build_text_failure_does_not_print_success_footer() {
    let (_dir, config_path, binary_path, _work_path) = setup_project();
    write_build_script(&binary_path, Some("/UpdateDBCfg -Extension ext"));

    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "build",
            "--full-rebuild",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Build failed"));
    assert!(!stdout.contains("Build completed successfully"));
}

#[test]
fn build_text_stdout_includes_action_logs() {
    let (_dir, config_path, _binary_path, _work_path) = setup_project();

    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args(["--config", &config_path.display().to_string(), "build"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("starting command"));
    assert!(stdout.contains("T"));
    assert!(stdout.contains("found_changes="));
    assert!(stdout.contains("executing build step"));
    assert!(stdout.contains("running process"));
}

#[test]
fn build_json_writes_action_log_file_without_polluting_stdout() {
    let (_dir, config_path, _binary_path, work_path) = setup_project();

    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "build",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let _payload: Value = serde_json::from_slice(&output.stdout).expect("json");

    let action_log = work_path.join("logs").join("mcp").join("actions.log");
    let contents = fs::read_to_string(action_log).expect("action log");
    assert!(contents.contains("starting command"));
    assert!(contents.contains("running process"));
}
