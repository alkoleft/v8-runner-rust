#![cfg(unix)]

mod support;

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use support::{
    temp_workspace, v8_runner_command, wait_for_received_line, write_shell_script as write_script,
};

const V8_CONFIGURATION_NATURE: &str = "com._1c.g5.v8.dt.core.V8ConfigurationNature";
const V8_EXTENSION_NATURE: &str = "com._1c.g5.v8.dt.core.V8ExtensionNature";
const EDT_RUNTIME_VERSION: &str = "8.3.27";

fn write_native_edt_project(
    path: &Path,
    project_name: &str,
    nature: &str,
    base_project: Option<&str>,
) {
    fs::create_dir_all(path.join("metadata")).expect("metadata");
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

fn write_edt_configuration_source(path: &Path, project_name: &str) {
    write_native_edt_project(path, project_name, V8_CONFIGURATION_NATURE, None);
    fs::write(
        path.join("metadata").join("Configuration.xml"),
        "<Configuration />",
    )
    .expect("descriptor");
}

fn write_edt_extension_source(path: &Path, project_name: &str) {
    write_native_edt_project(
        path,
        project_name,
        V8_EXTENSION_NATURE,
        Some("configuration"),
    );
    fs::write(
        path.join("metadata").join("Configuration.xml"),
        "<Configuration><ConfigurationExtensionPurpose>Extension</ConfigurationExtensionPurpose></Configuration>",
    )
    .expect("descriptor");
}

fn setup_extensions_project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let config_path = base_path.join("v8project.yaml");
    let ibcmd_path = dir.path().join("ibcmd");
    let calls_log = dir.path().join("ibcmd.calls.log");

    fs::create_dir_all(base_path.join("configuration")).expect("configuration dir");
    fs::create_dir_all(base_path.join("exts").join("client-mcp")).expect("client_mcp dir");
    fs::create_dir_all(base_path.join("tests")).expect("tests dir");
    fs::create_dir_all(&work_path).expect("work");
    write_edt_configuration_source(&base_path.join("configuration"), "configuration");
    write_edt_extension_source(
        &base_path.join("exts").join("client-mcp"),
        "client-mcp-project",
    );
    write_edt_extension_source(&base_path.join("tests"), "tests-project");
    write_script(
        &ibcmd_path,
        &format!("printf '%s\\n' \"$*\" >> '{}'\nexit 0", calls_log.display()),
    );

    let config = format!(
        "workPath: '{}'\nformat: EDT\nbuilder: DESIGNER\ninfobase:\n  connection: 'File={}'\nsource-set:\n  - name: configuration\n    type: CONFIGURATION\n    path: configuration\n  - name: client_mcp\n    type: EXTENSION\n    path: exts/client-mcp\n  - name: tests\n    type: EXTENSION\n    path: tests\ntools:\n  platform:\n    path: '{}'\n",
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

    let output = v8_runner_command()
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
    assert!(stdout.contains("│"));
    assert!(stdout.contains("● client_mcp: disable_safety"));
    assert!(stdout.contains("updating extension properties"));
    assert!(stdout.contains("│   безопасный режим"));
    assert!(stdout.contains("● tests: disable_safety"));
    assert!(stdout.contains("● Extension properties updated successfully"));
    assert_eq!(stdout.matches("● client_mcp: disable_safety").count(), 1);
    assert!(!stdout.contains("[Расширения]"));

    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("extension update"));
    assert!(calls.contains("--name client_mcp"));
    assert!(calls.contains("--name tests"));
    assert!(calls.contains("--safe-mode no"));
    assert!(calls.contains("--unsafe-action-protection no"));
}

#[test]
fn extensions_command_streams_stage_before_pipeline_finishes() {
    let (_dir, config_path, _calls_log, ibcmd_path) = setup_extensions_project();
    write_script(
        &ibcmd_path,
        "case \"$*\" in\n  *\"--name tests\"*) sleep 2 ;;\nesac\nexit 0",
    );

    let mut command = v8_runner_command();
    command
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "extensions",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().expect("spawn command");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let saw_first_stage = wait_for_received_line(
        &rx,
        Duration::from_secs(1),
        Duration::from_millis(100),
        |line| line.contains("● client_mcp: disable_safety"),
    );

    assert!(saw_first_stage, "first extension stage was not streamed");
    assert!(
        child.try_wait().expect("try wait").is_none(),
        "process finished before the delayed second extension"
    );

    let status = child.wait().expect("wait");
    assert!(status.success());
}

#[test]
fn extensions_command_filters_by_requested_source_set_names() {
    let (_dir, config_path, calls_log, _ibcmd_path) = setup_extensions_project();

    let output = v8_runner_command()
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
    assert!(calls.contains("--name client_mcp"));
    assert!(!calls.contains("--name tests"));
}

#[test]
fn extensions_command_json_failure_reports_operation_target_and_exit_code() {
    let (_dir, config_path, _calls_log, ibcmd_path) = setup_extensions_project();
    write_script(&ibcmd_path, "echo 'cannot update extension' >&2\nexit 17");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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

#[test]
fn extensions_command_json_failure_without_payload_keeps_machine_readable_error() {
    let (_dir, config_path, _calls_log, _ibcmd_path) = setup_extensions_project();

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "extensions",
            "--name",
            "missing",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "extensions");
    assert_eq!(payload["duration_ms"], 0);
    assert_eq!(
        payload["error"]["code"],
        serde_json::Value::String("invalid_argument".to_owned())
    );
    assert!(payload["error"]["message"]
        .as_str()
        .expect("message")
        .contains("unknown extension source-set 'missing'"));
    assert!(payload["data"]["message"]
        .as_str()
        .expect("data message")
        .contains("unknown extension source-set 'missing'"));
}
