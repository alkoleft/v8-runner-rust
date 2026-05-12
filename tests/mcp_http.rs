#![cfg(unix)]

mod support;

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use support::{
    temp_workspace, v8_runner_binary, v8_runner_command, wait_for_log_contains,
    wait_until_async_condition, write_shell_script as write_script,
};

const ACCEPT_BOTH: &str = "application/json, text/event-stream";
const V8_CONFIGURATION_NATURE: &str = "com._1c.g5.v8.dt.core.V8ConfigurationNature";
const EDT_RUNTIME_VERSION: &str = "8.3.27";

fn assert_envelope_success(payload: &Value, command: &str) {
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], command);
    assert!(payload.get("error").is_none());
}

fn assert_envelope_business_failure(payload: &Value, command: &str) {
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], command);
    assert!(payload["error"]["code"].is_string());
    assert!(payload["error"]["kind"].is_string());
    assert!(payload["error"]["message"].is_string());
}

fn reserve_local_address() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral listener");
    let address = listener.local_addr().expect("local addr");
    address.to_string()
}

fn write_interactive_edt_script(
    path: &Path,
    workspace: &Path,
    command_log_path: &Path,
    lifecycle_log_path: &Path,
    validate_handler: &str,
) {
    let body = format!(
        "set -eu\n\
         prompt() {{ printf '1C:EDT>'; }}\n\
         workspace='{}'\n\
         lifecycle_log='{}'\n\
         cwd=\"$workspace\"\n\
         printf 'startup\\n' >> \"$lifecycle_log\"\n\
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
               fi\n\
               prompt\n\
               ;;\n\
             validate)\n\
               out=\"\"\n\
               while [ \"$#\" -gt 0 ]; do\n\
                 case \"$1\" in\n\
                   --file)\n\
                     out=\"$2\"\n\
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
        lifecycle_log_path.display(),
        command_log_path.display(),
        validate_handler
    );
    write_script(path, &body);
}

fn write_http_designer_config(
    path: &Path,
    _base_path: &Path,
    work_path: &Path,
    platform_path: &Path,
    bind_address: &str,
    stateful_sessions: bool,
    max_sessions: usize,
    idle_ttl_secs: u64,
) {
    let config = format!(
        "workPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: project\nmcp:\n  http:\n    bind_address: {}\n    path: /mcp\n    stateful_sessions: {}\n    max_sessions: {}\n    idle_ttl_secs: {}\ntools:\n  platform:\n    path: '{}'\n",
        work_path.display(),
        bind_address,
        stateful_sessions,
        max_sessions,
        idle_ttl_secs,
        platform_path.display(),
    );
    fs::write(path, config).expect("designer config");
}

fn write_http_ibcmd_config_with_infobase(
    path: &Path,
    _base_path: &Path,
    work_path: &Path,
    ibcmd_path: &Path,
    bind_address: &str,
    max_sessions: usize,
    idle_ttl_secs: u64,
    infobase_yaml: &str,
) {
    let config = format!(
        "workPath: '{}'\nformat: DESIGNER\nbuilder: IBCMD\ninfobase:\n{}source-set:\n  - name: main\n    type: CONFIGURATION\n    path: project/main\nmcp:\n  http:\n    bind_address: {}\n    path: /mcp\n    stateful_sessions: true\n    max_sessions: {}\n    idle_ttl_secs: {}\ntools:\n  platform:\n    path: '{}'\n",
        work_path.display(),
        infobase_yaml,
        bind_address,
        max_sessions,
        idle_ttl_secs,
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
        "<Configuration />\n",
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

fn write_http_edt_config(
    path: &Path,
    _base_path: &Path,
    work_path: &Path,
    edt_path: &Path,
    bind_address: &str,
    max_sessions: usize,
    idle_ttl_secs: u64,
    max_concurrent_calls: usize,
    command_timeout_ms: u64,
) {
    let config = format!(
        "workPath: '{}'\nformat: EDT\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: project/main-edt\nmcp:\n  http:\n    bind_address: {}\n    path: /mcp\n    stateful_sessions: true\n    max_sessions: {}\n    idle_ttl_secs: {}\n  execution:\n    max_concurrent_calls: {}\ntools:\n  edt_cli:\n    path: '{}'\n    interactive-mode: true\n    command_timeout_ms: {}\n",
        work_path.display(),
        bind_address,
        max_sessions,
        idle_ttl_secs,
        max_concurrent_calls,
        edt_path.display(),
        command_timeout_ms,
    );
    fs::write(path, config).expect("edt config");
}

fn setup_http_designer_project(
    stateful_sessions: bool,
    max_sessions: usize,
    idle_ttl_secs: u64,
) -> (tempfile::TempDir, PathBuf, String) {
    setup_http_designer_project_with_script(
        "printf 'designer stub\\n'\nexit 0",
        stateful_sessions,
        max_sessions,
        idle_ttl_secs,
    )
}

fn setup_http_designer_project_with_script(
    script_body: &str,
    stateful_sessions: bool,
    max_sessions: usize,
    idle_ttl_secs: u64,
) -> (tempfile::TempDir, PathBuf, String) {
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let platform_dir = dir.path().join("platform");
    let config_path = dir.path().join("v8project.yaml");
    let bind_address = reserve_local_address();

    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    write_script(&platform_dir.join("bin").join("1cv8"), script_body);
    write_http_designer_config(
        &config_path,
        &base_path,
        &work_path,
        &platform_dir,
        &bind_address,
        stateful_sessions,
        max_sessions,
        idle_ttl_secs,
    );

    (dir, config_path, format!("http://{bind_address}/mcp"))
}

fn setup_http_ibcmd_dump_project(
    fail_pattern: Option<&str>,
    max_sessions: usize,
    idle_ttl_secs: u64,
) -> (tempfile::TempDir, PathBuf, String, PathBuf) {
    setup_http_ibcmd_dump_project_with_infobase(
        fail_pattern,
        max_sessions,
        idle_ttl_secs,
        "  connection: 'File=/tmp/ib'\n",
    )
}

fn setup_http_ibcmd_dump_project_with_infobase(
    fail_pattern: Option<&str>,
    max_sessions: usize,
    idle_ttl_secs: u64,
    infobase_yaml: &str,
) -> (tempfile::TempDir, PathBuf, String, PathBuf) {
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let ibcmd_path = dir.path().join("ibcmd");
    let calls_log = dir.path().join("ibcmd.calls.log");
    let config_path = dir.path().join("v8project.yaml");
    let bind_address = reserve_local_address();

    fs::create_dir_all(base_path.join("main")).expect("main");
    fs::create_dir_all(&work_path).expect("work");
    fs::write(base_path.join("main").join("old.txt"), "old").expect("old");
    write_ibcmd_script(&ibcmd_path, &calls_log, fail_pattern);
    write_http_ibcmd_config_with_infobase(
        &config_path,
        &base_path,
        &work_path,
        &ibcmd_path,
        &bind_address,
        max_sessions,
        idle_ttl_secs,
        infobase_yaml,
    );

    (
        dir,
        config_path,
        format!("http://{bind_address}/mcp"),
        calls_log,
    )
}

fn setup_http_edt_project(
    validate_handler: &str,
    max_sessions: usize,
    idle_ttl_secs: u64,
    max_concurrent_calls: usize,
    command_timeout_ms: u64,
) -> (tempfile::TempDir, PathBuf, String, PathBuf) {
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let edt_dir = dir.path().join("edt");
    let edt_path = edt_dir.join("1cedtcli");
    let command_log = dir.path().join("edt-commands.log");
    let lifecycle_log = dir.path().join("edt-lifecycle.log");
    let config_path = dir.path().join("v8project.yaml");
    let bind_address = reserve_local_address();

    write_edt_configuration_source(&base_path.join("main-edt"), "main");
    fs::create_dir_all(&work_path).expect("work");
    fs::create_dir_all(&edt_dir).expect("edt dir");
    write_interactive_edt_script(
        &edt_path,
        &work_path.join("edt-workspace"),
        &command_log,
        &lifecycle_log,
        validate_handler,
    );
    write_http_edt_config(
        &config_path,
        &base_path,
        &work_path,
        &edt_path,
        &bind_address,
        max_sessions,
        idle_ttl_secs,
        max_concurrent_calls,
        command_timeout_ms,
    );

    (
        dir,
        config_path,
        format!("http://{bind_address}/mcp"),
        lifecycle_log,
    )
}

struct HttpServerProcess {
    child: tokio::process::Child,
}

impl HttpServerProcess {
    async fn spawn(config_path: &Path, url: &str) -> Self {
        let child = tokio::process::Command::new(v8_runner_binary())
            .arg("--config")
            .arg(config_path)
            .arg("mcp")
            .arg("serve")
            .arg("http")
            .spawn()
            .expect("spawn http server");

        wait_for_server(url).await;
        Self { child }
    }

    async fn shutdown(&mut self) {
        if let Some(_status) = self.child.try_wait().expect("poll child") {
            return;
        }
        self.child.kill().await.expect("kill child");
        let _ = self.child.wait().await.expect("wait child");
    }
}

async fn wait_for_server(url: &str) {
    let authority = url
        .strip_prefix("http://")
        .expect("http url")
        .split('/')
        .next()
        .expect("authority")
        .to_owned();
    if wait_until_async_condition(100, Duration::from_millis(20), || {
        let authority = authority.clone();
        async move {
            tokio::net::TcpStream::connect(authority.as_str())
                .await
                .is_ok()
        }
    })
    .await
    {
        return;
    }
    panic!("timed out waiting for HTTP server at {url}");
}

fn extract_sse_json(body: &str) -> Value {
    for event in body.split("\n\n").filter(|event| !event.trim().is_empty()) {
        let data = event
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if !data.is_empty() {
            return serde_json::from_str(&data).expect("sse json");
        }
    }

    panic!("no JSON SSE payload in response: {body}");
}

async fn initialize_session(client: &reqwest::Client, url: &str) -> (String, Value) {
    let response = client
        .post(url)
        .header("Accept", ACCEPT_BOTH)
        .header("Content-Type", "application/json")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "1.0.0" }
            }
        }))
        .send()
        .await
        .expect("initialize request");

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let session_id = response
        .headers()
        .get("Mcp-Session-Id")
        .expect("session id header")
        .to_str()
        .expect("session id")
        .to_owned();
    let body = response.text().await.expect("initialize body");
    (session_id, extract_sse_json(&body))
}

async fn initialize_stateless(client: &reqwest::Client, url: &str) -> reqwest::Response {
    client
        .post(url)
        .header("Accept", ACCEPT_BOTH)
        .header("Content-Type", "application/json")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "1.0.0" }
            }
        }))
        .send()
        .await
        .expect("stateless initialize request")
}

async fn send_initialized(client: &reqwest::Client, url: &str, session_id: &str) {
    let response = client
        .post(url)
        .header("Accept", ACCEPT_BOTH)
        .header("Content-Type", "application/json")
        .header("Mcp-Session-Id", session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .send()
        .await
        .expect("initialized notification");

    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
}

async fn tools_list(client: &reqwest::Client, url: &str, session_id: &str) -> reqwest::Response {
    client
        .post(url)
        .header("Accept", ACCEPT_BOTH)
        .header("Content-Type", "application/json")
        .header("Mcp-Session-Id", session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        }))
        .send()
        .await
        .expect("tools/list request")
}

async fn call_tool(
    client: &reqwest::Client,
    url: &str,
    session_id: &str,
    tool_name: &str,
    arguments: Value,
    request_id: u64,
) -> reqwest::Response {
    client
        .post(url)
        .header("Accept", ACCEPT_BOTH)
        .header("Content-Type", "application/json")
        .header("Mcp-Session-Id", session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "tools/call",
            "params": {
                "name": tool_name,
                "arguments": arguments
            }
        }))
        .send()
        .await
        .expect("tools/call request")
}

async fn eventually_status<F, Fut>(mut request: F, expected: reqwest::StatusCode)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = reqwest::Response>,
{
    if wait_until_async_condition(100, Duration::from_millis(20), || {
        let response = request();
        async move { response.await.status() == expected }
    })
    .await
    {
        return;
    }

    panic!("timed out waiting for status {expected}");
}

#[test]
fn mcp_http_missing_config_reports_error_on_stderr() {
    let output = v8_runner_command()
        .args([
            "--config",
            "/definitely/missing/v8project.yaml",
            "mcp",
            "serve",
            "http",
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_http_initialize_reuses_session_and_lists_tools() {
    let (_dir, config_path, url) = setup_http_designer_project(true, 4, 900);
    let mut server = HttpServerProcess::spawn(&config_path, &url).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("http client");

    let (session_id, initialize_payload) = initialize_session(&client, &url).await;
    assert_eq!(initialize_payload["jsonrpc"], "2.0");
    assert!(initialize_payload["result"]["capabilities"]["tools"].is_object());

    send_initialized(&client, &url, &session_id).await;
    let list_response = tools_list(&client, &url, &session_id).await;
    assert_eq!(list_response.status(), reqwest::StatusCode::OK);
    let list_payload = extract_sse_json(&list_response.text().await.expect("tools/list body"));
    let mut names = list_payload["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name").to_owned())
        .collect::<Vec<_>>();
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

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_http_dump_config_full_ibcmd_server_contract_passes_dbms_and_infobase_credentials() {
    let (_dir, config_path, url, calls_log) = setup_http_ibcmd_dump_project_with_infobase(
        None,
        4,
        900,
        "  connection: 'Srvr=server;Ref=main'\n  user: Admin\n  password: secret\n  dbms:\n    kind: PostgreSQL\n    server: localhost\n    name: maindb\n    user: postgres\n    password: pg-secret\n",
    );
    let mut server = HttpServerProcess::spawn(&config_path, &url).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("http client");

    let (session_id, _) = initialize_session(&client, &url).await;
    send_initialized(&client, &url, &session_id).await;

    let response = call_tool(
        &client,
        &url,
        &session_id,
        "dump_config",
        json!({ "mode": "FULL" }),
        29,
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let payload = extract_sse_json(&response.text().await.expect("dump body"));
    let structured = &payload["result"]["structuredContent"];
    assert_envelope_success(structured, "dump");
    assert_eq!(structured["data"]["ok"], true);
    let calls = fs::read_to_string(calls_log).expect("ibcmd calls");
    assert!(calls.contains("--dbms PostgreSQL --database-server localhost --database-name maindb"));
    assert!(calls.contains("--user Admin --password secret"));
    assert!(calls.contains("--database-user postgres --database-password pg-secret"));

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_http_launch_app_returns_success_payload_over_live_session() {
    let (_dir, config_path, url) = setup_http_designer_project_with_script("sleep 1", true, 4, 900);
    let mut server = HttpServerProcess::spawn(&config_path, &url).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("http client");

    let (session_id, _) = initialize_session(&client, &url).await;
    send_initialized(&client, &url, &session_id).await;

    let response = call_tool(
        &client,
        &url,
        &session_id,
        "launch_app",
        json!({ "utilityType": "designer" }),
        20,
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let payload = extract_sse_json(&response.text().await.expect("launch body"));
    let structured = &payload["result"]["structuredContent"];
    assert_envelope_success(structured, "launch");
    assert_eq!(structured["data"]["ok"], true);

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_http_dump_config_partial_ibcmd_returns_degraded_success() {
    let (_dir, config_path, url, calls_log) = setup_http_ibcmd_dump_project(None, 4, 900);
    let mut server = HttpServerProcess::spawn(&config_path, &url).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("http client");

    let (session_id, _) = initialize_session(&client, &url).await;
    send_initialized(&client, &url, &session_id).await;

    let response = call_tool(
        &client,
        &url,
        &session_id,
        "dump_config",
        json!({
            "mode": "PARTIAL",
            "objects": ["Catalog.Items"]
        }),
        30,
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let payload = extract_sse_json(&response.text().await.expect("dump body"));
    let structured = &payload["result"]["structuredContent"];
    assert_envelope_success(structured, "dump");
    assert_eq!(structured["data"]["ok"], true);
    assert_eq!(structured["data"]["mode"], "PARTIAL");
    assert!(structured["data"]["message"]
        .as_str()
        .expect("message")
        .contains("IBCMD does not support object-scoped partial dump"));
    assert!(fs::read_to_string(calls_log)
        .expect("ibcmd calls")
        .contains("--sync"));

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_http_dump_config_partial_ibcmd_preserves_partial_mode_on_failure() {
    let (_dir, config_path, url, calls_log) = setup_http_ibcmd_dump_project(Some("--sync"), 4, 900);
    let mut server = HttpServerProcess::spawn(&config_path, &url).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("http client");

    let (session_id, _) = initialize_session(&client, &url).await;
    send_initialized(&client, &url, &session_id).await;

    let response = call_tool(
        &client,
        &url,
        &session_id,
        "dump_config",
        json!({
            "mode": "PARTIAL",
            "objects": ["Catalog.Items"]
        }),
        31,
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let payload = extract_sse_json(&response.text().await.expect("dump failure body"));
    let structured = &payload["result"]["structuredContent"];
    assert_envelope_business_failure(structured, "dump");
    assert_eq!(structured["data"]["mode"], "PARTIAL");
    assert!(structured["data"]["message"]
        .as_str()
        .expect("message")
        .contains("IBCMD does not support object-scoped partial dump"));
    assert!(structured["data"]["message"]
        .as_str()
        .expect("message")
        .contains("dump failed for source-set 'main' with exit code 17"));
    assert!(fs::read_to_string(calls_log)
        .expect("ibcmd calls")
        .contains("--sync"));

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_http_missing_and_expired_sessions_are_deterministic() {
    let (_dir, config_path, url) = setup_http_designer_project(true, 4, 1);
    let mut server = HttpServerProcess::spawn(&config_path, &url).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("http client");

    let missing_session_post = client
        .post(&url)
        .header("Accept", ACCEPT_BOTH)
        .header("Content-Type", "application/json")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        }))
        .send()
        .await
        .expect("missing session post");
    assert_eq!(
        missing_session_post.status(),
        reqwest::StatusCode::BAD_REQUEST
    );

    let missing_session_get = client
        .get(&url)
        .header("Accept", "text/event-stream")
        .send()
        .await
        .expect("missing session get");
    assert_eq!(
        missing_session_get.status(),
        reqwest::StatusCode::BAD_REQUEST
    );

    let (session_id, _) = initialize_session(&client, &url).await;
    send_initialized(&client, &url, &session_id).await;
    tokio::time::sleep(Duration::from_millis(2_000)).await;

    eventually_status(
        || {
            let client = client.clone();
            let url = url.clone();
            let session_id = session_id.clone();
            async move {
                client
                    .post(url)
                    .header("Accept", ACCEPT_BOTH)
                    .header("Content-Type", "application/json")
                    .header("Mcp-Session-Id", session_id)
                    .json(&json!({
                        "jsonrpc": "2.0",
                        "id": 3,
                        "method": "tools/list"
                    }))
                    .send()
                    .await
                    .expect("expired session request")
            }
        },
        reqwest::StatusCode::NOT_FOUND,
    )
    .await;

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_http_delete_closes_session_and_reuse_returns_not_found() {
    let (_dir, config_path, url) = setup_http_designer_project(true, 4, 900);
    let mut server = HttpServerProcess::spawn(&config_path, &url).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("http client");

    let (session_id, _) = initialize_session(&client, &url).await;
    send_initialized(&client, &url, &session_id).await;

    let delete_response = client
        .delete(&url)
        .header("Mcp-Session-Id", &session_id)
        .send()
        .await
        .expect("delete session");
    assert_eq!(delete_response.status(), reqwest::StatusCode::ACCEPTED);

    let stale_response = tools_list(&client, &url, &session_id).await;
    assert_eq!(stale_response.status(), reqwest::StatusCode::NOT_FOUND);

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_http_stateless_mode_stays_post_only_and_validates_headers() {
    let (_dir, config_path, url) = setup_http_designer_project(false, 4, 900);
    let mut server = HttpServerProcess::spawn(&config_path, &url).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("http client");

    let initialize_response = initialize_stateless(&client, &url).await;
    assert_eq!(initialize_response.status(), reqwest::StatusCode::OK);
    assert!(initialize_response
        .headers()
        .get("Mcp-Session-Id")
        .is_none());

    let wrong_accept = client
        .post(&url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "1.0.0" }
            }
        }))
        .send()
        .await
        .expect("wrong accept");
    assert_eq!(wrong_accept.status(), reqwest::StatusCode::NOT_ACCEPTABLE);

    let wrong_content_type = client
        .post(&url)
        .header("Accept", ACCEPT_BOTH)
        .header("Content-Type", "text/plain")
        .body("not-json")
        .send()
        .await
        .expect("wrong content type");
    assert_eq!(
        wrong_content_type.status(),
        reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE
    );

    let get_response = client
        .get(&url)
        .header("Accept", "text/event-stream")
        .send()
        .await
        .expect("stateless get");
    assert_eq!(
        get_response.status(),
        reqwest::StatusCode::METHOD_NOT_ALLOWED
    );

    let delete_response = client.delete(&url).send().await.expect("stateless delete");
    assert_eq!(
        delete_response.status(),
        reqwest::StatusCode::METHOD_NOT_ALLOWED
    );

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_http_initialize_burst_respects_capacity_and_recovers_after_delete() {
    let (_dir, config_path, url) = setup_http_designer_project(true, 2, 900);
    let mut server = HttpServerProcess::spawn(&config_path, &url).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("http client");

    let mut handles = Vec::new();
    for _ in 0..6 {
        let client = client.clone();
        let url = url.clone();
        handles.push(tokio::spawn(async move {
            initialize_stateless(&client, &url).await
        }));
    }
    let mut responses = Vec::new();
    for handle in handles {
        responses.push(handle.await.expect("initialize join"));
    }
    let success_count = responses
        .iter()
        .filter(|response| response.status() == reqwest::StatusCode::OK)
        .count();
    let unavailable_count = responses
        .iter()
        .filter(|response| response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE)
        .count();
    assert_eq!(success_count, 2);
    assert_eq!(unavailable_count, 4);

    let session_ids = responses
        .into_iter()
        .filter(|response| response.status() == reqwest::StatusCode::OK)
        .map(|response| {
            response
                .headers()
                .get("Mcp-Session-Id")
                .expect("session id header")
                .to_str()
                .expect("session id")
                .to_owned()
        })
        .collect::<Vec<_>>();
    for session_id in &session_ids {
        let delete_response = client
            .delete(&url)
            .header("Mcp-Session-Id", session_id)
            .send()
            .await
            .expect("delete session");
        assert_eq!(delete_response.status(), reqwest::StatusCode::ACCEPTED);
    }

    let recovered = initialize_stateless(&client, &url).await;
    assert_eq!(recovered.status(), reqwest::StatusCode::OK);

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_http_max_sessions_returns_503_and_non_initialize_stays_400() {
    let (_dir, config_path, url) = setup_http_designer_project(true, 1, 900);
    let mut server = HttpServerProcess::spawn(&config_path, &url).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("http client");

    let (session_id, _) = initialize_session(&client, &url).await;

    let overload = initialize_stateless(&client, &url).await;
    assert_eq!(overload.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        overload.text().await.expect("overload body"),
        "Service Unavailable: MCP session capacity exhausted"
    );

    let non_initialize = client
        .post(&url)
        .header("Accept", ACCEPT_BOTH)
        .header("Content-Type", "application/json")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "tools/list"
        }))
        .send()
        .await
        .expect("non initialize");
    assert_eq!(non_initialize.status(), reqwest::StatusCode::BAD_REQUEST);

    let delete_response = client
        .delete(&url)
        .header("Mcp-Session-Id", &session_id)
        .send()
        .await
        .expect("delete session");
    assert_eq!(delete_response.status(), reqwest::StatusCode::ACCEPTED);

    let recovered = initialize_stateless(&client, &url).await;
    assert_eq!(recovered.status(), reqwest::StatusCode::OK);

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_http_parallel_initialize_respects_max_sessions() {
    let (_dir, config_path, url) = setup_http_designer_project(true, 1, 900);
    let mut server = HttpServerProcess::spawn(&config_path, &url).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("http client");

    let first = initialize_stateless(&client, &url);
    let second = initialize_stateless(&client, &url);
    let (first, second) = tokio::join!(first, second);
    let statuses = [first.status(), second.status()];

    assert!(statuses.contains(&reqwest::StatusCode::OK));
    assert!(statuses.contains(&reqwest::StatusCode::SERVICE_UNAVAILABLE));

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_http_reuses_one_edt_process_across_sessions_and_shares_capacity() {
    let validate_handler = "printf 'start\\n' >> \"$lifecycle_log\"\nif [ -n \"$out\" ]; then : > \"$out\"; fi\nsleep 0.15\nprintf 'finish\\n' >> \"$lifecycle_log\"\nprompt";
    let (_dir, config_path, url, lifecycle_log) =
        setup_http_edt_project(validate_handler, 4, 900, 1, 1000);
    let mut server = HttpServerProcess::spawn(&config_path, &url).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("http client");

    let (session_a, _) = initialize_session(&client, &url).await;
    let (session_b, _) = initialize_session(&client, &url).await;
    send_initialized(&client, &url, &session_a).await;
    send_initialized(&client, &url, &session_b).await;

    let first = call_tool(
        &client,
        &url,
        &session_a,
        "check_syntax_edt",
        json!({ "projectName": "main" }),
        10,
    );
    let second = call_tool(
        &client,
        &url,
        &session_b,
        "check_syntax_edt",
        json!({ "projectName": "main" }),
        11,
    );
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.status(), reqwest::StatusCode::OK);
    assert_eq!(second.status(), reqwest::StatusCode::OK);
    let first_payload = extract_sse_json(&first.text().await.expect("first edt body"));
    let second_payload = extract_sse_json(&second.text().await.expect("second edt body"));
    assert_envelope_success(&first_payload["result"]["structuredContent"], "syntax");
    assert_envelope_success(&second_payload["result"]["structuredContent"], "syntax");

    let lifecycle = fs::read_to_string(&lifecycle_log).expect("lifecycle log");
    let lines = lifecycle.lines().collect::<Vec<_>>();
    assert_eq!(lines.first().copied(), Some("startup"));
    assert_eq!(lines[1..], ["start", "finish", "start", "finish"]);

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_http_returns_terminal_business_failure_for_edt_syntax_timeout() {
    let validate_handler = "if [ -n \"$out\" ]; then : > \"$out\"; fi\nsleep 1\nprompt";
    let (_dir, config_path, url, _lifecycle_log) =
        setup_http_edt_project(validate_handler, 4, 900, 1, 80);
    let mut server = HttpServerProcess::spawn(&config_path, &url).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("http client");

    let (session_id, _) = initialize_session(&client, &url).await;
    send_initialized(&client, &url, &session_id).await;

    let response = call_tool(
        &client,
        &url,
        &session_id,
        "check_syntax_edt",
        json!({ "projectName": "main" }),
        13,
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let payload = extract_sse_json(&response.text().await.expect("edt timeout body"));
    let structured = &payload["result"]["structuredContent"];
    assert_envelope_business_failure(structured, "syntax");
    assert_eq!(structured["data"]["status"], "tool_failed");
    assert!(structured["error"]["message"]
        .as_str()
        .expect("message")
        .contains("terminal state was observed"));

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_http_edt_action_log_contains_runtime_telemetry_events() {
    let validate_handler = "if [ -n \"$out\" ]; then : > \"$out\"; fi\nsleep 0.05\nprompt";
    let (dir, config_path, url, _lifecycle_log) =
        setup_http_edt_project(validate_handler, 4, 900, 1, 1000);
    let action_log = dir
        .path()
        .join("work")
        .join("logs")
        .join("mcp")
        .join("actions.log");
    let mut server = HttpServerProcess::spawn(&config_path, &url).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("http client");

    let (session_id, _) = initialize_session(&client, &url).await;
    send_initialized(&client, &url, &session_id).await;

    let response = call_tool(
        &client,
        &url,
        &session_id,
        "check_syntax_edt",
        json!({ "projectName": "main" }),
        12,
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let payload = extract_sse_json(&response.text().await.expect("edt body"));
    assert_envelope_success(&payload["result"]["structuredContent"], "syntax");

    wait_for_log_contains(&action_log, "mcp_execution_semaphore_wait").await;
    wait_for_log_contains(&action_log, "mcp_edt_queue_depth").await;

    let contents = fs::read_to_string(&action_log).expect("action log");
    assert!(contents.contains("mcp_http"));
    assert!(contents.contains("check_syntax_edt"));
    assert!(contents.contains("acquired"));
    assert!(contents.contains("enqueue"));

    server.shutdown().await;
}
