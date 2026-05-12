#![cfg(unix)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use support::{temp_workspace, v8_runner_command, write_shell_script as write_script};

const V8_CONFIGURATION_NATURE: &str = "com._1c.g5.v8.dt.core.V8ConfigurationNature";
const V8_EXTENSION_NATURE: &str = "com._1c.g5.v8.dt.core.V8ExtensionNature";
const V8_EXTERNAL_OBJECTS_NATURE: &str = "com._1c.g5.v8.dt.core.V8ExternalObjectsNature";
const EDT_RUNTIME_VERSION: &str = "8.3.27";

#[derive(Clone, Copy)]
struct SourceSetSpec<'a> {
    name: &'a str,
    kind: &'a str,
    path: &'a str,
}

fn write_edt_script(path: &Path, calls_log: &Path) {
    let body = format!(
        r#"args="$*"
printf '%s\n' "$args" >> "{}"
mode=""
project=""
config_files=""
base_project_name=""
prev=""
write_native_project() {{
  target="$1"
  name="$2"
  nature="$3"
  base_project="$4"
  mkdir -p "$target/DT-INF" "$target/src/Configuration"
  cat > "$target/.project" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<projectDescription>
  <name>$name</name>
  <natures>
    <nature>$nature</nature>
  </natures>
</projectDescription>
EOF
  {{
    if [ -n "$base_project" ]; then printf 'Base-Project: %s\n' "$base_project"; fi
    printf 'Manifest-Version: 1.0\nRuntime-Version: {}\n'
  }} > "$target/DT-INF/PROJECT.PMF"
  printf '<Configuration />\n' > "$target/src/Configuration/Configuration.mdo"
  printf 'Procedure Test()\nEndProcedure\n' > "$target/src/Configuration/Module.bsl"
}}
write_external_project() {{
  target="$1"
  name="$2"
  descriptor="$3"
  mkdir -p "$target/DT-INF" "$target/src"
  cat > "$target/.project" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<projectDescription>
  <name>$name</name>
  <natures>
    <nature>{}</nature>
  </natures>
</projectDescription>
EOF
  printf 'Base-Project: BaseProject\nManifest-Version: 1.0\nRuntime-Version: {}\n' > "$target/DT-INF/PROJECT.PMF"
  cp "$descriptor" "$target/src/root.xml"
}}
read_project_name() {{
  project_file="$1/.project"
  if [ ! -f "$project_file" ]; then
    printf 'Imported'
    return
  fi
  name=$(sed -n 's:.*<name>\([^<][^<]*\)</name>.*:\1:p' "$project_file" | head -n 1)
  if [ -n "$name" ]; then
    printf '%s' "$name"
  else
    printf 'Imported'
  fi
}}
project_is_extension() {{
  project_file="$1/.project"
  [ -f "$project_file" ] && grep -q '{}' "$project_file"
}}
project_is_external() {{
  [ -f "$1/src/root.xml" ]
}}
read_configuration_name() {{
  config_file="$1/Configuration.xml"
  if [ ! -f "$config_file" ]; then
    printf 'Imported'
    return
  fi
  name=$(sed -n 's:.*<Name>\([^<][^<]*\)</Name>.*:\1:p' "$config_file" | head -n 1)
  if [ -n "$name" ]; then
    printf '%s' "$name"
  else
    printf 'Imported'
  fi
}}
configuration_is_extension() {{
  config_file="$1/Configuration.xml"
  [ -f "$config_file" ] && grep -q 'ConfigurationExtensionPurpose\|ObjectBelonging' "$config_file"
}}
for arg in "$@"; do
  if [ "$prev" = "-command" ]; then mode="$arg"; fi
  if [ "$prev" = "--project" ]; then project="$arg"; fi
  if [ "$prev" = "--configuration-files" ]; then config_files="$arg"; fi
  if [ "$prev" = "--base-project-name" ]; then base_project_name="$arg"; fi
  prev="$arg"
done
case "$mode" in
  export)
    mkdir -p "$config_files"
    rm -rf "$config_files"/*
    if project_is_external "$project"; then
      descriptor_name=$(basename "$project")
      cp "$project/src/root.xml" "$config_files/$descriptor_name.xml"
    else
      project_name=$(read_project_name "$project")
      if project_is_extension "$project"; then
        printf '<Configuration><Properties><Name>%s</Name></Properties><ConfigurationExtensionPurpose>Extension</ConfigurationExtensionPurpose></Configuration>\n' "$project_name" > "$config_files/Configuration.xml"
      else
        printf '<Configuration><Properties><Name>%s</Name></Properties></Configuration>\n' "$project_name" > "$config_files/Configuration.xml"
      fi
    fi
    ;;
  import)
    if [ -f "$config_files/Configuration.xml" ]; then
      mkdir -p "$project"
      imported_name=$(read_configuration_name "$config_files")
      if configuration_is_extension "$config_files"; then
        if [ "$base_project_name" != "BaseProject" ]; then
          printf 'unexpected base project: %s\n' "$base_project_name" >&2
          exit 23
        fi
        imported_nature="{}"
        imported_base="BaseProject"
      else
        imported_nature="{}"
        imported_base=""
      fi
      write_native_project "$project" "$imported_name" "$imported_nature" "$imported_base"
    else
      mkdir -p "$project"
      for descriptor in "$config_files"/*.xml; do
        if [ ! -f "$descriptor" ]; then continue; fi
        descriptor_name=$(basename "$descriptor" .xml)
        write_external_project "$project/$descriptor_name" "$descriptor_name" "$descriptor"
      done
    fi
    ;;
esac
exit 0"#,
        calls_log.display(),
        EDT_RUNTIME_VERSION,
        V8_EXTERNAL_OBJECTS_NATURE,
        EDT_RUNTIME_VERSION,
        V8_EXTENSION_NATURE,
        V8_EXTENSION_NATURE,
        V8_CONFIGURATION_NATURE
    );
    write_script(path, &body);
}

fn write_path_named_edt_import_script(path: &Path, calls_log: &Path) {
    let body = format!(
        r#"args="$*"
printf '%s\n' "$args" >> "{}"
mode=""
project=""
config_files=""
base_project_name=""
prev=""
write_native_project() {{
  target="$1"
  name="$2"
  nature="$3"
  base_project="$4"
  mkdir -p "$target/DT-INF" "$target/src/Configuration"
  cat > "$target/.project" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<projectDescription>
  <name>$name</name>
  <natures>
    <nature>$nature</nature>
  </natures>
</projectDescription>
EOF
  {{
    if [ -n "$base_project" ]; then printf 'Base-Project: %s\n' "$base_project"; fi
    printf 'Manifest-Version: 1.0\nRuntime-Version: {}\n'
  }} > "$target/DT-INF/PROJECT.PMF"
  printf '<Configuration />\n' > "$target/src/Configuration/Configuration.mdo"
}}
write_external_project() {{
  target="$1"
  name="$2"
  descriptor="$3"
  mkdir -p "$target/DT-INF" "$target/src"
  cat > "$target/.project" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<projectDescription>
  <name>$name</name>
  <natures>
    <nature>{}</nature>
  </natures>
</projectDescription>
EOF
  printf 'Base-Project: BaseProject\nManifest-Version: 1.0\nRuntime-Version: {}\n' > "$target/DT-INF/PROJECT.PMF"
  cp "$descriptor" "$target/src/root.xml"
}}
configuration_is_extension() {{
  config_file="$1/Configuration.xml"
  [ -f "$config_file" ] && grep -q 'ConfigurationExtensionPurpose\|ObjectBelonging' "$config_file"
}}
for arg in "$@"; do
  if [ "$prev" = "-command" ]; then mode="$arg"; fi
  if [ "$prev" = "--project" ]; then project="$arg"; fi
  if [ "$prev" = "--configuration-files" ]; then config_files="$arg"; fi
  if [ "$prev" = "--base-project-name" ]; then base_project_name="$arg"; fi
  prev="$arg"
done
case "$mode" in
  import)
    if [ -f "$config_files/Configuration.xml" ]; then
      mkdir -p "$project"
      imported_name=$(basename "$project")
      if configuration_is_extension "$config_files"; then
        if [ "$base_project_name" != "configuration" ]; then
          printf 'unexpected base project: %s\n' "$base_project_name" >&2
          exit 23
        fi
        write_native_project "$project" "$imported_name" "{}" "$base_project_name"
      else
        write_native_project "$project" "$imported_name" "{}" ""
      fi
    else
      mkdir -p "$project"
      for descriptor in "$config_files"/*.xml; do
        if [ ! -f "$descriptor" ]; then continue; fi
        descriptor_name=$(basename "$descriptor" .xml)
        write_external_project "$project/$descriptor_name" "$descriptor_name" "$descriptor"
      done
    fi
    ;;
esac
exit 0"#,
        calls_log.display(),
        EDT_RUNTIME_VERSION,
        V8_EXTERNAL_OBJECTS_NATURE,
        EDT_RUNTIME_VERSION,
        V8_EXTENSION_NATURE,
        V8_CONFIGURATION_NATURE
    );
    write_script(path, &body);
}

fn write_config(
    path: &Path,
    _base_path: &Path,
    work_path: &Path,
    edt_path: &Path,
    format: &str,
    source_sets: &[SourceSetSpec<'_>],
    platform_version: Option<&str>,
) {
    let mut config = format!(
        "workPath: '{}'\nformat: {format}\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\nsource-set:\n",
        work_path.display(),
    );
    for source_set in source_sets {
        config.push_str(&format!(
            "  - name: {}\n    type: {}\n    path: {}\n",
            source_set.name, source_set.kind, source_set.path
        ));
    }
    config.push_str("tools:\n");
    if let Some(version) = platform_version {
        config.push_str(&format!("  platform:\n    version: '{version}'\n"));
    }
    config.push_str(&format!(
        "  edt_cli:\n    path: '{}'\n    interactive-mode: false\n",
        edt_path.display()
    ));
    fs::write(path, config).expect("config");
}

fn write_live_workspace_lock(work_path: &Path, command: &str) {
    let canonical_work = fs::canonicalize(work_path).expect("canonical work");
    let lock_owner = "integration-test-lock-owner";
    let started_at = chrono::Utc::now().to_rfc3339();

    fs::write(
        canonical_work.join(".v8-runner.workspace.lock"),
        serde_json::json!({
            "tool": "v8-runner",
            "pid": std::process::id(),
            "owner_id": lock_owner,
            "created_at": started_at,
        })
        .to_string(),
    )
    .expect("workspace lock");
    fs::write(
        canonical_work.join(".v8-runner.workspace.lock.json"),
        serde_json::json!({
            "pid": std::process::id(),
            "lock_owner": lock_owner,
            "command": command,
            "started_at": started_at,
            "canonical_work_path": canonical_work,
        })
        .to_string(),
    )
    .expect("workspace lock sidecar");
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
    let config_path = base_path.join("v8project.yaml");
    let edt_cli_path = dir.path().join("edt").join("1cedtcli");
    let calls_log = dir.path().join("edt-calls.log");

    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    write_edt_script(&edt_cli_path, &calls_log);

    (
        dir,
        config_path,
        base_path,
        work_path,
        edt_cli_path,
        calls_log,
    )
}

fn write_designer_source(path: &Path, project_name: &str, is_extension: bool) {
    fs::create_dir_all(path).expect("designer source");
    let descriptor = if is_extension {
        format!(
            "<Configuration><Properties><Name>{project_name}</Name></Properties><ConfigurationExtensionPurpose>Extension</ConfigurationExtensionPurpose></Configuration>\n"
        )
    } else {
        format!(
            "<Configuration><Properties><Name>{project_name}</Name></Properties></Configuration>\n"
        )
    };
    fs::write(path.join("Configuration.xml"), descriptor).expect("xml");
}

fn write_designer_external_source(path: &Path, names: &[&str]) {
    fs::create_dir_all(path).expect("designer external source");
    for name in names {
        fs::write(
            path.join(format!("{name}.xml")),
            format!(
                "<ExternalDataProcessor><Properties><Name>{name}</Name></Properties></ExternalDataProcessor>\n"
            ),
        )
        .expect("xml");
    }
}

fn write_edt_source(path: &Path, name: &str, descriptor_xml: &str) {
    fs::create_dir_all(path).expect("edt source");
    fs::create_dir_all(path.join("DT-INF")).expect("dt-inf");
    fs::create_dir_all(path.join("src").join("Configuration")).expect("src");
    let is_extension = descriptor_xml.contains("ConfigurationExtensionPurpose")
        || descriptor_xml.contains("ObjectBelonging");
    let nature = if is_extension {
        V8_EXTENSION_NATURE
    } else {
        V8_CONFIGURATION_NATURE
    };
    let base_project_line = if is_extension {
        "Base-Project: BaseProject\n"
    } else {
        ""
    };
    fs::write(
        path.join(".project"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<projectDescription>\n  <name>{name}</name>\n  <natures>\n    <nature>{nature}</nature>\n  </natures>\n</projectDescription>\n"
        ),
    )
    .expect("project");
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

fn write_edt_external_project(path: &Path, name: &str) {
    fs::create_dir_all(path.join("DT-INF")).expect("dt-inf");
    fs::create_dir_all(path.join("src")).expect("src");
    fs::write(
        path.join(".project"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<projectDescription>\n  <name>{name}</name>\n  <natures>\n    <nature>{V8_EXTERNAL_OBJECTS_NATURE}</nature>\n  </natures>\n</projectDescription>\n"
        ),
    )
    .expect("project");
    fs::write(
        path.join("DT-INF").join("PROJECT.PMF"),
        format!(
            "Base-Project: BaseProject\nManifest-Version: 1.0\nRuntime-Version: {EDT_RUNTIME_VERSION}\n"
        ),
    )
    .expect("manifest");
    fs::write(
        path.join("src").join("root.xml"),
        format!(
            "<ExternalDataProcessor><Properties><Name>{name}</Name></Properties></ExternalDataProcessor>\n"
        ),
    )
    .expect("descriptor");
}

fn assert_native_edt_project(path: &Path) {
    assert!(path.join(".project").exists());
    assert!(path.join("DT-INF").join("PROJECT.PMF").exists());
    assert!(path.join("src/Configuration/Configuration.mdo").exists());
}

fn assert_native_edt_external_project(path: &Path) {
    assert!(path.join(".project").exists());
    assert!(path.join("DT-INF").join("PROJECT.PMF").exists());
    assert!(path.join("src").join("root.xml").exists());
}

#[test]
fn convert_without_source_set_processes_all_source_sets_into_work_path_out() {
    let (_dir, config_path, base_path, work_path, edt_cli_path, calls_log) = setup_project();
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &edt_cli_path,
        "DESIGNER",
        &[
            SourceSetSpec {
                name: "main",
                kind: "CONFIGURATION",
                path: "main",
            },
            SourceSetSpec {
                name: "ext-sales",
                kind: "EXTENSION",
                path: "ext-sales",
            },
        ],
        Some("8.3.24"),
    );
    write_designer_source(&base_path.join("main"), "BaseProject", false);
    write_designer_source(&base_path.join("ext-sales"), "SalesExtension", true);
    let stale_output = work_path.join("convert/out/main/edt/stale.txt");
    fs::create_dir_all(stale_output.parent().expect("parent")).expect("stale dir");
    fs::write(&stale_output, "stale").expect("stale file");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "convert",
        ])
        .output()
        .expect("run convert");

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "convert");
    assert_eq!(payload["data"]["direction"], "DESIGNER_TO_EDT");
    assert_eq!(payload["data"]["scope"], "ALL");
    assert_eq!(
        payload["data"]["outputs"]
            .as_array()
            .expect("outputs")
            .len(),
        2
    );
    assert_eq!(payload["data"]["outputs"][0]["source_set"], "main");
    assert_eq!(payload["data"]["outputs"][1]["source_set"], "ext-sales");

    let main_target = work_path.join("convert/out/main/edt");
    let extension_target = work_path.join("convert/out/ext-sales/edt");
    assert_native_edt_project(&main_target);
    assert_native_edt_project(&extension_target);
    assert!(!stale_output.exists());

    let calls = fs::read_to_string(calls_log).expect("calls");
    assert_eq!(calls.matches("-command import").count(), 2);
    assert!(calls.contains("--version 8.3.24"));
    assert!(calls.contains("--base-project-name BaseProject"));
    assert!(!calls.contains("--build true"));
}

#[test]
fn convert_single_source_set_uses_inferred_edt_to_designer_direction() {
    let (_dir, config_path, base_path, work_path, _edt_cli_path, calls_log) = setup_project();
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &_edt_cli_path,
        "EDT",
        &[
            SourceSetSpec {
                name: "main",
                kind: "CONFIGURATION",
                path: "main",
            },
            SourceSetSpec {
                name: "ext-sales",
                kind: "EXTENSION",
                path: "ext-sales",
            },
        ],
        None,
    );
    write_edt_source(
        &base_path.join("main"),
        "MainConfiguration",
        "<Configuration />",
    );
    write_edt_source(
        &base_path.join("ext-sales"),
        "SalesExtension",
        "<Configuration><ConfigurationExtensionPurpose>Extension</ConfigurationExtensionPurpose></Configuration>",
    );

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "convert",
            "--source-set",
            "main",
        ])
        .output()
        .expect("run convert");

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Convert completed successfully"));
    assert!(stdout.contains("direction: edt-to-designer"));
    assert!(stdout.contains("scope: source-set main"));
    assert!(stdout.contains(
        work_path
            .join("convert/out/main/designer")
            .display()
            .to_string()
            .as_str()
    ));

    let target = work_path.join("convert/out/main/designer");
    assert!(target.join("Configuration.xml").exists());

    let calls = fs::read_to_string(calls_log).expect("calls");
    assert_eq!(calls.matches("-command export").count(), 1);
    assert!(calls.contains(base_path.join("main").display().to_string().as_str()));
    assert!(!calls.contains(base_path.join("ext-sales").display().to_string().as_str()));
}

#[test]
fn convert_single_extension_source_set_infers_base_project_name_from_configuration_source() {
    let (_dir, config_path, base_path, work_path, edt_cli_path, calls_log) = setup_project();
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &edt_cli_path,
        "DESIGNER",
        &[
            SourceSetSpec {
                name: "main",
                kind: "CONFIGURATION",
                path: "main",
            },
            SourceSetSpec {
                name: "ext-sales",
                kind: "EXTENSION",
                path: "ext-sales",
            },
        ],
        Some("8.3.24"),
    );
    write_designer_source(&base_path.join("main"), "BaseProject", false);
    write_designer_source(&base_path.join("ext-sales"), "SalesExtension", true);

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "--log-level",
            "warn",
            "convert",
            "--source-set",
            "ext-sales",
        ])
        .output()
        .expect("run convert");

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("● convert: base project import"));
    assert!(!stdout.contains("started_at: "));
    assert!(stdout.contains("[EDT] importing Designer files for base project name"));
    assert!(
        stdout
            .find("convert: base project import")
            .expect("base project stage")
            < stdout
                .find("convert: designer import")
                .expect("extension import stage")
    );
    assert!(stdout.contains("● Convert completed successfully"));

    let target = work_path.join("convert/out/ext-sales/edt");
    assert_native_edt_project(&target);

    let calls = fs::read_to_string(calls_log).expect("calls");
    assert_eq!(calls.matches("-command import").count(), 2);
    assert!(calls.contains(base_path.join("main").display().to_string().as_str()));
    assert!(calls.contains("--base-project-name BaseProject"));
    assert!(calls.contains("--version 8.3.24"));
}

#[test]
fn convert_unknown_source_set_json_keeps_convert_command_identity_before_workspace_lock() {
    let (_dir, config_path, base_path, work_path, edt_cli_path, _calls_log) = setup_project();
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &edt_cli_path,
        "DESIGNER",
        &[SourceSetSpec {
            name: "main",
            kind: "CONFIGURATION",
            path: "main",
        }],
        None,
    );
    write_designer_source(&base_path.join("main"), "BaseProject", false);
    write_live_workspace_lock(&work_path, "convert");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "convert",
            "--source-set",
            "missing",
        ])
        .output()
        .expect("run convert");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "convert");
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("unknown source-set 'missing'"));
}

#[test]
fn convert_workspace_lock_conflict_uses_runtime_error_after_valid_preflight() {
    let (_dir, config_path, base_path, work_path, edt_cli_path, _calls_log) = setup_project();
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &edt_cli_path,
        "DESIGNER",
        &[SourceSetSpec {
            name: "main",
            kind: "CONFIGURATION",
            path: "main",
        }],
        None,
    );
    write_designer_source(&base_path.join("main"), "BaseProject", false);
    write_live_workspace_lock(&work_path, "convert");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "convert",
        ])
        .output()
        .expect("run convert");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ERROR: runtime error: cannot start convert"),
        "stderr:\n{stderr}"
    );
}

#[test]
fn convert_external_edt_source_set_preserves_all_exported_descriptors() {
    let (_dir, config_path, base_path, work_path, edt_cli_path, calls_log) = setup_project();
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &edt_cli_path,
        "EDT",
        &[SourceSetSpec {
            name: "processors",
            kind: "EXTERNAL_DATA_PROCESSORS",
            path: "processors",
        }],
        None,
    );
    write_edt_external_project(&base_path.join("processors/processor-a"), "ProcessorA");
    write_edt_external_project(&base_path.join("processors/processor-b"), "ProcessorB");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "convert",
            "--source-set",
            "processors",
        ])
        .output()
        .expect("run convert");

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "convert");
    assert_eq!(payload["data"]["direction"], "EDT_TO_DESIGNER");
    assert_eq!(payload["data"]["scope"], "SINGLE");

    let target = work_path.join("convert/out/processors/designer");
    assert!(target.join("processor-a.xml").exists());
    assert!(target.join("processor-b.xml").exists());

    let calls = fs::read_to_string(calls_log).expect("calls");
    assert_eq!(calls.matches("-command export").count(), 2);
}

#[test]
fn convert_external_designer_source_set_does_not_require_configuration_source_set() {
    let (_dir, config_path, base_path, work_path, edt_cli_path, calls_log) = setup_project();
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &edt_cli_path,
        "DESIGNER",
        &[SourceSetSpec {
            name: "processors",
            kind: "EXTERNAL_DATA_PROCESSORS",
            path: "processors",
        }],
        None,
    );
    write_designer_external_source(
        &base_path.join("processors"),
        &["processor-a", "processor-b"],
    );

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "convert",
        ])
        .output()
        .expect("run convert");

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "convert");
    assert_eq!(payload["data"]["direction"], "DESIGNER_TO_EDT");
    assert_eq!(payload["data"]["scope"], "ALL");

    let target = work_path.join("convert/out/processors/edt");
    assert_native_edt_external_project(&target.join("processor-a"));
    assert_native_edt_external_project(&target.join("processor-b"));

    let calls = fs::read_to_string(calls_log).expect("calls");
    assert_eq!(calls.matches("-command import").count(), 1);
    assert!(!calls.contains("--base-project-name"));
}

#[test]
fn convert_output_root_mirrors_source_set_layout_and_stabilizes_edt_project_names() {
    let dir = temp_workspace();
    let base_path = dir.path().join("designer");
    let work_path = dir.path().join("work");
    let output_root = dir.path().join("edt");
    let config_path = base_path.join("v8project.yaml");
    let edt_cli_path = dir.path().join("edt-cli").join("1cedtcli");
    let calls_log = dir.path().join("edt-calls.log");

    fs::create_dir_all(&base_path).expect("base");
    fs::create_dir_all(&work_path).expect("work");
    write_path_named_edt_import_script(&edt_cli_path, &calls_log);
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &edt_cli_path,
        "DESIGNER",
        &[
            SourceSetSpec {
                name: "configuration",
                kind: "CONFIGURATION",
                path: "configuration",
            },
            SourceSetSpec {
                name: "extension",
                kind: "EXTENSION",
                path: "extension",
            },
            SourceSetSpec {
                name: "processors",
                kind: "EXTERNAL_DATA_PROCESSORS",
                path: "external/processor",
            },
        ],
        None,
    );
    write_designer_source(&base_path.join("configuration"), "BaseProject", false);
    write_designer_source(&base_path.join("extension"), "SalesExtension", true);
    write_designer_external_source(
        &base_path.join("external/processor"),
        &["processor-a", "processor-b"],
    );
    let stale_file = output_root.join("configuration").join("stale.txt");
    fs::create_dir_all(stale_file.parent().expect("stale parent")).expect("stale dir");
    fs::write(&stale_file, "stale").expect("stale");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "convert",
            "--output",
            &output_root.display().to_string(),
        ])
        .output()
        .expect("run convert");

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "convert");
    assert_eq!(payload["data"]["direction"], "DESIGNER_TO_EDT");
    assert_eq!(payload["data"]["scope"], "ALL");

    let configuration_target = output_root.join("configuration");
    let extension_target = output_root.join("extension");
    let processors_target = output_root.join("external").join("processor");
    assert_native_edt_project(&configuration_target);
    assert_native_edt_project(&extension_target);
    assert_native_edt_external_project(&processors_target.join("processor-a"));
    assert_native_edt_external_project(&processors_target.join("processor-b"));
    assert!(!stale_file.exists());

    let configuration_project =
        fs::read_to_string(configuration_target.join(".project")).expect("configuration project");
    let extension_project =
        fs::read_to_string(extension_target.join(".project")).expect("extension project");
    let extension_manifest = fs::read_to_string(extension_target.join("DT-INF/PROJECT.PMF"))
        .expect("extension manifest");
    assert!(configuration_project.contains("<name>configuration</name>"));
    assert!(extension_project.contains("<name>extension</name>"));
    assert!(extension_manifest.contains("Base-Project: configuration"));

    assert_eq!(
        payload["data"]["outputs"][0]["target_path"],
        configuration_target.display().to_string()
    );
    assert_eq!(
        payload["data"]["outputs"][1]["target_path"],
        extension_target.display().to_string()
    );
    assert_eq!(
        payload["data"]["outputs"][2]["target_path"],
        processors_target.display().to_string()
    );

    let calls = fs::read_to_string(calls_log).expect("calls");
    assert_eq!(calls.matches("-command import").count(), 3);
    assert!(calls.contains("--base-project-name configuration"));
}

#[test]
fn convert_output_root_rejects_source_overlap_before_workspace_lock() {
    let (_dir, config_path, base_path, work_path, edt_cli_path, _calls_log) = setup_project();
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &edt_cli_path,
        "DESIGNER",
        &[SourceSetSpec {
            name: "main",
            kind: "CONFIGURATION",
            path: "main",
        }],
        None,
    );
    write_designer_source(&base_path.join("main"), "BaseProject", false);
    write_live_workspace_lock(&work_path, "convert");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "convert",
            "--output",
            &base_path.display().to_string(),
        ])
        .output()
        .expect("run convert");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "convert");
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("overlaps source-set 'main' path"));
}

#[test]
fn convert_single_source_output_rejects_unselected_source_set_overlap() {
    let (_dir, config_path, base_path, work_path, edt_cli_path, _calls_log) = setup_project();
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &edt_cli_path,
        "DESIGNER",
        &[
            SourceSetSpec {
                name: "main",
                kind: "CONFIGURATION",
                path: "main",
            },
            SourceSetSpec {
                name: "ext",
                kind: "EXTENSION",
                path: "ext",
            },
        ],
        None,
    );
    write_designer_source(&base_path.join("main"), "BaseProject", false);
    write_designer_source(&base_path.join("ext"), "SalesExtension", true);
    write_live_workspace_lock(&work_path, "convert");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "convert",
            "--source-set",
            "main",
            "--output",
            &base_path.join("ext").display().to_string(),
        ])
        .output()
        .expect("run convert");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "convert");
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("overlaps source-set 'ext' path"));
}

#[test]
fn convert_output_root_rejects_base_path_child_before_workspace_lock() {
    let (_dir, config_path, base_path, work_path, edt_cli_path, _calls_log) = setup_project();
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &edt_cli_path,
        "DESIGNER",
        &[SourceSetSpec {
            name: "main",
            kind: "CONFIGURATION",
            path: "main",
        }],
        None,
    );
    write_designer_source(&base_path.join("main"), "BaseProject", false);
    write_live_workspace_lock(&work_path, "convert");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "convert",
            "--output",
            &base_path.join("generated").display().to_string(),
        ])
        .output()
        .expect("run convert");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "convert");
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("must not be inside project base path"));
}

#[test]
fn convert_output_root_rejects_work_path_child_before_workspace_lock() {
    let (_dir, config_path, base_path, work_path, edt_cli_path, _calls_log) = setup_project();
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &edt_cli_path,
        "DESIGNER",
        &[SourceSetSpec {
            name: "main",
            kind: "CONFIGURATION",
            path: "main",
        }],
        None,
    );
    write_designer_source(&base_path.join("main"), "BaseProject", false);
    write_live_workspace_lock(&work_path, "convert");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "convert",
            "--output",
            &work_path
                .join("convert")
                .join("edt-workspace")
                .display()
                .to_string(),
        ])
        .output()
        .expect("run convert");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "convert");
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("must not be inside workPath"));
}

#[test]
fn convert_output_root_rejects_filesystem_root_before_workspace_lock() {
    let (_dir, config_path, base_path, work_path, edt_cli_path, _calls_log) = setup_project();
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &edt_cli_path,
        "DESIGNER",
        &[SourceSetSpec {
            name: "main",
            kind: "CONFIGURATION",
            path: "main",
        }],
        None,
    );
    write_designer_source(&base_path.join("main"), "BaseProject", false);
    write_live_workspace_lock(&work_path, "convert");
    let root_output = std::path::MAIN_SEPARATOR.to_string();

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "convert",
            "--output",
            &root_output,
        ])
        .output()
        .expect("run convert");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "convert");
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("filesystem root"));
}

#[test]
fn convert_output_root_rejects_overlapping_targets_before_workspace_lock() {
    let (_dir, config_path, base_path, work_path, edt_cli_path, _calls_log) = setup_project();
    write_config(
        &config_path,
        &base_path,
        &work_path,
        &edt_cli_path,
        "DESIGNER",
        &[
            SourceSetSpec {
                name: "main",
                kind: "CONFIGURATION",
                path: ".",
            },
            SourceSetSpec {
                name: "nested-ext",
                kind: "EXTENSION",
                path: "nested",
            },
        ],
        None,
    );
    write_designer_source(&base_path, "BaseProject", false);
    write_designer_source(&base_path.join("nested"), "NestedExtension", true);
    write_live_workspace_lock(&work_path, "convert");
    let output_root = base_path.parent().expect("parent").join("converted");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--json-message",
            "convert",
            "--output",
            &output_root.display().to_string(),
        ])
        .output()
        .expect("run convert");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "convert");
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("output targets overlap"));
}
