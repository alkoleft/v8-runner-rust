#![cfg(unix)]

mod support;

use std::fs;
use std::io::{BufRead, BufReader, ErrorKind};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use insta::assert_debug_snapshot;
use serde_json::Value;
use support::{
    temp_workspace, v8_runner_command, wait_for_file, wait_for_received_line,
    write_shell_script as write_script,
};

const V8_CONFIGURATION_NATURE: &str = "com._1c.g5.v8.dt.core.V8ConfigurationNature";
const V8_EXTENSION_NATURE: &str = "com._1c.g5.v8.dt.core.V8ExtensionNature";
const EDT_RUNTIME_VERSION: &str = "8.3.27";

const JUNIT_SMOKE_REPORT_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/parsers/junit_smoke_report.xml"
));
const YAXUNIT_LOG_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/parsers/yaxunit.log"
));

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
        "printf '%s\\n' \"$*\" >> '{}'\npayload=\"\"\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/C\" ]; then payload=\"$arg\"; fi\n  case \"$arg\" in /C*) payload=\"${{arg#/C}}\" ;; esac\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\ncfg=$(printf '%s' \"$payload\" | sed 's/^\"//; s/\"$//; s/^RunUnitTests=//')\ncp \"$cfg\" '{}'\nreport=$(awk -F '\"' '/reportPath/ {{print $4; exit}}' \"$cfg\")\nylog=$(awk -F '\"' '/\"file\"/ {{print $4; exit}}' \"$cfg\")\nmkdir -p \"$(dirname \"$report\")\" \"$(dirname \"$ylog\")\" \"$(dirname \"$out\")\"\ncat <<'XML' > \"$report\"\n{}\nXML\ncat <<'LOG' > \"$ylog\"\n{}\nLOG\nprintf 'platform /P secret uri http://user:pass@example\\n' > \"$out\"\n{}\nexit {}",
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

fn write_edt_script(path: &Path, calls_log: &Path) {
    let body = format!(
        "args=\"$*\"\ntarget=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"--configuration-files\" ]; then target=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$target\" ]; then mkdir -p \"$target\"; printf '<Configuration />\\n' > \"$target/Configuration.xml\"; fi\nprintf '%s\\n' \"$args\" >> \"{}\"\nexit 0",
        calls_log.display()
    );
    write_script(path, &body);
}

fn remove_file_if_exists(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => panic!("failed to remove '{}': {error}", path.display()),
    }
}

fn write_native_edt_project(
    path: &Path,
    project_name: &str,
    nature: &str,
    base_project: Option<&str>,
) {
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

fn write_va_test_script(
    path: &Path,
    calls_log: &Path,
    captured_params: &Path,
    report_xml: &str,
    exit_code: i32,
) {
    let body = format!(
        "printf '%s\\n' \"$*\" >> '{}'\npayload=\"\"\nout=\"\"\nexecute=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/C\" ]; then payload=\"$arg\"; fi\n  case \"$arg\" in /C*) payload=\"${{arg#/C}}\" ;; esac\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  if [ \"$prev\" = \"/Execute\" ]; then execute=\"$arg\"; fi\n  prev=\"$arg\"\ndone\ncfg=$(printf '%s' \"$payload\" | sed 's/^\"//; s/\"$//; s/^StartFeaturePlayer;VAParams=//')\ncp \"$cfg\" '{}'\nreport_dir=$(python3 - <<'PY' \"$cfg\"\nimport json, sys\nwith open(sys.argv[1], 'r', encoding='utf-8') as fh:\n    data = json.load(fh)\nprint(data['КаталогВыгрузкиJUnit'])\nPY\n)\ntext_log=$(python3 - <<'PY' \"$cfg\"\nimport json, sys\nwith open(sys.argv[1], 'r', encoding='utf-8') as fh:\n    data = json.load(fh)\nprint(data['ИмяФайлаЛогВыполненияСценариев'])\nPY\n)\nmkdir -p \"$report_dir\" \"$(dirname \"$out\")\" \"$(dirname \"$text_log\")\"\ncat <<'XML' > \"$report_dir/result.xml\"\n{}\nXML\nprintf 'va execute=%s\\n' \"$execute\" > \"$out\"\nprintf 'INFO ok\\nОшибка VA из текстового лога\\n' > \"$text_log\"\nexit {}",
        calls_log.display(),
        captured_params.display(),
        report_xml,
        exit_code
    );
    write_script(path, &body);
}

fn write_config(
    path: &Path,
    _base_path: &Path,
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
        "workPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\n  password: secret\ntests:\n  execution_timeout_seconds: {}\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: main\ntools:\n  platform:\n    path: '{}'\n{}",
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
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join(work_dir_name);
    let install_dir = dir.path().join("platform");
    let config_path = base_path.join("v8project.yaml");
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
    setup_va_project_with_work_name(report_xml, additional_launch_keys, "work")
}

fn setup_va_project_with_work_name(
    report_xml: &str,
    additional_launch_keys: &[&str],
    work_dir_name: &str,
) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, PathBuf) {
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join(work_dir_name);
    let install_dir = dir.path().join("platform");
    let config_path = base_path.join("v8project.yaml");
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
        "workPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\n  password: secret\ntests:\n  execution_timeout_seconds: 5\n  va:\n    params_path: '{}'\n    profile: smoke\n    profiles:\n      smoke:\n        feature_path: '{}'\n        features_to_run:\n          - login\n        filter_tags:\n          - '@smoke'\n        ignore_tags:\n          - '@draft'\n        scenario_filter:\n          - Проверка логина\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: main\ntools:\n  va:\n    epf_path: '{}'\n  platform:\n    path: '{}'\n{}",
        work_path.display(),
        va_params.display(),
        features_dir.display(),
        va_epf.display(),
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
    if value["data"]["execution"]["artifacts"]["root_dir"].is_string() {
        value["data"]["execution"]["artifacts"]["root_dir"] = Value::String("<run_dir>".to_owned());
    }
    if let Some(items) = value["data"]["execution"]["artifacts"]["items"].as_array_mut() {
        for item in items {
            let replacement = match item["role"].as_str() {
                Some("run_dir") => "<run_dir>",
                Some("config") => "<config_json>",
                Some("report") => "<junit_xml>",
                Some("runner_log") => "<yaxunit_log>",
                Some("platform_log") => "<platform_log>",
                Some("sentinel") => "<sentinel>",
                _ => continue,
            };
            item["path"] = Value::String(replacement.to_owned());
        }
    }
    if let Some(steps) = value["steps"].as_array_mut() {
        for step in steps {
            step["duration_ms"] = Value::String("<duration>".to_owned());
            if step["target"].is_string() {
                let replacement = match step["name"].as_str() {
                    Some("prepare_artifacts") => "<run_dir>",
                    Some("prepare_runner") => "<config_json>",
                    Some("run") => "<platform_log>",
                    Some("parse_junit") => "<junit_xml>",
                    Some("parse_log") => "<yaxunit_log>",
                    _ => "<target>",
                };
                step["target"] = Value::String(replacement.to_owned());
            }
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

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("started_at: "));
    assert!(!stdout.contains("test: enterprise run"));
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

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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

    let output = v8_runner_command()
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

    assert!(stdout.contains("● Tests completed successfully"));
    assert!(stdout.contains("● test: build prerequisite"));
    assert!(stdout.contains("● test: enterprise run"));
    assert!(!stdout.contains("started_at: "));
    assert!(stdout.contains("│   target: all"));
    assert!(stdout.contains("│   summary: total=1, passed=1, failed=0, skipped=0, errors=0"));
    assert!(!stdout.contains("prepare artifacts"));
    assert!(!stdout.contains("parse JUnit report"));
    assert!(!stdout.contains("parse runner log"));
    assert!(!stdout.contains("secret"));
    assert!(!stdout.contains(" INFO "));
    assert!(!stdout.contains("Test target: all"));
    assert!(!stdout.contains("Summary: total="));
    assert!(!stdout.contains("starting test run"));
    assert!(!stdout.contains("preparing test run artifacts"));
    assert!(!stdout.contains("launching enterprise test run"));
    assert!(!stdout.contains("parsing JUnit report"));
}

#[test]
fn test_command_streams_enterprise_stage_before_runner_finishes() {
    let (dir, config_path, _build_calls, test_calls, captured_config) = setup_project(
        "work",
        JUNIT_SMOKE_REPORT_FIXTURE,
        "12:00:00.000 [INF] ok",
        0,
        false,
        5,
        None,
    );
    let runner_started = dir.path().join("runner-started");
    let release_runner = dir.path().join("release-runner");
    write_script(
        &dir.path().join("platform").join("bin").join("1cv8c"),
        &format!(
            "printf '%s\\n' \"$*\" >> '{}'\npayload=\"\"\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/C\" ]; then payload=\"$arg\"; fi\n  case \"$arg\" in /C*) payload=\"${{arg#/C}}\" ;; esac\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\ncfg=$(printf '%s' \"$payload\" | sed 's/^\"//; s/\"$//; s/^RunUnitTests=//')\ncp \"$cfg\" '{}'\ntouch '{}'\nwhile [ ! -f '{}' ]; do sleep 0.05; done\nreport=$(awk -F '\"' '/reportPath/ {{print $4; exit}}' \"$cfg\")\nylog=$(awk -F '\"' '/\"file\"/ {{print $4; exit}}' \"$cfg\")\nmkdir -p \"$(dirname \"$report\")\" \"$(dirname \"$ylog\")\" \"$(dirname \"$out\")\"\ncat <<'XML' > \"$report\"\n{}\nXML\ncat <<'LOG' > \"$ylog\"\n12:00:00.000 [INF] ok\nLOG\nprintf 'platform ok\\n' > \"$out\"\nexit 0",
            test_calls.display(),
            captured_config.display(),
            runner_started.display(),
            release_runner.display(),
            JUNIT_SMOKE_REPORT_FIXTURE
        ),
    );

    let mut command = v8_runner_command();
    command
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "--log-level",
            "warn",
            "test",
            "yaxunit",
            "all",
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

    let saw_enterprise_stage = wait_for_received_line(
        &rx,
        Duration::from_secs(5),
        Duration::from_millis(100),
        |line| line.contains("● test: enterprise run"),
    );

    let runner_started_before_release = wait_for_file(&runner_started, Duration::from_secs(5));
    let early_status = child.try_wait().ok().flatten();
    fs::write(&release_runner, b"release").expect("release runner");

    let status = child.wait().expect("wait");
    assert!(saw_enterprise_stage, "enterprise stage was not streamed");
    assert!(
        runner_started_before_release,
        "enterprise runner did not reach handshake"
    );
    assert!(
        early_status.is_none(),
        "process finished before release handshake"
    );
    assert!(status.success());
}

#[test]
fn test_text_output_surfaces_failure_code_and_retained_artifacts() {
    let (_dir, config_path, _build_calls, _test_calls, _captured_config) =
        setup_project("work", JUNIT_SMOKE_REPORT_FIXTURE, "", 0, false, 1, Some(2));

    let output = v8_runner_command()
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

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(3));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("● Tests failed"));
    assert!(stdout.contains("✗ enterprise run: runtime error: enterprise test run timed out"));
    assert!(stdout.contains("[warning] enterprise test run timed out"));
    assert!(stdout.contains("[artifact] run_dir -> "));
    assert!(stdout.contains("[diagnostic] platform_log -> "));
}

#[test]
fn test_text_output_surfaces_success_log_findings_without_full_step_noise() {
    let (_dir, config_path, _build_calls, _test_calls, _captured_config) = setup_project(
        "work",
        JUNIT_SMOKE_REPORT_FIXTURE,
        YAXUNIT_LOG_FIXTURE,
        0,
        false,
        5,
        None,
    );

    let output = v8_runner_command()
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
    assert!(stdout.contains("● Tests completed with warnings"));
    assert!(stdout.contains("[error:test_report]"));
    assert!(!stdout.contains("prepare artifacts"));
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
            "printf '%s\\n' \"$*\" >> '{}'\npayload=\"\"\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/C\" ]; then payload=\"$arg\"; fi\n  case \"$arg\" in /C*) payload=\"${{arg#/C}}\" ;; esac\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif printf '%s' \"$payload\" | grep -F -q -- 'RunUnitTests='; then\n  cfg=$(printf '%s' \"$payload\" | sed 's/^\"//; s/\"$//; s/^RunUnitTests=//')\n  report=$(awk -F '\"' '/reportPath/ {{print $4; exit}}' \"$cfg\")\n  ylog=$(awk -F '\"' '/\"file\"/ {{print $4; exit}}' \"$cfg\")\n  mkdir -p \"$(dirname \"$report\")\" \"$(dirname \"$ylog\")\"\n  cat <<'XML' > \"$report\"\n{}\nXML\n  cat <<'LOG' > \"$ylog\"\n12:00:00.000 [INF] ok\nLOG\n  if [ -n \"$out\" ]; then mkdir -p \"$(dirname \"$out\")\" && : > \"$out\"; fi\n  exit 0\nfi\nif [ -n \"$out\" ]; then printf 'build /P secret\\n' > \"$out\"; fi\nexit 0",
            test_calls.display(),
            JUNIT_SMOKE_REPORT_FIXTURE
        ),
    );

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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

    let output = v8_runner_command()
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
fn test_rejects_reserved_raw_launch_payloads() {
    let (_dir, config_path, _build_calls, test_calls, _captured_config) =
        setup_project("work", JUNIT_SMOKE_REPORT_FIXTURE, "", 0, false, 5, None);

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "test",
            "--raw-key",
            "/C\"RunOther\"",
            "yaxunit",
            "all",
        ])
        .output()
        .expect("run");

    assert!(!output.status.success());
    assert_ne!(output.status.code(), Some(0));
    assert!(!test_calls.exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not support raw /C"));
}

#[test]
fn test_va_builds_vanessa_command_and_overlay() {
    let (_dir, config_path, build_calls, test_calls, captured_params) =
        setup_va_project(JUNIT_SMOKE_REPORT_FIXTURE, &["/VAUSER", "ci-user"]);

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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
    assert!(params["WorkspaceRoot"]
        .as_str()
        .expect("WorkspaceRoot")
        .contains("/project"));
    assert_eq!(params["ОстановкаПриВозникновенииОшибки"], false);
    assert_eq!(params["ВыполнитьСценарии"], true);
    assert_eq!(params["ЗавершитьРаботуСистемы"], true);
    assert_eq!(params["ДелатьОтчетВФорматеjUnit"], true);
    assert!(params["КаталогВыгрузкиJUnit"]
        .as_str()
        .expect("КаталогВыгрузкиJUnit")
        .contains("/junit"));
    assert!(params["ОтчетJUnit"]["КаталогВыгрузкиJUnit"]
        .as_str()
        .expect("ОтчетJUnit.КаталогВыгрузкиJUnit")
        .contains("/junit"));
    assert_eq!(params["ДелатьЛогВыполненияСценариевВТекстовыйФайл"], true);
    assert_eq!(params["ВыводитьВЛогВыполнениеШагов"], true);
    assert_eq!(params["ПодробныйЛогВыполненияСценариев"], 1);
    assert_eq!(params["ВыгружатьСтатусВыполненияСценариевВФайл"], true);
    assert!(params["ПутьКФайлуДляВыгрузкиСтатусаВыполненияСценариев"]
        .as_str()
        .expect("status path")
        .ends_with("/va-status.log"));
    assert!(params["ИмяФайлаЛогВыполненияСценариев"]
        .as_str()
        .expect("text log path")
        .ends_with("/runner.log"));
    assert!(params["КаталогФич"]
        .as_str()
        .expect("КаталогФич")
        .contains("/features/smoke"));
    assert_eq!(params["СписокФичДляВыполнения"][0], "login");
    assert_eq!(params["СписокТеговОтбор"][0], "smoke");
    assert_eq!(params["СписокТеговИсключение"][0], "draft");
    assert_eq!(params["СписокСценариевДляВыполнения"][0], "Проверка логина");

    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["data"]["report"]["summary"]["total"], 1);
    assert_eq!(
        payload["data"]["report"]["extracted_errors"][0],
        "Ошибка VA из текстового лога"
    );
}

#[test]
fn test_va_cli_filter_options_override_configured_profile_lists() {
    let (_dir, config_path, _build_calls, _test_calls, captured_params) =
        setup_va_project(JUNIT_SMOKE_REPORT_FIXTURE, &[]);

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "test",
            "va",
            "--feature",
            "checkout",
            "--filter-tag",
            "@critical",
            "--ignore-tag",
            "@flaky",
            "--scenario-filter",
            "Проверка оформления",
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

    let params: Value =
        serde_json::from_slice(&fs::read(captured_params).expect("params")).expect("params json");
    assert_eq!(params["СписокФичДляВыполнения"][0], "checkout");
    assert_eq!(params["СписокТеговОтбор"][0], "critical");
    assert_eq!(params["СписокТеговИсключение"][0], "flaky");
    assert_eq!(
        params["СписокСценариевДляВыполнения"][0],
        "Проверка оформления"
    );
}

#[test]
fn test_va_preserves_workspace_root_from_params_template() {
    let (dir, config_path, _build_calls, _test_calls, captured_params) =
        setup_va_project(JUNIT_SMOKE_REPORT_FIXTURE, &[]);
    fs::write(
        dir.path().join("cfg").join("va-base.json"),
        "{\n  \"existing\": true,\n  \"WorkspaceRoot\": \"/configured/workspace\"\n}\n",
    )
    .expect("params template");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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

    let params: Value =
        serde_json::from_slice(&fs::read(captured_params).expect("params")).expect("params json");
    assert_eq!(params["WorkspaceRoot"], "/configured/workspace");
}

#[test]
fn test_va_rejects_semicolon_in_generated_params_path() {
    let (_dir, config_path, _build_calls, test_calls, _captured_params) =
        setup_va_project_with_work_name(JUNIT_SMOKE_REPORT_FIXTURE, &[], "bad;work");

    let output = v8_runner_command()
        .args(["--config", &config_path.display().to_string(), "test", "va"])
        .output()
        .expect("run");

    assert!(!output.status.success());
    assert_ne!(output.status.code(), Some(0));
    assert!(!test_calls.exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("must not contain ';'"));
}

#[test]
fn test_va_does_not_duplicate_explicit_testmanager_raw_key() {
    let (_dir, config_path, _build_calls, test_calls, _captured_params) =
        setup_va_project(JUNIT_SMOKE_REPORT_FIXTURE, &["/TESTMANAGER"]);

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "test",
            "--raw-key",
            "/TESTMANAGER",
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

    let calls = fs::read_to_string(test_calls).expect("test calls");
    let test_manager_count = calls
        .split_whitespace()
        .filter(|arg| arg.eq_ignore_ascii_case("/TESTMANAGER"))
        .count();
    assert_eq!(test_manager_count, 1);
}

#[test]
fn test_va_adds_testmanager_when_raw_value_matches_name() {
    let (_dir, config_path, _build_calls, test_calls, _captured_params) =
        setup_va_project(JUNIT_SMOKE_REPORT_FIXTURE, &[]);

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "test",
            "--raw-key",
            "/VAUser",
            "--raw-key",
            "TESTMANAGER",
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

    let calls = fs::read_to_string(test_calls).expect("test calls");
    assert!(calls.contains("/VAUser"));
    assert!(calls.contains("TESTMANAGER"));
    assert!(calls
        .split_whitespace()
        .any(|arg| arg.eq_ignore_ascii_case("/TESTMANAGER")));
}

#[test]
fn test_module_build_failure_prevents_enterprise_launch() {
    let (_dir, config_path, _build_calls, test_calls, _captured_config) =
        setup_project("work", JUNIT_SMOKE_REPORT_FIXTURE, "", 0, true, 5, None);

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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
fn test_module_edt_extension_build_uses_full_load_before_enterprise_launch() {
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let install_dir = dir.path().join("platform");
    let edt_cli_path = dir.path().join("edt").join("1cedtcli");
    let config_path = base_path.join("v8project.yaml");
    let build_calls = dir.path().join("build.calls.log");
    let test_calls = dir.path().join("test.calls.log");
    let edt_calls = dir.path().join("edt.calls.log");
    let captured_config = dir.path().join("captured-config.json");

    fs::create_dir_all(base_path.join("configuration").join("Catalogs.Items"))
        .expect("configuration");
    fs::create_dir_all(base_path.join("exts").join("client-mcp")).expect("extension");
    fs::create_dir_all(&work_path).expect("work");
    write_native_edt_project(
        &base_path.join("configuration"),
        "configuration",
        V8_CONFIGURATION_NATURE,
        None,
    );
    fs::write(
        base_path
            .join("configuration")
            .join("Catalogs.Items")
            .join("ObjectModule.bsl"),
        "procedure Test() endprocedure",
    )
    .expect("configuration bsl");
    fs::write(
        base_path
            .join("configuration")
            .join("Catalogs.Items")
            .join("ObjectModule.xml"),
        "<MetaDataObject />",
    )
    .expect("configuration xml");
    write_native_edt_project(
        &base_path.join("exts").join("client-mcp"),
        "client_mcp",
        V8_EXTENSION_NATURE,
        Some("configuration"),
    );
    fs::write(
        base_path.join("exts").join("client-mcp").join("Module.bsl"),
        "procedure Test() endprocedure",
    )
    .expect("extension bsl");

    write_build_script(&install_dir.join("bin").join("1cv8"), &build_calls, false);
    write_test_script(
        &install_dir.join("bin").join("1cv8c"),
        &test_calls,
        &captured_config,
        JUNIT_SMOKE_REPORT_FIXTURE,
        YAXUNIT_LOG_FIXTURE,
        0,
        None,
    );
    write_edt_script(&edt_cli_path, &edt_calls);

    let config = format!(
        "workPath: '{}'\nformat: EDT\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\ntests:\n  execution_timeout_seconds: 5\nsource-set:\n  - name: configuration\n    type: CONFIGURATION\n    path: configuration\n  - name: client_mcp\n    type: EXTENSION\n    path: exts/client-mcp\ntools:\n  platform:\n    path: '{}'\n  edt_cli:\n    path: '{}'\n",
        work_path.display(),
        install_dir.display(),
        edt_cli_path.display(),
    );
    fs::write(&config_path, config).expect("config");

    let first = v8_runner_command()
        .args(["--config", &config_path.display().to_string(), "build"])
        .output()
        .expect("prime build");
    assert!(first.status.success());

    fs::write(
        base_path.join("exts").join("client-mcp").join("Module.bsl"),
        "procedure Test()\n  // changed after snapshot\nendprocedure",
    )
    .expect("modify extension");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "test",
            "yaxunit",
            "module",
            "ClientMcpSmoke",
        ])
        .output()
        .expect("run");

    assert!(output.status.success());
    let build_calls_text = fs::read_to_string(&build_calls).expect("build calls");
    let edt_calls_text = fs::read_to_string(&edt_calls).expect("edt calls");
    let test_calls_text = fs::read_to_string(&test_calls).expect("test calls");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");

    assert!(build_calls_text.contains("-Extension client_mcp"));
    assert!(!build_calls_text.contains("-partial"));
    assert!(edt_calls_text.contains(&format!(
        "export --project {}",
        base_path.join("exts").join("client-mcp").display()
    )));
    assert!(test_calls_text.contains("RunUnitTests="));
    assert_eq!(payload["ok"], true);
    assert_eq!(
        payload["data"]["target"]["module"]["name"],
        "ClientMcpSmoke"
    );
}

#[test]
fn repeated_test_skips_unchanged_source_backed_tool_extension_build() {
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let install_dir = dir.path().join("platform");
    let edt_cli_path = dir.path().join("edt").join("1cedtcli");
    let config_path = base_path.join("v8project.yaml");
    let build_calls = dir.path().join("build.calls.log");
    let test_calls = dir.path().join("test.calls.log");
    let edt_calls = dir.path().join("edt.calls.log");
    let captured_config = dir.path().join("captured-config.json");
    let tool_source = base_path.join("tools").join("client-mcp");

    fs::create_dir_all(&work_path).expect("work");
    write_native_edt_project(
        &base_path.join("configuration"),
        "configuration",
        V8_CONFIGURATION_NATURE,
        None,
    );
    write_native_edt_project(
        &tool_source,
        "client-mcp-project",
        V8_EXTENSION_NATURE,
        Some("configuration"),
    );

    write_build_script(&install_dir.join("bin").join("1cv8"), &build_calls, false);
    write_test_script(
        &install_dir.join("bin").join("1cv8c"),
        &test_calls,
        &captured_config,
        JUNIT_SMOKE_REPORT_FIXTURE,
        YAXUNIT_LOG_FIXTURE,
        0,
        None,
    );
    write_edt_script(&edt_cli_path, &edt_calls);

    let config = format!(
        "workPath: '{}'\nformat: EDT\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\ntests:\n  execution_timeout_seconds: 5\nsource-set:\n  - name: configuration\n    type: CONFIGURATION\n    path: configuration\ntools:\n  platform:\n    path: '{}'\n  edt_cli:\n    path: '{}'\n  client_mcp:\n    extension:\n      name: client_mcp\n      source:\n        path: '{}'\n        format: EDT\n",
        work_path.display(),
        install_dir.display(),
        edt_cli_path.display(),
        tool_source.display(),
    );
    fs::write(&config_path, config).expect("config");

    let first = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "test",
            "yaxunit",
            "all",
        ])
        .output()
        .expect("first test");
    assert!(
        first.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        first.status.code(),
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(fs::read_to_string(&edt_calls)
        .expect("first edt calls")
        .contains("export --project-name client-mcp-project"));
    assert!(fs::read_to_string(&build_calls)
        .expect("first build calls")
        .contains("-Extension client_mcp"));

    remove_file_if_exists(&build_calls);
    remove_file_if_exists(&edt_calls);
    remove_file_if_exists(&test_calls);

    let second = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "test",
            "yaxunit",
            "all",
        ])
        .output()
        .expect("second test");

    assert!(
        second.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        second.status.code(),
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(
        !edt_calls.exists(),
        "unchanged nested test build must not run EDT export"
    );
    assert!(
        !build_calls.exists(),
        "unchanged nested test build must not load/apply tool extension"
    );
    assert!(fs::read_to_string(&test_calls)
        .expect("second test calls")
        .contains("RunUnitTests="));
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

    let compact = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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

    let full = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
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

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "test",
            "yaxunit",
            "all",
        ])
        .output()
        .expect("run");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(3));

    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["data"]["execution"]["status"], "timed_out");
    let platform_log = payload["data"]["retained_paths"]["platform_log"]
        .as_str()
        .expect("platform log");
    assert!(!platform_log.is_empty());
}
