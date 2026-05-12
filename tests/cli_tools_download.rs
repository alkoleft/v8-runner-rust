#![cfg(unix)]

mod support;

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use support::{free_tcp_port, temp_workspace, v8_runner_command, wait_until};

fn write_minimal_config(root: &Path) -> PathBuf {
    write_minimal_config_with_builder(root, "DESIGNER")
}

fn write_minimal_config_with_builder(root: &Path, builder: &str) -> PathBuf {
    let base_path = root.join("project");
    let work_path = root.join("work");
    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(base_path.join("configuration")).expect("configuration");
    fs::create_dir_all(&work_path).expect("work");
    let config_path = root.join("v8project.yaml");
    fs::write(
        &config_path,
        format!(
            "# yaml-language-server: $schema=./docs/schemas/v8project.schema.json\nworkPath: '{}'\nformat: DESIGNER\nbuilder: {builder}\ninfobase:\n  connection: 'File=/tmp/ib'\nsource-set:\n  - name: configuration\n    type: CONFIGURATION\n    path: project/configuration\ntools:\n  edt_cli:\n    path: /tmp/edt\n",
            work_path.display(),
        ),
    )
    .expect("config");
    config_path
}

fn write_config_with_pending_va(root: &Path) -> PathBuf {
    let config_path = write_minimal_config(root);
    let mut config = fs::read_to_string(&config_path).expect("config");
    config.push_str(
        "tests:\n  va:\n    params_path: missing/params.json\n    profile: smoke\n    profiles:\n      smoke:\n        feature_path: missing/features\n",
    );
    fs::write(&config_path, config).expect("pending va config");
    config_path
}

fn write_config_with_execution_timeout(root: &Path, timeout_ms: u64) -> PathBuf {
    let config_path = write_minimal_config(root);
    let mut config = fs::read_to_string(&config_path).expect("config");
    config.push_str(&format!("execution_timeout: {timeout_ms}\n"));
    fs::write(&config_path, config).expect("timeout config");
    config_path
}

fn fixture_server(root: &Path, port: u16) -> Child {
    let script = format!(
        "import http.server, socketserver\nclass Handler(http.server.SimpleHTTPRequestHandler):\n    def do_GET(self):\n        redirects = [('/redirect302/', 302), ('/redirect/', 301)]\n        for prefix, status in redirects:\n            if self.path.startswith(prefix):\n                self.send_response(status)\n                self.send_header('Location', self.path[len(prefix) - 1:])\n                self.end_headers()\n                return\n        super().do_GET()\nsocketserver.TCPServer.allow_reuse_address = True\nwith socketserver.TCPServer(('127.0.0.1', {port}), Handler) as httpd:\n    httpd.serve_forever()\n"
    );
    Command::new("python3")
        .arg("-c")
        .arg(script)
        .current_dir(root)
        .spawn()
        .expect("fixture server")
}

fn sleeping_server(port: u16) -> Child {
    let script = format!(
        "import http.server, socketserver, time\nclass Handler(http.server.BaseHTTPRequestHandler):\n    def do_GET(self):\n        time.sleep(5)\n        self.send_response(200)\n        self.end_headers()\n        self.wfile.write(b'{{}}')\nsocketserver.TCPServer.allow_reuse_address = True\nwith socketserver.TCPServer(('127.0.0.1', {port}), Handler) as httpd:\n    httpd.serve_forever()\n"
    );
    Command::new("python3")
        .arg("-c")
        .arg(script)
        .spawn()
        .expect("sleeping server")
}

struct FixtureServer {
    child: Child,
}

impl FixtureServer {
    fn start(root: &Path, port: u16) -> Self {
        Self {
            child: fixture_server(root, port),
        }
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write_http_fixture(root: &Path, port: u16) {
    write_http_fixture_with_redirect_prefix(root, port, "");
}

fn write_http_fixture_with_redirects(root: &Path, port: u16, redirects: bool) {
    let prefix = if redirects { "/redirect" } else { "" };
    write_http_fixture_with_redirect_prefix(root, port, prefix);
}

fn write_http_fixture_with_redirect_prefix(root: &Path, port: u16, prefix: &str) {
    let api = root.join("repos");
    write_release(
        &api.join("bia-technologies")
            .join("yaxunit")
            .join("releases"),
        "25.12",
        &format!("http://127.0.0.1:{port}{prefix}/archives/yaxunit.zip"),
        &[(
            "YAxUnit-25.12.cfe",
            &format!("http://127.0.0.1:{port}{prefix}/assets/YAxUnit-25.12.cfe"),
        )],
    );
    write_release(
        &api.join("Pr-Mex")
            .join("vanessa-automation-single")
            .join("releases"),
        "1.2.043.1",
        &format!("http://127.0.0.1:{port}{prefix}/archives/vanessa-source.zip"),
        &[(
            "vanessa-automation-single.1.2.043.1.zip",
            &format!(
                "http://127.0.0.1:{port}{prefix}/assets/vanessa-automation-single.1.2.043.1.zip"
            ),
        )],
    );
    write_release(
        &api.join("1c-neurofish")
            .join("onec-client-mcp-devkit")
            .join("releases"),
        "v0.6.4",
        &format!("http://127.0.0.1:{port}{prefix}/archives/client-mcp.zip"),
        &[(
            "client_mcp.cfe",
            &format!("http://127.0.0.1:{port}{prefix}/assets/client_mcp.cfe"),
        )],
    );

    fs::create_dir_all(root.join("assets")).expect("assets");
    fs::write(root.join("assets").join("YAxUnit-25.12.cfe"), "yaxunit cfe").expect("yax asset");
    fs::write(root.join("assets").join("client_mcp.cfe"), "client cfe").expect("client asset");
    make_zip(
        &root
            .join("assets")
            .join("vanessa-automation-single.1.2.043.1.zip"),
        &[("vanessa-automation-single.epf", "va epf")],
    );

    fs::create_dir_all(root.join("archives")).expect("archives");
    make_zip(
        &root.join("archives").join("yaxunit.zip"),
        &[(
            "bia-technologies-yaxunit/exts/yaxunit/src/Configuration/Configuration.mdo",
            "yaxunit source",
        )],
    );
    make_zip(
        &root.join("archives").join("client-mcp.zip"),
        &[(
            "1c-neurofish-onec-client-mcp-devkit/exts/client-mcp/src/Configuration/Configuration.mdo",
            "client source",
        )],
    );
    make_zip(
        &root.join("archives").join("vanessa-source.zip"),
        &[("unused/readme.txt", "unused")],
    );
}

fn write_release(path: &Path, tag: &str, zipball_url: &str, assets: &[(&str, &str)]) {
    fs::create_dir_all(path).expect("release dir");
    let assets_json = assets
        .iter()
        .map(|(name, url)| format!(r#"{{"name":"{name}","browser_download_url":"{url}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    fs::write(
        path.join("latest"),
        format!(
            r#"{{"tag_name":"{tag}","html_url":"https://example.invalid/{tag}","zipball_url":"{zipball_url}","assets":[{assets_json}]}}"#
        ),
    )
    .expect("release json");
}

fn make_zip(path: &Path, entries: &[(&str, &str)]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("zip parent");
    }
    let status = Command::new("python3")
        .arg("-c")
        .arg(
            "import sys, zipfile\nwith zipfile.ZipFile(sys.argv[1], 'w') as z:\n    for pair in sys.argv[2:]:\n        name, value = pair.split('=', 1)\n        z.writestr(name, value)\n",
        )
        .arg(path)
        .args(entries.iter().map(|(name, value)| format!("{name}={value}")))
        .status()
        .expect("zip");
    assert!(status.success());
}

#[test]
fn tools_download_sources_writes_source_set_and_local_tool_settings() {
    let dir = temp_workspace();
    let config_path = write_minimal_config(dir.path());
    let server_root = dir.path().join("server");
    let port = free_tcp_port();
    write_http_fixture(&server_root, port);
    let _server = FixtureServer::start(&server_root, port);
    assert!(wait_until(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(50),
        || std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    ));

    let output = v8_runner_command()
        .env(
            "V8TR_GITHUB_API_BASE_URL",
            format!("http://127.0.0.1:{port}"),
        )
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "tools",
            "download",
            "yaxunit",
            "--sources",
        ])
        .output()
        .expect("run command");
    let repeat = v8_runner_command()
        .env(
            "V8TR_GITHUB_API_BASE_URL",
            format!("http://127.0.0.1:{port}"),
        )
        .args([
            "--config",
            &config_path.display().to_string(),
            "tools",
            "download",
            "yaxunit",
            "--sources",
        ])
        .output()
        .expect("run command again");
    let client_mcp = v8_runner_command()
        .env(
            "V8TR_GITHUB_API_BASE_URL",
            format!("http://127.0.0.1:{port}"),
        )
        .args([
            "--config",
            &config_path.display().to_string(),
            "tools",
            "download",
            "client-mcp",
            "--sources",
        ])
        .output()
        .expect("run client-mcp command");
    let vanessa = v8_runner_command()
        .env(
            "V8TR_GITHUB_API_BASE_URL",
            format!("http://127.0.0.1:{port}"),
        )
        .args([
            "--config",
            &config_path.display().to_string(),
            "tools",
            "download",
            "vanessa",
        ])
        .output()
        .expect("run vanessa command");

    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        repeat.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        repeat.status.code(),
        String::from_utf8_lossy(&repeat.stdout),
        String::from_utf8_lossy(&repeat.stderr)
    );
    assert!(
        client_mcp.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        client_mcp.status.code(),
        String::from_utf8_lossy(&client_mcp.stdout),
        String::from_utf8_lossy(&client_mcp.stderr)
    );
    assert!(
        vanessa.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        vanessa.status.code(),
        String::from_utf8_lossy(&vanessa.stdout),
        String::from_utf8_lossy(&vanessa.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["command"], "tools download");
    assert_eq!(payload["data"]["tool"], "yaxunit");
    assert_eq!(payload["data"]["mode"], "sources");

    let config = fs::read_to_string(&config_path).expect("config");
    assert!(config.starts_with("# yaml-language-server:"));
    assert!(config.contains("name: tests"));
    assert!(config.contains("type: EXTENSION"));
    assert!(config.contains("path: tests"));
    assert!(dir
        .path()
        .join("tests/src/Configuration/Configuration.mdo")
        .exists());
    assert!(!dir
        .path()
        .join("tests/.v8-runner-tools-download.json")
        .exists());
    assert!(!dir
        .path()
        .join(".tests.v8-runner-tools-download.json")
        .exists());
    assert!(dir
        .path()
        .join("build/.tests.v8-runner-tools-download.json")
        .exists());
    assert!(dir
        .path()
        .join("build/tools/vanessa-automation-single.epf")
        .exists());
    assert!(dir
        .path()
        .join("build/tools/onec-client-mcp-devkit/exts/client-mcp/src/Configuration/Configuration.mdo")
        .exists());
    assert!(!dir
        .path()
        .join("build/tools/onec-client-mcp-devkit/exts/client-mcp/.v8-runner-tools-download.json")
        .exists());
    assert!(dir
        .path()
        .join("build/tools/onec-client-mcp-devkit/exts/.client-mcp.v8-runner-tools-download.json")
        .exists());

    let local = fs::read_to_string(dir.path().join("v8project.local.yaml")).expect("local");
    assert!(local.contains("epf_path:"));
    assert!(local.contains("epf_path: build/tools/vanessa-automation-single.epf"));
    assert!(local.contains("client_mcp:"));
    assert!(local.contains("source:"));
    assert!(local.contains("path: build/tools/onec-client-mcp-devkit/exts/client-mcp"));
    assert!(local.contains("format: EDT"));
    assert!(!local.contains(&dir.path().display().to_string()));
}

#[test]
fn tools_download_sources_rejects_legacy_tests_markers_outside_build() {
    let dir = temp_workspace();
    let config_path = write_minimal_config(dir.path());
    fs::create_dir_all(dir.path().join("tests")).expect("tests");
    fs::write(
        dir.path().join(".tests.v8-runner-tools-download.json"),
        "{}\n",
    )
    .expect("legacy root marker");
    fs::write(
        dir.path().join("tests/.v8-runner-tools-download.json"),
        "{}\n",
    )
    .expect("legacy nested marker");

    let server_root = dir.path().join("server");
    let port = free_tcp_port();
    write_http_fixture(&server_root, port);
    let _server = FixtureServer::start(&server_root, port);
    assert!(wait_until(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(50),
        || std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    ));

    let output = v8_runner_command()
        .env(
            "V8TR_GITHUB_API_BASE_URL",
            format!("http://127.0.0.1:{port}"),
        )
        .args([
            "--config",
            &config_path.display().to_string(),
            "tools",
            "download",
            "yaxunit",
            "--sources",
        ])
        .output()
        .expect("run command");

    assert!(
        !output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(combined.contains("download target already exists and is not managed by v8-runner"));
    assert!(!dir
        .path()
        .join("build/.tests.v8-runner-tools-download.json")
        .exists());
}

#[test]
fn tools_download_repairs_pending_vanessa_configuration() {
    let dir = temp_workspace();
    let config_path = write_config_with_pending_va(dir.path());
    let server_root = dir.path().join("server");
    let port = free_tcp_port();
    write_http_fixture(&server_root, port);
    let _server = FixtureServer::start(&server_root, port);
    assert!(wait_until(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(50),
        || std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    ));

    let output = v8_runner_command()
        .env(
            "V8TR_GITHUB_API_BASE_URL",
            format!("http://127.0.0.1:{port}"),
        )
        .args([
            "--config",
            &config_path.display().to_string(),
            "tools",
            "download",
            "vanessa",
        ])
        .output()
        .expect("run command");

    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let local = fs::read_to_string(dir.path().join("v8project.local.yaml")).expect("local");
    assert!(local.contains("epf_path:"));
    assert!(local.contains("epf_path: build/tools/vanessa-automation-single.epf"));
    assert!(!local.contains(&dir.path().display().to_string()));
}

#[test]
fn tools_download_follows_latest_release_and_asset_redirects() {
    let dir = temp_workspace();
    let config_path = write_minimal_config(dir.path());
    let server_root = dir.path().join("server");
    let port = free_tcp_port();
    write_http_fixture_with_redirects(&server_root, port, true);
    let _server = FixtureServer::start(&server_root, port);
    assert!(wait_until(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(50),
        || std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    ));

    let output = v8_runner_command()
        .env(
            "V8TR_GITHUB_API_BASE_URL",
            format!("http://127.0.0.1:{port}/redirect"),
        )
        .args([
            "--config",
            &config_path.display().to_string(),
            "tools",
            "download",
            "client-mcp",
        ])
        .output()
        .expect("run command");

    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(dir.path().join("build/tools/client_mcp.cfe").exists());
    let local = fs::read_to_string(dir.path().join("v8project.local.yaml")).expect("local");
    assert!(local.contains("artifact:"));
    assert!(local.contains("client_mcp.cfe"));
}

#[test]
fn tools_download_follows_302_redirects() {
    let dir = temp_workspace();
    let config_path = write_minimal_config(dir.path());
    let server_root = dir.path().join("server");
    let port = free_tcp_port();
    write_http_fixture_with_redirect_prefix(&server_root, port, "/redirect302");
    let _server = FixtureServer::start(&server_root, port);
    assert!(wait_until(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(50),
        || std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    ));

    let output = v8_runner_command()
        .env(
            "V8TR_GITHUB_API_BASE_URL",
            format!("http://127.0.0.1:{port}/redirect302"),
        )
        .args([
            "--config",
            &config_path.display().to_string(),
            "tools",
            "download",
            "client-mcp",
        ])
        .output()
        .expect("run command");

    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(dir.path().join("build/tools/client_mcp.cfe").exists());
}

#[test]
fn tools_download_artifacts_keeps_yaxunit_out_of_source_sets() {
    let dir = temp_workspace();
    let config_path = write_minimal_config(dir.path());
    let server_root = dir.path().join("server");
    let port = free_tcp_port();
    write_http_fixture(&server_root, port);
    let _server = FixtureServer::start(&server_root, port);
    assert!(wait_until(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(50),
        || std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    ));

    let output = v8_runner_command()
        .env(
            "V8TR_GITHUB_API_BASE_URL",
            format!("http://127.0.0.1:{port}"),
        )
        .args([
            "--config",
            &config_path.display().to_string(),
            "tools",
            "download",
            "yaxunit",
        ])
        .output()
        .expect("run command");

    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let config = fs::read_to_string(&config_path).expect("config");
    assert!(!config.contains("name: tests"));
    assert!(dir.path().join("build/tools/YAxUnit-25.12.cfe").exists());
    assert!(!dir.path().join("v8project.local.yaml").exists());
}

#[test]
fn tools_download_artifacts_handles_large_assets_without_pipe_deadlock() {
    let dir = temp_workspace();
    let config_path = write_config_with_execution_timeout(dir.path(), 3_000);
    let server_root = dir.path().join("server");
    let port = free_tcp_port();
    write_http_fixture(&server_root, port);
    fs::write(
        server_root.join("assets").join("YAxUnit-25.12.cfe"),
        vec![b'x'; 8 * 1024 * 1024],
    )
    .expect("large yax asset");
    let _server = FixtureServer::start(&server_root, port);
    assert!(wait_until(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(50),
        || std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    ));

    let output = v8_runner_command()
        .env(
            "V8TR_GITHUB_API_BASE_URL",
            format!("http://127.0.0.1:{port}"),
        )
        .args([
            "--config",
            &config_path.display().to_string(),
            "tools",
            "download",
            "yaxunit",
        ])
        .output()
        .expect("run command");

    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::metadata(dir.path().join("build/tools/YAxUnit-25.12.cfe"))
            .expect("large asset")
            .len(),
        8 * 1024 * 1024
    );
}

#[test]
fn tools_download_sources_refuses_to_replace_unmanaged_tests_dir() {
    let dir = temp_workspace();
    let config_path = write_minimal_config(dir.path());
    let user_file = dir.path().join("tests/custom.feature");
    fs::create_dir_all(user_file.parent().expect("tests parent")).expect("tests dir");
    fs::write(&user_file, "user content").expect("user test");

    let server_root = dir.path().join("server");
    let port = free_tcp_port();
    write_http_fixture(&server_root, port);
    let _server = FixtureServer::start(&server_root, port);
    assert!(wait_until(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(50),
        || std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    ));

    let output = v8_runner_command()
        .env(
            "V8TR_GITHUB_API_BASE_URL",
            format!("http://127.0.0.1:{port}"),
        )
        .args([
            "--config",
            &config_path.display().to_string(),
            "tools",
            "download",
            "yaxunit",
            "--sources",
            "--force",
        ])
        .output()
        .expect("run command");

    assert!(
        !output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(&user_file).expect("user file"),
        "user content"
    );
}

#[test]
fn tools_download_artifacts_requires_designer_builder() {
    let dir = temp_workspace();
    let config_path = write_minimal_config_with_builder(dir.path(), "IBCMD");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "tools",
            "download",
            "client-mcp",
        ])
        .output()
        .expect("run command");

    assert!(
        !output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(combined.contains("requires builder=DESIGNER"));
}

#[test]
fn tools_download_force_refuses_to_replace_unmanaged_tool_file() {
    let dir = temp_workspace();
    let config_path = write_minimal_config(dir.path());
    let user_file = dir.path().join("build/tools/vanessa-automation-single.epf");
    fs::create_dir_all(user_file.parent().expect("tools parent")).expect("tools dir");
    fs::write(&user_file, "user epf").expect("user epf");

    let server_root = dir.path().join("server");
    let port = free_tcp_port();
    write_http_fixture(&server_root, port);
    let _server = FixtureServer::start(&server_root, port);
    assert!(wait_until(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(50),
        || std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    ));

    let output = v8_runner_command()
        .env(
            "V8TR_GITHUB_API_BASE_URL",
            format!("http://127.0.0.1:{port}"),
        )
        .args([
            "--config",
            &config_path.display().to_string(),
            "tools",
            "download",
            "vanessa",
            "--force",
        ])
        .output()
        .expect("run command");

    assert!(
        !output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(&user_file).expect("user file"),
        "user epf"
    );
}

#[test]
fn tools_download_respects_execution_timeout_during_http_download() {
    let dir = temp_workspace();
    let config_path = write_config_with_execution_timeout(dir.path(), 200);
    let port = free_tcp_port();
    let mut server = sleeping_server(port);
    assert!(wait_until(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(50),
        || std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    ));

    let started = std::time::Instant::now();
    let output = v8_runner_command()
        .env(
            "V8TR_GITHUB_API_BASE_URL",
            format!("http://127.0.0.1:{port}"),
        )
        .args([
            "--config",
            &config_path.display().to_string(),
            "tools",
            "download",
            "yaxunit",
        ])
        .output()
        .expect("run command");
    let elapsed = started.elapsed();
    let _ = server.kill();
    let _ = server.wait();

    assert!(
        !output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "elapsed={elapsed:?}"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(combined.contains("timed out"));
}

#[test]
fn tools_download_sources_rejects_conflicting_tests_source_set() {
    let dir = temp_workspace();
    let config_path = write_minimal_config(dir.path());
    let mut config = fs::read_to_string(&config_path).expect("config");
    config = config.replace(
        "tools:\n",
        "  - name: tests\n    type: CONFIGURATION\n    path: custom-tests\ntools:\n",
    );
    fs::write(&config_path, config).expect("config");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "tools",
            "download",
            "yaxunit",
            "--sources",
        ])
        .output()
        .expect("run command");

    assert!(
        !output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(combined.contains("source-set 'tests' already exists"));
    assert!(!dir.path().join("tests").exists());
}
