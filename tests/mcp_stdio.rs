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
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    purpose: CONFIGURATION\n    path: .\ntools:\n  platform:\n    path: '{}'\n",
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
        "basePath: '{}'\nworkPath: '{}'\nformat: EDT\nbuilder: DESIGNER\nconnection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    purpose: CONFIGURATION\n    path: main-edt\nmcp:\n  execution:\n    max_concurrent_calls: {}\ntools:\n  edt_cli:\n    path: '{}'\n    command_timeout_ms: {}\n",
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
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    purpose: CONFIGURATION\n    path: .\nmcp:\n  execution:\n    max_concurrent_calls: {}\ntools:\n  platform:\n    path: '{}'\n  edt_cli:\n    command_timeout_ms: {}\n",
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
        "basePath: '{}'\nworkPath: '{}'\nformat: EDT\nbuilder: DESIGNER\nconnection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    purpose: CONFIGURATION\n    path: main-edt\nmcp:\n  execution:\n    max_concurrent_calls: {}\ntools:\n  platform:\n    path: '{}'\n  edt_cli:\n    path: '{}'\n    command_timeout_ms: {}\n",
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
    let config_path = dir.path().join("application.yaml");

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
    let config_path = dir.path().join("application.yaml");

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
    let config_path = dir.path().join("application.yaml");

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
    let config_path = dir.path().join("application.yaml");

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

#[test]
fn mcp_missing_config_reports_error_on_stderr() {
    let output = std::process::Command::new(cargo_bin("v8-test-runner"))
        .args([
            "--config",
            "/definitely/missing/application.yaml",
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

#[tokio::test]
async fn mcp_stdio_exposes_expected_tools_and_capabilities() {
    let (_dir, config_path) = setup_project();
    let (transport, _stderr) = TokioChildProcess::builder(
        tokio::process::Command::new(cargo_bin("v8-test-runner")).configure(|cmd| {
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

    client.cancel().await.expect("cancel client");
}

#[tokio::test]
async fn mcp_stdio_returns_structured_business_failure() {
    let (_dir, config_path) = setup_project();
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-test-runner")).configure(|cmd| {
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
async fn mcp_stdio_returns_transport_timeout_for_edt_syntax() {
    let (_dir, config_path) = setup_edt_project();
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(cargo_bin("v8-test-runner")).configure(|cmd| {
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
        tokio::process::Command::new(cargo_bin("v8-test-runner")).configure(|cmd| {
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
        tokio::process::Command::new(cargo_bin("v8-test-runner")).configure(|cmd| {
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
        tokio::process::Command::new(cargo_bin("v8-test-runner")).configure(|cmd| {
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
        tokio::process::Command::new(cargo_bin("v8-test-runner")).configure(|cmd| {
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
        tokio::process::Command::new(cargo_bin("v8-test-runner")).configure(|cmd| {
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
        tokio::process::Command::new(cargo_bin("v8-test-runner")).configure(|cmd| {
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
        tokio::process::Command::new(cargo_bin("v8-test-runner")).configure(|cmd| {
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
