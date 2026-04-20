#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::prelude::*;
use insta::assert_debug_snapshot;
use serde_json::Value;
use tempfile::tempdir;

const JUNIT_SMOKE_REPORT_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/parsers/junit_smoke_report.xml"
));
const YAXUNIT_LOG_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/parsers/yaxunit.log"
));

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
        "printf '%s\\n' \"$*\" >> '{}'\npayload=\"\"\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/C\" ]; then payload=\"$arg\"; fi\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\ncfg=$(printf '%s' \"$payload\" | sed 's/^RunUnitTests=//; s/^\"//; s/\"$//')\ncp \"$cfg\" '{}'\nreport=$(awk -F '\"' '/reportPath/ {{print $4; exit}}' \"$cfg\")\nylog=$(awk -F '\"' '/\"file\"/ {{print $4; exit}}' \"$cfg\")\nmkdir -p \"$(dirname \"$report\")\" \"$(dirname \"$ylog\")\" \"$(dirname \"$out\")\"\ncat <<'XML' > \"$report\"\n{}\nXML\ncat <<'LOG' > \"$ylog\"\n{}\nLOG\nprintf 'platform /P secret uri http://user:pass@example\\n' > \"$out\"\n{}\nexit {}",
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

fn write_va_test_script(
    path: &Path,
    calls_log: &Path,
    captured_params: &Path,
    report_xml: &str,
    exit_code: i32,
) {
    let body = format!(
        "printf '%s\\n' \"$*\" >> '{}'\npayload=\"\"\nout=\"\"\nexecute=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/C\" ]; then payload=\"$arg\"; fi\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  if [ \"$prev\" = \"/Execute\" ]; then execute=\"$arg\"; fi\n  prev=\"$arg\"\ndone\ncfg=$(printf '%s' \"$payload\" | sed 's/^StartFeaturePlayer;VAParams=//; s/^\"//; s/\"$//')\ncp \"$cfg\" '{}'\nreport_dir=$(python3 - <<'PY' \"$cfg\"\nimport json, sys\nwith open(sys.argv[1], 'r', encoding='utf-8') as fh:\n    data = json.load(fh)\nprint(data['junitpath'])\nPY\n)\nmkdir -p \"$report_dir\" \"$(dirname \"$out\")\"\ncat <<'XML' > \"$report_dir/result.xml\"\n{}\nXML\nprintf 'va execute=%s\\n' \"$execute\" > \"$out\"\nexit {}",
        calls_log.display(),
        captured_params.display(),
        report_xml,
        exit_code
    );
    write_script(path, &body);
}

fn write_config(
    path: &Path,
    base_path: &Path,
    work_path: &Path,
    install_dir: &Path,
    timeout_seconds: u64,
    additional_launch_keys: &[&str],
) {
    let additional_launch_keys_block = if additional_launch_keys.is_empty() {
        String::new()
    } else {
        format!(
            "  enterprise:\n    additional-launch-keys:\n{}",
            additional_launch_keys
                .iter()
                .map(|key| format!("      - '{}'\n", key))
                .collect::<String>()
        )
    };
    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: 'File=/tmp/ib'\ncredentials:\n  password: secret\ntests:\n  execution_timeout_seconds: {}\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: main\ntools:\n  platform:\n    path: '{}'\n{}",
        base_path.display(),
        work_path.display(),
        timeout_seconds,
        install_dir.display(),
        additional_launch_keys_block,
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
    setup_project_with_additional_launch_keys(
        work_dir_name,
        report_xml,
        yax_log,
        enterprise_exit,
        build_fail,
        timeout_seconds,
        sleep_seconds,
        &[],
    )
}

fn setup_project_with_additional_launch_keys(
    work_dir_name: &str,
    report_xml: &str,
    yax_log: &str,
    enterprise_exit: i32,
    build_fail: bool,
    timeout_seconds: u64,
    sleep_seconds: Option<u64>,
    additional_launch_keys: &[&str],
) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join(work_dir_name);
    let install_dir = dir.path().join("platform");
    let config_path = dir.path().join("v8project.yaml");
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
        additional_launch_keys,
    );

    (dir, config_path, build_calls, test_calls, captured_config)
}

fn setup_va_project(
    report_xml: &str,
    additional_launch_keys: &[&str],
) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, PathBuf) {
    let dir = tempdir().expect("tempdir");
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let install_dir = dir.path().join("platform");
    let config_path = dir.path().join("v8project.yaml");
    let build_calls = dir.path().join("build.calls.log");
    let test_calls = dir.path().join("test.calls.log");
    let captured_params = dir.path().join("captured-va-params.json");
    let va_epf = dir.path().join("va").join("vanessa-automation.epf");
    let va_params = dir.path().join("cfg").join("va-base.json");
    let features_dir = dir.path().join("features").join("smoke");

    fs::create_dir_all(base_path.join("main")).expect("main");
    fs::create_dir_all(&work_path).expect("work");
    fs::create_dir_all(va_epf.parent().expect("va dir")).expect("va dir");
    fs::create_dir_all(va_params.parent().expect("cfg dir")).expect("cfg dir");
    fs::create_dir_all(&features_dir).expect("features");
    fs::write(
        base_path.join("main").join("Module.bsl"),
        "procedure Test() endprocedure",
    )
    .expect("module");
    fs::write(&va_epf, "epf").expect("epf");
    fs::write(&va_params, "{\n  \"existing\": true\n}\n").expect("params");
    fs::write(features_dir.join("login.feature"), "Feature: Login\n").expect("feature");

    write_build_script(&install_dir.join("bin").join("1cv8"), &build_calls, false);
    write_va_test_script(
        &install_dir.join("bin").join("1cv8c"),
        &test_calls,
        &captured_params,
        report_xml,
        0,
    );

    let additional_launch_keys_block = if additional_launch_keys.is_empty() {
        String::new()
    } else {
        format!(
            "  enterprise:\n    additional-launch-keys:\n{}",
            additional_launch_keys
                .iter()
                .map(|key| format!("      - '{}'\n", key))
                .collect::<String>()
        )
    };
    let config = format!(
        "basePath: '{}'\nworkPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: 'File=/tmp/ib'\ncredentials:\n  password: secret\ntests:\n  execution_timeout_seconds: 5\n  va:\n    epf_path: '{}'\n    params_path: '{}'\n    profile: smoke\n    fail_fast: true\n    profiles:\n      smoke:\n        feature_path: '{}'\n        features_to_run:\n          - login\n        filter_tags:\n          - '@smoke'\n        ignore_tags:\n          - '@draft'\n        scenario_filter:\n          - Проверка логина\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: main\ntools:\n  platform:\n    path: '{}'\n{}",
        base_path.display(),
        work_path.display(),
        va_epf.display(),
        va_params.display(),
        features_dir.display(),
        install_dir.display(),
        additional_launch_keys_block,
    );
    fs::write(&config_path, config).expect("config");

    (dir, config_path, build_calls, test_calls, captured_params)
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
            if step["name"] == "prepare_artifacts" {
                step["message"] = Value::String("created <run_dir>".to_owned());
            }
        }
    }
}

#[test]
fn test_all_full_json_runs_build_first_and_returns_report() {
    let (_dir, config_path, build_calls, test_calls, _captured_config) = setup_project(
        "work path",
        JUNIT_SMOKE_REPORT_FIXTURE,
        "12:00:00.000 [INF] ok",
        0,
        false,
        5,
        None,
    );

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "test",
            "--full",
            "yaxunit",
            "all",
        ])
        .output()
        .expect("run");

    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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
fn test_run_appends_enterprise_additional_launch_keys() {
    let (_dir, config_path, _build_calls, test_calls, _captured_config) =
        setup_project_with_additional_launch_keys(
            "work",
            JUNIT_SMOKE_REPORT_FIXTURE,
            "12:00:00.000 [INF] ok",
            0,
            false,
            5,
            None,
            &["/TESTMANAGER", "/TCUser", "ci-user"],
        );

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "test",
            "yaxunit",
            "all",
        ])
        .output()
        .expect("run");

    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = fs::read_to_string(test_calls).expect("test calls");
    assert!(calls.contains("RunUnitTests="));
    assert!(calls.contains("/TESTMANAGER"));
    assert!(calls.contains("/TCUser"));
    assert!(calls.contains("ci-user"));
}

#[test]
fn test_text_output_splits_pipeline_into_timeline_stages() {
    let (_dir, config_path, _build_calls, _test_calls, _captured_config) = setup_project(
        "work",
        JUNIT_SMOKE_REPORT_FIXTURE,
        "12:00:00.000 [INF] ok",
        0,
        false,
        5,
        None,
    );

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "test",
            "yaxunit",
            "all",
        ])
        .output()
        .expect("run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("● Tests: target"));
    assert!(stdout.contains("│   all"));
    assert!(stdout.contains("● Tests: build prerequisite"));
    assert!(stdout.contains("│   build completed"));
    assert!(stdout.contains("● Tests: prepare artifacts"));
    assert!(stdout.contains("│   created "));
    assert!(stdout.contains("● Tests: prepare runner"));
    assert!(stdout.contains("│   YaXUnit config written"));
    assert!(stdout.contains("● Tests: enterprise run"));
    assert!(stdout.contains("│   enterprise exit code 0"));
    assert!(stdout.contains("● Tests: parse JUnit report"));
    assert!(stdout.contains("│   parsed 1 test cases"));
    assert!(stdout.contains("● Tests: parse runner log"));
    assert!(stdout.contains("● Tests completed successfully"));
    assert!(stdout.contains("│   total=1, passed=1, failed=0, skipped=0, errors=0"));
    assert!(!stdout.contains("Test target: all"));
    assert!(!stdout.contains("Summary: total="));
    assert!(!stdout.contains("starting test run"));
    assert!(!stdout.contains("preparing test run artifacts"));
    assert!(!stdout.contains("launching enterprise test run"));
    assert!(!stdout.contains("parsing JUnit report"));
}

#[test]
fn test_accepts_explicit_client_mode_for_vanessa_and_yaxunit() {
    let (dir, config_path, _build_calls, test_calls, _captured_config) = setup_project(
        "work",
        JUNIT_SMOKE_REPORT_FIXTURE,
        "12:00:00.000 [INF] ok",
        0,
        false,
        5,
        None,
    );
    write_script(
        &dir.path().join("platform").join("bin").join("1cv8"),
        &format!(
            "printf '%s\\n' \"$*\" >> '{}'\npayload=\"\"\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/C\" ]; then payload=\"$arg\"; fi\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif printf '%s' \"$payload\" | grep -F -q -- 'RunUnitTests='; then\n  cfg=$(printf '%s' \"$payload\" | sed 's/^RunUnitTests=//; s/^\"//; s/\"$//')\n  report=$(awk -F '\"' '/reportPath/ {{print $4; exit}}' \"$cfg\")\n  ylog=$(awk -F '\"' '/\"file\"/ {{print $4; exit}}' \"$cfg\")\n  mkdir -p \"$(dirname \"$report\")\" \"$(dirname \"$ylog\")\"\n  cat <<'XML' > \"$report\"\n{}\nXML\n  cat <<'LOG' > \"$ylog\"\n12:00:00.000 [INF] ok\nLOG\n  if [ -n \"$out\" ]; then mkdir -p \"$(dirname \"$out\")\" && : > \"$out\"; fi\n  exit 0\nfi\nif [ -n \"$out\" ]; then printf 'build /P secret\\n' > \"$out\"; fi\nexit 0",
            test_calls.display(),
            JUNIT_SMOKE_REPORT_FIXTURE
        ),
    );

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "test",
            "--client-mode",
            "ordinary",
            "yaxunit",
            "all",
        ])
        .output()
        .expect("run");

    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = fs::read_to_string(test_calls).expect("test calls");
    assert!(calls.contains("/RunModeOrdinaryApplication"));
}

#[test]
fn test_rejects_c_and_execute_on_test_surface() {
    let (_dir, config_path, _build_calls, test_calls, _captured_config) =
        setup_project("work", JUNIT_SMOKE_REPORT_FIXTURE, "", 0, false, 5, None);

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "test",
            "--c",
            "ignored",
            "yaxunit",
            "all",
        ])
        .output()
        .expect("run");

    assert!(!output.status.success());
    assert_ne!(output.status.code(), Some(0));
    assert!(!test_calls.exists());
}

#[test]
fn test_va_builds_vanessa_command_and_overlay() {
    let (_dir, config_path, build_calls, test_calls, captured_params) = setup_va_project(
        JUNIT_SMOKE_REPORT_FIXTURE,
        &["/TESTMANAGER", "/VAUSER", "ci-user"],
    );

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "test",
            "va",
        ])
        .output()
        .expect("run");

    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fs::read_to_string(build_calls)
        .expect("build calls")
        .contains("/UpdateDBCfg"));

    let calls = fs::read_to_string(test_calls).expect("test calls");
    assert!(calls.contains("/Execute"));
    assert!(calls.contains("vanessa-automation.epf"));
    assert!(calls.contains("StartFeaturePlayer;VAParams="));
    assert!(calls.contains("/TESTMANAGER"));
    assert!(calls.contains("/VAUSER"));
    assert!(calls.contains("ci-user"));

    let params: Value =
        serde_json::from_slice(&fs::read(captured_params).expect("params")).expect("params json");
    assert_eq!(params["existing"], true);
    assert_eq!(params["stoponerror"], true);
    assert_eq!(params["junitcreatereport"], true);
    assert!(params["junitpath"]
        .as_str()
        .expect("junitpath")
        .contains("/junit"));
    assert!(params["featurepath"]
        .as_str()
        .expect("featurepath")
        .contains("/features/smoke"));
    assert_eq!(params["FeaturesToRun"][0], "login");
    assert_eq!(params["filtertags"][0], "@smoke");
    assert_eq!(params["ignoretags"][0], "@draft");
    assert_eq!(params["scenariofilter"][0], "Проверка логина");

    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["data"]["report"]["summary"]["total"], 1);
}

#[test]
fn test_module_build_failure_prevents_enterprise_launch() {
    let (_dir, config_path, _build_calls, test_calls, _captured_config) =
        setup_project("work", JUNIT_SMOKE_REPORT_FIXTURE, "", 0, true, 5, None);

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "test",
            "yaxunit",
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
    let (_dir, config_path, _build_calls, _test_calls, captured_config) =
        setup_project("work", report, YAXUNIT_LOG_FIXTURE, 0, false, 5, None);

    let compact = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "test",
            "yaxunit",
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

    let full = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "test",
            "--full",
            "yaxunit",
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
    let (_dir, config_path, _build_calls, _test_calls, _captured_config) =
        setup_project("work", JUNIT_SMOKE_REPORT_FIXTURE, "", 0, false, 1, Some(2));

    let output = std::process::Command::cargo_bin("v8-runner")
        .expect("binary")
        .args([
            "--config",
            &config_path.display().to_string(),
            "--output",
            "json",
            "test",
            "yaxunit",
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
