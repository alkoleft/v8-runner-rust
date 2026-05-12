#![cfg(unix)]

mod support;

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
        "args=\"$*\"\nprintf '%s\\n' \"$args\" >> \"{}\"\n{}\nmkdir -p \"$(printf '%s' \"$args\" | awk '{{print $NF}}')\"\nexit 0",
        calls_log.display(),
        pattern_branch
    );
    write_script(path, &body);
}

fn write_designer_dump_script_for_edt(path: &Path, calls_log: &Path) {
    let body = format!(
        "args=\"$*\"\nout=\"\"\ntarget=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  if [ \"$prev\" = \"/DumpConfigToFiles\" ]; then target=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf 'designer log for %s\\n' \"$args\" > \"$out\"; fi\nprintf '%s\\n' \"$args\" >> \"{}\"\nmkdir -p \"$target\"\nprintf '<Configuration />\\n' > \"$target/Configuration.xml\"\nexit 0",
        calls_log.display()
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
    fs::write(base_path.join("main").join("old.txt"), "old").expect("old");

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
    fs::write(base_path.join("main").join("old.txt"), "old").expect("old");

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

#[test]
fn dump_ibcmd_full_json_success() {
    let (_dir, config_path, _binary_path, _work_path, base_path, calls_log) = setup_project();

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
    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("--force"));
    assert!(base_path.join("main").exists());
}

#[test]
fn dump_edt_full_json_success_updates_designer_mirror_and_edt_target() {
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
        base_path.join("main").display().to_string()
    );
    assert_native_edt_project(&base_path.join("main"));
    assert!(!base_path.join("main").join("old.txt").exists());
    assert!(work_path
        .join("designer")
        .join("main")
        .join("Configuration.xml")
        .exists());

    let designer_calls = fs::read_to_string(designer_calls).expect("designer calls");
    let edt_calls = fs::read_to_string(edt_calls).expect("edt calls");
    assert!(designer_calls.contains(work_path.join("designer").display().to_string().as_str()));
    assert!(edt_calls.contains(
        work_path
            .join("designer/main")
            .display()
            .to_string()
            .as_str()
    ));
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
    let (_dir, config_path, _binary_path, _work_path, base_path, calls_log) = setup_project();
    fs::remove_dir_all(base_path.join("main")).expect("remove target");

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
    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("--sync"));
    assert!(calls.contains(base_path.join("main").display().to_string().as_str()));
}

#[test]
fn dump_ibcmd_partial_json_success_uses_degraded_fallback() {
    let (_dir, config_path, _binary_path, _work_path, _base_path, calls_log) = setup_project();

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
    assert!(data["message"]
        .as_str()
        .expect("message")
        .contains("IBCMD does not support object-scoped partial dump"));
    let calls = fs::read_to_string(calls_log).expect("calls");
    assert!(calls.contains("--sync"));
}

#[test]
fn dump_text_warning_shows_degraded_fallback_reason() {
    let (_dir, config_path, _binary_path, _work_path, _base_path, _calls_log) = setup_project();

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
fn dump_ibcmd_full_server_connection_passes_dbms_and_infobase_credentials() {
    let (_dir, config_path, _binary_path, _work_path, _base_path, calls_log) = setup_project();
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
}
