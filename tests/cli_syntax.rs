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

fn write_config(path: &Path, base_path: &Path, work_path: &Path, platform_path: &Path) {
    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    purpose: CONFIGURATION\n    path: .\ntools:\n  platform:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        platform_path.display(),
    );
    fs::write(path, config).expect("config");
}

fn setup_project(script_body: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let install_dir = dir.path().join("platform");
    let config_path = dir.path().join("application.yaml");

    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    write_script(&install_dir.join("bin").join("1cv8"), script_body);
    write_config(&config_path, &base_path, &work_path, &install_dir);

    (dir, config_path)
}

#[test]
fn syntax_designer_config_json_returns_clean_envelope() {
    let (_dir, config_path) = setup_project(
        "out=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf '' > \"$out\"; fi\nprintf 'RAW_STDOUT\\n'\nexit 0",
    );

    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "syntax",
            "designer-config",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "syntax");
    assert_eq!(payload["data"]["check_name"], "designer-config");
    assert_eq!(payload["data"]["status"], "clean");
    assert_eq!(payload["data"]["exit_code"], 0);
}

#[test]
fn syntax_designer_modules_json_returns_structured_validation_failure() {
    let (_dir, config_path) = setup_project(
        "args=\"$*\"\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif printf '%s' \"$args\" | grep -F -q -- '/CheckModules'; then\n  cat <<'LOG' > \"$out\"\n{CommonModules.TestModule(4,2)}: Ошибка компиляции\n{1}: context\nLOG\n  exit 101\nfi\nexit 0",
    );

    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "syntax",
            "designer-modules",
            "--server",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(3));

    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["data"]["status"], "issues_found");
    assert_eq!(payload["data"]["exit_code"], 101);
    assert_eq!(payload["data"]["summary"]["errors"], 1);
    assert_eq!(payload["data"]["issues"][0]["kind"], "module");
    assert_eq!(
        payload["data"]["issues"][0]["path"],
        "CommonModules.TestModule"
    );
}

#[test]
fn syntax_text_output_hides_raw_stdout_and_prints_structured_issue() {
    let (_dir, config_path) = setup_project(
        "args=\"$*\"\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nprintf 'RAW_STDOUT\\n'\nif printf '%s' \"$args\" | grep -F -q -- '/CheckModules'; then\n  cat <<'LOG' > \"$out\"\nCommonModules.TestModule Warning: потенциальная проблема\nLOG\n  exit 101\nfi\nexit 0",
    );

    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "syntax",
            "designer-modules",
            "--server",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(3));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("designer-modules"));
    assert!(stdout.contains("WARNING CommonModules.TestModule"));
    assert!(!stdout.contains("RAW_STDOUT"));
}
