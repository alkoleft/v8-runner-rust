#![cfg(unix)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use support::{temp_workspace, v8_runner_command, write_shell_script as write_script};

const V8_CONFIGURATION_NATURE: &str = "com._1c.g5.v8.dt.core.V8ConfigurationNature";
const EDT_RUNTIME_VERSION: &str = "8.3.27";

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
    edt_cli_path: Option<&Path>,
) {
    let edt_section = edt_cli_path
        .map(|path| format!("  edt_cli:\n    path: '{}'\n", path.display()))
        .unwrap_or_default();
    let config = format!(
        "workPath: '{}'\nformat: {}\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: .\ntools:\n  platform:\n    path: '{}'\n{}",
        work_path.display(),
        format,
        platform_path.display(),
        edt_section
    );
    fs::write(path, config).expect("config");
}

fn setup_project(script_body: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let install_dir = dir.path().join("platform");
    let config_path = base_path.join("v8project.yaml");

    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    write_script(&install_dir.join("bin").join("1cv8"), script_body);
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &install_dir,
        "DESIGNER",
        None,
    );

    (dir, config_path)
}

fn setup_edt_project(script_body: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let install_dir = dir.path().join("platform");
    let edt_cli = dir.path().join("edt").join("1cedtcli");
    let config_path = base_path.join("v8project.yaml");

    fs::create_dir_all(&base_path).expect("base");
    write_edt_configuration_source(&base_path, "main");
    fs::create_dir_all(&work_path).expect("work");
    write_script(&install_dir.join("bin").join("1cv8"), "exit 0");
    write_script(&edt_cli, script_body);
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &install_dir,
        "EDT",
        Some(&edt_cli),
    );

    (dir, config_path)
}

#[test]
fn syntax_designer_config_json_returns_clean_envelope() {
    let (_dir, config_path) = setup_project(
        "out=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf '' > \"$out\"; fi\nprintf 'RAW_STDOUT\\n'\nexit 0",
    );

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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
fn syntax_text_clean_success_stays_compact() {
    let (_dir, config_path) = setup_project(
        "out=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf '' > \"$out\"; fi\nexit 0",
    );

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "syntax",
            "designer-config",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("● Syntax check designer-config completed successfully"));
    assert!(stdout.contains("│   status: clean (exit 0, errors 0, warnings 0, info 0"));
    assert!(!stdout.contains("platform log"));
}

#[test]
fn syntax_text_success_warning_includes_diagnostic_path() {
    let (_dir, config_path) = setup_project(
        "args=\"$*\"\nprintf 'RAW_STDOUT\\n'\nif printf '%s' \"$args\" | grep -F -q -- '/Out'; then\n  exit 0\nfi\nexit 0",
    );

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "syntax",
            "designer-config",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("● Syntax check designer-config completed successfully"));
    assert!(stdout.contains("[warning] log"));
    assert!(stdout.contains("[diagnostic] platform log -> "));
}

#[test]
fn syntax_designer_modules_json_returns_structured_validation_failure() {
    let (_dir, config_path) = setup_project(
        "args=\"$*\"\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif printf '%s' \"$args\" | grep -F -q -- '/CheckModules'; then\n  cat <<'LOG' > \"$out\"\n{CommonModules.TestModule(4,2)}: Ошибка компиляции\n{1}: context\nLOG\n  exit 101\nfi\nexit 0",
    );

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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
    assert_eq!(payload["error"]["code"], "runtime_failure");
    assert_eq!(payload["error"]["kind"], "runtime");
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
fn syntax_designer_modules_without_modes_renders_json_error() {
    let (_dir, config_path) = setup_project("exit 0");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "syntax",
            "designer-modules",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());

    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "syntax");
    assert_eq!(
        payload["data"]["message"],
        "syntax designer-modules requires at least one mode flag"
    );
}

#[test]
fn syntax_text_output_hides_raw_stdout_and_prints_structured_issue() {
    let (_dir, config_path) = setup_project(
        "args=\"$*\"\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nprintf 'RAW_STDOUT\\n'\nif printf '%s' \"$args\" | grep -F -q -- '/CheckModules'; then\n  cat <<'LOG' > \"$out\"\nCommonModules.TestModule Warning: потенциальная проблема\nLOG\n  exit 101\nfi\nexit 0",
    );

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "syntax",
            "designer-modules",
            "--server",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(3));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Syntax check designer-modules found issues"));
    assert!(stdout.contains("CommonModules.TestModule"));
    assert!(stdout.contains("[issue] WARNING"));
    assert!(!stdout.contains("RAW_STDOUT"));
}

#[test]
fn syntax_edt_json_returns_structured_edt_issues() {
    let (_dir, config_path) = setup_edt_project(
        "out=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"--file\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then cat <<'LOG' > \"$out\"\nERROR\tCommonModules.Test\t7\t2\tRule\tbad call\nLOG\nfi\nexit 1",
    );

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "syntax",
            "edt",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(3));

    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["data"]["check_name"], "edt");
    assert_eq!(payload["data"]["status"], "issues_found");
    assert_eq!(payload["data"]["summary"]["errors"], 1);
    assert_eq!(payload["data"]["issues"][0]["kind"], "edt");
    assert_eq!(payload["data"]["issues"][0]["path"], "CommonModules.Test");
}
