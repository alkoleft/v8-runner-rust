mod guardrail_support;

use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};

use guardrail_support::{
    collect_rust_files, free_function_tokens, production_tokens, trait_impl_method_tokens,
};

const EXPECTED_MCP_TOOLS: &[&str] = &[
    "run_all_tests",
    "run_module_tests",
    "build_project",
    "dump_config",
    "launch_app",
    "check_syntax_edt",
    "check_syntax_designer_config",
    "check_syntax_designer_modules",
];

const FORBIDDEN_PROCESS_PATTERNS: &[&str] = &[
    "std::process::Command",
    "tokio::process::Command",
    "usestd::process::Command",
    "usestd::process::{Command",
    "usestd::process::Stdio",
    "usestd::process::{Stdio",
    "usestd::process::Child",
    "usestd::process::{Child",
    "usestd::process::ExitStatus",
    "usestd::process::{ExitStatus",
    "Command::new(",
    "Stdio::",
];

fn repo_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn read(relative: &str) -> String {
    fs::read_to_string(repo_path(relative)).expect("read repository file")
}

fn extract_between<'a>(contents: &'a str, start_marker: &str, end_marker: &str) -> &'a str {
    let start = contents
        .find(start_marker)
        .unwrap_or_else(|| panic!("missing marker: {start_marker}"));
    let tail = &contents[start..];
    let end = tail
        .find(end_marker)
        .unwrap_or_else(|| panic!("missing marker: {end_marker}"));
    &tail[..end]
}

fn extract_backticked_items(section: &str) -> Vec<String> {
    let regex = Regex::new(r"`([^`]+)`").expect("regex");
    regex
        .captures_iter(section)
        .map(|capture| capture[1].to_owned())
        .collect()
}

#[test]
fn raw_process_spawn_apis_stay_inside_platform_layer() {
    let root = repo_path("src");
    let files = collect_rust_files(&root);
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_main = Path::new("src").join("main.rs");
    let src_platform = Path::new("src").join("platform");

    for file in files {
        let relative = file.strip_prefix(repo_root).expect("relative path");
        if relative == src_main || relative.starts_with(&src_platform) {
            continue;
        }

        let production = production_tokens(&file);
        for forbidden in FORBIDDEN_PROCESS_PATTERNS {
            assert!(
                !production.contains(forbidden),
                "{} must keep raw process API '{}' inside src/platform",
                relative.display(),
                forbidden
            );
        }
    }
}

#[test]
fn mcp_surface_snapshot_stays_explicit_and_documented() {
    let source = read("src/mcp/server.rs");
    let source_section = extract_between(
        &source,
        "const fn as_str(self) -> &'static str {",
        "fn execution_policy",
    );
    let source_tools = Regex::new(r#""([a-z_]+)""#)
        .expect("regex")
        .captures_iter(source_section)
        .map(|capture| capture[1].to_owned())
        .collect::<Vec<_>>();

    let invariants = read("spec/architecture/invariants.md");
    let invariants_section = extract_between(
        &invariants,
        "3. Текущая MCP-поверхность состоит из 8 tool-операций:",
        "4. Добавление",
    );
    let invariants_tools = extract_backticked_items(invariants_section);

    let adr = read("spec/decisions/0005-razdelit-cli-i-mcp-publichnye-poverhnosti.md");
    let adr_section = extract_between(
        &adr,
        "2. Текущая MCP-поверхность состоит ровно из 8 опубликованных tool-операций:",
        "3. CLI может иметь команды, не опубликованные в MCP.",
    );
    let adr_tools = extract_backticked_items(adr_section);

    let expected = EXPECTED_MCP_TOOLS
        .iter()
        .map(|tool| (*tool).to_owned())
        .collect::<Vec<_>>();

    assert_eq!(source_tools, expected);
    assert_eq!(invariants_tools, expected);
    assert_eq!(adr_tools, expected);
}

#[test]
fn public_command_adapters_keep_workspace_lock_boundary() {
    for function in [
        "execute_extensions",
        "execute_init",
        "execute_build",
        "execute_test",
        "execute_load",
        "execute_dump",
        "execute_convert",
        "execute_artifacts",
        "execute_syntax",
        "execute_launch",
    ] {
        let window = free_function_tokens(repo_path("src/cli/execute.rs").as_path(), function);
        assert!(
            window.contains("with_cli_workspace_lock("),
            "{function} must keep the CLI workspace-lock boundary"
        );
    }

    for function in [
        "build_project",
        "run_tests",
        "dump_config",
        "launch_app",
        "check_syntax",
    ] {
        let window = trait_impl_method_tokens(
            repo_path("src/mcp/port.rs").as_path(),
            "McpUseCasePort",
            "DefaultMcpUseCasePort",
            function,
        );
        assert!(
            window.contains("with_workspace_lock("),
            "{function} must keep the MCP workspace-lock boundary"
        );
    }
}

#[test]
fn change_checklist_covers_mcp_workspace_lock_and_config_contract() {
    let checklist = read("spec/architecture/change-checklist.md");
    for required in [
        "## Изменение MCP public surface",
        "## Новая public CLI/MCP команда, работающая с `workPath`",
        "## Новый public config field, `source-set` type или `infobase` subtree",
        "src/config/model.rs",
        "src/config/validate.rs",
        "spec/architecture/invariants.md",
    ] {
        assert!(
            checklist.contains(required),
            "checklist must mention '{required}'"
        );
    }
}

#[test]
fn windows_runner_log_materialization_keeps_reparse_and_atomic_replace_guards() {
    let source = read("src/use_cases/run_tests.rs");
    let open = extract_between(&source, "fn open_file_no_follow", "fn create_private_file");
    for required in [
        "FILE_FLAG_OPEN_REPARSE_POINT",
        "GetFileInformationByHandle",
        "FILE_ATTRIBUTE_REPARSE_POINT",
    ] {
        assert!(
            open.contains(required),
            "Windows no-follow open must use '{required}'"
        );
    }

    let replace = extract_between(&source, "fn replace_file", "#[cfg(test)]");
    for required in [
        "MoveFileExW",
        "MOVEFILE_REPLACE_EXISTING",
        "MOVEFILE_WRITE_THROUGH",
        "fs::canonicalize(parent)",
    ] {
        assert!(
            replace.contains(required),
            "Windows atomic replacement must use '{required}'"
        );
    }
    assert!(
        !replace.contains("fs::remove_file(destination)"),
        "Windows replacement must not expose a remove-then-rename gap"
    );
}
