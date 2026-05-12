#![cfg(unix)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use support::{temp_workspace, v8_runner_command, write_shell_script as write_script};

const V8_CONFIGURATION_NATURE: &str = "com._1c.g5.v8.dt.core.V8ConfigurationNature";
const EDT_RUNTIME_VERSION: &str = "8.3.27";

fn write_designer_script(path: &Path, calls_log: &Path) {
    let body = format!(
        "args=\"$*\"\nout=\"\"\nreport=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  if [ \"$prev\" = \"-ReportFile\" ]; then report=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nprintf '%s\\n' \"$args\" >> \"{}\"\nif [ -n \"$out\" ]; then mkdir -p \"$(dirname \"$out\")\"; printf 'designer log for %s\\n' \"$args\" > \"$out\"; fi\nif printf '%s' \"$args\" | grep -F -q -- '/CompareCfg'; then\n  if printf '%s' \"$args\" | grep -F -q -- 'VendorConfiguration'; then\n    printf 'configuration is not on support\\n' >&2\n    exit 17\n  fi\n  if printf '%s' \"$args\" | grep -F -q -- 'ExtensionDBConfiguration'; then\n    if printf '%s' \"$args\" | grep -F -q -- 'ExistingExt'; then\n      : > \"$report\"\n      exit 0\n    fi\n    printf 'extension not found\\n' >&2\n    exit 19\n  fi\nfi\nexit 0",
        calls_log.display()
    );
    write_script(path, &body);
}

fn write_edt_configuration_source(path: &Path, project_name: &str) {
    fs::create_dir_all(path.join("metadata")).expect("metadata");
    fs::create_dir_all(path.join("DT-INF")).expect("dt-inf");
    fs::create_dir_all(path.join("src").join("Configuration")).expect("src");
    fs::write(
        path.join(".project"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<projectDescription>\n  <name>{project_name}</name>\n  <natures>\n    <nature>{V8_CONFIGURATION_NATURE}</nature>\n  </natures>\n</projectDescription>\n"
        ),
    )
    .expect("project");
    fs::write(
        path.join("DT-INF").join("PROJECT.PMF"),
        format!("Manifest-Version: 1.0\nRuntime-Version: {EDT_RUNTIME_VERSION}\n"),
    )
    .expect("manifest");
    fs::write(
        path.join("metadata").join("Configuration.xml"),
        "<Configuration />",
    )
    .expect("descriptor");
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

fn write_config(
    path: &Path,
    _base_path: &Path,
    work_path: &Path,
    platform_path: &Path,
    format: &str,
) {
    let config = format!(
        "workPath: '{}'\nformat: {}\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: main\ntools:\n  platform:\n    path: '{}'\n",
        work_path.display(),
        format,
        platform_path.display(),
    );
    fs::write(path, config).expect("config");
}

fn setup_project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, PathBuf) {
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let config_path = base_path.join("v8project.yaml");
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

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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
fn load_text_success_is_compact_and_keeps_target_visible() {
    let (_dir, config_path, _binary_path, base_path, _calls_log) = setup_project();
    let artifact_path = base_path.join("release.cf");
    fs::write(&artifact_path, "cf").expect("artifact");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "load",
            "--path",
            "release.cf",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("● Artifact load completed successfully"));
    assert!(stdout.contains("│   target: configuration"));
    assert!(stdout.contains("│   action: load cf"));
    assert!(stdout.contains(artifact_path.display().to_string().as_str()));
    assert!(!stdout.contains("platform log"));
}

#[test]
fn merge_cfe_json_success_requires_extension_and_settings() {
    let (_dir, config_path, _binary_path, base_path, calls_log) = setup_project();
    fs::write(base_path.join("release.cfe"), "cfe").expect("artifact");
    fs::write(base_path.join("merge.xml"), "<settings/>").expect("settings");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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
fn load_text_failure_surfaces_structured_error() {
    let (_dir, config_path, _binary_path, base_path, _calls_log) = setup_project();
    fs::write(base_path.join("release.cf"), "cf").expect("artifact");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
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

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("● Artifact load failed"));
    assert!(stdout.contains("[error:artifact_load_failed]"));
    assert!(stdout.contains("not supported"));
}

#[test]
fn load_rejects_edt_format_even_with_designer_builder() {
    let (_dir, config_path, _binary_path, base_path, _calls_log) = setup_project();
    fs::write(base_path.join("release.cf"), "cf").expect("artifact");
    write_edt_configuration_source(&base_path.join("main"), "main");
    write_config(
        &config_path,
        &base_path,
        &config_path.parent().expect("parent").join("work"),
        &config_path.parent().expect("parent").join("1cv8"),
        "EDT",
    );

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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
