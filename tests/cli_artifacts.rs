#![cfg(unix)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use support::{temp_workspace, v8_runner_command, write_shell_script as write_script};

fn write_designer_script(path: &Path, fail: bool) {
    let failure_branch = if fail { "exit 17" } else { "exit 0" };
    write_script(
        path,
        &format!(
            "out=''\nprev=''\nfor arg in \"$@\"; do\n  if [ \"$prev\" = '/DumpCfg' ]; then printf 'cf' > \"$arg\"; fi\n  if [ \"$prev\" = '/Out' ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -n \"$out\" ]; then printf 'designer log' > \"$out\"; fi\n{failure_branch}"
        ),
    );
}

fn write_config(path: &Path, work_path: &Path, platform_path: &Path) {
    let config = format!(
        "workPath: '{}'\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=/tmp/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: project/main\ntools:\n  platform:\n    path: '{}'\n",
        work_path.display(),
        platform_path.display(),
    );
    fs::write(path, config).expect("config");
}

fn setup_project(fail: bool) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = temp_workspace();
    let base_path = dir.path().join("project");
    let work_path = dir.path().join("work");
    let config_path = dir.path().join("v8project.yaml");
    let binary_path = dir.path().join("1cv8");

    fs::create_dir_all(base_path.join("main")).expect("main");
    fs::create_dir_all(&work_path).expect("work");
    write_designer_script(&binary_path, fail);
    write_config(&config_path, &work_path, &binary_path);

    (dir, config_path, base_path)
}

#[test]
fn artifacts_text_success_keeps_output_artifact_visible() {
    let (_dir, config_path, base_path) = setup_project(false);
    let output_path = base_path.join("dist").join("release.cf");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "artifacts",
            "--output",
            &output_path.display().to_string(),
        ])
        .output()
        .expect("run command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("● Artifacts export completed successfully"));
    assert!(stdout.contains("│   source-set: main"));
    assert!(stdout.contains("│   mode: cf"));
    assert!(stdout.contains(output_path.display().to_string().as_str()));
    assert!(stdout.contains("[artifact] package_file ->"));
    assert!(!stdout.contains("platform log"));
}

#[test]
fn artifacts_text_failure_surfaces_error_and_diagnostic_path() {
    let (_dir, config_path, base_path) = setup_project(true);
    let output_path = base_path.join("dist").join("release.cf");

    let output = v8_runner_command()
        .args([
            "--config",
            &config_path.display().to_string(),
            "--no-color",
            "artifacts",
            "--output",
            &output_path.display().to_string(),
        ])
        .output()
        .expect("run command");

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("● Artifacts export failed"));
    assert!(stdout.contains("[error:designer_export_failed]"));
    assert!(stdout.contains("[diagnostic] platform log -> "));
}
