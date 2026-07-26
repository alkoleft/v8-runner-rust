#![cfg(unix)]

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use support::{temp_workspace, v8_runner_command, write_shell_script as write_script};

const V8_CONFIGURATION_NATURE: &str = "com._1c.g5.v8.dt.core.V8ConfigurationNature";
const EDT_RUNTIME_VERSION: &str = "8.3.27";

fn write_ibcmd_script(path: &Path, calls_log: &Path, fail_pattern: Option<&str>) {
    let pattern_branch = fail_pattern
        .map(|pattern| {
            format!(
                "if printf '%s' \"$args\" | grep -F -q -- '{}'; then exit 17; fi",
                pattern
            )
        })
        .unwrap_or_default();
    let body = format!(
        "args=\"$*\"\ntarget=\"$(printf '%s' \"$args\" | awk '{{print $NF}}')\"\nprintf '%s\\n' \"$args\" >> \"{}\"\n{}\nmkdir -p \"$target\"\nprintf '<ConfigDumpInfo version=\"2.17\"><Metadata id=\"private-id\" configVersion=\"7\"/></ConfigDumpInfo>\\n' > \"$target/ConfigDumpInfo.xml\"\nexit 0",
        calls_log.display(),
        pattern_branch
    );
    write_script(path, &body);
}

fn write_designer_dump_script_for_edt(path: &Path, calls_log: &Path) {
    let body = format!(
        "args=\"$*\"\nout=\"\"\ntarget=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  if [ \"$prev\" = \"/DumpConfigToFiles\" ]; then target=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf 'designer log for %s\\n' \"$args\" > \"$out\"; fi\nprintf '%s\\n' \"$args\" >> \"{}\"\nmkdir -p \"$target\"\nprintf '<Configuration />\\n' > \"$target/Configuration.xml\"\nprintf '<ConfigDumpInfo version=\"2.17\"><Metadata id=\"private-id\" configVersion=\"7\"/></ConfigDumpInfo>\\n' > \"$target/ConfigDumpInfo.xml\"\nexit 0",
        calls_log.display()
    );
    write_script(path, &body);
}

fn write_designer_partial_dump_script(path: &Path, captured_list: &Path) {
    let body = format!(
        "list_file=\"\"\ntarget=\"\"\nprevious=\"\"\nfor argument in \"$@\"; do\n  if [ \"$previous\" = \"-listFile\" ]; then list_file=\"$argument\"; fi\n  if [ \"$previous\" = \"/DumpConfigToFiles\" ]; then target=\"$argument\"; fi\n  previous=\"$argument\"\ndone\nif [ -n \"$list_file\" ]; then cp \"$list_file\" \"{}\"; fi\nmkdir -p \"$target\"\nprintf '<Configuration />\\n' > \"$target/Configuration.xml\"\nprintf '<ConfigDumpInfo version=\"2.17\"><Metadata id=\"private-id\" configVersion=\"7\"/></ConfigDumpInfo>\\n' > \"$target/ConfigDumpInfo.xml\"\nexit 0",
        captured_list.display()
    );
    write_script(path, &body);
}

fn write_edt_import_script(path: &Path, calls_log: &Path) {
    let body = format!(
        r#"args="$*"
printf '%s\n' "$args" >> "{}"
project=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--project" ]; then project="$arg"; fi
  prev="$arg"
done
mkdir -p "$project/DT-INF" "$project/src/Configuration"
cat > "$project/.project" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<projectDescription>
  <name>BaseProject</name>
  <natures>
    <nature>{}</nature>
  </natures>
</projectDescription>
EOF
printf 'Manifest-Version: 1.0\nRuntime-Version: {}\n' > "$project/DT-INF/PROJECT.PMF"
printf '<Configuration />\n' > "$project/src/Configuration/Configuration.mdo"
printf 'Procedure Test()\nEndProcedure\n' > "$project/src/Configuration/Module.bsl"
exit 0"#,
        calls_log.display(),
        V8_CONFIGURATION_NATURE,
        EDT_RUNTIME_VERSION
    );
    write_script(path, &body);
}

fn assert_native_edt_project(path: &Path) {
    assert!(path.join(".project").exists());
    assert!(path.join("DT-INF").join("PROJECT.PMF").exists());
    assert!(path.join("src/Configuration/Configuration.mdo").exists());
}

fn write_edt_configuration_source(path: &Path, project_name: &str) {
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

fn write_config(path: &Path, base_path: &Path, work_path: &Path, platform_path: &Path) {
    write_config_with_infobase(
        path,
        base_path,
        work_path,
        platform_path,
        "  connection: 'File=/tmp/ib'\n",
    );
}

fn write_config_with_infobase(
    path: &Path,
    _base_path: &Path,
    work_path: &Path,
    platform_path: &Path,
    infobase_yaml: &str,
) {
    let config = format!(
        "workPath: '{}'\nformat: DESIGNER\nbuilder: IBCMD\ninfobase:\n{}source-set:\n  - name: main\n    type: CONFIGURATION\n    path: project/main\ntools:\n  platform:\n    path: '{}'\n",
        work_path.display(),
        infobase_yaml,
        platform_path.display(),
    );

    fs::write(path, config).expect("config");
}

fn setup_project() -> (
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
    let config_path = dir.path().join("v8project.yaml");
    let binary_path = dir.path().join("ibcmd");
    let calls_log = dir.path().join("calls.log");

    fs::create_dir_all(base_path.join("main")).expect("main");
    fs::create_dir_all(&work_path).expect("work");
    write_ibcmd_script(&binary_path, &calls_log, None);
    write_config(&config_path, &base_path, &work_path, &binary_path);

    (
        dir,
        config_path,
        binary_path,
        work_path,
        base_path,
        calls_log,
    )
}

fn assert_ibcmd_data_path(calls: &str, work_path: &Path) {
    let expected_fragment = format!("infobase --data {}", work_path.join("ibcmd-data").display());
    assert!(
        calls.contains(&expected_fragment),
        "expected isolated IBCMD data path fragment: {expected_fragment}"
    );
}

fn write_designer_config(path: &Path, work_path: &Path, platform_path: &Path) {
    let config = format!(
        "workPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: project/main\ntools:\n  platform:\n    path: '{}'\n",
        work_path.display(),
        platform_path.display(),
    );

    fs::write(path, config).expect("config");
}

fn write_edt_dump_config(
    path: &Path,
    _base_path: &Path,
    work_path: &Path,
    platform_path: &Path,
    edt_path: &Path,
) {
    let config = format!(
        "workPath: '{}'\nformat: EDT\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: project/main\ntools:\n  platform:\n    path: '{}'\n  edt_cli:\n    path: '{}'\n    interactive-mode: false\n",
        work_path.display(),
        platform_path.display(),
        edt_path.display(),
    );

    fs::write(path, config).expect("config");
}

fn setup_edt_project() -> (
    tempfile::TempDir,
    PathBuf,
    PathBuf,
    PathBuf,
    PathBuf,
    PathBuf,
    PathBuf,
    PathBuf,
) {
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let config_path = dir.path().join("v8project.yaml");
    let platform_path = dir.path().join("1cv8");
    let edt_path = dir.path().join("edt").join("1cedtcli");
    let designer_calls = dir.path().join("designer-calls.log");
    let edt_calls = dir.path().join("edt-calls.log");

    fs::create_dir_all(base_path.join("main")).expect("main");
    fs::create_dir_all(&work_path).expect("work");
    write_edt_configuration_source(&base_path.join("main"), "BaseProject");
    write_designer_dump_script_for_edt(&platform_path, &designer_calls);
    write_edt_import_script(&edt_path, &edt_calls);
    write_edt_dump_config(
        &config_path,
        &base_path,
        &work_path,
        &platform_path,
        &edt_path,
    );

    (
        dir,
        config_path,
        platform_path,
        edt_path,
        work_path,
        base_path,
        designer_calls,
        edt_calls,
    )
}

fn bootstrap_full(config_path: &Path) {
    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "dump",
            "--mode",
            "full",
            "--source-set",
            "main",
        ])
        .output()
        .expect("bootstrap dump");
    assert!(
        output.status.success(),
        "bootstrap stdout:\n{}\nbootstrap stderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn snapshot_runtime_generations(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, current: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if !current.exists() {
            return;
        }
        let mut entries = fs::read_dir(current)
            .expect("read snapshot directory")
            .collect::<Result<Vec<_>, _>>()
            .expect("snapshot entries");
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type().expect("snapshot file type");
            if file_type.is_dir() {
                visit(root, &path, files);
            } else if file_type.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .expect("relative snapshot path")
                    .to_path_buf();
                if relative
                    .components()
                    .any(|component| component.as_os_str() == "generations")
                {
                    files.insert(relative, fs::read(path).expect("snapshot file"));
                }
            }
        }
    }

    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

#[test]
fn dump_ibcmd_full_json_success() {
    let (_dir, config_path, _binary_path, work_path, base_path, calls_log) = setup_project();

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "dump",
            "--mode",
            "full",
            "--source-set",
            "main",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["data"]["receipt"]["status"], "applied");
    assert_eq!(
        payload["data"]["receipt"]["requested"],
        serde_json::json!([])
    );
    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("--force"));
    assert_ibcmd_data_path(&calls, &work_path);
    assert!(base_path.join("main").exists());
}

#[test]
fn dump_edt_full_json_success_updates_runtime_designer_baseline_and_edt_target() {
    let (
        _dir,
        config_path,
        _platform_path,
        _edt_path,
        work_path,
        base_path,
        designer_calls,
        edt_calls,
    ) = setup_edt_project();

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "dump",
            "--mode",
            "full",
            "--source-set",
            "main",
        ])
        .output()
        .expect("run command");

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "dump");
    assert_eq!(
        payload["data"]["target_path"],
        fs::canonicalize(base_path.join("main"))
            .expect("canonical target")
            .display()
            .to_string()
    );
    assert_eq!(payload["data"]["receipt"]["status"], "applied");
    assert_native_edt_project(&base_path.join("main"));
    let runtime_generations = snapshot_runtime_generations(&work_path);
    assert!(runtime_generations.keys().any(|path| {
        path.ends_with("ib-baseline/edt-platform-designer/files/Configuration.xml")
    }));

    let designer_calls = fs::read_to_string(designer_calls).expect("designer calls");
    let edt_calls = fs::read_to_string(edt_calls).expect("edt calls");
    assert!(designer_calls.contains(work_path.display().to_string().as_str()));
    assert!(edt_calls.contains(work_path.display().to_string().as_str()));
}

#[test]
fn dump_text_success_is_compact_and_keeps_output_visible() {
    let (_dir, config_path, _binary_path, _work_path, base_path, _calls_log) = setup_project();

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "dump",
            "--mode",
            "full",
            "--source-set",
            "main",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("● dump: full"));
    assert!(!stdout.contains("started_at: "));
    assert!(stdout.contains("[ibcmd] exporting configuration files"));
    assert!(
        stdout
            .find("[ibcmd] exporting configuration files")
            .expect("dump detail")
            < stdout
                .find("● Dump completed successfully")
                .expect("summary")
    );
    assert!(stdout.contains("● Dump completed successfully"));
    assert!(stdout.contains("│   source-set: main"));
    assert!(stdout.contains("│   mode: full"));
    assert!(stdout.contains(base_path.join("main").display().to_string().as_str()));
    assert!(!stdout.contains("platform log"));
}

#[test]
fn dump_ibcmd_incremental_json_success() {
    let (_dir, config_path, _binary_path, work_path, base_path, calls_log) = setup_project();
    bootstrap_full(&config_path);
    fs::write(&calls_log, []).expect("clear calls");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "dump",
            "--mode",
            "incremental",
            "--source-set",
            "main",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["data"]["receipt"]["status"], "applied");
    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("--sync"));
    assert_ibcmd_data_path(&calls, &work_path);
    assert!(calls.contains(work_path.display().to_string().as_str()));
    assert!(!calls.contains(base_path.join("main").display().to_string().as_str()));
}

#[test]
fn dump_ibcmd_partial_json_success_uses_degraded_fallback() {
    let (_dir, config_path, _binary_path, work_path, _base_path, calls_log) = setup_project();
    bootstrap_full(&config_path);
    fs::write(&calls_log, []).expect("clear calls");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "dump",
            "--mode",
            "partial",
            "--source-set",
            "main",
            "--object",
            "Catalog.Items",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    let data = &payload["data"];
    assert_eq!(payload["ok"], true);
    assert_eq!(data["mode"], "PARTIAL");
    assert_eq!(data["receipt"]["status"], "applied");
    assert!(data["message"]
        .as_str()
        .expect("message")
        .contains("IBCMD does not support object-scoped partial dump"));
    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("--sync"));
    assert_ibcmd_data_path(&calls, &work_path);
}

#[test]
fn dump_text_warning_shows_degraded_fallback_reason() {
    let (_dir, config_path, _binary_path, _work_path, _base_path, _calls_log) = setup_project();
    bootstrap_full(&config_path);

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "dump",
            "--mode",
            "partial",
            "--source-set",
            "main",
            "--object",
            "Catalog.Items",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("● Dump completed with warnings"));
    assert!(stdout.contains("[warning] IBCMD does not support object-scoped partial dump"));
}

#[test]
fn dump_ibcmd_partial_failure_keeps_partial_mode_and_warning() {
    let (_dir, config_path, binary_path, _work_path, _base_path, calls_log) = setup_project();
    bootstrap_full(&config_path);
    write_ibcmd_script(&binary_path, &calls_log, Some("--sync"));

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "dump",
            "--mode",
            "partial",
            "--source-set",
            "main",
            "--object",
            "Catalog.Items",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(4));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    let data = &payload["data"];
    assert_eq!(payload["ok"], false);
    assert_eq!(data["mode"], "PARTIAL");
    assert_eq!(data["receipt"]["status"], "failed");
    assert_eq!(data["receipt"]["requested"], serde_json::json!([]));
    assert_eq!(data["receipt"]["processed"], serde_json::json!([]));
    assert_eq!(data["receipt"]["skipped"], serde_json::json!([]));
    assert_eq!(data["receipt"]["conflicted"], serde_json::json!([]));
    assert!(data["message"]
        .as_str()
        .expect("message")
        .contains("IBCMD does not support object-scoped partial dump"));
    assert!(data["message"]
        .as_str()
        .expect("message")
        .contains("dump failed for source-set 'main' with exit code 17"));
}

#[test]
fn dump_designer_partial_json_normalizes_colon_selector_and_reports_both_forms() {
    let (_dir, config_path, binary_path, work_path, _base_path, _calls_log) = setup_project();
    let designer_binary = binary_path.with_file_name("1cv8");
    let captured_list = config_path
        .parent()
        .expect("project directory")
        .join("partial-list.txt");
    write_designer_partial_dump_script(&designer_binary, &captured_list);
    write_designer_config(&config_path, &work_path, &designer_binary);
    bootstrap_full(&config_path);

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "dump",
            "--mode",
            "partial",
            "--object",
            "  Catalog:Items  ",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    assert_eq!(
        fs::read_to_string(captured_list).expect("captured selector list"),
        "Catalog.Items\n"
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(
        payload["data"]["selectors"][0]["requested"],
        "  Catalog:Items  "
    );
    assert_eq!(
        payload["data"]["selectors"][0]["normalized"],
        "Catalog.Items"
    );
}

#[test]
fn dump_text_failure_shows_error_message() {
    let (_dir, config_path, binary_path, _work_path, _base_path, calls_log) = setup_project();
    write_ibcmd_script(&binary_path, &calls_log, Some("--force"));

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "dump",
            "--mode",
            "full",
            "--source-set",
            "main",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("● Dump failed"));
    assert!(stdout.contains("[error]"));
    assert!(stdout.contains("exit code 17"));
}

#[test]
fn dump_json_conflict_preserves_local_file_and_runtime_state() {
    let (_dir, config_path, _binary_path, work_path, base_path, _calls_log) = setup_project();
    bootstrap_full(&config_path);
    let local_path = base_path.join("main/old.txt");
    fs::write(&local_path, "old").expect("local file");
    let state_before = snapshot_runtime_generations(&work_path);

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "dump",
            "--mode",
            "full",
            "--source-set",
            "main",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(3));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    let receipt = &payload["data"]["receipt"];
    assert_eq!(payload["error"]["kind"], "runtime");
    assert_eq!(receipt["status"], "conflict");
    assert_eq!(receipt["processed"], serde_json::json!([]));
    assert_eq!(receipt["requested"], receipt["conflicted"]);
    assert_eq!(receipt["requested"].as_array().expect("requested").len(), 1);
    assert_eq!(receipt["requested"][0]["path"], "old.txt");
    assert!(receipt["requested"][0]["preHash"].is_string());
    assert_eq!(receipt["requested"][0]["postHash"], Value::Null);
    assert_eq!(
        fs::read(&local_path).expect("local file after conflict"),
        b"old"
    );
    assert_eq!(snapshot_runtime_generations(&work_path), state_before);
}

#[test]
fn dump_json_incremental_and_partial_conflicts_preserve_source_and_runtime_state() {
    for (mode, objects) in [
        ("incremental", Vec::<&str>::new()),
        ("partial", vec!["--object", "Catalog.Items"]),
    ] {
        let (_dir, config_path, _binary_path, work_path, base_path, calls_log) = setup_project();
        bootstrap_full(&config_path);
        fs::write(&calls_log, []).expect("clear calls");
        let local_path = base_path.join("main/old.txt");
        fs::write(&local_path, "old").expect("local file");
        let state_before = snapshot_runtime_generations(&work_path);

        let mut args = vec![
            "--config",
            config_path.to_str().expect("config path"),
            "--json-message",
            "dump",
            "--mode",
            mode,
            "--source-set",
            "main",
        ];
        args.extend(objects);
        let output = v8_runner_command()
            .args(args)
            .output()
            .expect("run command");

        assert!(!output.status.success(), "{mode} dump must conflict");
        assert_eq!(output.status.code(), Some(3), "unexpected {mode} exit code");
        let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
        let receipt = &payload["data"]["receipt"];
        assert_eq!(payload["data"]["mode"], mode.to_uppercase());
        assert_eq!(receipt["status"], "conflict");
        assert_eq!(receipt["processed"], serde_json::json!([]));
        assert_eq!(receipt["requested"], receipt["conflicted"]);
        assert_eq!(receipt["requested"][0]["path"], "old.txt");
        assert_eq!(
            fs::read(&local_path).expect("local file after conflict"),
            b"old",
            "{mode} dump changed local source"
        );
        assert_eq!(
            snapshot_runtime_generations(&work_path),
            state_before,
            "{mode} dump advanced runtime state after conflict"
        );
        assert!(
            fs::read_to_string(&calls_log)
                .expect("calls")
                .contains("--sync"),
            "{mode} dump did not exercise the incremental shadow branch"
        );
    }
}

#[test]
fn dump_ibcmd_full_server_connection_passes_dbms_and_infobase_credentials() {
    let (_dir, config_path, _binary_path, work_path, _base_path, calls_log) = setup_project();
    write_config_with_infobase(
        &config_path,
        &config_path.parent().expect("dir").join("project"),
        &config_path.parent().expect("dir").join("work"),
        &config_path.parent().expect("dir").join("ibcmd"),
        "  connection: 'Srvr=server;Ref=main'\n  user: Admin\n  password: secret\n  dbms:\n    kind: PostgreSQL\n    server: localhost\n    name: maindb\n    user: postgres\n    password: pg-secret\n",
    );

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "dump",
            "--mode",
            "full",
            "--source-set",
            "main",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("--dbms PostgreSQL --database-server localhost --database-name maindb"));
    assert!(calls.contains("--user Admin --password secret"));
    assert!(calls.contains("--database-user postgres --database-password pg-secret"));
    assert_ibcmd_data_path(&calls, &work_path);
}
