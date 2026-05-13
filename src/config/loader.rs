use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::config::model::AppConfig;
use crate::config::schema::{
    validate_local_overlay_schema_boundary, validate_main_config_schema_boundary,
};
use crate::config::validate::{validate, validate_tools_download_bootstrap, ConfigValidationError};
use crate::support::path::normalize_windows_verbatim_path;

pub const DEFAULT_CONFIG_FILE_NAME: &str = "v8project.yaml";
pub const LOCAL_CONFIG_FILE_NAME: &str = "v8project.local.yaml";

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

    #[error("{0} is a local overlay and cannot be used as --config")]
    LocalOverlayAsPrimaryConfig(String),

    #[error("local config overlay cannot override project identity key '{0}'")]
    LocalOverlayForbiddenKey(&'static str),

    #[error("local config overlay does not support top-level key '{0}'")]
    LocalOverlayUnsupportedKey(String),

    #[error("local config overlay contains unsupported key or value: {0}")]
    LocalOverlayUnsupportedShape(String),

    #[error("config contains unsupported key or value: {0}")]
    UnsupportedShape(String),
}

pub fn load_config(
    config_path: Option<&str>,
    workdir_override: Option<&str>,
) -> Result<AppConfig, ConfigLoadError> {
    load_config_with_mode(config_path, workdir_override, ConfigValidationMode::Full)
}

pub fn load_config_for_tools_download(
    config_path: Option<&str>,
    workdir_override: Option<&str>,
) -> Result<AppConfig, ConfigLoadError> {
    load_config_with_mode(
        config_path,
        workdir_override,
        ConfigValidationMode::ToolsDownload,
    )
}

enum ConfigValidationMode {
    Full,
    ToolsDownload,
}

fn load_config_with_mode(
    config_path: Option<&str>,
    workdir_override: Option<&str>,
    validation_mode: ConfigValidationMode,
) -> Result<AppConfig, ConfigLoadError> {
    let path = resolve_config_path(config_path)?;
    reject_local_overlay_as_primary_config(&path)?;
    let path = normalize_windows_verbatim_path(&std::fs::canonicalize(&path)?);
    let config_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut root = read_yaml_file(&path)?;
    reject_legacy_config_keys(&root)?;

    let local_path = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(LOCAL_CONFIG_FILE_NAME);
    if local_path.exists() {
        let overlay = read_yaml_file(&local_path)?;
        reject_legacy_config_keys(&overlay)?;
        reject_local_overlay_keys(&overlay)?;
        validate_local_overlay_schema_boundary(overlay.clone())
            .map_err(|error| ConfigLoadError::LocalOverlayUnsupportedShape(error.to_string()))?;
        merge_yaml_values(&mut root, overlay);
    }

    reject_legacy_config_keys(&root)?;
    validate_main_config_schema_boundary(root.clone())
        .map_err(|error| ConfigLoadError::UnsupportedShape(error.to_string()))?;
    default_base_path_to_config_dir(&mut root, config_dir)?;

    let mut config: AppConfig = serde_yaml::from_value(root)?;
    normalize_config_paths(&mut config, config_dir);

    if let Some(wd) = workdir_override {
        config.work_path = normalize_optional_path(Path::new(wd), config_dir);
    }

    match validation_mode {
        ConfigValidationMode::Full => validate(&config)?,
        ConfigValidationMode::ToolsDownload => validate_tools_download_bootstrap(&config)?,
    }
    Ok(config)
}

pub fn resolve_primary_config_path(config_path: Option<&str>) -> Result<PathBuf, ConfigLoadError> {
    let path = resolve_config_path(config_path)?;
    reject_local_overlay_as_primary_config(&path)?;
    Ok(normalize_windows_verbatim_path(&std::fs::canonicalize(
        &path,
    )?))
}

fn read_yaml_file(path: &Path) -> Result<serde_yaml::Value, ConfigLoadError> {
    let content = std::fs::read_to_string(path)?;
    Ok(serde_yaml::from_str(&content)?)
}

fn reject_local_overlay_as_primary_config(path: &Path) -> Result<(), ConfigLoadError> {
    if path.file_name().and_then(|name| name.to_str()) == Some(LOCAL_CONFIG_FILE_NAME) {
        return Err(ConfigLoadError::LocalOverlayAsPrimaryConfig(
            path.display().to_string(),
        ));
    }
    Ok(())
}

fn reject_local_overlay_keys(root: &serde_yaml::Value) -> Result<(), ConfigLoadError> {
    let Some(mapping) = root.as_mapping() else {
        return Err(ConfigLoadError::ValidationError(
            ConfigValidationError::InvalidYamlRoot(
                "expected a YAML mapping at the document root".to_owned(),
            ),
        ));
    };

    for key in mapping.keys() {
        let Some(key) = key.as_str() else {
            return Err(ConfigLoadError::LocalOverlayUnsupportedKey(
                "<non-string>".to_owned(),
            ));
        };
        match key {
            "source-set" => return Err(ConfigLoadError::LocalOverlayForbiddenKey("source-set")),
            "format" => return Err(ConfigLoadError::LocalOverlayForbiddenKey("format")),
            "builder" => return Err(ConfigLoadError::LocalOverlayForbiddenKey("builder")),
            "workPath" | "infobase" | "tools" | "tests" | "mcp" => {}
            unsupported => {
                return Err(ConfigLoadError::LocalOverlayUnsupportedKey(
                    unsupported.to_owned(),
                ));
            }
        }
    }

    Ok(())
}

fn default_base_path_to_config_dir(
    root: &mut serde_yaml::Value,
    config_dir: &Path,
) -> Result<(), ConfigLoadError> {
    let Some(mapping) = root.as_mapping_mut() else {
        return Err(ConfigLoadError::ValidationError(
            ConfigValidationError::InvalidYamlRoot(
                "expected a YAML mapping at the document root".to_owned(),
            ),
        ));
    };

    let key = serde_yaml::Value::String("basePath".to_owned());
    if !mapping.contains_key(&key) {
        mapping.insert(
            key,
            serde_yaml::Value::String(config_dir.display().to_string()),
        );
    }

    Ok(())
}

fn merge_yaml_values(base: &mut serde_yaml::Value, overlay: serde_yaml::Value) {
    match (base, overlay) {
        (serde_yaml::Value::Mapping(base), serde_yaml::Value::Mapping(overlay)) => {
            for (key, overlay_value) in overlay {
                match base.get_mut(&key) {
                    Some(base_value) => merge_yaml_values(base_value, overlay_value),
                    None => {
                        base.insert(key, overlay_value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

fn normalize_config_paths(config: &mut AppConfig, config_dir: &Path) {
    config.base_path = normalize_optional_path(&config.base_path, config_dir);
    config.work_path = normalize_optional_path(&config.work_path, config_dir);
    config.infobase.connection =
        normalize_connection_string(&config.infobase.connection, config_dir);

    if let Some(path) = config.tools.va.epf_path.as_mut() {
        *path = normalize_optional_path(path, config_dir);
    }
    if let Some(extension) = config.tools.client_mcp.extension.as_mut() {
        if let Some(source) = extension.source_mut() {
            source.path = normalize_optional_path(&source.path, config_dir);
        }
        if let Some(artifact) = extension.artifact_mut() {
            artifact.path = normalize_optional_path(&artifact.path, config_dir);
        }
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

    if mapping_contains_key(mapping, "execution_timeout_seconds") {
        return Err(ConfigValidationError::LegacyTopLevelExecutionTimeoutSeconds);
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
    use super::{load_config, ConfigLoadError, LOCAL_CONFIG_FILE_NAME};
    use crate::change_detection::partial_load::DEFAULT_PARTIAL_LOAD_THRESHOLD;
    use crate::config::validate::ConfigValidationError;
    use std::path::Path;
    use tempfile::tempdir;

    fn write_minimal_project_config(config_dir: &Path, body: &str) -> std::path::PathBuf {
        std::fs::create_dir_all(config_dir.join("src")).expect("src dir");
        let config_path = config_dir.join("v8project.yaml");
        std::fs::write(&config_path, body).expect("write config");
        config_path
    }

    fn minimal_config_without_base_path(extra: &str) -> String {
        format!(
            "workPath: work\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=build/ib\"\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: src\n{extra}"
        )
    }

    #[test]
    fn load_config_defaults_missing_base_path_to_primary_config_dir() {
        let dir = tempdir().expect("tempdir");
        let config_dir = dir.path().join("project");
        let config_path =
            write_minimal_project_config(&config_dir, &minimal_config_without_base_path(""));

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(config.base_path, config_dir);
        assert_eq!(config.work_path, config.base_path.join("work"));
        assert_eq!(
            config.infobase.connection,
            format!("File={}", config.base_path.join("build/ib").display())
        );
    }

    #[test]
    fn load_config_applies_local_overlay_next_to_primary_config() {
        let dir = tempdir().expect("tempdir");
        let config_dir = dir.path().join("nested").join("project");
        let config_path = write_minimal_project_config(
            &config_dir,
            "workPath: work\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=build/ib\"\n  user: ProjectUser\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: src\ntools:\n  client_mcp:\n    port: 1111\n  enterprise:\n    additional-launch-keys:\n      - /PROJECT\nmcp:\n  http:\n    path: /project-mcp\n",
        );
        std::fs::write(
            config_dir.join(LOCAL_CONFIG_FILE_NAME),
            "workPath: local-work\ninfobase:\n  user: LocalUser\n  password: secret\ntools:\n  client_mcp:\n    port: 9874\n  enterprise:\n    additional-launch-keys:\n      - /LOCAL\nmcp:\n  http:\n    path: /local-mcp\n",
        )
        .expect("write local overlay");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(config.base_path, config_dir);
        assert_eq!(config.work_path, config.base_path.join("local-work"));
        assert_eq!(config.infobase.user.as_deref(), Some("LocalUser"));
        assert_eq!(config.infobase.password.as_deref(), Some("secret"));
        assert_eq!(config.tools.client_mcp.port, Some(9874));
        assert_eq!(
            config.tools.enterprise.additional_launch_keys,
            vec!["/LOCAL".to_owned()]
        );
        assert_eq!(config.mcp.http.path, "/local-mcp");
    }

    #[test]
    fn load_config_discovers_local_overlay_next_to_explicit_config() {
        let dir = tempdir().expect("tempdir");
        let config_dir = dir.path().join("subproject");
        let config_path =
            write_minimal_project_config(&config_dir, &minimal_config_without_base_path(""));
        std::fs::write(config_dir.join(LOCAL_CONFIG_FILE_NAME), "workPath: right\n")
            .expect("local overlay");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(config.work_path, config.base_path.join("right"));
    }

    #[test]
    fn load_config_cli_workdir_override_wins_over_local_overlay() {
        let dir = tempdir().expect("tempdir");
        let config_dir = dir.path().join("project");
        let config_path =
            write_minimal_project_config(&config_dir, &minimal_config_without_base_path(""));
        std::fs::write(
            config_dir.join(LOCAL_CONFIG_FILE_NAME),
            "workPath: local-work\n",
        )
        .expect("local overlay");

        let config = load_config(config_path.to_str(), Some("cli-work")).expect("load config");

        assert_eq!(config.work_path, config_dir.join("cli-work"));
    }

    #[test]
    fn load_config_rejects_project_identity_keys_in_local_overlay() {
        for key in ["source-set", "format", "builder"] {
            let dir = tempdir().expect("tempdir");
            let config_dir = dir.path().join("project");
            let config_path =
                write_minimal_project_config(&config_dir, &minimal_config_without_base_path(""));
            let value = if key == "source-set" {
                "[]".to_owned()
            } else {
                "DESIGNER".to_owned()
            };
            std::fs::write(
                config_dir.join(LOCAL_CONFIG_FILE_NAME),
                format!("{key}: {value}\n"),
            )
            .expect("local overlay");

            let error =
                load_config(config_path.to_str(), None).expect_err("forbidden local overlay key");

            assert!(
                error
                    .to_string()
                    .contains(&format!("cannot override project identity key '{key}'")),
                "{error}"
            );
        }
    }

    #[test]
    fn load_config_rejects_unsupported_top_level_keys_in_local_overlay() {
        for key in ["basePath", "build", "execution_timeout", "unknown"] {
            let dir = tempdir().expect("tempdir");
            let config_dir = dir.path().join("project");
            let config_path =
                write_minimal_project_config(&config_dir, &minimal_config_without_base_path(""));
            std::fs::write(
                config_dir.join(LOCAL_CONFIG_FILE_NAME),
                format!("{key}: value\n"),
            )
            .expect("local overlay");

            let error =
                load_config(config_path.to_str(), None).expect_err("unsupported local overlay key");

            assert!(
                error
                    .to_string()
                    .contains(&format!("does not support top-level key '{key}'")),
                "{error}"
            );
        }
    }

    #[test]
    fn load_config_rejects_local_overlay_as_primary_config() {
        let dir = tempdir().expect("tempdir");
        let local_path = dir.path().join(LOCAL_CONFIG_FILE_NAME);
        std::fs::write(&local_path, "workPath: local\n").expect("local overlay");

        let error =
            load_config(local_path.to_str(), None).expect_err("local overlay cannot be primary");

        assert!(error.to_string().contains("cannot be used as --config"));
    }

    #[test]
    fn load_config_rejects_legacy_keys_from_local_overlay() {
        let dir = tempdir().expect("tempdir");
        let config_dir = dir.path().join("project");
        let config_path =
            write_minimal_project_config(&config_dir, &minimal_config_without_base_path(""));
        std::fs::write(
            config_dir.join(LOCAL_CONFIG_FILE_NAME),
            "credentials:\n  user: Admin\n",
        )
        .expect("local overlay");

        let error = load_config(config_path.to_str(), None).expect_err("reject legacy local key");

        assert!(matches!(
            error,
            ConfigLoadError::ValidationError(ConfigValidationError::LegacyTopLevelCredentials)
        ));
    }

    #[test]
    fn load_config_allows_local_null_for_optional_fields_only() {
        let dir = tempdir().expect("tempdir");
        let config_dir = dir.path().join("project");
        let config_path = write_minimal_project_config(
            &config_dir,
            "workPath: work\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=build/ib\"\n  user: Admin\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: src\n",
        );
        std::fs::write(
            config_dir.join(LOCAL_CONFIG_FILE_NAME),
            "infobase:\n  user: null\n",
        )
        .expect("local overlay");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(config.infobase.user, None);
    }

    #[test]
    fn load_config_rejects_local_null_for_required_fields() {
        let dir = tempdir().expect("tempdir");
        let config_dir = dir.path().join("project");
        let config_path =
            write_minimal_project_config(&config_dir, &minimal_config_without_base_path(""));
        std::fs::write(config_dir.join(LOCAL_CONFIG_FILE_NAME), "workPath: null\n")
            .expect("local overlay");

        let error = load_config(config_path.to_str(), None)
            .expect_err("required field cannot be reset to null");

        assert!(
            error
                .to_string()
                .contains("local config overlay contains unsupported key or value"),
            "{error}"
        );
    }

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
                "workPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: base/src\n",
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
                "workPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\nsource-set:\n  - name: main\n    purpose: CONFIGURATION\n    path: base/src\n",
                work.display()
            ),
        )
        .expect("write config");

        let error = load_config(config_path.to_str(), None).expect_err("reject legacy purpose key");

        assert!(
            error
                .to_string()
                .contains("config contains unsupported key or value"),
            "{error}"
        );
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
                "workPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\nbuild:\n  partialLoadThreshold: 7\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: base/src\n",
                work.display()
            ),
        )
        .expect("write config");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(config.build.partial_load_threshold, 7);
    }

    #[test]
    fn load_config_reads_build_dynamic_update_and_infobase_unlock_code() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "workPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\n  unlock_code: seal-1\nbuild:\n  dynamicUpdate: true\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: base/src\n",
                work.display()
            ),
        )
        .expect("write config");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert!(config.build.dynamic_update);
        assert_eq!(config.infobase.unlock_code.as_deref(), Some("seal-1"));

        // And the resulting V8Connection carries the unlock code into the platform layer.
        let connection = config.v8_connection();
        assert_eq!(connection.unlock_code.as_deref(), Some("seal-1"));
        let args = connection.args();
        let uc_index = args.iter().position(|arg| arg == "/UC").expect("/UC");
        assert_eq!(args.get(uc_index + 1).map(String::as_str), Some("seal-1"));
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
                "workPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\ntests:\n  execution_timeout_seconds: 17\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: base/src\n",
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
                "workPath: {}\nexecution_timeout: 4321\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: base/src\n",
                work.display()
            ),
        )
        .expect("write config");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(config.execution_timeout, 4321);
    }

    #[test]
    fn load_config_rejects_top_level_execution_timeout_seconds() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "basePath: {}\nworkPath: {}\nexecution_timeout_seconds: 300\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: src\n",
                base.display(),
                work.display()
            ),
        )
        .expect("write config");

        let err = load_config(config_path.to_str(), None).expect_err("expected legacy key error");

        assert!(matches!(
            err,
            ConfigLoadError::ValidationError(
                ConfigValidationError::LegacyTopLevelExecutionTimeoutSeconds
            )
        ));
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
                "workPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\ntools:\n  va:\n    epf_path: va/runner.epf\ntests:\n  va:\n    params_path: va/params.json\n    profile: smoke\n    profiles:\n      smoke:\n        feature_path: features\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: ../base/src\n",
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
            "workPath: build\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=build/ib\"\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: sources\n",
        )
        .expect("write config");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(config.base_path, config_dir);
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
            "workPath: build\nformat: DESIGNER\nbuilder: IBCMD\ninfobase:\n  connection: \"Srvr=cluster:1541;Ref=demo\"\n  dbms:\n    kind: PostgreSQL\n    server: localhost\n    name: demo\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: sources\n",
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
            "workPath: build\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: '/F \"build/my ib\"'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: sources\n",
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
        let extension_src = dir.path().join("exts").join("client-mcp");
        std::fs::create_dir_all(&src).expect("src dir");
        std::fs::create_dir_all(&extension_src).expect("extension dir");
        std::fs::write(
            extension_src.join("Configuration.xml"),
            "<Configuration><ConfigurationExtensionPurpose>Extension</ConfigurationExtensionPurpose></Configuration>",
        )
        .expect("extension marker");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "workPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\nmcp:\n  http:\n    bind_address: 127.0.0.1:4000\n    path: /custom-mcp\n    stateful_sessions: false\n    max_sessions: 12\n    idle_ttl_secs: 45\n  execution:\n    max_concurrent_calls: 3\n    shutdown_grace_period_secs: 9\ntools:\n  client_mcp:\n    port: 9874\n    extension:\n      name: client_mcp\n      source:\n        path: exts/client-mcp\n        format: DESIGNER\n  enterprise:\n    additional-launch-keys:\n      - /TESTMANAGER\n  edt_cli:\n    interactive-mode: true\n    startup_timeout_ms: 1234\n    command_timeout_ms: 5678\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: base/src\n",
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
        let extension = config
            .tools
            .client_mcp
            .extension
            .expect("client mcp extension");
        assert_eq!(extension.name, "client_mcp");
        assert_eq!(
            extension.source().expect("source").path,
            dir.path().join("exts").join("client-mcp")
        );
    }

    #[test]
    fn load_config_rejects_client_mcp_extension_with_multiple_or_missing_inputs() {
        for (case, extension_body) in [
            (
                "both",
                "      name: client_mcp\n      source:\n        path: ext\n      artifact:\n        path: ext.cfe\n",
            ),
            ("neither", "      name: client_mcp\n"),
        ] {
            let dir = tempdir().expect("tempdir");
            let base = dir.path().join("base");
            let work = dir.path().join("work");
            let src = base.join("src");
            std::fs::create_dir_all(&src).expect("src dir");
            let config_path = dir.path().join(format!("{case}.yaml"));
            std::fs::write(
                &config_path,
                format!(
                    "workPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\ntools:\n  client_mcp:\n    extension:\n{extension_body}source-set:\n  - name: main\n    type: CONFIGURATION\n    path: base/src\n",
                    work.display()
                ),
            )
            .expect("write config");

            let error = load_config(config_path.to_str(), None)
                .expect_err("invalid extension input should fail");

            assert!(
                error
                    .to_string()
                    .contains("must specify exactly one of source or artifact"),
                "{error}"
            );
        }
    }

    #[test]
    fn load_config_accepts_enterprise_additional_launch_keys() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");

        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "workPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\ntools:\n  enterprise:\n    additional-launch-keys:\n      - /TESTMANAGER\n      - /TCUser\n      - ci-user\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: base/src\n",
                work.display()
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

    #[test]
    fn load_config_rejects_removed_config_aliases() {
        let cases = [
            ("executionTimeout", "executionTimeout: 300000\n"),
            ("execution_timeout_ms", "execution_timeout_ms: 300000\n"),
            (
                "edt-cli",
                "tools:\n  edt-cli:\n    startup_timeout_ms: 2222\n",
            ),
            (
                "additional_launch_keys",
                "tools:\n  enterprise:\n    additional_launch_keys:\n      - /TESTMANAGER\n",
            ),
            (
                "additionalLaunchKeys",
                "tools:\n  enterprise:\n    additionalLaunchKeys:\n      - /TESTMANAGER\n",
            ),
            (
                "startup-timeout-ms",
                "tools:\n  edt_cli:\n    startup-timeout-ms: 2222\n",
            ),
            (
                "command-timeout-ms",
                "tools:\n  edt_cli:\n    command-timeout-ms: 3333\n",
            ),
        ];

        for (name, extra_yaml) in cases {
            let dir = tempdir().expect("tempdir");
            let base = dir.path().join("base");
            let work = dir.path().join("work");
            let src = base.join("src");
            std::fs::create_dir_all(&src).expect("src dir");
            let config_path = dir.path().join(format!("{name}.yaml"));
            std::fs::write(
                &config_path,
                format!(
                    "workPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\n{extra_yaml}source-set:\n  - name: main\n    type: CONFIGURATION\n    path: base/src\n",
                    work.display()
                ),
            )
            .expect("write config");

            load_config(config_path.to_str(), None)
                .expect_err(&format!("{name} alias must be rejected"));
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
                "workPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: base/src\n",
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
    fn load_config_accepts_canonical_edt_timeout_keys() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "workPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\ntools:\n  edt_cli:\n    startup_timeout_ms: 2222\n    command_timeout_ms: 3333\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: base/src\n",
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
                "workPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: \"File=/tmp/ib\"\ntools:\n  platform:\n    version: 8.3.27.1859\n  edt_cli:\n    path: 1c-edt-2025.2.3\n    version: 1c-edt-2025.2.3\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: base/src\n",
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
