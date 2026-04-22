#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::prelude::*;
use serde_json::Value;
use tempfile::tempdir;

const V8_CONFIGURATION_NATURE: &str = "com._1c.g5.v8.dt.core.V8ConfigurationNature";
const V8_EXTENSION_NATURE: &str = "com._1c.g5.v8.dt.core.V8ExtensionNature";
const EDT_RUNTIME_VERSION: &str = "8.3.27";

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
        "args=\"$*\"\ntarget=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"--configuration-files\" ]; then target=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$target\" ]; then mkdir -p \"$target\"; printf '<Configuration />\\n' > \"$target/Configuration.xml\"; fi\nprintf '%s\\n' \"$args\" >> \"{}\"\nexit 0",
        calls_log.display()
    );
    write_script(path, &body);
}

fn write_native_edt_project(
    path: &Path,
    project_name: &str,
    nature: &str,
    base_project: Option<&str>,
) {
    fs::create_dir_all(path.join("DT-INF")).expect("dt-inf");
    fs::create_dir_all(path.join("src").join("Configuration")).expect("src");
    fs::write(
        path.join(".project"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<projectDescription>\n  <name>{project_name}</name>\n  <natures>\n    <nature>{nature}</nature>\n  </natures>\n</projectDescription>\n"
        ),
    )
    .expect("project");
    let base_project_line = base_project
        .map(|value| format!("Base-Project: {value}\n"))
        .unwrap_or_default();
    fs::write(
        path.join("DT-INF").join("PROJECT.PMF"),
        format!(
            "{base_project_line}Manifest-Version: 1.0\nRuntime-Version: {EDT_RUNTIME_VERSION}\n"
        ),
    )
    .expect("manifest");
    fs::write(
        path.join("src")
            .join("Configuration")
            .join("Configuration.mdo"),
        "<Configuration />\n",
    )
    .expect("configuration marker");
    fs::write(
        path.join("src").join("Configuration").join("Module.bsl"),
        "Procedure Test()\nEndProcedure\n",
    )
    .expect("module marker");
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
    let infobase = format!("  connection: '{}'\n", connection);
    write_config_with_builder_and_infobase(
        path,
        base_path,
        work_path,
        platform_path,
        builder,
        &infobase,
    );
}

fn write_config_with_builder_and_infobase(
    path: &Path,
    base_path: &Path,
    work_path: &Path,
    platform_path: &Path,
    builder: &str,
    infobase_yaml: &str,
) {
    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: {}\ninfobase:\n{}build:\n  partialLoadThreshold: 20\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: main\n  - name: ext\n    type: EXTENSION\n    path: ext\ntools:\n  platform:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        builder,
        infobase_yaml,
        platform_path.display(),
    );

    fs::write(path, config).expect("config");
}

fn write_live_workspace_lock(work_path: &Path, command: &str) {
    let canonical_work = fs::canonicalize(work_path).expect("canonical work");
    let lock_owner = "integration-test-lock-owner";
    let started_at = chrono::Utc::now().to_rfc3339();

    fs::write(
        canonical_work.join(".v8-runner.workspace.lock"),
        serde_json::json!({
            "tool": "v8-runner",
            "pid": std::process::id(),
            "owner_id": lock_owner,
            "created_at": started_at,
        })
        .to_string(),
    )
    .expect("workspace lock");
    fs::write(
        canonical_work.join(".v8-runner.workspace.lock.json"),
        serde_json::json!({
            "pid": std::process::id(),
            "lock_owner": lock_owner,
            "command": command,
            "started_at": started_at,
            "canonical_work_path": canonical_work,
        })
        .to_string(),
    )
    .expect("workspace lock sidecar");
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
    write_native_edt_project(
        &base_path.join("configuration"),
        "configuration",
        V8_CONFIGURATION_NATURE,
        None,
    );
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
        "basePath: '{}'\nworkPath: '{}'\nformat: EDT\nbuilder: IBCMD\ninfobase:\n  connection: 'File=/tmp/ib'\nbuild:\n  partialLoadThreshold: 20\nsource-set:\n  - name: configuration\n    type: CONFIGURATION\n    path: configuration\ntools:\n  platform:\n    path: '{}'\n  edt_cli:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        ibcmd_path.display(),
        edt_cli_path.display(),
    );
    fs::write(&config_path, config).expect("config");

    (dir, config_path, ibcmd_calls_log, edt_calls_log)
}

fn setup_edt_extension_project() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let config_path = dir.path().join("v8project.yaml");
    let platform_path = dir.path().join("platform").join("bin").join("1cv8");
    let edt_cli_path = dir.path().join("edt").join("1cedtcli");
    let edt_calls_log = dir.path().join("edt-calls.log");

    fs::create_dir_all(base_path.join("configuration").join("Catalogs.Items")).expect("base");
    fs::create_dir_all(base_path.join("exts").join("client-mcp")).expect("ext");
    fs::create_dir_all(&work_path).expect("work");
    write_native_edt_project(
        &base_path.join("configuration"),
        "configuration",
        V8_CONFIGURATION_NATURE,
        None,
    );
    fs::write(
        base_path
            .join("configuration")
            .join("Catalogs.Items")
            .join("ObjectModule.bsl"),
        "procedure Test() endprocedure",
    )
    .expect("configuration bsl");
    fs::write(
        base_path
            .join("configuration")
            .join("Catalogs.Items")
            .join("ObjectModule.xml"),
        "<MetaDataObject />",
    )
    .expect("configuration xml");
    write_native_edt_project(
        &base_path.join("exts").join("client-mcp"),
        "client_mcp",
        V8_EXTENSION_NATURE,
        Some("configuration"),
    );
    fs::write(
        base_path.join("exts").join("client-mcp").join("Module.bsl"),
        "procedure Test() endprocedure",
    )
    .expect("extension bsl");

    write_build_script(&platform_path, None);
    write_edt_script(&edt_cli_path, &edt_calls_log);

    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: EDT\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\nbuild:\n  partialLoadThreshold: 20\nsource-set:\n  - name: configuration\n    type: CONFIGURATION\n    path: configuration\n  - name: client_mcp\n    type: EXTENSION\n    path: exts/client-mcp\ntools:\n  platform:\n    path: '{}'\n  edt_cli:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        platform_path.display(),
        edt_cli_path.display(),
    );
    fs::write(&config_path, config).expect("config");

    (dir, config_path, work_path)
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
    assert!(stdout.contains("● main:"));
    assert!(stdout.contains("│   Изменения: найдено"));
    assert!(stdout.contains("изменено"));
    assert!(stdout.contains("│   [Конфигуратор] Загрузка изменений в базу"));
    assert!(stdout.contains("│   ✓ partial load"));
    assert_eq!(stdout.matches("● main").count(), 1);
    assert!(stdout.contains("main"));
    assert!(stdout.contains("Build completed successfully"));
}

#[test]
fn build_text_highlights_timeline_detail_prefixes() {
    let (_dir, config_path, _binary_path, _work_path) = setup_project();

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args(["--config", &config_path.display().to_string(), "build"])
        .output()
        .expect("run designer build");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\x1b[1mmain\x1b[0m:"));
    assert!(stdout.contains("\x1b[1;34mИзменения\x1b[0m:"));
    assert!(stdout.contains("\x1b[1;34m[Конфигуратор]\x1b[0m"));
    assert!(stdout.contains("\x1b[1;32m✓\x1b[0m partial load"));

    let (_dir, config_path, _ibcmd_calls_log, _edt_calls_log) = setup_edt_ibcmd_project();
    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args(["--config", &config_path.display().to_string(), "build"])
        .output()
        .expect("run edt build");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\x1b[1;34m[EDT]\x1b[0m"));
    assert!(stdout.contains("\x1b[1;34m[ibcmd]\x1b[0m"));
}

#[test]
fn build_text_workspace_lock_conflict_prints_single_error() {
    let (_dir, config_path, _binary_path, work_path) = setup_project();
    write_live_workspace_lock(&work_path, "build");

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

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(3));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let error_prefix = "runtime error: cannot start build";
    let combined = format!("{stdout}{stderr}");

    assert_eq!(
        combined.matches(error_prefix).count(),
        1,
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("ERROR: runtime error: cannot start build"),
        "stderr:\n{stderr}"
    );
    assert!(
        !stdout.contains(error_prefix),
        "stdout should not contain duplicate error log:\n{stdout}"
    );
}

#[test]
fn build_text_no_changes_collapses_per_source_set_noise() {
    let (_dir, config_path, _binary_path, _work_path) = setup_project();

    let first = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--no-color",
            "--config",
            &config_path.display().to_string(),
            "build",
        ])
        .output()
        .expect("first build");
    assert!(first.status.success());

    let second = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--no-color",
            "--config",
            &config_path.display().to_string(),
            "build",
        ])
        .output()
        .expect("second build");

    assert!(second.status.success());
    let stdout = String::from_utf8_lossy(&second.stdout);

    assert!(
        stdout.contains("Build completed: no changes"),
        "stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("changes - no changes"),
        "stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("skipped - no changes"),
        "stdout:\n{stdout}"
    );
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
    let source_set_stage_index = lines
        .iter()
        .position(|line| *line == "● configuration:")
        .expect("source-set timeline stage");
    assert!(lines
        .iter()
        .skip(source_set_stage_index + 1)
        .any(|line| *line == "│   [EDT] Конвертация в файлы конфигуратора"));
    assert!(stdout.contains("│   ✓ completed"));
    assert!(stdout.contains("│   [ibcmd] Загрузка в базу"));
    assert!(stdout.contains("│   [ibcmd] Применение изменений"));
    assert!(stdout.contains("│   ✓ full load selected by partial-load rules"));
    assert_eq!(stdout.matches("● configuration").count(), 1);

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
    assert!(contents.contains("main:"));
    assert!(contents.contains("Изменения: найдено"));
    assert!(contents.contains("[Конфигуратор] Загрузка изменений в базу"));
    assert!(contents.contains("✓ partial load"));
}

#[test]
fn build_json_edt_extension_uses_full_load_and_writes_platform_log() {
    let (_dir, config_path, work_path) = setup_edt_extension_project();

    let first = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "build",
        ])
        .output()
        .expect("prime build");
    assert!(first.status.success());

    fs::write(
        config_path
            .parent()
            .expect("config dir")
            .join("project")
            .join("exts")
            .join("client-mcp")
            .join("Module.bsl"),
        "procedure Test()\n  // changed after snapshot\nendprocedure",
    )
    .expect("modify extension");

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
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["data"]["ok"], true);

    let platform_log = work_path
        .join("logs")
        .join("platform")
        .join("build-01-client_mcp-load.log");
    let contents = fs::read_to_string(platform_log).expect("platform log");
    assert!(contents.contains("/LoadConfigFromFiles"));
    assert!(contents.contains("-Extension client_mcp"));
    assert!(!contents.contains("-partial"));
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
    assert!(stdout.contains("● main:"));
    assert!(stdout.contains("│   [ibcmd] Загрузка в базу"));
    assert!(stdout.contains("│   [ibcmd] Применение изменений"));
    assert_eq!(stdout.matches("● main").count(), 1);
    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("config import"));
    assert!(calls.contains("config apply"));
}

#[test]
fn build_ibcmd_passes_credentials_to_import_and_apply() {
    let (dir, config_path, binary_path, work_path, base_path, calls_log) = setup_ibcmd_project();
    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: IBCMD\ninfobase:\n  connection: 'File=/tmp/ib'\n  user: Admin\n  password: secret\nbuild:\n  partialLoadThreshold: 20\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: main\ntools:\n  platform:\n    path: '{}'\n",
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
fn build_ibcmd_server_connection_passes_dbms_and_infobase_credentials() {
    let (dir, config_path, binary_path, _work_path, _base_path, calls_log) = setup_ibcmd_project();
    write_config_with_builder_and_infobase(
        &config_path,
        &dir.path().join("project"),
        &dir.path().join("work"),
        &binary_path,
        "IBCMD",
        "  connection: 'Srvr=server;Ref=main'\n  user: Admin\n  password: secret\n  dbms:\n    kind: PostgreSQL\n    server: localhost\n    name: maindb\n    user: postgres\n    password: pg-secret\n",
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
    assert!(calls.contains("--dbms PostgreSQL --database-server localhost --database-name maindb"));
    assert!(calls.contains("--user Admin --password secret"));
    assert!(calls.contains("--database-user postgres --database-password pg-secret"));
    assert!(calls.contains("config import"));
    assert!(calls.contains("config apply"));
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
