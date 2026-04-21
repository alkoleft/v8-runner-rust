#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use assert_cmd::cargo::cargo_bin;
use rmcp::{
    model::ErrorCode,
    model::{CallToolRequest, CallToolRequestParams, CancelledNotificationParam, ClientRequest},
    service::PeerRequestOptions,
    transport::{ConfigureCommandExt, TokioChildProcess},
    ServiceError, ServiceExt,
};
use serde_json::{json, Value};
use tempfile::tempdir;

fn write_config(path: &Path, base_path: &Path, work_path: &Path, platform_path: &Path) {
    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: .\ntools:\n  platform:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        platform_path.display(),
    );
    fs::write(path, config).expect("config");
}

fn write_edt_config_with_options(
    path: &Path,
    base_path: &Path,
    work_path: &Path,
    edt_path: &Path,
    command_timeout_ms: u64,
    max_concurrent_calls: usize,
) {
    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: EDT\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: main-edt\nmcp:\n  execution:\n    max_concurrent_calls: {}\ntools:\n  edt_cli:\n    path: '{}'\n    interactive-mode: true\n    command_timeout_ms: {}\n",
        base_path.display(),
        work_path.display(),
        max_concurrent_calls,
        edt_path.display(),
        command_timeout_ms,
    );
    fs::write(path, config).expect("edt config");
}

fn write_designer_config_with_options(
    path: &Path,
    base_path: &Path,
    work_path: &Path,
    platform_path: &Path,
    command_timeout_ms: u64,
    max_concurrent_calls: usize,
) {
    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: .\nmcp:\n  execution:\n    max_concurrent_calls: {}\ntools:\n  platform:\n    path: '{}'\n  edt_cli:\n    command_timeout_ms: {}\n",
        base_path.display(),
        work_path.display(),
        max_concurrent_calls,
        platform_path.display(),
        command_timeout_ms,
    );
    fs::write(path, config).expect("designer config");
}

fn write_edt_config_with_platform(
    path: &Path,
    base_path: &Path,
    work_path: &Path,
    platform_path: &Path,
    edt_path: &Path,
    command_timeout_ms: u64,
    max_concurrent_calls: usize,
) {
    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: EDT\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: main-edt\nmcp:\n  execution:\n    max_concurrent_calls: {}\ntools:\n  platform:\n    path: '{}'\n  edt_cli:\n    path: '{}'\n    interactive-mode: true\n    command_timeout_ms: {}\n",
        base_path.display(),
        work_path.display(),
        max_concurrent_calls,
        platform_path.display(),
        edt_path.display(),
        command_timeout_ms,
    );
    fs::write(path, config).expect("hybrid edt config");
}

fn setup_project() -> (tempfile::TempDir, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let platform_path = dir.path().join("platform");
    let config_path = dir.path().join("v8project.yaml");

    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    fs::create_dir_all(&platform_path).expect("platform");
    write_config(&config_path, &base_path, &work_path, &platform_path);

    (dir, config_path)
}

fn setup_edt_project() -> (tempfile::TempDir, PathBuf) {
    setup_edt_project_with_options("sleep 1\nprompt", 80, 1)
}

fn setup_edt_project_with_options(
    validate_handler: &str,
    command_timeout_ms: u64,
    max_concurrent_calls: usize,
) -> (tempfile::TempDir, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let edt_dir = dir.path().join("edt");
    let edt_path = edt_dir.join("1cedtcli");
    let config_path = dir.path().join("v8project.yaml");

    fs::create_dir_all(base_path.join("main-edt")).expect("main edt");
    fs::create_dir_all(&work_path).expect("work");
    fs::create_dir_all(&edt_dir).expect("edt dir");
    write_interactive_edt_script(
        &edt_path,
        &work_path.join("edt-workspace"),
        &dir.path().join("edt-commands.log"),
        validate_handler,
    );
    write_edt_config_with_options(
        &config_path,
        &base_path,
        &work_path,
        &edt_path,
        command_timeout_ms,
        max_concurrent_calls,
    );

    (dir, config_path)
}

fn setup_designer_project_with_options(
    script_body: &str,
    command_timeout_ms: u64,
    max_concurrent_calls: usize,
) -> (tempfile::TempDir, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let platform_dir = dir.path().join("platform");
    let config_path = dir.path().join("v8project.yaml");

    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    write_script(&platform_dir.join("bin").join("1cv8"), script_body);
    write_designer_config_with_options(
        &config_path,
        &base_path,
        &work_path,
        &platform_dir,
        command_timeout_ms,
        max_concurrent_calls,
    );

    (dir, config_path)
}

fn setup_hybrid_edt_project_with_options(
    edt_validate_handler: &str,
    platform_script_body: &str,
    command_timeout_ms: u64,
    max_concurrent_calls: usize,
) -> (tempfile::TempDir, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let edt_dir = dir.path().join("edt");
    let edt_path = edt_dir.join("1cedtcli");
    let platform_dir = dir.path().join("platform");
    let config_path = dir.path().join("v8project.yaml");

    fs::create_dir_all(base_path.join("main-edt")).expect("main edt");
    fs::create_dir_all(&work_path).expect("work");
    fs::create_dir_all(&edt_dir).expect("edt dir");
    write_interactive_edt_script(
        &edt_path,
        &work_path.join("edt-workspace"),
        &dir.path().join("edt-commands.log"),
        edt_validate_handler,
    );
    write_script(&platform_dir.join("bin").join("1cv8"), platform_script_body);
    write_edt_config_with_platform(
        &config_path,
        &base_path,
        &work_path,
        &platform_dir,
        &edt_path,
        command_timeout_ms,
        max_concurrent_calls,
    );

    (dir, config_path)
}

fn write_designer_suite_config(
    path: &Path,
    base_path: &Path,
    work_path: &Path,
    platform_path: &Path,
) {
    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\ntests:\n  execution_timeout_seconds: 5\nmcp:\n  execution:\n    max_concurrent_calls: 1\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: main\ntools:\n  platform:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        platform_path.display(),
    );
    fs::write(path, config).expect("designer suite config");
}

fn setup_designer_suite_project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let platform_dir = dir.path().join("platform");
    let config_path = dir.path().join("v8project.yaml");
    let designer_calls_log = dir.path().join("designer.calls.log");
    let enterprise_calls_log = dir.path().join("enterprise.calls.log");
    let captured_config = dir.path().join("captured-config.json");

    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(base_path.join("main")).expect("main");
    fs::create_dir_all(&work_path).expect("work");
    fs::write(
        base_path.join("main").join("Module.bsl"),
        "procedure Test() endprocedure",
    )
    .expect("module");

    let designer_script = format!(
        "args=\"$*\"\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nprintf '%s\\n' \"$args\" >> '{}'\nif [ -n \"$out\" ]; then\n  mkdir -p \"$(dirname \"$out\")\"\n  case \"$args\" in\n    *\"/CheckModules\"*)\n      cat <<'LOG' > \"$out\"\n{{CommonModules.TestModule(4,2)}}: Ошибка компиляции\n{{1}}: context\nLOG\n      exit 101\n      ;;\n    *)\n      : > \"$out\"\n      ;;\n  esac\nfi\nexit 0",
        designer_calls_log.display()
    );
    write_script(&platform_dir.join("bin").join("1cv8"), &designer_script);

    let enterprise_script = format!(
        "args=\"$*\"\nprintf '%s\\n' \"$args\" >> '{}'\npayload=\"\"\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/C\" ]; then payload=\"$arg\"; fi\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\ncase \"$args\" in\n  *\"RunUnitTests=\"*)\n    cfg=$(printf '%s' \"$payload\" | sed 's/^RunUnitTests=//; s/^\"//; s/\"$//')\n    cp \"$cfg\" '{}'\n    report=$(awk -F '\"' '/reportPath/ {{print $4; exit}}' \"$cfg\")\n    ylog=$(awk -F '\"' '/\"file\"/ {{print $4; exit}}' \"$cfg\")\n    mkdir -p \"$(dirname \"$report\")\" \"$(dirname \"$ylog\")\"\n    cat <<'XML' > \"$report\"\n<testsuites><testsuite name=\"suite\"><testcase name=\"ok\" classname=\"Sample\" time=\"0.1\"/></testsuite></testsuites>\nXML\n    cat <<'LOG' > \"$ylog\"\n12:00:00.000 [INF] ok\nLOG\n    if [ -n \"$out\" ]; then mkdir -p \"$(dirname \"$out\")\" && : > \"$out\"; fi\n    exit 0\n    ;;\n  *)\n    sleep 1\n    exit 0\n    ;;\nesac",
        enterprise_calls_log.display(),
        captured_config.display()
    );
    write_script(&platform_dir.join("bin").join("1cv8c"), &enterprise_script);

    write_designer_suite_config(&config_path, &base_path, &work_path, &platform_dir);

    (
        dir,
        config_path,
        designer_calls_log,
        enterprise_calls_log,
        captured_config,
    )
}

fn write_ibcmd_config_with_infobase(
    path: &Path,
    base_path: &Path,
    work_path: &Path,
    ibcmd_path: &Path,
    infobase_yaml: &str,
) {
    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: IBCMD\ninfobase:\n{}mcp:\n  execution:\n    max_concurrent_calls: 1\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: main\ntools:\n  platform:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        infobase_yaml,
        ibcmd_path.display(),
    );
    fs::write(path, config).expect("ibcmd config");
}

fn write_ibcmd_script(path: &Path, calls_log: &Path, fail_pattern: Option<&str>) {
    let fail_branch = fail_pattern
        .map(|pattern| {
            format!(
                "if printf '%s' \"$args\" | grep -F -q -- '{}'; then exit 17; fi",
                pattern
            )
        })
        .unwrap_or_default();
    let body = format!(
        "args=\"$*\"\nprintf '%s\\n' \"$args\" >> '{}'\n{}\nmkdir -p \"$(printf '%s' \"$args\" | awk '{{print $NF}}')\"\nexit 0",
        calls_log.display(),
        fail_branch
    );
    write_script(path, &body);
}

fn setup_ibcmd_dump_project(fail_pattern: Option<&str>) -> (tempfile::TempDir, PathBuf, PathBuf) {
    setup_ibcmd_dump_project_with_infobase(fail_pattern, "  connection: 'File=/tmp/ib'\n")
}

fn setup_ibcmd_dump_project_with_infobase(
    fail_pattern: Option<&str>,
    infobase_yaml: &str,
) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let ibcmd_path = dir.path().join("ibcmd");
    let config_path = dir.path().join("v8project.yaml");
    let calls_log = dir.path().join("ibcmd.calls.log");

    fs::create_dir_all(base_path.join("main")).expect("main");
    fs::create_dir_all(&work_path).expect("work");
    fs::write(base_path.join("main").join("old.txt"), "old").expect("old");
    write_ibcmd_script(&ibcmd_path, &calls_log, fail_pattern);
    write_ibcmd_config_with_infobase(
        &config_path,
        &base_path,
        &work_path,
        &ibcmd_path,
        infobase_yaml,
    );

    (dir, config_path, calls_log)
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut perms = fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod");
}

#[cfg(unix)]
fn write_script(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create dirs");
    }
    fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write script");
    make_executable(path);
}

fn write_interactive_edt_script(
    path: &Path,
    workspace: &Path,
    command_log_path: &Path,
    validate_handler: &str,
) {
    let body = format!(
        "set -eu\n\
         prompt() {{ printf '1C:EDT>'; }}\n\
         workspace='{}'\n\
         cwd=\"$workspace\"\n\
         dirty=0\n\
         prompt\n\
         while IFS= read -r line; do\n\
           printf '%s\\n' \"$line\" >> '{}'\n\
           eval \"set -- $line\"\n\
           cmd=\"${{1:-}}\"\n\
           if [ \"$#\" -gt 0 ]; then shift; fi\n\
           case \"$cmd\" in\n\
             cd)\n\
               if [ \"$#\" -eq 0 ]; then\n\
                 printf '%s\\n' \"$cwd\"\n\
               else\n\
                 cwd=\"$1\"\n\
                 if [ \"$cwd\" = \"$workspace\" ]; then dirty=0; fi\n\
               fi\n\
               prompt\n\
               ;;\n\
             validate)\n\
               out=\"\"\n\
               project=\"\"\n\
               while [ \"$#\" -gt 0 ]; do\n\
                 case \"$1\" in\n\
                   --file)\n\
                     out=\"$2\"\n\
                     shift 2\n\
                     ;;\n\
                   --project-list)\n\
                     project=\"$2\"\n\
                     shift 2\n\
                     ;;\n\
                   *)\n\
                     shift\n\
                     ;;\n\
                 esac\n\
               done\n\
               {}\n\
               ;;\n\
             *)\n\
               printf 'unknown:%s\\n' \"$line\"\n\
               prompt\n\
               ;;\n\
           esac\n\
         done\n",
        workspace.display(),
        command_log_path.display(),
        validate_handler
    );
    write_script(path, &body);
}

fn read_invocation_count(path: &Path) -> usize {
    fs::read_to_string(path)
        .ok()
        .map(|contents| contents.lines().count())
        .unwrap_or(0)
}

async fn wait_for_invocation_count(path: &Path, expected: usize) {
    for _ in 0..100 {
        if read_invocation_count(path) >= expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    panic!(
        "timed out waiting for {expected} invocation(s), current count={}",
        read_invocation_count(path)
    );
}

fn schema_supports_type(value: &Value, expected: &str) -> bool {
    value == expected
        || value
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item == expected))
}

#[test]
fn mcp_missing_config_reports_error_on_stderr() {
    let output = std::process::Command::new(cargo_bin("v8-runner"))
        .args([
            "--config",
            "/definitely/missing/v8project.yaml",
            "mcp",
            "serve",
            "stdio",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("config"));
    assert!(stderr.contains("not found"));
}

#[test]
fn mcp_legacy_top_level_connection_reports_error_on_stderr() {
    let dir = tempdir().expect("tempdir");
    let config_path = dir.path().join("v8project.yaml");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    fs::write(
        &config_path,
        format!(
            "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: .\n",
            base_path.display(),
            work_path.display()
        ),
    )
    .expect("config");

    let output = std::process::Command::new(cargo_bin("v8-runner"))
        .args([
            "--config",
            &config_path.display().to_string(),
            "mcp",
            "serve",
            "stdio",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("legacy top-level key 'connection'"));
}

#[test]
fn mcp_legacy_top_level_credentials_reports_error_on_stderr() {
    let dir = tempdir().expect("tempdir");
    let config_path = dir.path().join("v8project.yaml");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    fs::write(
        &config_path,
        format!(
            "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\ncredentials:\n  user: Admin\n  password: secret\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: .\n",
            base_path.display(),
            work_path.display()
        ),
    )
    .expect("config");

    let output = std::process::Command::new(cargo_bin("v8-runner"))
        .args([
            "--config",
            &config_path.display().to_string(),
            "mcp",
            "serve",
            "stdio",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("legacy top-level key 'credentials'"));
}

#[tokio::test]
async fn mcp_stdio_exposes_expected_tools_and_capabilities() {
    let (_dir, config_path) = setup_project();
    let (transport, _stderr) = TokioChildProcess::builder(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .stderr(Stdio::piped())
    .spawn()
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let info = serde_json::to_value(client.peer().peer_info().expect("peer info")).expect("info");
    let tools = client.list_all_tools().await.expect("list tools");

    let mut names: Vec<String> = tools.iter().map(|tool| tool.name.to_string()).collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "build_project",
            "check_syntax_designer_config",
            "check_syntax_designer_modules",
            "check_syntax_edt",
            "dump_config",
            "launch_app",
            "run_all_tests",
            "run_module_tests",
        ]
    );
    assert!(info["capabilities"]["tools"].is_object());
    assert!(info["capabilities"]["resources"].is_null());
    assert!(info["capabilities"]["prompts"].is_null());

    let launch_schema = tools
        .iter()
        .find(|tool| tool.name == "launch_app")
        .map(|tool| serde_json::to_value(&tool.input_schema).expect("launch schema"))
        .expect("launch tool");
    assert_eq!(launch_schema["properties"]["utilityType"]["type"], "string");

    let module_schema = tools
        .iter()
        .find(|tool| tool.name == "run_module_tests")
        .map(|tool| serde_json::to_value(&tool.input_schema).expect("module schema"))
        .expect("module tool");
    assert!(module_schema["required"]
        .as_array()
        .expect("required")
        .iter()
        .any(|value| value == "moduleName"));
    let build_schema = tools
        .iter()
        .find(|tool| tool.name == "build_project")
        .map(|tool| serde_json::to_value(&tool.input_schema).expect("build schema"))
        .expect("build tool");
    assert!(schema_supports_type(
        &build_schema["properties"]["fullRebuild"]["type"],
        "boolean"
    ));

    let tests_schema = tools
        .iter()
        .find(|tool| tool.name == "run_all_tests")
        .map(|tool| serde_json::to_value(&tool.input_schema).expect("tests schema"))
        .expect("tests tool");
    assert!(schema_supports_type(
        &tests_schema["properties"]["full"]["type"],
        "boolean"
    ));

    let dump_schema = tools
        .iter()
        .find(|tool| tool.name == "dump_config")
        .map(|tool| serde_json::to_value(&tool.input_schema).expect("dump schema"))
        .expect("dump tool");
    assert!(schema_supports_type(
        &dump_schema["properties"]["mode"]["type"],
        "string"
    ));
    assert_eq!(dump_schema["properties"]["objects"]["type"], "array");

    let edt_schema = tools
        .iter()
        .find(|tool| tool.name == "check_syntax_edt")
        .map(|tool| serde_json::to_value(&tool.input_schema).expect("edt schema"))
        .expect("edt tool");
    assert!(schema_supports_type(
        &edt_schema["properties"]["projectName"]["type"],
        "string"
    ));

    let designer_config_schema = tools
        .iter()
        .find(|tool| tool.name == "check_syntax_designer_config")
        .map(|tool| serde_json::to_value(&tool.input_schema).expect("designer config schema"))
        .expect("designer config tool");
    assert!(schema_supports_type(
        &designer_config_schema["properties"]["allExtensions"]["type"],
        "boolean"
    ));
    assert!(schema_supports_type(
        &designer_config_schema["properties"]["checkUseSynchronousCalls"]["type"],
        "boolean"
    ));

    let designer_modules_schema = tools
        .iter()
        .find(|tool| tool.name == "check_syntax_designer_modules")
        .map(|tool| serde_json::to_value(&tool.input_schema).expect("designer modules schema"))
        .expect("designer modules tool");
    assert!(schema_supports_type(
        &designer_modules_schema["properties"]["server"]["type"],
        "boolean"
    ));
    assert!(schema_supports_type(
        &designer_modules_schema["properties"]["extendedModulesCheck"]["type"],
        "boolean"
    ));

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_returns_structured_business_failure() {
    let (_dir, config_path) = setup_project();
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let response = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("run_module_tests").with_arguments(
                serde_json::from_value(json!({ "moduleName": "   " })).expect("arguments"),
            ),
        )
        .await
        .expect("call tool");

    assert_eq!(response.is_error, Some(true));
    let payload: Value = response.structured_content.expect("structured payload");
    assert_eq!(payload["status"], "business_failure");
    assert_eq!(payload["error"]["code"], "invalid_argument");
    assert_eq!(payload["response"]["success"], false);

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_run_all_tests_returns_success_payload() {
    let (_dir, config_path, designer_calls_log, enterprise_calls_log, _captured_config) =
        setup_designer_suite_project();
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let response = client
        .peer()
        .call_tool(CallToolRequestParams::new("run_all_tests"))
        .await
        .expect("call tool");

    assert_eq!(response.is_error, Some(false));
    let payload: Value = response.structured_content.expect("structured payload");
    assert_eq!(payload["status"], "success");
    assert_eq!(payload["result"]["success"], true);
    assert_eq!(payload["result"]["total_tests"], 1);
    assert!(fs::read_to_string(designer_calls_log)
        .expect("designer calls")
        .contains("/UpdateDBCfg"));
    assert!(fs::read_to_string(enterprise_calls_log)
        .expect("enterprise calls")
        .contains("RunUnitTests="));

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_run_module_tests_preserves_module_scope() {
    let (_dir, config_path, _designer_calls_log, enterprise_calls_log, captured_config) =
        setup_designer_suite_project();
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let response = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("run_module_tests").with_arguments(
                serde_json::from_value(json!({ "moduleName": "Billing" })).expect("arguments"),
            ),
        )
        .await
        .expect("call tool");

    assert_eq!(response.is_error, Some(false));
    let payload: Value = response.structured_content.expect("structured payload");
    assert_eq!(payload["status"], "success");
    assert_eq!(payload["result"]["success"], true);
    let captured: Value =
        serde_json::from_slice(&fs::read(captured_config).expect("captured config json"))
            .expect("captured config value");
    assert_eq!(captured["filter"]["modules"][0], "Billing");
    assert!(fs::read_to_string(enterprise_calls_log)
        .expect("enterprise calls")
        .contains("RunUnitTests="));

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_build_project_runs_full_rebuild_successfully() {
    let (_dir, config_path, designer_calls_log, _enterprise_calls_log, _captured_config) =
        setup_designer_suite_project();
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let response = client
        .peer()
        .call_tool(CallToolRequestParams::new("build_project").with_arguments(
            serde_json::from_value(json!({ "fullRebuild": true })).expect("arguments"),
        ))
        .await
        .expect("call tool");

    assert_eq!(response.is_error, Some(false));
    let payload: Value = response.structured_content.expect("structured payload");
    assert_eq!(payload["status"], "success");
    assert_eq!(payload["result"]["success"], true);
    assert!(fs::read_to_string(designer_calls_log)
        .expect("designer calls")
        .contains("/UpdateDBCfg"));

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_launch_app_returns_success_for_thin_client() {
    let (_dir, config_path, _designer_calls_log, enterprise_calls_log, _captured_config) =
        setup_designer_suite_project();
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let response = client
        .peer()
        .call_tool(CallToolRequestParams::new("launch_app").with_arguments(
            serde_json::from_value(json!({ "utilityType": "thin" })).expect("arguments"),
        ))
        .await
        .expect("call tool");

    assert_eq!(response.is_error, Some(false));
    let payload: Value = response.structured_content.expect("structured payload");
    assert_eq!(payload["status"], "success");
    assert_eq!(payload["result"]["success"], true);
    assert!(!fs::read_to_string(enterprise_calls_log)
        .expect("enterprise calls")
        .contains("RunUnitTests="));

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_check_syntax_designer_modules_returns_structured_issues() {
    let (_dir, config_path, designer_calls_log, _enterprise_calls_log, _captured_config) =
        setup_designer_suite_project();
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let response = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("check_syntax_designer_modules").with_arguments(
                serde_json::from_value(json!({ "server": true })).expect("arguments"),
            ),
        )
        .await
        .expect("call tool");

    assert_eq!(response.is_error, Some(true));
    let payload: Value = response.structured_content.expect("structured payload");
    assert_eq!(payload["status"], "business_failure");
    assert_eq!(payload["response"]["check_result"], "issues_found");
    assert_eq!(payload["response"]["issues"][0]["kind"], "module");
    assert_eq!(
        payload["response"]["issues"][0]["path"],
        "CommonModules.TestModule"
    );
    assert!(fs::read_to_string(designer_calls_log)
        .expect("designer calls")
        .contains("/CheckModules"));

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_dump_config_full_returns_success_payload() {
    let (_dir, config_path, designer_calls_log, _enterprise_calls_log, _captured_config) =
        setup_designer_suite_project();
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let response =
        client
            .peer()
            .call_tool(CallToolRequestParams::new("dump_config").with_arguments(
                serde_json::from_value(json!({ "mode": "FULL" })).expect("arguments"),
            ))
            .await
            .expect("call tool");

    assert_eq!(response.is_error, Some(false));
    let payload: Value = response.structured_content.expect("structured payload");
    assert_eq!(payload["status"], "success");
    assert_eq!(payload["result"]["success"], true);
    assert_eq!(payload["result"]["mode"], "FULL");
    assert!(fs::read_to_string(designer_calls_log)
        .expect("designer calls")
        .contains("DumpConfigToFiles"));

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_dump_config_partial_designer_preserves_partial_mode() {
    let (_dir, config_path, designer_calls_log, _enterprise_calls_log, _captured_config) =
        setup_designer_suite_project();
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let response = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("dump_config").with_arguments(
                serde_json::from_value(json!({
                    "mode": "PARTIAL",
                    "objects": ["Catalog.Items"]
                }))
                .expect("arguments"),
            ),
        )
        .await
        .expect("call tool");

    assert_eq!(response.is_error, Some(false));
    let payload: Value = response.structured_content.expect("structured payload");
    assert_eq!(payload["status"], "success");
    assert_eq!(payload["result"]["success"], true);
    assert_eq!(payload["result"]["mode"], "PARTIAL");
    let calls = fs::read_to_string(designer_calls_log).expect("designer calls");
    assert!(calls.contains("DumpConfigToFiles"));
    assert!(calls.contains("-partial"));

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_dump_config_partial_ibcmd_returns_degraded_success() {
    let (_dir, config_path, calls_log) = setup_ibcmd_dump_project(None);
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let response = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("dump_config").with_arguments(
                serde_json::from_value(json!({
                    "mode": "PARTIAL",
                    "objects": ["Catalog.Items"]
                }))
                .expect("arguments"),
            ),
        )
        .await
        .expect("call tool");

    assert_eq!(response.is_error, Some(false));
    let payload: Value = response.structured_content.expect("structured payload");
    assert_eq!(payload["status"], "success");
    assert_eq!(payload["result"]["success"], true);
    assert_eq!(payload["result"]["mode"], "PARTIAL");
    assert!(payload["result"]["message"]
        .as_str()
        .expect("message")
        .contains("IBCMD does not support object-scoped partial dump"));
    assert!(fs::read_to_string(calls_log)
        .expect("ibcmd calls")
        .contains("--sync"));

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_dump_config_full_ibcmd_server_contract_passes_dbms_and_infobase_credentials() {
    let (_dir, config_path, calls_log) = setup_ibcmd_dump_project_with_infobase(
        None,
        "  connection: 'Srvr=server;Ref=main'\n  user: Admin\n  password: secret\n  dbms:\n    kind: PostgreSQL\n    server: localhost\n    name: maindb\n    user: postgres\n    password: pg-secret\n",
    );
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let response =
        client
            .peer()
            .call_tool(CallToolRequestParams::new("dump_config").with_arguments(
                serde_json::from_value(json!({ "mode": "FULL" })).expect("arguments"),
            ))
            .await
            .expect("call tool");

    assert_eq!(response.is_error, Some(false));
    let payload: Value = response.structured_content.expect("structured payload");
    assert_eq!(payload["status"], "success");
    assert_eq!(payload["result"]["success"], true);
    let calls = fs::read_to_string(calls_log).expect("ibcmd calls");
    assert!(calls.contains("--dbms PostgreSQL --database-server localhost --database-name maindb"));
    assert!(calls.contains("--user Admin --password secret"));
    assert!(calls.contains("--database-user postgres --database-password pg-secret"));

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_dump_config_partial_ibcmd_preserves_partial_mode_on_failure() {
    let (_dir, config_path, calls_log) = setup_ibcmd_dump_project(Some("--sync"));
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let response = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("dump_config").with_arguments(
                serde_json::from_value(json!({
                    "mode": "PARTIAL",
                    "objects": ["Catalog.Items"]
                }))
                .expect("arguments"),
            ),
        )
        .await
        .expect("call tool");

    assert_eq!(response.is_error, Some(true));
    let payload: Value = response.structured_content.expect("structured payload");
    assert_eq!(payload["status"], "business_failure");
    assert_eq!(payload["response"]["mode"], "PARTIAL");
    assert!(payload["response"]["message"]
        .as_str()
        .expect("message")
        .contains("IBCMD does not support object-scoped partial dump"));
    assert!(payload["response"]["message"]
        .as_str()
        .expect("message")
        .contains("dump failed for source-set 'main' with exit code 17"));
    assert!(fs::read_to_string(calls_log)
        .expect("ibcmd calls")
        .contains("--sync"));

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_returns_transport_timeout_for_edt_syntax() {
    let (_dir, config_path) = setup_edt_project();
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let error = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("check_syntax_edt").with_arguments(
                serde_json::from_value(json!({ "projectName": "main" })).expect("arguments"),
            ),
        )
        .await
        .expect_err("tool call must return MCP transport error");

    match error {
        ServiceError::McpError(error_data) => {
            assert_eq!(error_data.code, ErrorCode::INTERNAL_ERROR);
            assert_eq!(
                error_data
                    .data
                    .as_ref()
                    .and_then(|data| data.get("timeoutMs")),
                Some(&json!(80))
            );
            assert_eq!(
                error_data.data.as_ref().and_then(|data| data.get("reason")),
                Some(&json!("timeout"))
            );
            assert_eq!(
                error_data.data.as_ref().and_then(|data| data.get("stage")),
                Some(&json!("running"))
            );
        }
        other => panic!("expected MCP error, got {other:?}"),
    }

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_edt_syntax_resets_interactive_state_before_each_call() {
    let validate_handler = "if [ \"$cwd\" != \"$workspace\" ]; then\n  printf 'cwd mismatch:%s\\n' \"$cwd\"\nelif [ \"$dirty\" -ne 0 ]; then\n  printf 'state leaked\\n'\nelse\n  if [ -n \"$out\" ]; then : > \"$out\"; fi\n  dirty=1\nfi\nprompt";
    let (dir, config_path) = setup_edt_project_with_options(validate_handler, 200, 1);
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    for _ in 0..2 {
        let response = client
            .peer()
            .call_tool(
                CallToolRequestParams::new("check_syntax_edt").with_arguments(
                    serde_json::from_value(json!({ "projectName": "main" })).expect("arguments"),
                ),
            )
            .await
            .expect("edt syntax call");
        assert_eq!(response.is_error, Some(false));
        let payload: Value = response.structured_content.expect("structured payload");
        assert_eq!(payload["status"], "success");
    }

    let commands = fs::read_to_string(dir.path().join("edt-commands.log")).expect("command log");
    let lines: Vec<&str> = commands.lines().collect();
    assert_eq!(lines.len(), 6);
    assert!(lines[0].starts_with("cd "));
    assert_eq!(lines[1], "cd");
    assert!(lines[2].starts_with("validate --file "));
    assert!(lines[3].starts_with("cd "));
    assert_eq!(lines[4], "cd");
    assert!(lines[5].starts_with("validate --file "));
    assert!(lines[0].contains("work/edt-workspace"));

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_cancels_running_edt_tool_and_retains_capacity_until_detached_completion() {
    let dir = tempdir().expect("tempdir");
    let starts_log = dir.path().join("edt-starts.log");
    let validate_handler = format!(
        "printf 'start\\n' >> '{}'\nif [ -n \"$out\" ]; then : > \"$out\"; fi\nsleep 0.2\nprompt",
        starts_log.display()
    );
    let (_project, config_path) = setup_edt_project_with_options(&validate_handler, 1200, 1);
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let handle = client
        .peer()
        .send_cancellable_request(
            ClientRequest::CallToolRequest(CallToolRequest::new(
                CallToolRequestParams::new("check_syntax_edt").with_arguments(
                    serde_json::from_value(json!({ "projectName": "main" })).expect("arguments"),
                ),
            )),
            PeerRequestOptions::default(),
        )
        .await
        .expect("send cancellable request");

    wait_for_invocation_count(&starts_log, 1).await;
    handle
        .peer
        .notify_cancelled(CancelledNotificationParam {
            request_id: handle.id.clone(),
            reason: Some(String::from("integration-test")),
        })
        .await
        .expect("cancel request");

    let error = handle
        .await_response()
        .await
        .expect_err("cancelled call must return transport error");
    match error {
        ServiceError::Cancelled { reason } => {
            assert_eq!(reason.as_deref(), Some("integration-test"));
        }
        other => panic!("expected cancelled request, got {other:?}"),
    }

    let follow_up = tokio::spawn({
        let peer = client.peer().clone();
        async move {
            peer.call_tool(
                CallToolRequestParams::new("check_syntax_edt").with_arguments(
                    serde_json::from_value(json!({ "projectName": "main" })).expect("arguments"),
                ),
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(read_invocation_count(&starts_log), 1);

    let follow_up = follow_up
        .await
        .expect("follow-up task join")
        .expect("capacity must recover after detached work finishes");
    assert_eq!(follow_up.is_error, Some(false));
    assert_eq!(read_invocation_count(&starts_log), 2);

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_edt_syntax_preserves_issues_found_when_stdout_is_non_empty() {
    let validate_handler = "printf 'informational stdout\\n'\nif [ -n \"$out\" ]; then printf 'ERROR\\tCatalogs.Items\\t1\\t2\\tUnusedVariables\\tunused variable\\n' > \"$out\"; fi\nprompt";
    let (_dir, config_path) = setup_edt_project_with_options(validate_handler, 200, 1);
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let response = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("check_syntax_edt").with_arguments(
                serde_json::from_value(json!({ "projectName": "main" })).expect("arguments"),
            ),
        )
        .await
        .expect("tool call");

    assert_eq!(response.is_error, Some(true));
    let payload: Value = response.structured_content.expect("structured payload");
    assert_eq!(payload["status"], "business_failure");
    assert_eq!(payload["response"]["check_result"], "issues_found");
    assert_eq!(payload["response"]["issues"][0]["path"], "Catalogs.Items");

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_edt_syntax_treats_stdout_without_issues_as_tool_failure() {
    let validate_handler =
        "printf 'unexpected stdout\\n'\nif [ -n \"$out\" ]; then : > \"$out\"; fi\nprompt";
    let (_dir, config_path) = setup_edt_project_with_options(validate_handler, 200, 1);
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let response = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("check_syntax_edt").with_arguments(
                serde_json::from_value(json!({ "projectName": "main" })).expect("arguments"),
            ),
        )
        .await
        .expect("tool call");

    assert_eq!(response.is_error, Some(true));
    let payload: Value = response.structured_content.expect("structured payload");
    assert_eq!(payload["status"], "business_failure");
    assert_eq!(payload["response"]["check_result"], "tool_failed");

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_cancels_running_standard_tool_and_retains_capacity_until_detached_completion() {
    let dir = tempdir().expect("tempdir");
    let starts_log = dir.path().join("designer-starts.log");
    let script_body = format!(
        "printf 'start\\n' >> '{}'\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf '' > \"$out\"; fi\nsleep 1\nexit 0",
        starts_log.display()
    );
    let (_project, config_path) = setup_designer_project_with_options(&script_body, 20, 1);
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let handle = client
        .peer()
        .send_cancellable_request(
            ClientRequest::CallToolRequest(CallToolRequest::new(CallToolRequestParams::new(
                "check_syntax_designer_config",
            ))),
            PeerRequestOptions::default(),
        )
        .await
        .expect("send cancellable request");

    wait_for_invocation_count(&starts_log, 1).await;
    handle
        .peer
        .notify_cancelled(CancelledNotificationParam {
            request_id: handle.id.clone(),
            reason: Some(String::from("integration-test")),
        })
        .await
        .expect("cancel request");

    let error = handle
        .await_response()
        .await
        .expect_err("cancelled call must return transport error");
    match error {
        ServiceError::Cancelled { reason } => {
            assert_eq!(reason.as_deref(), Some("integration-test"));
        }
        other => panic!("expected cancelled request, got {other:?}"),
    }

    let follow_up = tokio::spawn({
        let peer = client.peer().clone();
        async move {
            peer.call_tool(CallToolRequestParams::new("check_syntax_designer_config"))
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(read_invocation_count(&starts_log), 1);

    follow_up
        .await
        .expect("follow-up task join")
        .expect("capacity must recover after detached work finishes");
    assert_eq!(read_invocation_count(&starts_log), 2);

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_queued_timeout_reports_full_payload_for_bounded_tool() {
    let dir = tempdir().expect("tempdir");
    let edt_starts_log = dir.path().join("edt-starts.log");
    let launch_starts_log = dir.path().join("launch-starts.log");
    let edt_script_body = format!(
        "printf 'start\\n' >> '{}'\nsleep 1\nprompt",
        edt_starts_log.display()
    );
    let platform_script_body = format!(
        "printf 'start\\n' >> '{}'\nsleep 1\nexit 0",
        launch_starts_log.display()
    );
    let (_project, config_path) =
        setup_hybrid_edt_project_with_options(&edt_script_body, &platform_script_body, 20, 1);
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let first = tokio::spawn({
        let peer = client.peer().clone();
        async move {
            peer.call_tool(CallToolRequestParams::new("launch_app").with_arguments(
                serde_json::from_value(json!({ "utilityType": "thick" })).expect("arguments"),
            ))
            .await
        }
    });

    wait_for_invocation_count(&launch_starts_log, 1).await;
    let error = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("check_syntax_edt").with_arguments(
                serde_json::from_value(json!({ "projectName": "main" })).expect("arguments"),
            ),
        )
        .await
        .expect_err("queued bounded call must time out");

    match error {
        ServiceError::McpError(error_data) => {
            assert_eq!(error_data.code, ErrorCode::INTERNAL_ERROR);
            assert_eq!(
                error_data.data.as_ref().and_then(|data| data.get("reason")),
                Some(&json!("timeout"))
            );
            assert_eq!(
                error_data.data.as_ref().and_then(|data| data.get("stage")),
                Some(&json!("queued"))
            );
            assert_eq!(
                error_data
                    .data
                    .as_ref()
                    .and_then(|data| data.get("timeoutMs")),
                Some(&json!(20))
            );
        }
        other => panic!("expected MCP error, got {other:?}"),
    }

    let first_result = first.await.expect("first task join");
    let launch_result = first_result.expect("first launch call");
    assert_eq!(launch_result.is_error, Some(false));
    assert_eq!(read_invocation_count(&edt_starts_log), 0);

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_standard_tools_do_not_inherit_edt_running_timeout() {
    let (_dir, config_path) = setup_designer_project_with_options(
        "out=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf '' > \"$out\"; fi\nsleep 1\nexit 0",
        20,
        1,
    );
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-runner")).configure(|cmd| {
            cmd.arg("--config")
                .arg(config_path.as_os_str())
                .arg("mcp")
                .arg("serve")
                .arg("stdio");
        }),
    )
    .expect("spawn stdio transport");

    let client = ().serve(transport).await.expect("connect rmcp client");
    let response = client
        .peer()
        .call_tool(CallToolRequestParams::new("check_syntax_designer_config"))
        .await
        .expect("standard tool should not time out");

    assert_eq!(response.is_error, Some(false));
    let payload: Value = response.structured_content.expect("structured payload");
    assert_eq!(payload["status"], "success");
    assert_eq!(payload["error"], Value::Null);

    client.cancel().await.expect("cancel client");
}
