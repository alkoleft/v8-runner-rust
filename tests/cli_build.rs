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
        "args=\"$*\"\nprintf '%s\\n' \"$args\" >> \"{}\"\n{}\nexit 0",
        calls_log.display(),
        pattern_branch
    );
    write_script(path, &body);
}

fn write_edt_script(path: &Path, calls_log: &Path) {
    let body = format!(
        "args=\"$*\"\nprintf '%s\\n' \"$args\" >> \"{}\"\nexit 0",
        calls_log.display()
    );
    write_script(path, &body);
}

fn write_config(path: &Path, base_path: &Path, work_path: &Path, platform_path: &Path) {
    write_config_with_builder(
        path,
        base_path,
        work_path,
        platform_path,
        "DESIGNER",
        "File=/tmp/ib",
    );
}

fn write_config_with_builder(
    path: &Path,
    base_path: &Path,
    work_path: &Path,
    platform_path: &Path,
    builder: &str,
    connection: &str,
) {
    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: {}\nconnection: '{}'\nbuild:\n  partialLoadThreshold: 20\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: main\n  - name: ext\n    type: EXTENSION\n    path: ext\ntools:\n  platform:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        builder,
        connection,
        platform_path.display(),
    );

    fs::write(path, config).expect("config");
}

fn setup_project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let config_path = dir.path().join("v8project.yaml");
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

fn setup_ibcmd_project() -> (
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

    write_ibcmd_script(&binary_path, &calls_log, None);
    write_config_with_builder(
        &config_path,
        &base_path,
        &work_path,
        &binary_path,
        "IBCMD",
        "File=/tmp/ib",
    );

    (
        dir,
        config_path,
        binary_path,
        work_path,
        base_path,
        calls_log,
    )
}

fn setup_edt_ibcmd_project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let config_path = dir.path().join("v8project.yaml");
    let ibcmd_path = dir.path().join("ibcmd");
    let edt_cli_path = dir.path().join("edt").join("1cedtcli");
    let ibcmd_calls_log = dir.path().join("ibcmd-calls.log");
    let edt_calls_log = dir.path().join("edt-calls.log");

    fs::create_dir_all(base_path.join("configuration").join("Catalogs.Items")).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    fs::write(
        base_path.join("configuration").join(".project"),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<projectDescription>\n  <name>configuration</name>\n</projectDescription>\n",
    )
    .expect("project file");
    fs::write(
        base_path
            .join("configuration")
            .join("Catalogs.Items")
            .join("ObjectModule.bsl"),
        "procedure Test() endprocedure",
    )
    .expect("bsl");

    write_ibcmd_script(&ibcmd_path, &ibcmd_calls_log, None);
    write_edt_script(&edt_cli_path, &edt_calls_log);

    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: EDT\nbuilder: IBCMD\nconnection: 'File=/tmp/ib'\nbuild:\n  partialLoadThreshold: 20\nsource-set:\n  - name: configuration\n    type: CONFIGURATION\n    path: configuration\ntools:\n  platform:\n    path: '{}'\n  edt_cli:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        ibcmd_path.display(),
        edt_cli_path.display(),
    );
    fs::write(&config_path, config).expect("config");

    (dir, config_path, ibcmd_calls_log, edt_calls_log)
}

#[test]
fn build_json_failure_returns_step_payload() {
    let (_dir, config_path, binary_path, _work_path) = setup_project();
    write_build_script(&binary_path, Some("/UpdateDBCfg -Extension ext"));

    let output = std::process::Command::cargo_bin("v8-runner")
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
fn build_ibcmd_json_failure_reports_operation_target_and_exit_code() {
    let (_dir, config_path, binary_path, _work_path, _base_path, calls_log) = setup_ibcmd_project();
    write_ibcmd_script(&binary_path, &calls_log, Some("config apply"));

    let output = std::process::Command::cargo_bin("v8-runner")
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
    assert!(payload["data"]["steps"][0]["message"]
        .as_str()
        .expect("message")
        .contains("apply failed for source-set 'main' with exit code 17"));
}

#[test]
fn build_text_failure_does_not_print_success_footer() {
    let (_dir, config_path, binary_path, _work_path) = setup_project();
    write_build_script(&binary_path, Some("/UpdateDBCfg -Extension ext"));

    let output = std::process::Command::cargo_bin("v8-runner")
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

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--no-color",
            "--config",
            &config_path.display().to_string(),
            "build",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout
        .lines()
        .any(|line| line.contains("│") && line.contains("[Изменения]")));
    assert!(stdout.contains("[Изменения]"));
    assert!(stdout.contains("● main: changes - found"));
    assert!(stdout.contains("● main: partial"));
    assert!(stdout.contains("main"));
    assert!(stdout.contains("[Конфигуратор]"));
    assert!(stdout.contains("Загрузка изменений в базу:"));
    assert!(stdout.contains("Build completed successfully"));
}

#[test]
fn build_edt_text_interleaves_export_stage_after_edt_log() {
    let (_dir, config_path, ibcmd_calls_log, edt_calls_log) = setup_edt_ibcmd_project();

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--no-color",
            "--config",
            &config_path.display().to_string(),
            "build",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    let edt_log_index = lines
        .iter()
        .position(|line| line.contains("[EDT] Конвертация в файлы конфигуратора: configuration"))
        .expect("EDT log line");
    assert_eq!(
        lines.get(edt_log_index + 1).copied(),
        Some("● configuration: edt_export - EDT export completed")
    );
    assert!(stdout.contains("● configuration: changes - found"));
    assert!(stdout.contains("● configuration: ibcmd_import - launching ibcmd full import"));
    assert!(stdout.contains("● configuration: ibcmd_apply - launching ibcmd apply"));
    assert!(
        stdout.contains("● configuration: full - full load from EDT export after change detection")
    );

    let ibcmd_calls = fs::read_to_string(ibcmd_calls_log).expect("ibcmd calls");
    let edt_calls = fs::read_to_string(edt_calls_log).expect("edt calls");
    assert!(edt_calls.contains("export --project-name configuration"));
    assert!(ibcmd_calls.contains("config import"));
    assert!(ibcmd_calls.contains("config apply"));
}

#[test]
fn build_json_writes_action_log_file_without_polluting_stdout() {
    let (_dir, config_path, _binary_path, work_path) = setup_project();

    let output = std::process::Command::cargo_bin("v8-runner")
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
    assert!(contents.contains("[Изменения]"));
    assert!(contents.contains("[Конфигуратор]"));
    assert!(contents.contains("Загрузка изменений в базу:"));
}

#[test]
fn build_ibcmd_full_rebuild_invokes_import_and_apply() {
    let (_dir, config_path, _binary_path, _work_path, _base_path, calls_log) =
        setup_ibcmd_project();

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--no-color",
            "--config",
            &config_path.display().to_string(),
            "build",
            "--full-rebuild",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("● main: ibcmd_import - launching ibcmd full import"));
    assert!(stdout.contains("● main: ibcmd_apply - launching ibcmd apply"));
    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("config import"));
    assert!(calls.contains("config apply"));
}

#[test]
fn build_ibcmd_passes_credentials_to_import_and_apply() {
    let (dir, config_path, binary_path, work_path, base_path, calls_log) = setup_ibcmd_project();
    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: IBCMD\nconnection: 'File=/tmp/ib'\ncredentials:\n  user: Admin\n  password: secret\nbuild:\n  partialLoadThreshold: 20\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: main\ntools:\n  platform:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        binary_path.display(),
    );
    fs::write(&config_path, config).expect("config");

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "build",
            "--full-rebuild",
        ])
        .current_dir(dir.path())
        .output()
        .expect("run command");

    assert!(output.status.success());
    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(
        calls.contains("infobase --db-path /tmp/ib config import --user Admin --password secret")
    );
    assert!(
        calls.contains("infobase --db-path /tmp/ib config apply --user Admin --password secret")
    );
    assert!(calls.contains("--user Admin"));
    assert!(calls.contains("--password secret"));
}

#[test]
fn build_ibcmd_partial_uses_relative_positional_args_and_base_dir() {
    let (_dir, config_path, _binary_path, _work_path, base_path, calls_log) = setup_ibcmd_project();

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "build",
            "--full-rebuild",
        ])
        .output()
        .expect("run command");
    assert!(output.status.success());

    let changed_file = base_path
        .join("main")
        .join("Catalogs.Items")
        .join("ObjectModule.bsl");
    fs::write(&changed_file, "procedure Test() // changed endprocedure").expect("change");

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args(["--config", &config_path.display().to_string(), "build"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("config import files"));
    assert!(calls.contains("--partial"));
    assert!(calls.contains("--base-dir "));
    assert!(calls.contains("Catalogs.Items/ObjectModule.bsl"));
}

#[test]
fn build_ibcmd_server_connection_fails_at_config_load() {
    let (dir, config_path, binary_path, _work_path, _base_path, _calls_log) = setup_ibcmd_project();
    write_config_with_builder(
        &config_path,
        &dir.path().join("project"),
        &dir.path().join("work"),
        &binary_path,
        "IBCMD",
        "Srvr=server;Ref=main",
    );

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args(["--config", &config_path.display().to_string(), "build"])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn build_ibcmd_accepts_raw_f_connection() {
    let (dir, config_path, binary_path, _work_path, _base_path, calls_log) = setup_ibcmd_project();
    write_config_with_builder(
        &config_path,
        &dir.path().join("project"),
        &dir.path().join("work"),
        &binary_path,
        "IBCMD",
        "/F /tmp/ib",
    );

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "build",
            "--full-rebuild",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("--db-path /tmp/ib"));
    assert!(calls.contains("config apply"));
}
