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

fn write_designer_script(path: &Path, calls_log: &Path) {
    let body = format!(
        "args=\"$*\"\nout=\"\"\nreport=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  if [ \"$prev\" = \"-ReportFile\" ]; then report=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nprintf '%s\\n' \"$args\" >> \"{}\"\nif [ -n \"$out\" ]; then mkdir -p \"$(dirname \"$out\")\"; printf 'designer log for %s\\n' \"$args\" > \"$out\"; fi\nif printf '%s' \"$args\" | grep -F -q -- '/CompareCfg'; then\n  if printf '%s' \"$args\" | grep -F -q -- 'VendorConfiguration'; then\n    printf 'configuration is not on support\\n' >&2\n    exit 17\n  fi\n  if printf '%s' \"$args\" | grep -F -q -- 'ExtensionDBConfiguration'; then\n    if printf '%s' \"$args\" | grep -F -q -- 'ExistingExt'; then\n      : > \"$report\"\n      exit 0\n    fi\n    printf 'extension not found\\n' >&2\n    exit 19\n  fi\nfi\nexit 0",
        calls_log.display()
    );
    write_script(path, &body);
}

fn write_config(
    path: &Path,
    base_path: &Path,
    work_path: &Path,
    platform_path: &Path,
    format: &str,
) {
    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: {}\nbuilder: DESIGNER\nconnection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: main\ntools:\n  platform:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        format,
        platform_path.display(),
    );
    fs::write(path, config).expect("config");
}

fn setup_project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let config_path = dir.path().join("v8project.yaml");
    let binary_path = dir.path().join("1cv8");
    let calls_log = dir.path().join("calls.log");

    fs::create_dir_all(base_path.join("main")).expect("main");
    fs::create_dir_all(&work_path).expect("work");
    write_designer_script(&binary_path, &calls_log);
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &binary_path,
        "DESIGNER",
    );

    (dir, config_path, binary_path, base_path, calls_log)
}

#[test]
fn load_cf_json_success_runs_probe_load_and_update() {
    let (_dir, config_path, _binary_path, base_path, calls_log) = setup_project();
    fs::write(base_path.join("release.cf"), "cf").expect("artifact");

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "load",
            "--path",
            "release.cf",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "load");
    assert_eq!(payload["data"]["artifact_type"], "configuration_cf");
    assert_eq!(payload["data"]["compatibility_state"], "not_supported");
    assert_eq!(payload["data"]["execution"]["payload"]["applied"], true);
    assert_eq!(
        payload["data"]["execution"]["payload"]["update_db_cfg_ran"],
        true
    );
    assert!(payload["data"]["platform_log_path"].is_string());

    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("/CompareCfg"));
    assert!(calls.contains("/LoadCfg"));
    assert!(calls.contains("/UpdateDBCfg"));
}

#[test]
fn merge_cfe_json_success_requires_extension_and_settings() {
    let (_dir, config_path, _binary_path, base_path, calls_log) = setup_project();
    fs::write(base_path.join("release.cfe"), "cfe").expect("artifact");
    fs::write(base_path.join("merge.xml"), "<settings/>").expect("settings");

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "load",
            "--path",
            "release.cfe",
            "--mode",
            "merge",
            "--settings",
            "merge.xml",
            "--extension",
            "ExistingExt",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["data"]["artifact_type"], "extension_cfe");
    assert_eq!(payload["data"]["extension"], "ExistingExt");
    assert_eq!(payload["data"]["compatibility_state"], "supported");
    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("ExtensionConfiguration"));
    assert!(calls.contains("/MergeCfg"));
    assert!(calls.contains("-Settings"));
    assert!(calls.contains("-Extension ExistingExt"));
    assert!(calls.contains("/UpdateDBCfg -Extension ExistingExt"));
}

#[test]
fn load_update_mode_returns_validation_payload() {
    let (_dir, config_path, _binary_path, base_path, _calls_log) = setup_project();
    fs::write(base_path.join("release.cf"), "cf").expect("artifact");

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "load",
            "--path",
            "release.cf",
            "--mode",
            "update",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "load");
    assert_eq!(payload["data"]["mode"], "update");
    assert_eq!(payload["data"]["execution"]["payload"]["applied"], false);
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("not supported"));
}

#[test]
fn load_rejects_edt_format_even_with_designer_builder() {
    let (_dir, config_path, _binary_path, base_path, _calls_log) = setup_project();
    fs::write(base_path.join("release.cf"), "cf").expect("artifact");
    write_config(
        &config_path,
        &base_path,
        &config_path.parent().expect("parent").join("work"),
        &config_path.parent().expect("parent").join("1cv8"),
        "EDT",
    );

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "load",
            "--path",
            "release.cf",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("builder=DESIGNER and format=DESIGNER"));
}

#[test]
fn load_rejects_unknown_artifact_type_with_unknown_payload_metadata() {
    let (_dir, config_path, _binary_path, base_path, _calls_log) = setup_project();
    fs::write(base_path.join("release.zip"), "zip").expect("artifact");

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "load",
            "--path",
            "release.zip",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["data"]["artifact_type"], "unknown");
    assert_eq!(payload["data"]["target_kind"], "unknown");
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("only .cf and .cfe"));
}

#[test]
fn load_rejects_external_artifact_type_with_unknown_target_kind_payload_metadata() {
    let (_dir, config_path, _binary_path, base_path, _calls_log) = setup_project();
    fs::write(base_path.join("tool.epf"), "epf").expect("artifact");

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "load",
            "--path",
            "tool.epf",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(
        payload["data"]["artifact_type"],
        "external_data_processor_epf"
    );
    assert_eq!(payload["data"]["target_kind"], "unknown");
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("only .cf and .cfe"));
}
