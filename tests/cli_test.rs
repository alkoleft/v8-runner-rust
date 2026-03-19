#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::prelude::*;
use insta::assert_debug_snapshot;
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

fn write_test_script(
    path: &Path,
    calls_log: &Path,
    captured_config: &Path,
    report_xml: &str,
    yax_log: &str,
    exit_code: i32,
    sleep_seconds: Option<u64>,
) {
    let sleep_branch = sleep_seconds
        .map(|value| format!("sleep {value}"))
        .unwrap_or_default();
    let body = format!(
        "printf '%s\\n' \"$*\" >> '{}'\npayload=\"\"\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/C\" ]; then payload=\"$arg\"; fi\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\ncfg=$(printf '%s' \"$payload\" | sed 's/^RunUnitTests=\"//; s/\"$//')\ncp \"$cfg\" '{}'\nreport=$(awk -F '\"' '/reportPath/ {{print $4; exit}}' \"$cfg\")\nylog=$(awk -F '\"' '/\"file\"/ {{print $4; exit}}' \"$cfg\")\nmkdir -p \"$(dirname \"$report\")\" \"$(dirname \"$ylog\")\" \"$(dirname \"$out\")\"\ncat <<'XML' > \"$report\"\n{}\nXML\ncat <<'LOG' > \"$ylog\"\n{}\nLOG\nprintf 'platform /P secret uri http://user:pass@example\\n' > \"$out\"\n{}\nexit {}",
        calls_log.display(),
        captured_config.display(),
        report_xml,
        yax_log,
        sleep_branch,
        exit_code
    );
    write_script(path, &body);
}

fn write_build_script(path: &Path, calls_log: &Path, fail: bool) {
    let fail_branch = if fail {
        "if printf '%s' \"$args\" | grep -F -q -- '/UpdateDBCfg'; then exit 17; fi"
    } else {
        ""
    };
    let body = format!(
        "args=\"$*\"\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nprintf '%s\\n' \"$args\" >> '{}'\nif [ -n \"$out\" ]; then printf 'build /P secret\\n' > \"$out\"; fi\n{}\nexit 0",
        calls_log.display(),
        fail_branch
    );
    write_script(path, &body);
}

fn write_config(
    path: &Path,
    base_path: &Path,
    work_path: &Path,
    install_dir: &Path,
    timeout_seconds: u64,
) {
    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: 'File=/tmp/ib;Pwd=secret'\ntests:\n  execution_timeout_seconds: {}\nsource-set:\n  - name: main\n    purpose: CONFIGURATION\n    path: main\ntools:\n  platform:\n    path: '{}'\n",
        base_path.display(),
        work_path.display(),
        timeout_seconds,
        install_dir.display(),
    );
    fs::write(path, config).expect("config");
}

fn setup_project(
    work_dir_name: &str,
    report_xml: &str,
    yax_log: &str,
    enterprise_exit: i32,
    build_fail: bool,
    timeout_seconds: u64,
    sleep_seconds: Option<u64>,
) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join(work_dir_name);
    let install_dir = dir.path().join("platform");
    let config_path = dir.path().join("application.yaml");
    let build_calls = dir.path().join("build.calls.log");
    let test_calls = dir.path().join("test.calls.log");
    let captured_config = dir.path().join("captured-config.json");

    fs::create_dir_all(base_path.join("main")).expect("main");
    fs::create_dir_all(&work_path).expect("work");
    fs::write(
        base_path.join("main").join("Module.bsl"),
        "procedure Test() endprocedure",
    )
    .expect("module");

    write_build_script(
        &install_dir.join("bin").join("1cv8"),
        &build_calls,
        build_fail,
    );
    write_test_script(
        &install_dir.join("bin").join("1cv8c"),
        &test_calls,
        &captured_config,
        report_xml,
        yax_log,
        enterprise_exit,
        sleep_seconds,
    );
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &install_dir,
        timeout_seconds,
    );

    (dir, config_path, build_calls, test_calls, captured_config)
}

fn scrub_snapshot(value: &mut Value) {
    value["duration_ms"] = Value::String("<duration>".to_owned());
    value["data"]["retained_paths"]["run_dir"] = Value::String("<run_dir>".to_owned());
    value["data"]["retained_paths"]["config_json"] = Value::String("<config_json>".to_owned());
    value["data"]["retained_paths"]["junit_xml"] = Value::String("<junit_xml>".to_owned());
    value["data"]["retained_paths"]["yaxunit_log"] = Value::String("<yaxunit_log>".to_owned());
    value["data"]["retained_paths"]["platform_log"] = Value::String("<platform_log>".to_owned());
    value["data"]["retained_paths"]["sentinel"] = Value::String("<sentinel>".to_owned());
    if let Some(steps) = value["steps"].as_array_mut() {
        for step in steps {
            step["duration_ms"] = Value::String("<duration>".to_owned());
        }
    }
}

#[test]
fn test_all_full_json_runs_build_first_and_returns_report() {
    let report = r#"<testsuites><testsuite name="suite"><testcase name="ok" classname="Sample" time="0.1"/></testsuite></testsuites>"#;
    let (_dir, config_path, build_calls, test_calls, _captured_config) = setup_project(
        "work path",
        report,
        "12:00:00.000 [INF] ok",
        0,
        false,
        5,
        None,
    );

    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "test",
            "--full",
            "all",
        ])
        .output()
        .expect("run");

    assert!(output.status.success());
    assert!(fs::read_to_string(build_calls)
        .expect("build calls")
        .contains("/UpdateDBCfg"));
    assert!(fs::read_to_string(test_calls)
        .expect("test calls")
        .contains("RunUnitTests="));

    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["data"]["report"]["summary"]["total"], 1);
    assert_eq!(
        payload["data"]["report"]["suites"][0]["cases"][0]["name"],
        "ok"
    );
    assert_eq!(payload["data"]["retained_paths"], Value::Null);
}

#[test]
fn test_module_build_failure_prevents_enterprise_launch() {
    let report =
        r#"<testsuites><testsuite name="suite"><testcase name="ok"/></testsuite></testsuites>"#;
    let (_dir, config_path, _build_calls, test_calls, _captured_config) =
        setup_project("work", report, "", 0, true, 5, None);

    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "test",
            "module",
            "Foo",
        ])
        .output()
        .expect("run");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(4));
    assert!(!test_calls.exists());

    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["data"]["error_kind"], "build_failed");
}

#[test]
fn test_module_compact_and_full_json_are_stable() {
    let report = r#"
<testsuites>
  <testsuite name="suite">
    <testcase name="ok" classname="Sample" time="0.1"/>
    <testcase name="bad" classname="Sample" time="0.2">
      <failure message="boom">stack trace line 1
stack trace line 2</failure>
    </testcase>
  </testsuite>
</testsuites>
"#;
    let (_dir, config_path, _build_calls, _test_calls, captured_config) = setup_project(
        "work",
        report,
        "12:00:01.000 [ERR] failed block\nmore details",
        0,
        false,
        5,
        None,
    );

    let compact = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "test",
            "module",
            "Foo",
        ])
        .output()
        .expect("compact");
    assert!(!compact.status.success());
    assert_eq!(compact.status.code(), Some(3));
    let compact_json: Value = serde_json::from_slice(&compact.stdout).expect("compact json");
    assert_eq!(compact_json["data"]["target"]["module"]["name"], "Foo");
    assert!(compact_json["data"]["retained_paths"]["run_dir"].is_string());
    assert_eq!(
        compact_json["data"]["report"]["suites"][0]["cases"]
            .as_array()
            .expect("cases")
            .len(),
        1
    );

    let retained_config_path = compact_json["data"]["retained_paths"]["config_json"]
        .as_str()
        .expect("config path");
    let retained_sentinel = compact_json["data"]["retained_paths"]["sentinel"]
        .as_str()
        .expect("sentinel");
    let retained_config = fs::read_to_string(retained_config_path).expect("retained config");
    assert!(retained_config.contains("\"modules\": ["));
    assert!(Path::new(retained_sentinel).exists());
    assert!(fs::read_to_string(captured_config)
        .expect("captured config")
        .contains("Foo"));

    let full = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "test",
            "--full",
            "module",
            "Foo",
        ])
        .output()
        .expect("full");
    assert!(!full.status.success());
    let full_json: Value = serde_json::from_slice(&full.stdout).expect("full json");
    assert_eq!(
        full_json["data"]["report"]["suites"][0]["cases"]
            .as_array()
            .expect("cases")
            .len(),
        2
    );

    let mut compact_snapshot = compact_json.clone();
    let mut full_snapshot = full_json.clone();
    scrub_snapshot(&mut compact_snapshot);
    scrub_snapshot(&mut full_snapshot);
    assert_debug_snapshot!("test_module_compact_json", compact_snapshot);
    assert_debug_snapshot!("test_module_full_json", full_snapshot);
}

#[test]
fn test_timeout_retains_artifacts() {
    let report =
        r#"<testsuites><testsuite name="suite"><testcase name="ok"/></testsuite></testsuites>"#;
    let (_dir, config_path, _build_calls, _test_calls, _captured_config) =
        setup_project("work", report, "", 0, false, 1, Some(2));

    let output = std::process::Command::cargo_bin("v8-test-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "test",
            "all",
        ])
        .output()
        .expect("run");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(3));

    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["data"]["error_kind"], "enterprise_timed_out");
    let platform_log = payload["data"]["retained_paths"]["platform_log"]
        .as_str()
        .expect("platform log");
    assert!(!platform_log.is_empty());
}
