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
    let staged = path.with_extension("tmp");
    fs::write(&staged, "#!/bin/sh\nsleep 1\n").expect("write");
    make_executable(&staged);
    fs::rename(&staged, path).expect("rename");
}

fn write_logging_script(path: &Path, args_log: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    let staged = path.with_extension("tmp");
    fs::write(
        &staged,
        format!(
            "#!/bin/sh\nprintf '%s\n' \"$@\" > '{}'\nsleep 1\n",
            args_log.display()
        ),
    )
    .expect("write");
    make_executable(&staged);
    fs::rename(&staged, path).expect("rename");
}

fn write_config(
    path: &Path,
    base_path: &Path,
    work_path: &Path,
    platform_path: &Path,
    platform_version: Option<&str>,
) {
    let mut config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    purpose: CONFIGURATION\n    path: .\ntools:\n  platform:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        platform_path.display(),
    );
    if let Some(platform_version) = platform_version {
        config.push_str(&format!("    version: '{}'\n", platform_version));
    }

    fs::write(path, config).expect("config");
}

fn setup_project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let install_dir = dir.path().join("platform");
    let config_path = dir.path().join("v8project.yaml");

    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    write_script(&install_dir.join("bin").join("1cv8"));
    write_script(&install_dir.join("bin").join("1cv8c"));
    write_config(&config_path, &base_path, &work_path, &install_dir, None);

    (dir, config_path, install_dir, work_path)
}

fn setup_versioned_project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let root_path = dir.path().join("platform-root");
    let version = root_path.join("8.3.25.1234");
    let config_path = dir.path().join("v8project.yaml");

    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    write_script(&version.join("bin").join("1cv8"));
    write_script(&version.join("bin").join("1cv8c"));
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &root_path,
        Some("8.3.25.1234"),
    );

    (dir, config_path, version, work_path)
}

#[test]
fn launch_json_returns_pid_and_selected_binary() {
    let (_dir, config_path, install_dir, _work_path) = setup_project();
    let output = std::process::Command::cargo_bin("v8-runner")
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

    let output = std::process::Command::cargo_bin("v8-runner")
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
    assert!(stdout.contains("[Запуск]"));
    assert!(stdout.contains("Launched конфигуратор via"));
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
    let output = std::process::Command::cargo_bin("v8-runner")
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

#[test]
fn launch_uses_versioned_root_hint() {
    let (_dir, config_path, version_dir, _work_path) = setup_versioned_project();
    let output = std::process::Command::cargo_bin("v8-runner")
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
    assert_eq!(
        payload["data"]["binary"].as_str().expect("binary"),
        version_dir.join("bin").join("1cv8c").to_string_lossy()
    );
}

#[test]
fn launch_fails_when_process_exits_during_startup_probe() {
    let (_dir, config_path, install_dir, _work_path) = setup_project();
    let thin = install_dir.join("bin").join("1cv8c");
    let staged = thin.with_extension("tmp");
    fs::write(&staged, "#!/bin/sh\nexit 9\n").expect("write");
    make_executable(&staged);
    fs::rename(&staged, &thin).expect("rename");

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "launch",
            "--mode",
            "thin",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&output.stderr).contains("exited before startup completed"));
}

#[test]
fn launch_json_failure_keeps_stdout_empty_and_exit_code() {
    let (_dir, config_path, install_dir, _work_path) = setup_project();
    let thin = install_dir.join("bin").join("1cv8c");
    let staged = thin.with_extension("tmp");
    fs::write(&staged, "#!/bin/sh\nexit 9\n").expect("write");
    make_executable(&staged);
    fs::rename(&staged, &thin).expect("rename");

    let output = std::process::Command::cargo_bin("v8-runner")
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

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn launch_ordinary_supports_typed_keys_and_filters_reserved_raw_duplicates() {
    let (_dir, config_path, install_dir, _work_path) = setup_project();
    let args_log = install_dir.join("ordinary.args.log");
    write_logging_script(&install_dir.join("bin").join("1cv8"), &args_log);

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "launch",
            "--mode",
            "ordinary",
            "--c",
            "DoWork",
            "--execute",
            "/tmp/tool.epf",
            "--use-privileged-mode",
            "--out",
            "/tmp/user.out.log",
            "--raw-key",
            "/RunModeOrdinaryApplication",
            "--raw-key",
            "/Out",
            "--raw-key",
            "/tmp/ignored.out.log",
            "--raw-key",
            "/WA-",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let args = fs::read_to_string(args_log).expect("args log");
    assert!(args.contains("ENTERPRISE"));
    assert!(args.contains("/DisableStartupDialogs"));
    assert_eq!(args.matches("/RunModeOrdinaryApplication").count(), 1);
    assert!(args.contains("/UsePrivilegedMode"));
    assert!(args.contains("/Execute"));
    assert!(args.contains("/tmp/tool.epf"));
    assert!(args.contains("/C"));
    assert!(args.contains("DoWork"));
    assert!(args.contains("/WA-"));
    assert!(args.contains("/tmp/user.out.log"));
    assert!(!args.contains("/tmp/ignored.out.log"));
}
