#![cfg(unix)]

mod support;

use support::v8_runner_command;

#[test]
fn root_help_splits_commands_and_global_options() {
    let output = v8_runner_command()
        .args(["--help"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Commands:"));
    assert!(stdout.contains("Global options:"));
    assert!(stdout.contains("Build configured source-sets into the infobase"));
    assert!(stdout.contains("--json-message"));
}

#[test]
fn config_init_help_separates_global_and_command_options() {
    let output = v8_runner_command()
        .args(["config", "init", "--help"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Command options:"));
    assert!(stdout.contains("Global options:"));
    assert!(stdout.contains("--output <OUTPUT>"));
    assert!(!stdout.contains("--file <FILE>"));
    assert!(stdout.contains("--json-message"));
}

#[test]
fn build_help_exposes_source_set_selector() {
    let output = v8_runner_command()
        .args(["build", "--help"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Command options:"));
    assert!(stdout.contains("--source-set <SOURCE_SET>"));
    assert!(stdout.contains("--full-rebuild"));
    assert!(stdout.contains("--json-message"));
}

#[test]
fn tools_download_help_exposes_tool_commands() {
    let output = v8_runner_command()
        .args(["tools", "download", "--help"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Global options:"));
    assert!(stdout.contains("Commands:"));
    assert!(stdout.contains("yaxunit"));
    assert!(stdout.contains("vanessa"));
    assert!(stdout.contains("client-mcp"));
    assert!(!stdout.contains("--extensions"));
}

#[test]
fn tools_download_extension_help_exposes_sources_flag() {
    let output = v8_runner_command()
        .args(["tools", "download", "yaxunit", "--help"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Command options:"));
    assert!(stdout.contains("--sources"));
    assert!(stdout.contains("--force"));
}

#[test]
fn launch_help_uses_output_path_name_and_global_json_selector() {
    let output = v8_runner_command()
        .args(["launch", "--help"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Command options:"));
    assert!(stdout.contains("Global options:"));
    assert!(stdout.contains("--output <OUTPUT>"));
    assert!(!stdout.contains("--out <OUT>"));
    assert!(!stdout.contains("--mode <MODE>"));
    assert!(stdout.contains("--json-message"));
}

#[test]
fn make_help_keeps_output_path_under_command_options() {
    let output = v8_runner_command()
        .args(["make", "--help"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Command options:"));
    assert!(stdout.contains("Global options:"));
    assert!(stdout.contains("--output <OUTPUT>"));
    assert!(stdout.contains("--json-message"));
}

#[test]
fn convert_help_uses_output_target_root_name() {
    let output = v8_runner_command()
        .args(["convert", "--help"])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Command options:"));
    assert!(stdout.contains("Global options:"));
    assert!(stdout.contains("--output <OUTPUT>"));
    assert!(stdout.contains("--source-set <SOURCE_SET>"));
    assert!(stdout.contains("--json-message"));
}
