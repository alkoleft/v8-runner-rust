#![cfg(unix)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use support::{temp_workspace, v8_runner_command, write_shell_script as write_script};

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
    write_native_edt_project(path, project_name, V8_EXTENSION_NATURE, Some("main"));
    fs::write(
        path.join("metadata").join("Configuration.xml"),
        "<Configuration><ConfigurationExtensionPurpose>Extension</ConfigurationExtensionPurpose></Configuration>",
    )
    .expect("descriptor");
}

fn setup_designer_init_project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    setup_designer_init_project_with_body(
        "if [ \"$1\" = \"CREATEINFOBASE\" ]; then mkdir -p \"$ib_path\" && : > \"$ib_path/1Cv8.1CD\"; fi\nexit 0",
    )
}

fn setup_designer_init_project_with_body(
    script_body: &str,
) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let config_path = base_path.join("v8project.yaml");
    let v8_path = dir.path().join("1cv8");
    let infobase_path = dir.path().join("ib");

    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    write_script(
        &v8_path,
        &script_body.replace("$ib_path", &infobase_path.display().to_string()),
    );

    let config = format!(
        "workPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: 'File={}'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: main\ntools:\n  platform:\n    path: '{}'\n",
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
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let config_path = base_path.join("v8project.yaml");
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

    if format == "EDT" {
        write_edt_configuration_source(&base_path.join("main"), "main");
        write_edt_extension_source(&base_path.join("ext"), "ext");
    } else {
        fs::create_dir_all(base_path.join("main")).expect("main");
        fs::create_dir_all(base_path.join("ext")).expect("ext");
    }
    fs::create_dir_all(&work_path).expect("work");
    let platform_body = if builder == "IBCMD" {
        "if [ \"$1\" = \"infobase\" ]; then\n  shift\n  command=\"\"\n  path=\"\"\n  while [ \"$#\" -gt 0 ]; do\n    case \"$1\" in\n      create) command=create ;;\n      --db-path|--database-path) shift; path=$1 ;;\n      --db-path=*|--database-path=*) path=${1#*=} ;;\n    esac\n    shift\n  done\n  if [ \"$command\" = \"create\" ]; then\n    mkdir -p \"$path\" && : > \"$path/1Cv8.1CD\"\n  fi\nfi\nexit 0"
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
        "workPath: '{}'\nformat: {}\nbuilder: {}\ninfobase:\n  connection: '{}'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: main\n  - name: ext\n    type: EXTENSION\n    path: ext\ntools:\n  platform:\n    path: '{}'\n  edt_cli:\n    path: '{}'\n",
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

fn setup_ibcmd_server_init_project(
    script_body: &str,
) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let config_path = base_path.join("v8project.yaml");
    let ibcmd_path = dir.path().join("ibcmd");
    let calls_log = dir.path().join("ibcmd.calls.log");

    fs::create_dir_all(base_path.join("main")).expect("main");
    fs::create_dir_all(&work_path).expect("work");
    write_script(
        &ibcmd_path,
        &format!(
            "printf '%s\\n' \"$*\" >> '{}'\n{}\n",
            calls_log.display(),
            script_body
        ),
    );

    let config = format!(
        "workPath: '{}'\nformat: DESIGNER\nbuilder: IBCMD\ninfobase:\n  connection: 'Srvr=cluster:1541;Ref=demo'\n  user: Admin\n  password: secret\n  dbms:\n    kind: PostgreSQL\n    server: localhost\n    name: demo\n    user: postgres\n    password: pg-secret\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: main\ntools:\n  platform:\n    path: '{}'\n",
        work_path.display(),
        ibcmd_path.display(),
    );
    fs::write(&config_path, config).expect("config");

    (dir, config_path, work_path, calls_log)
}

#[test]
fn init_designer_creates_infobase_and_skips_edt_workspace() {
    let (_dir, config_path, work_path, infobase_path) = setup_designer_init_project();

    let output = v8_runner_command()
        .args(["--config", &config_path.display().to_string(), "init"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    assert!(infobase_path.join("1Cv8.1CD").exists());
    assert!(!work_path.join("edt-workspace").exists());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("infobase: create"));
    assert!(!stdout.contains("edt_workspace: import"));
    assert!(!stdout.contains("format=DESIGNER"));
}

#[test]
fn init_designer_non_zero_create_exit_stays_fatal_even_when_marker_appears() {
    let (_dir, config_path, _work_path, _infobase_path) = setup_designer_init_project_with_body(
        "if [ \"$1\" = \"CREATEINFOBASE\" ]; then mkdir -p \"$ib_path\" && : > \"$ib_path/1Cv8.1CD\"; fi\nprintf 'designer create failed\\n' >&2\nexit 1",
    );

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "init",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["data"]["steps"][0]["status"], "failed");
    assert!(payload["data"]["steps"][0]["message"]
        .as_str()
        .expect("message")
        .contains("designer create failed"));
}

#[test]
fn init_text_reports_infobase_failure_before_continuing_edt_import() {
    let (_dir, config_path, _work_path, _base_path, platform_path, edt_calls_log) =
        setup_edt_init_project("EDT", "DESIGNER", "__AUTO_FILE__");
    write_script(
        &platform_path,
        "printf 'designer create failed\\n' >&2\nexit 1",
    );

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "init",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let failed_step = stdout
        .find("✗ infobase: create")
        .expect("live failed infobase status");
    let edt_import = stdout
        .find("importing source-set project")
        .expect("continued edt import");
    let final_summary = stdout.find("Init failed").expect("final summary");
    assert!(failed_step < edt_import);
    assert!(failed_step < final_summary);
    assert!(stdout.contains("✓ edt_workspace: import"));
    assert!(edt_calls_log.exists());
}

#[test]
fn init_ibcmd_creates_infobase_and_imports_edt_projects_in_order() {
    let (_dir, config_path, work_path, _base_path, _platform_path, edt_calls_log) =
        setup_edt_init_project("DESIGNER", "IBCMD", "__AUTO_FILE__");

    let output = v8_runner_command()
        .args(["--config", &config_path.display().to_string(), "init"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let config = fs::read_to_string(&config_path).expect("config");
    let connection_line = config
        .lines()
        .find(|line| line.trim_start().starts_with("connection:"))
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
fn init_ibcmd_file_already_exists_without_marker_is_fatal() {
    let (_dir, config_path, _work_path, _base_path, platform_path, _edt_calls_log) =
        setup_edt_init_project("DESIGNER", "IBCMD", "__AUTO_FILE__");
    write_script(
        &platform_path,
        "if [ \"$1\" = \"infobase\" ]; then printf 'already exists\\n' >&2; exit 17; fi\nexit 0",
    );

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "init",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["data"]["steps"][0]["status"], "failed");
    assert!(payload["data"]["steps"][0]["message"]
        .as_str()
        .expect("message")
        .contains("marker file is missing"));
}

#[test]
fn init_edt_with_ibcmd_creates_infobase_and_imports_projects_in_order() {
    let (_dir, config_path, work_path, base_path, _platform_path, edt_calls_log) =
        setup_edt_init_project("EDT", "IBCMD", "__AUTO_FILE__");

    let output = v8_runner_command()
        .args(["--config", &config_path.display().to_string(), "init"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let config = fs::read_to_string(&config_path).expect("config");
    let connection_line = config
        .lines()
        .find(|line| line.trim_start().starts_with("connection:"))
        .expect("connection line");
    let infobase_dir = connection_line
        .split("File=")
        .nth(1)
        .expect("file path")
        .trim_matches('\'');
    assert!(Path::new(infobase_dir).join("1Cv8.1CD").exists());
    assert!(work_path.join("edt-workspace").exists());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("importing source-set project 'main'"));
    assert!(stdout.contains("importing source-set project 'ext'"));
    assert!(stdout.contains("imported EDT projects: main, ext"));
    let calls = fs::read_to_string(edt_calls_log).expect("calls");
    let lines: Vec<_> = calls.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains(&base_path.join("main").display().to_string()));
    assert!(lines[1].contains(&base_path.join("ext").display().to_string()));
}

#[test]
fn init_edt_imports_projects_in_configuration_then_extension_order() {
    let (_dir, config_path, work_path, base_path, _platform_path, edt_calls_log) =
        setup_edt_init_project("EDT", "DESIGNER", "__AUTO_FILE__");

    let output = v8_runner_command()
        .args(["--config", &config_path.display().to_string(), "init"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let config = fs::read_to_string(&config_path).expect("config");
    let connection_line = config
        .lines()
        .find(|line| line.trim_start().starts_with("connection:"))
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

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "init",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["command"], "init");
    assert_eq!(payload["data"]["steps"][0]["status"], "skipped");
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

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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

    let first = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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

    let second = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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

#[test]
fn init_ibcmd_server_provisions_infobase_without_precheck() {
    let (_dir, config_path, work_path, calls_log) = setup_ibcmd_server_init_project("exit 0");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "init",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["data"]["steps"][0]["status"], "ok");
    assert_eq!(payload["data"]["steps"][1]["status"], "skipped");
    assert!(!work_path.join("edt-workspace").exists());
    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("infobase --dbms PostgreSQL --database-server localhost --database-name demo create --create-database --user Admin --password secret --database-user postgres --database-password pg-secret"));
    assert!(!calls.contains(" info "));
    assert!(!calls.contains(" list "));
}

#[test]
fn init_ibcmd_server_already_exists_is_non_fatal() {
    let (_dir, config_path, _work_path, _calls_log) =
        setup_ibcmd_server_init_project("printf 'already exists\\n' >&2\nexit 17");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "init",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["data"]["steps"][0]["status"], "skipped");
    assert!(payload["data"]["steps"][0]["message"]
        .as_str()
        .expect("message")
        .contains("already exists"));
}

#[test]
fn init_ibcmd_server_auth_failure_stays_fatal() {
    let (_dir, config_path, _work_path, _calls_log) =
        setup_ibcmd_server_init_project("printf 'access denied\\n' >&2\nexit 17");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "init",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["data"]["steps"][0]["status"], "failed");
    assert!(payload["data"]["steps"][0]["message"]
        .as_str()
        .expect("message")
        .contains("access denied"));
}
