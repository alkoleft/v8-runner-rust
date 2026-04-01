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

fn setup_designer_init_project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let config_path = dir.path().join("application.yaml");
    let v8_path = dir.path().join("1cv8");
    let infobase_path = dir.path().join("ib");

    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    write_script(
        &v8_path,
        &format!(
            "if [ \"$1\" = \"CREATEINFOBASE\" ]; then mkdir -p \"{}\" && : > \"{}/1Cv8.1CD\"; fi\nexit 0",
            infobase_path.display(),
            infobase_path.display()
        ),
    );

    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: 'File={}'\nsource-set:\n  - name: main\n    purpose: CONFIGURATION\n    path: main\ntools:\n  platform:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        infobase_path.display(),
        v8_path.display(),
    );
    fs::write(&config_path, config).expect("config");

    (dir, config_path, work_path, infobase_path)
}

fn setup_edt_init_project(
    format: &str,
    builder: &str,
    connection: &str,
) -> (
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
    let config_path = dir.path().join("application.yaml");
    let platform_path = dir
        .path()
        .join(if builder == "IBCMD" { "ibcmd" } else { "1cv8" });
    let edt_path = dir.path().join("1cedtcli");
    let edt_calls_log = dir.path().join("edt.calls.log");
    let infobase_path = dir.path().join("ib");
    let resolved_connection = if connection == "__AUTO_FILE__" {
        format!("File={}", infobase_path.display())
    } else {
        connection.to_owned()
    };

    fs::create_dir_all(base_path.join("main")).expect("main");
    fs::create_dir_all(base_path.join("ext")).expect("ext");
    fs::create_dir_all(&work_path).expect("work");
    let platform_body = if builder == "IBCMD" {
        "if [ \"$1\" = \"infobase\" ] && [ \"$2\" = \"create\" ]; then\n  for arg in \"$@\"; do\n    case \"$arg\" in --database-path=*) path=${arg#--database-path=} ;; esac\n  done\n  mkdir -p \"$path\" && : > \"$path/1Cv8.1CD\"\nfi\nexit 0"
            .to_owned()
    } else {
        "if [ \"$1\" = \"CREATEINFOBASE\" ]; then\n  path=\"$2\"\n  path=${path#File=\\'}\n  path=${path%\\'}\n  mkdir -p \"$path\" && : > \"$path/1Cv8.1CD\"\nfi\nexit 0"
            .to_owned()
    };
    write_script(&platform_path, &platform_body);
    write_script(
        &edt_path,
        &format!(
            "printf '%s\\n' \"$*\" >> \"{}\"\nexit 0",
            edt_calls_log.display()
        ),
    );

    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: {}\nbuilder: {}\nconnection: '{}'\nsource-set:\n  - name: main\n    purpose: CONFIGURATION\n    path: main\n  - name: ext\n    purpose: EXTENSION\n    path: ext\ntools:\n  platform:\n    path: '{}'\n  edt_cli:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        format,
        builder,
        resolved_connection,
        platform_path.display(),
        edt_path.display(),
    );
    fs::write(&config_path, config).expect("config");

    (
        dir,
        config_path,
        work_path,
        base_path,
        platform_path,
        edt_calls_log,
    )
}

#[test]
fn init_designer_creates_infobase_and_skips_edt_workspace() {
    let (_dir, config_path, work_path, infobase_path) = setup_designer_init_project();

    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args(["--config", &config_path.display().to_string(), "init"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    assert!(infobase_path.join("1Cv8.1CD").exists());
    assert!(!work_path.join("edt-workspace").exists());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("infobase: create"));
    assert!(stdout.contains("edt_workspace: import"));
}

#[test]
fn init_ibcmd_creates_infobase_and_imports_edt_projects_in_order() {
    let (_dir, config_path, work_path, _base_path, _platform_path, edt_calls_log) =
        setup_edt_init_project("DESIGNER", "IBCMD", "__AUTO_FILE__");

    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args(["--config", &config_path.display().to_string(), "init"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let config = fs::read_to_string(&config_path).expect("config");
    let connection_line = config
        .lines()
        .find(|line| line.starts_with("connection:"))
        .expect("connection line");
    let infobase_dir = connection_line
        .split("File=")
        .nth(1)
        .expect("file path")
        .trim_matches('\'');
    assert!(Path::new(infobase_dir).join("1Cv8.1CD").exists());
    assert!(!work_path.join("edt-workspace").exists());
    assert!(!edt_calls_log.exists());
}

#[test]
fn init_edt_imports_projects_in_configuration_then_extension_order() {
    let (_dir, config_path, work_path, base_path, _platform_path, edt_calls_log) =
        setup_edt_init_project("EDT", "DESIGNER", "__AUTO_FILE__");

    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args(["--config", &config_path.display().to_string(), "init"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let config = fs::read_to_string(&config_path).expect("config");
    let connection_line = config
        .lines()
        .find(|line| line.starts_with("connection:"))
        .expect("connection line");
    let infobase_dir = connection_line
        .split("File=")
        .nth(1)
        .expect("file path")
        .trim_matches('\'');
    assert!(Path::new(infobase_dir).join("1Cv8.1CD").exists());
    assert!(work_path.join("edt-workspace").exists());
    let calls = fs::read_to_string(edt_calls_log).expect("calls");
    let lines: Vec<_> = calls.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains(&base_path.join("main").display().to_string()));
    assert!(lines[1].contains(&base_path.join("ext").display().to_string()));
}

#[test]
fn init_non_file_connection_keeps_running_workspace_step_and_returns_payload() {
    let (_dir, config_path, work_path, base_path, _platform_path, edt_calls_log) =
        setup_edt_init_project("EDT", "DESIGNER", "Srvr=demo;Ref=test");

    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "init",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["command"], "init");
    assert_eq!(payload["data"]["steps"][0]["status"], "failed");
    assert_eq!(payload["data"]["steps"][1]["status"], "ok");
    assert!(work_path.join("edt-workspace").exists());
    let calls = fs::read_to_string(edt_calls_log).expect("calls");
    let lines: Vec<_> = calls.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains(&base_path.join("main").display().to_string()));
}

#[test]
fn init_skips_existing_workspace() {
    let (_dir, config_path, work_path, _base_path, _platform_path, edt_calls_log) =
        setup_edt_init_project("EDT", "DESIGNER", "__AUTO_FILE__");
    fs::create_dir_all(work_path.join("edt-workspace")).expect("workspace");
    fs::write(
        work_path.join("edt-workspace").join(".v8tr-initialized"),
        "ok\n",
    )
    .expect("marker");

    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "init",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["data"]["steps"][1]["status"], "skipped");
    assert!(!edt_calls_log.exists());
}

#[test]
fn init_retries_edt_import_when_previous_run_left_incomplete_workspace() {
    let (_dir, config_path, work_path, base_path, _platform_path, edt_calls_log) =
        setup_edt_init_project("EDT", "DESIGNER", "__AUTO_FILE__");
    let edt_path = work_path.parent().expect("parent").join("1cedtcli");
    write_script(
        &edt_path,
        &format!(
            "printf '%s\\n' \"$*\" >> \"{}\"\nexit 1",
            edt_calls_log.display()
        ),
    );

    let first = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "init",
        ])
        .output()
        .expect("first run");

    assert!(!first.status.success());
    let first_payload: Value = serde_json::from_slice(&first.stdout).expect("json");
    assert_eq!(first_payload["command"], "init");
    assert_eq!(first_payload["data"]["steps"][0]["status"], "ok");
    assert_eq!(first_payload["data"]["steps"][1]["status"], "failed");
    assert!(work_path.join("edt-workspace").exists());
    assert!(!work_path
        .join("edt-workspace")
        .join(".v8tr-initialized")
        .exists());
    let first_calls = fs::read_to_string(&edt_calls_log).expect("calls");
    let first_lines: Vec<_> = first_calls.lines().collect();
    assert_eq!(first_lines.len(), 1);
    assert!(first_lines[0].contains(&base_path.join("main").display().to_string()));

    write_script(
        &edt_path,
        &format!(
            "printf '%s\\n' \"$*\" >> \"{}\"\nexit 0",
            edt_calls_log.display()
        ),
    );

    let second = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "init",
        ])
        .output()
        .expect("second run");

    assert!(second.status.success());
    let payload: Value = serde_json::from_slice(&second.stdout).expect("json");
    assert_eq!(payload["data"]["steps"][1]["status"], "ok");
    assert!(work_path
        .join("edt-workspace")
        .join(".v8tr-initialized")
        .exists());
    let calls = fs::read_to_string(edt_calls_log).expect("calls");
    let lines: Vec<_> = calls.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[1].contains(&base_path.join("main").display().to_string()));
    assert!(lines[2].contains(&base_path.join("ext").display().to_string()));
}

#[test]
fn init_rejects_workspace_path_that_is_not_a_directory() {
    let (_dir, config_path, work_path, _base_path, _platform_path, _edt_calls_log) =
        setup_edt_init_project("EDT", "DESIGNER", "__AUTO_FILE__");
    fs::write(work_path.join("edt-workspace"), "not a dir\n").expect("workspace file");

    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "init",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["data"]["steps"][1]["status"], "failed");
    assert!(payload["data"]["steps"][1]["message"]
        .as_str()
        .expect("message")
        .contains("is not a directory"));
}
