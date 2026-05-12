#![cfg(unix)]

mod support;

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use support::{temp_workspace, v8_runner_command};

const V8_EXTERNAL_OBJECTS_NATURE: &str = "com._1c.g5.v8.dt.core.V8ExternalObjectsNature";
const LOCAL_CONFIG_SCHEMA_MODEL_LINE: &str = "# yaml-language-server: $schema=https://raw.githubusercontent.com/alkoleft/v8-runner-rust/master/docs/schemas/v8project.local.schema.json";

fn copy_dir_all(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create dst");
    for entry in fs::read_dir(src).expect("read dir") {
        let entry = entry.expect("entry");
        let path = entry.path();
        let target = dst.join(entry.file_name());
        let file_type = entry.file_type().expect("file type");
        if file_type.is_dir() {
            copy_dir_all(&path, &target);
        } else {
            fs::copy(&path, &target).expect("copy file");
        }
    }
}

fn edt_fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("edt")
}

fn copy_native_edt_fixture(dest_root: &Path) {
    let fixture_root = edt_fixture_root();
    copy_dir_all(
        &fixture_root.join("configuration"),
        &dest_root.join("configuration"),
    );
    copy_dir_all(
        &fixture_root.join("extension"),
        &dest_root.join("extension"),
    );
}

fn create_native_edt_external_project(project_dir: &Path, name: &str, descriptor_xml: &str) {
    fs::create_dir_all(project_dir.join("DT-INF")).expect("dt-inf");
    fs::create_dir_all(project_dir.join("src")).expect("src");
    fs::write(
        project_dir.join(".project"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<projectDescription>\n  <name>{name}</name>\n  <natures>\n    <nature>{V8_EXTERNAL_OBJECTS_NATURE}</nature>\n  </natures>\n</projectDescription>\n"
        ),
    )
    .expect("project");
    fs::write(
        project_dir.join("DT-INF").join("PROJECT.PMF"),
        "Base-Project: configuration\nManifest-Version: 1.0\nRuntime-Version: 8.3.27\n",
    )
    .expect("manifest");
    fs::write(project_dir.join("src").join("root.xml"), descriptor_xml).expect("descriptor");
}

#[test]
fn config_init_creates_yaml_with_detected_designer_sources() {
    let dir = temp_workspace();
    let main = dir.path().join("src").join("configuration");
    let ext = dir.path().join("extensions").join("sales");
    fs::create_dir_all(&main).expect("main");
    fs::create_dir_all(&ext).expect("ext");
    fs::write(main.join("Configuration.xml"), "<Configuration/>").expect("main xml");
    fs::write(
        ext.join("Configuration.xml"),
        "<Configuration><Properties><Name>SalesAddon</Name><ConfigurationExtensionPurpose kind=\"Customization\">Customization</ConfigurationExtensionPurpose></Properties></Configuration>",
    )
    .expect("ext xml");

    let output = v8_runner_command()
        .current_dir(dir.path())
        .args(["config", "init"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let config = fs::read_to_string(dir.path().join("v8project.yaml")).expect("config");
    assert!(config.starts_with(
        "# yaml-language-server: $schema=https://raw.githubusercontent.com/alkoleft/v8-runner-rust/master/docs/schemas/v8project.schema.json\n"
    ));
    serde_yaml::from_str::<serde_yaml::Value>(&config).expect("generated config remains YAML");
    assert!(config.contains("format: DESIGNER"));
    assert!(!config.contains("basePath:"));
    assert!(config.contains("workPath: 'build'"));
    assert!(config.contains("infobase:"));
    assert!(config.contains("  connection: 'File=build/ib'"));
    assert!(config.contains("path: 'src/configuration'"));
    assert!(config.contains("name: 'SalesAddon'"));
    assert!(config.contains("type: EXTENSION"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Config written"));
    let local_config =
        fs::read_to_string(dir.path().join("v8project.local.yaml")).expect("local config");
    assert!(local_config.starts_with(LOCAL_CONFIG_SCHEMA_MODEL_LINE));
    serde_yaml::from_str::<serde_yaml::Value>(&local_config)
        .expect("generated local config remains YAML");
    let gitignore = fs::read_to_string(dir.path().join(".gitignore")).expect("gitignore");
    assert!(gitignore.lines().any(|line| line == "v8project.local.yaml"));
}

#[test]
fn config_init_uses_json_envelope_and_output_override() {
    let dir = temp_workspace();
    fs::write(dir.path().join("Configuration.xml"), "<Configuration/>").expect("xml");
    let config_path = dir.path().join("custom.yaml");

    let output = v8_runner_command()
        .current_dir(dir.path())
        .args([
            "--json-message",
            "config",
            "init",
            "--output",
            &config_path.display().to_string(),
            "--connection",
            "File=/tmp/test-ib",
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    assert!(config_path.exists());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "config init");
    assert_eq!(
        payload["data"]["local_path"],
        dir.path()
            .join("v8project.local.yaml")
            .display()
            .to_string()
    );
    assert_eq!(
        payload["data"]["gitignore_path"],
        dir.path().join(".gitignore").display().to_string()
    );
    assert_eq!(payload["data"]["source_sets"][0]["path"], ".");
    assert_eq!(payload["data"]["source_sets"][0]["type"], "CONFIGURATION");
    let config = fs::read_to_string(config_path).expect("config");
    assert!(config.contains("infobase:"));
    assert!(config.contains("  connection: 'File=/tmp/test-ib'"));
    assert!(!config.contains("basePath:"));
}

#[test]
fn config_init_creates_local_overlay_next_to_output_override() {
    let dir = temp_workspace();
    fs::write(dir.path().join("Configuration.xml"), "<Configuration/>").expect("xml");

    let output = v8_runner_command()
        .current_dir(dir.path())
        .args(["config", "init", "--output", "config/v8project.yaml"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let config =
        fs::read_to_string(dir.path().join("config").join("v8project.yaml")).expect("config");
    assert!(!config.contains("basePath:"));
    assert!(config.contains("path: '..'"));
    let local_config = fs::read_to_string(dir.path().join("config").join("v8project.local.yaml"))
        .expect("local config");
    assert!(local_config.starts_with(LOCAL_CONFIG_SCHEMA_MODEL_LINE));
    let gitignore =
        fs::read_to_string(dir.path().join("config").join(".gitignore")).expect("gitignore");
    assert!(gitignore.lines().any(|line| line == "v8project.local.yaml"));
}

#[test]
fn config_init_rejects_global_config_shortcut_in_text_mode() {
    let dir = temp_workspace();
    fs::write(dir.path().join("Configuration.xml"), "<Configuration/>").expect("xml");

    let output = v8_runner_command()
        .current_dir(dir.path())
        .args(["--config", "custom.yaml", "config", "init"])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains(
        "global --config flag is not supported for `config init`; use `config init --output <FILE>`"
    ));
}

#[test]
fn config_init_rejects_global_config_shortcut_in_json_mode() {
    let dir = temp_workspace();
    fs::write(dir.path().join("Configuration.xml"), "<Configuration/>").expect("xml");

    let output = v8_runner_command()
        .current_dir(dir.path())
        .args([
            "--config",
            "custom.yaml",
            "--json-message",
            "config",
            "init",
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "config init");
    assert_eq!(payload["error"]["code"], "invalid_argument");
    assert_eq!(payload["error"]["kind"], "validation");
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("use `config init --output <FILE>`"));
}

#[test]
fn config_init_ignores_v8tr_config_env_for_output_path_selection() {
    let dir = temp_workspace();
    fs::write(dir.path().join("Configuration.xml"), "<Configuration/>").expect("xml");

    let output = v8_runner_command()
        .current_dir(dir.path())
        .env("V8TR_CONFIG", dir.path().join("existing.yaml"))
        .args(["config", "init"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    assert!(dir.path().join("v8project.yaml").exists());
    assert!(!dir.path().join("existing.yaml").exists());
}

#[test]
fn config_init_detects_native_edt_fixture_source_sets() {
    let dir = temp_workspace();
    let workspace = dir.path().join("workspace");
    copy_native_edt_fixture(&workspace);

    let output = v8_runner_command()
        .current_dir(dir.path())
        .args(["config", "init"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let config = fs::read_to_string(dir.path().join("v8project.yaml")).expect("config");
    assert!(config.contains("format: EDT"));
    assert!(config.contains("tools:\n  platform:\n    version: '8.3.27'"));
    assert!(config.contains("path: 'workspace/configuration'"));
    assert!(config.contains("path: 'workspace/extension'"));
    assert!(config.contains("name: 'Расширение1'"));
    assert!(config.contains("type: CONFIGURATION"));
    assert!(config.contains("type: EXTENSION"));
}

#[test]
fn config_init_detects_edt_extension_without_base_project_and_warns() {
    let dir = temp_workspace();
    let workspace = dir.path().join("workspace");
    copy_native_edt_fixture(&workspace);
    fs::write(
        workspace
            .join("extension")
            .join("DT-INF")
            .join("PROJECT.PMF"),
        "Manifest-Version: 1.0\nRuntime-Version: 8.3.27\n",
    )
    .expect("manifest");

    let output = v8_runner_command()
        .current_dir(dir.path())
        .args(["--no-color", "config", "init"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("source-set Расширение1: workspace/extension (EXTENSION)"));
    assert!(stdout.contains("platform version: 8.3.27"));
    assert!(stdout.contains("[warning] EDT extension source-set 'Расширение1'"));
    assert!(stdout.contains("Base-Project"));
    assert!(stdout.contains("Config written with warnings"));

    let config = fs::read_to_string(dir.path().join("v8project.yaml")).expect("config");
    assert!(config.contains("tools:\n  platform:\n    version: '8.3.27'"));
    assert!(config.contains("name: 'Расширение1'"));
    assert!(config.contains("path: 'workspace/extension'"));
    assert!(config.contains("type: EXTENSION"));

    let json_output = v8_runner_command()
        .current_dir(dir.path())
        .args([
            "--json-message",
            "config",
            "init",
            "--force",
            "--output",
            "json-v8project.yaml",
        ])
        .output()
        .expect("run json command");

    assert!(json_output.status.success());
    let payload: Value = serde_json::from_slice(&json_output.stdout).expect("json");
    assert_eq!(payload["data"]["platform_version"], "8.3.27");
    let source_sets = payload["data"]["source_sets"]
        .as_array()
        .expect("source sets");
    assert!(source_sets.iter().any(|source_set| {
        source_set["name"] == "Расширение1"
            && source_set["path"] == "workspace/extension"
            && source_set["type"] == "EXTENSION"
    }));
    assert!(payload["data"]["warnings"][0]
        .as_str()
        .expect("warning")
        .contains("Base-Project"));
    assert!(payload["warnings"][0]
        .as_str()
        .expect("envelope warning")
        .contains("Base-Project"));
}

#[test]
fn config_init_refuses_to_overwrite_without_force() {
    let dir = temp_workspace();
    fs::write(dir.path().join("v8project.yaml"), "existing").expect("existing");

    let output = v8_runner_command()
        .current_dir(dir.path())
        .args(["config", "init"])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("already exists"));

    let json_output = v8_runner_command()
        .current_dir(dir.path())
        .args(["--json-message", "config", "init"])
        .output()
        .expect("run json command");

    assert!(!json_output.status.success());
    assert_eq!(json_output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&json_output.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "config init");
    assert_eq!(payload["error"]["code"], "invalid_argument");
    assert!(payload["data"]["message"]
        .as_str()
        .expect("message")
        .contains("already exists"));
}

#[test]
fn config_init_detects_designer_external_aggregate_source_set() {
    let dir = temp_workspace();
    fs::write(dir.path().join("Configuration.xml"), "<Configuration/>").expect("config xml");
    fs::create_dir_all(dir.path().join("tools")).expect("tools dir");
    fs::write(
        dir.path().join("tools").join("alpha.xml"),
        "<ExternalDataProcessor><Properties><Name>Alpha</Name></Properties></ExternalDataProcessor>",
    )
    .expect("alpha xml");
    fs::write(
        dir.path().join("tools").join("beta.xml"),
        "<MetaDataObject><ExternalDataProcessor><Properties><Name>Beta</Name></Properties></ExternalDataProcessor></MetaDataObject>",
    )
    .expect("beta xml");

    let output = v8_runner_command()
        .current_dir(dir.path())
        .args(["config", "init", "--format", "designer"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let config = fs::read_to_string(dir.path().join("v8project.yaml")).expect("config");
    assert!(config.contains("type: EXTERNAL_DATA_PROCESSORS"));
    assert!(config.contains("path: 'tools'"));
}

#[test]
fn config_init_rejects_external_only_autodiscovery_without_configuration() {
    let dir = temp_workspace();
    fs::create_dir_all(dir.path().join("tools")).expect("tools dir");
    fs::write(
        dir.path().join("tools").join("alpha.xml"),
        "<ExternalDataProcessor><Properties><Name>Alpha</Name></Properties></ExternalDataProcessor>",
    )
    .expect("alpha xml");

    let output = v8_runner_command()
        .current_dir(dir.path())
        .args(["config", "init", "--format", "designer"])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("did not find a CONFIGURATION source-set")
    );
}

#[test]
fn config_init_auto_prefers_edt_when_designer_only_has_external_root() {
    let dir = temp_workspace();
    let workspace = dir.path().join("workspace");
    copy_dir_all(
        &edt_fixture_root().join("configuration"),
        &workspace.join("configuration"),
    );
    fs::create_dir_all(dir.path().join("tools")).expect("tools dir");
    fs::write(
        dir.path().join("tools").join("alpha.xml"),
        "<ExternalDataProcessor><Properties><Name>Alpha</Name></Properties></ExternalDataProcessor>",
    )
    .expect("alpha xml");

    let output = v8_runner_command()
        .current_dir(dir.path())
        .args(["config", "init"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let config = fs::read_to_string(dir.path().join("v8project.yaml")).expect("config");
    assert!(config.contains("format: EDT"));
    assert!(config.contains("path: 'workspace/configuration'"));
    assert!(!config.contains("path: 'tools'"));
}

#[test]
fn config_init_keeps_nested_edt_configuration_under_external_root() {
    let dir = temp_workspace();
    let external_root = dir.path().join("processors");
    for name in ["alpha", "beta"] {
        let project = external_root.join(name);
        create_native_edt_external_project(
            &project,
            name,
            &format!(
                "<ExternalDataProcessor><Properties><Name>{name}</Name></Properties></ExternalDataProcessor>"
            ),
        )
    }
    let config_project = external_root.join("apps").join("cfg");
    copy_dir_all(&edt_fixture_root().join("configuration"), &config_project);

    let output = v8_runner_command()
        .current_dir(dir.path())
        .args(["config", "init", "--format", "edt"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let config = fs::read_to_string(dir.path().join("v8project.yaml")).expect("config");
    assert!(config.contains("path: 'processors'"));
    assert!(config.contains("type: EXTERNAL_DATA_PROCESSORS"));
    assert!(config.contains("path: 'processors/apps/cfg'"));
    assert!(config.contains("type: CONFIGURATION"));
}

#[test]
fn config_init_ignores_non_edt_root_project_marker_when_nested_project_exists() {
    let dir = temp_workspace();
    fs::write(dir.path().join(".project"), "<root/>").expect("root project marker");
    let workspace = dir.path().join("workspace");
    copy_dir_all(
        &edt_fixture_root().join("configuration"),
        &workspace.join("configuration"),
    );

    let output = v8_runner_command()
        .current_dir(dir.path())
        .args(["config", "init", "--format", "edt"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let config = fs::read_to_string(dir.path().join("v8project.yaml")).expect("config");
    assert!(config.contains("path: 'workspace/configuration'"));
    assert!(config.contains("type: CONFIGURATION"));
}
