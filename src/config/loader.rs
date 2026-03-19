use std::path::Path;
use thiserror::Error;

use crate::config::model::AppConfig;
use crate::config::validate::{validate, ConfigValidationError};

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

impl ConfigLoadError {
    pub fn exit_code(&self) -> i32 {
        crate::output::exit_codes::VALIDATION_ERROR
    }
}

pub fn load_config(
    config_path: Option<&str>,
    workdir_override: Option<&str>,
) -> Result<AppConfig, ConfigLoadError> {
    let path = resolve_config_path(config_path)?;
    let content = std::fs::read_to_string(&path)?;
    let mut config: AppConfig = serde_yaml::from_str(&content)?;

    if let Some(wd) = workdir_override {
        config.work_path = Path::new(wd).to_path_buf();
    }

    validate(&config)?;
    Ok(config)
}

fn resolve_config_path(config_path: Option<&str>) -> Result<std::path::PathBuf, ConfigLoadError> {
    if let Some(p) = config_path {
        let path = Path::new(p);
        if !path.exists() {
            return Err(ConfigLoadError::NotFound(p.to_string()));
        }
        return Ok(path.to_path_buf());
    }

    // Default search locations
    let candidates = [
        "application.yaml",
        "application.yml",
        "config/application.yaml",
    ];
    for candidate in &candidates {
        let p = Path::new(candidate);
        if p.exists() {
            return Ok(p.to_path_buf());
        }
    }

    Err(ConfigLoadError::NotFound(
        "application.yaml (searched default locations)".to_string(),
    ))
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
        let config_path = dir.path().join("application.yaml");
        std::fs::write(
            &config_path,
            format!(
                "basePath: {}\nworkPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: \"File=/tmp/ib\"\nsource-set:\n  - name: main\n    purpose: CONFIGURATION\n    path: src\n",
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
    fn load_config_reads_custom_partial_load_threshold() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let src = base.join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        let config_path = dir.path().join("application.yaml");
        std::fs::write(
            &config_path,
            format!(
                "basePath: {}\nworkPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: \"File=/tmp/ib\"\nbuild:\n  partialLoadThreshold: 7\nsource-set:\n  - name: main\n    purpose: CONFIGURATION\n    path: src\n",
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
        let config_path = dir.path().join("application.yaml");
        std::fs::write(
            &config_path,
            format!(
                "basePath: {}\nworkPath: {}\nformat: DESIGNER\nbuilder: DESIGNER\nconnection: \"File=/tmp/ib\"\ntests:\n  execution_timeout_seconds: 17\nsource-set:\n  - name: main\n    purpose: CONFIGURATION\n    path: src\n",
                base.display(),
                work.display()
            ),
        )
        .expect("write config");

        let config = load_config(config_path.to_str(), None).expect("load config");

        assert_eq!(config.tests.execution_timeout_seconds, 17);
    }
}
