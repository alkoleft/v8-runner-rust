use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::config::model::AppConfig;
use crate::config::validate::{validate, ConfigValidationError};
use crate::support::path::normalize_windows_verbatim_path;

const DEFAULT_CONFIG_FILE_NAME: &str = "v8project.yaml";

#[derive(Debug, Error)]
pub enum ConfigLoadError {
    #[error("config file not found: {0}")]
    NotFound(String),

    #[error("failed to read config file: {0}")]
    ReadError(#[from] std::io::Error),

    #[error("failed to parse YAML config: {0}")]
    ParseError(#[from] serde_yaml::Error),

    #[error("config validation failed: {0}")]
    ValidationError(#[from] ConfigValidationError),
}

pub fn load_config(
    config_path: Option<&str>,
    workdir_override: Option<&str>,
) -> Result<AppConfig, ConfigLoadError> {
    let path = resolve_config_path(config_path)?;
    let path = normalize_windows_verbatim_path(&std::fs::canonicalize(&path)?);
    let content = std::fs::read_to_string(&path)?;
    let root: serde_yaml::Value = serde_yaml::from_str(&content)?;
    reject_legacy_config_keys(&root)?;
    let mut config: AppConfig = serde_yaml::from_str(&content)?;
    normalize_config_paths(&mut config, path.parent().unwrap_or_else(|| Path::new(".")));

    if let Some(wd) = workdir_override {
        config.work_path = normalize_optional_path(
            Path::new(wd),
            path.parent().unwrap_or_else(|| Path::new(".")),
        );
    }

    validate(&config)?;
    Ok(config)
}

fn normalize_config_paths(config: &mut AppConfig, config_dir: &Path) {
    config.base_path = normalize_optional_path(&config.base_path, config_dir);
    config.work_path = normalize_optional_path(&config.work_path, config_dir);
    config.infobase.connection =
        normalize_connection_string(&config.infobase.connection, config_dir);

    if let Some(path) = config.tools.va.epf_path.as_mut() {
        *path = normalize_optional_path(path, config_dir);
    }
    let va = &mut config.tests.va;
    if let Some(path) = va.params_path.as_mut() {
        *path = normalize_optional_path(path, config_dir);
    }
    for profile in va.profiles.values_mut() {
        if let Some(path) = profile.feature_path.as_mut() {
            *path = normalize_optional_path(path, config_dir);
        }
    }
}

fn reject_legacy_config_keys(root: &serde_yaml::Value) -> Result<(), ConfigValidationError> {
    let Some(mapping) = root.as_mapping() else {
        return Err(ConfigValidationError::InvalidYamlRoot(
            "expected a YAML mapping at the document root".to_owned(),
        ));
    };

    if mapping_contains_key(mapping, "connection") {
        return Err(ConfigValidationError::LegacyTopLevelConnection);
    }

    if mapping_contains_key(mapping, "credentials") {
        return Err(ConfigValidationError::LegacyTopLevelCredentials);
    }

    if let Some(mcp) = mapping
        .get(serde_yaml::Value::String("mcp".to_owned()))
        .and_then(serde_yaml::Value::as_mapping)
    {
        if mapping_contains_key(mcp, "client") {
            return Err(ConfigValidationError::LegacyMcpClientConfig);
        }
    }

    if let Some(va) = mapping
        .get(serde_yaml::Value::String("tests".to_owned()))
        .and_then(serde_yaml::Value::as_mapping)
        .and_then(|tests| tests.get(serde_yaml::Value::String("va".to_owned())))
        .and_then(serde_yaml::Value::as_mapping)
    {
        if mapping_contains_key(va, "epf_path") {
            return Err(ConfigValidationError::LegacyVanessaEpfPath);
        }
    }

    Ok(())
}

fn mapping_contains_key(mapping: &serde_yaml::Mapping, key: &str) -> bool {
    mapping.contains_key(serde_yaml::Value::String(key.to_owned()))
}

fn normalize_optional_path(path: &Path, config_dir: &Path) -> PathBuf {
    let normalized = if path.is_absolute() {
        path.to_path_buf()
    } else {
        config_dir.join(path)
    };
    normalize_windows_verbatim_path(&normalized)
}

fn normalize_connection_string(connection: &str, config_dir: &Path) -> String {
    let trimmed = connection.trim();
    if trimmed.starts_with('/') || trimmed.starts_with('-') {
        return normalize_raw_connection_args(trimmed, config_dir);
    }

    let mut changed = false;
    let parts: Vec<_> = connection
        .split(';')
        .map(|part| {
            let part = part.trim();
            let lower = part.to_ascii_lowercase();
            if lower.starts_with("file=") {
                let normalized = normalize_connection_file_path(&part[5..], config_dir);
                changed |= normalized != part[5..];
                format!("{}{}", &part[..5], normalized)
            } else {
                part.to_owned()
            }
        })
        .collect();

    if changed {
        parts.join(";")
    } else {
        connection.to_owned()
    }
}

fn normalize_raw_connection_args(connection: &str, config_dir: &Path) -> String {
    let mut args = split_arg_string(connection);
    let mut changed = false;
    let mut index = 0;
    while index + 1 < args.len() {
        if args[index].eq_ignore_ascii_case("/f") || args[index].eq_ignore_ascii_case("-f") {
            let normalized = normalize_connection_file_path(&args[index + 1], config_dir);
            changed |= normalized != args[index + 1];
            args[index + 1] = normalized;
            index += 2;
        } else {
            index += 1;
        }
    }

    if changed {
        join_arg_string(&args)
    } else {
        connection.to_owned()
    }
}

fn normalize_connection_file_path(path: &str, config_dir: &Path) -> String {
    let path = path.trim();
    let path = strip_matching_quotes(path).unwrap_or(path);
    let path = Path::new(path);
    let normalized = if path.is_absolute() {
        path.to_path_buf()
    } else {
        config_dir.join(path)
    };
    normalize_windows_verbatim_path(&normalized)
        .display()
        .to_string()
}

fn strip_matching_quotes(value: &str) -> Option<&str> {
    if value.len() < 2 {
        return None;
    }

    let quote = value.as_bytes()[0];
    let last = *value.as_bytes().last()?;
    if (quote == b'\'' || quote == b'"') && quote == last {
        Some(&value[1..value.len() - 1])
    } else {
        None
    }
}

fn split_arg_string(raw: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in raw.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ch if ch.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    args
}

fn join_arg_string(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            if arg.is_empty() || arg.chars().any(char::is_whitespace) {
                format!("\"{}\"", arg.replace('"', "\\\""))
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn resolve_config_path(config_path: Option<&str>) -> Result<PathBuf, ConfigLoadError> {
    if let Some(p) = config_path {
        let path = Path::new(p);
        if !path.exists() {
            return Err(ConfigLoadError::NotFound(p.to_string()));
        }
        return Ok(path.to_path_buf());
    }

    let path = Path::new(DEFAULT_CONFIG_FILE_NAME);
    if path.exists() {
        return Ok(path.to_path_buf());
    }

    Err(ConfigLoadError::NotFound(format!(
        "{DEFAULT_CONFIG_FILE_NAME} (default config file)"
    )))
}

#[cfg(test)]
mod tests {
    use super::load_config;
    use crate::change_detection::partial_load::DEFAULT_PARTIAL_LOAD_THRESHOLD;
    use tempfile::tempdir;

    #[test]
    fn load_config_uses_default_build_settings_when_section_is_omitted() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "basePath: {}\nworkPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: src\n",
                base.display(),
                work.display()
            ),
        )
        .expect("write config");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(
            config.build.partial_load_threshold,
            DEFAULT_PARTIAL_LOAD_THRESHOLD
        );
    }

    #[test]
    fn load_config_rejects_legacy_source_set_purpose_key() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "basePath: {}\nworkPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\nsource-set:\n  - name: main\n    purpose: CONFIGURATION\n    path: src\n",
                base.display(),
                work.display()
            ),
        )
        .expect("write config");

        let error = load_config(config_path.to_str(), None).expect_err("reject legacy purpose key");

        assert!(error.to_string().contains("missing field `type`"));
    }

    #[test]
    fn load_config_reads_custom_partial_load_threshold() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "basePath: {}\nworkPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\nbuild:\n  partialLoadThreshold: 7\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: src\n",
                base.display(),
                work.display()
            ),
        )
        .expect("write config");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(config.build.partial_load_threshold, 7);
    }

    #[test]
    fn load_config_reads_test_timeout_from_exact_yaml_key() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "basePath: {}\nworkPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\ntests:\n  execution_timeout_seconds: 17\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: src\n",
                base.display(),
                work.display()
            ),
        )
        .expect("write config");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(config.tests.execution_timeout_seconds, 17);
    }

    #[test]
    fn load_config_reads_global_execution_timeout_from_public_yaml_key() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "basePath: {}\nworkPath: {}\nexecution_timeout: 4321\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: src\n",
                base.display(),
                work.display()
            ),
        )
        .expect("write config");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(config.execution_timeout, 4321);
    }

    #[test]
    fn load_config_normalizes_vanessa_paths_relative_to_config_dir() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        let features = dir.path().join("cfg").join("features");
        let va = dir.path().join("cfg").join("va");
        std::fs::create_dir_all(&src).expect("src dir");
        std::fs::create_dir_all(&features).expect("features dir");
        std::fs::create_dir_all(&va).expect("va dir");
        std::fs::write(va.join("runner.epf"), "epf").expect("epf");
        std::fs::write(va.join("params.json"), "{}").expect("params");
        let config_dir = dir.path().join("cfg");
        std::fs::create_dir_all(&config_dir).expect("config dir");
        let config_path = config_dir.join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "basePath: {}\nworkPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\ntools:\n  va:\n    epf_path: va/runner.epf\ntests:\n  va:\n    params_path: va/params.json\n    profile: smoke\n    profiles:\n      smoke:\n        feature_path: features\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: src\n",
                base.display(),
                work.display()
            ),
        )
        .expect("write config");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(
            config.tools.va.epf_path.expect("epf"),
            config_dir.join("va/runner.epf")
        );
        assert_eq!(
            config.tests.va.params_path.expect("params"),
            config_dir.join("va/params.json")
        );
        assert_eq!(
            config
                .tests
                .va
                .profiles
                .get("smoke")
                .and_then(|profile| profile.feature_path.clone())
                .expect("feature path"),
            config_dir.join("features")
        );
    }

    #[test]
    fn load_config_absolutizes_relative_core_paths_from_config_dir() {
        let dir = tempdir().expect("tempdir");
        let config_dir = dir.path().join("project");
        let base = config_dir.join("sources");
        std::fs::create_dir_all(&base).expect("base dir");
        let config_path = config_dir.join("v8project.yaml");
        std::fs::write(
            &config_path,
            "basePath: sources\nworkPath: build\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=build/ib\"\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: .\n",
        )
        .expect("write config");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(config.base_path, base);
        assert_eq!(config.work_path, config_dir.join("build"));
        assert_eq!(
            config.infobase.connection,
            format!("File={}", config_dir.join("build/ib").display())
        );
    }

    #[test]
    fn load_config_preserves_server_connection_string() {
        let dir = tempdir().expect("tempdir");
        let config_dir = dir.path().join("project");
        let base = config_dir.join("sources");
        std::fs::create_dir_all(&base).expect("base dir");
        let config_path = config_dir.join("v8project.yaml");
        std::fs::write(
            &config_path,
            "basePath: sources\nworkPath: build\nformat: DESIGNER\nbuilder: IBCMD\ninfobase:\n  connection: \"Srvr=cluster:1541;Ref=demo\"\n  dbms:\n    kind: PostgreSQL\n    server: localhost\n    name: demo\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: .\n",
        )
        .expect("write config");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(config.infobase.connection, "Srvr=cluster:1541;Ref=demo");
    }

    #[test]
    fn load_config_rejects_legacy_top_level_connection_key() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "basePath: {}\nworkPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: \"File=/tmp/ib\"\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: src\n",
                base.display(),
                work.display()
            ),
        )
        .expect("write config");

        let error = load_config(config_path.to_str(), None).expect_err("reject legacy connection");

        assert!(error
            .to_string()
            .contains("legacy top-level key 'connection'"));
    }

    #[test]
    fn load_config_rejects_legacy_mcp_client_section() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "basePath: {}\nworkPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\nmcp:\n  client:\n    port: 9874\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: src\n",
                base.display(),
                work.display()
            ),
        )
        .expect("write config");

        let error = load_config(config_path.to_str(), None).expect_err("reject legacy mcp client");

        assert!(error.to_string().contains("use tools.client_mcp"));
    }

    #[test]
    fn load_config_rejects_legacy_tests_va_epf_path() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "basePath: {}\nworkPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\ntests:\n  va:\n    epf_path: /tmp/vanessa.epf\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: src\n",
                base.display(),
                work.display()
            ),
        )
        .expect("write config");

        let error = load_config(config_path.to_str(), None).expect_err("reject legacy va epf");

        assert!(error.to_string().contains("use tools.va.epf_path"));
    }

    #[test]
    fn load_config_rejects_mixed_new_and_legacy_infobase_keys() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "basePath: {}\nworkPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\ncredentials:\n  password: secret\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: src\n",
                base.display(),
                work.display()
            ),
        )
        .expect("write config");

        let error = load_config(config_path.to_str(), None).expect_err("reject legacy credentials");

        assert!(error
            .to_string()
            .contains("legacy top-level key 'credentials'"));
    }

    #[test]
    fn load_config_absolutizes_raw_file_connection_from_config_dir() {
        let dir = tempdir().expect("tempdir");
        let config_dir = dir.path().join("project");
        let base = config_dir.join("sources");
        std::fs::create_dir_all(&base).expect("base dir");
        let config_path = config_dir.join("v8project.yaml");
        std::fs::write(
            &config_path,
            "basePath: sources\nworkPath: build\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: '/F \"build/my ib\"'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: .\n",
        )
        .expect("write config");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(
            config.v8_connection().file_path(),
            Some(config_dir.join("build/my ib").to_string_lossy().as_ref())
        );
    }

    #[test]
    fn load_config_reads_mcp_sections_and_edt_timeouts() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "basePath: {}\nworkPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\nmcp:\n  http:\n    bind_address: 127.0.0.1:4000\n    path: /custom-mcp\n    stateful_sessions: false\n    max_sessions: 12\n    idle_ttl_secs: 45\n  execution:\n    max_concurrent_calls: 3\n    shutdown_grace_period_secs: 9\ntools:\n  client_mcp:\n    port: 9874\n  enterprise:\n    additional-launch-keys:\n      - /TESTMANAGER\n  edt_cli:\n    interactive-mode: true\n    startup_timeout_ms: 1234\n    command_timeout_ms: 5678\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: src\n",
                base.display(),
                work.display()
            ),
        )
        .expect("write config");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(config.mcp.http.bind_address, "127.0.0.1:4000");
        assert_eq!(config.mcp.http.path, "/custom-mcp");
        assert!(!config.mcp.http.stateful_sessions);
        assert_eq!(config.mcp.http.max_sessions, 12);
        assert_eq!(config.mcp.http.idle_ttl_secs, 45);
        assert_eq!(config.mcp.execution.max_concurrent_calls, 3);
        assert_eq!(config.mcp.execution.shutdown_grace_period_secs, 9);
        assert_eq!(config.tools.client_mcp.port, Some(9874));
        assert_eq!(
            config.tools.enterprise.additional_launch_keys,
            vec!["/TESTMANAGER".to_owned()]
        );
        assert!(config.tools.edt_cli.interactive_mode);
        assert_eq!(config.tools.edt_cli.startup_timeout_ms, 1234);
        assert_eq!(config.tools.edt_cli.command_timeout_ms, 5678);
    }

    #[test]
    fn load_config_accepts_enterprise_additional_keys_aliases() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");

        for key in [
            "additional-launch-keys",
            "additional_launch_keys",
            "additionalLaunchKeys",
        ] {
            let config_path = dir.path().join(format!("{key}.yaml"));
            std::fs::write(
                &config_path,
                format!(
                    "basePath: {}\nworkPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\ntools:\n  enterprise:\n    {}:\n      - /TESTMANAGER\n      - /TCUser\n      - ci-user\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: src\n",
                    base.display(),
                    work.display(),
                    key
                ),
            )
            .expect("write config");

            let config = load_config(config_path.to_str(), None).expect("load config");
            assert_eq!(
                config.tools.enterprise.additional_launch_keys,
                vec![
                    "/TESTMANAGER".to_owned(),
                    "/TCUser".to_owned(),
                    "ci-user".to_owned()
                ]
            );
        }
    }

    #[test]
    fn load_config_uses_mcp_and_edt_timeout_defaults_when_sections_are_omitted() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "basePath: {}\nworkPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: src\n",
                base.display(),
                work.display()
            ),
        )
        .expect("write config");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(config.mcp.http.bind_address, "127.0.0.1:3000");
        assert_eq!(config.mcp.http.path, "/mcp");
        assert!(config.mcp.http.stateful_sessions);
        assert_eq!(config.mcp.http.max_sessions, 64);
        assert_eq!(config.mcp.http.idle_ttl_secs, 900);
        assert_eq!(config.mcp.execution.max_concurrent_calls, 1);
        assert_eq!(config.mcp.execution.shutdown_grace_period_secs, 30);
        assert_eq!(config.tools.edt_cli.startup_timeout_ms, 300_000);
        assert_eq!(config.tools.edt_cli.command_timeout_ms, 300_000);
    }

    #[test]
    fn load_config_accepts_kebab_case_edt_timeout_aliases() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "basePath: {}\nworkPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\ntools:\n  edt-cli:\n    startup-timeout-ms: 2222\n    command-timeout-ms: 3333\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: src\n",
                base.display(),
                work.display()
            ),
        )
        .expect("write config");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(config.tools.edt_cli.startup_timeout_ms, 2222);
        assert_eq!(config.tools.edt_cli.command_timeout_ms, 3333);
    }

    #[test]
    fn load_config_reads_edt_version_hint_fields() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "basePath: {}\nworkPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\ntools:\n  platform:\n    version: 8.3.27.1859\n  edt-cli:\n    path: 1c-edt-2025.2.3\n    version: 1c-edt-2025.2.3\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: src\n",
                base.display(),
                work.display()
            ),
        )
        .expect("write config");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(
            config.tools.platform.version.as_deref(),
            Some("8.3.27.1859")
        );
        assert_eq!(
            config.tools.edt_cli.path.as_deref(),
            Some(std::path::Path::new("1c-edt-2025.2.3"))
        );
        assert_eq!(
            config.tools.edt_cli.version.as_deref(),
            Some("1c-edt-2025.2.3")
        );
    }
}
