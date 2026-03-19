use std::collections::HashSet;
use std::path::Path;
use thiserror::Error;

use crate::config::model::{AppConfig, BuilderBackend, SourceFormat, SourceSetPurpose};
use crate::platform::locator::PlatformVersion;
use crate::support::path::is_safe_path_segment;

#[derive(Debug, Error)]
pub enum ConfigValidationError {
    #[error("basePath does not exist or is not a directory: {0}")]
    BasePathInvalid(String),

    #[error("workPath could not be created: {0}")]
    WorkPathInvalid(String),

    #[error("source-set must contain at least one CONFIGURATION entry")]
    NoConfigurationSourceSet,

    #[error("source-set entry '{name}' path does not exist: {path}")]
    SourceSetPathInvalid { name: String, path: String },

    #[error("source-set name must be unique, duplicate: {0}")]
    DuplicateSourceSetName(String),

    #[error("source-set name contains unsafe path or filename characters: {0}")]
    InvalidSourceSetName(String),

    #[error("source-set resolved path must be unique, duplicate: {0}")]
    DuplicateSourceSetPath(String),

    #[error("connection string is empty")]
    EmptyConnection,

    #[error("IBCMD builder requires a file-based connection string (File=...)")]
    IbcmdRequiresFileConnection,

    #[error("format EDT requires at least one source-set with a valid EDT project path")]
    EdtNoProjects,

    #[error("platform version must use exact format major.minor.patch.build: {0}")]
    InvalidPlatformVersion(String),

    #[error("build.partialLoadThreshold must be greater than or equal to 1")]
    InvalidPartialLoadThreshold,

    #[error("tests.execution_timeout_seconds must be between 1 and 86400 seconds")]
    InvalidTestExecutionTimeout,
}

/// Validate high-level application configuration consistency and filesystem references.
pub fn validate(config: &AppConfig) -> Result<(), ConfigValidationError> {
    validate_base_path(&config.base_path)?;
    validate_work_path(&config.work_path)?;
    validate_source_sets(config)?;
    validate_connection(config)?;
    validate_platform_version(config)?;
    validate_build_config(config)?;
    validate_test_config(config)?;
    Ok(())
}

fn validate_base_path(path: &Path) -> Result<(), ConfigValidationError> {
    if !path.exists() || !path.is_dir() {
        return Err(ConfigValidationError::BasePathInvalid(
            path.display().to_string(),
        ));
    }
    Ok(())
}

fn validate_work_path(path: &Path) -> Result<(), ConfigValidationError> {
    if !path.exists() {
        std::fs::create_dir_all(path).map_err(|e| {
            ConfigValidationError::WorkPathInvalid(format!("{}: {e}", path.display()))
        })?;
    }
    Ok(())
}

fn validate_source_sets(config: &AppConfig) -> Result<(), ConfigValidationError> {
    let has_config = config
        .source_sets
        .iter()
        .any(|s| s.purpose == SourceSetPurpose::Configuration);

    if !has_config {
        return Err(ConfigValidationError::NoConfigurationSourceSet);
    }

    let mut names = HashSet::<String>::new();
    let mut resolved_paths = HashSet::<String>::new();
    for ss in &config.source_sets {
        validate_source_set_name(&ss.name)?;

        if !names.insert(ss.name.clone()) {
            return Err(ConfigValidationError::DuplicateSourceSetName(
                ss.name.clone(),
            ));
        }

        let full_path = if ss.path.is_absolute() {
            ss.path.clone()
        } else {
            config.base_path.join(&ss.path)
        };

        if config.format == SourceFormat::Designer && !full_path.exists() {
            return Err(ConfigValidationError::SourceSetPathInvalid {
                name: ss.name.clone(),
                path: full_path.display().to_string(),
            });
        }

        let normalized = std::fs::canonicalize(&full_path).unwrap_or(full_path.clone());
        let normalized_key = normalized.display().to_string();
        if !resolved_paths.insert(normalized_key.clone()) {
            return Err(ConfigValidationError::DuplicateSourceSetPath(
                normalized_key,
            ));
        }
    }

    Ok(())
}

fn validate_source_set_name(name: &str) -> Result<(), ConfigValidationError> {
    if !is_safe_path_segment(name) {
        return Err(ConfigValidationError::InvalidSourceSetName(name.to_owned()));
    }

    Ok(())
}

fn validate_connection(config: &AppConfig) -> Result<(), ConfigValidationError> {
    if config.connection.trim().is_empty() {
        return Err(ConfigValidationError::EmptyConnection);
    }

    if config.builder == BuilderBackend::Ibcmd {
        let conn = config.connection.to_lowercase();
        if !conn.contains("file=") {
            return Err(ConfigValidationError::IbcmdRequiresFileConnection);
        }
    }

    Ok(())
}

fn validate_platform_version(config: &AppConfig) -> Result<(), ConfigValidationError> {
    if let Some(version) = config.tools.platform.version.as_deref() {
        if PlatformVersion::parse_strict(version).is_none() {
            return Err(ConfigValidationError::InvalidPlatformVersion(
                version.to_owned(),
            ));
        }
    }

    Ok(())
}

fn validate_build_config(config: &AppConfig) -> Result<(), ConfigValidationError> {
    if config.build.partial_load_threshold == 0 {
        return Err(ConfigValidationError::InvalidPartialLoadThreshold);
    }

    Ok(())
}

fn validate_test_config(config: &AppConfig) -> Result<(), ConfigValidationError> {
    if !(1..=86_400).contains(&config.tests.execution_timeout_seconds) {
        return Err(ConfigValidationError::InvalidTestExecutionTimeout);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{validate, ConfigValidationError};
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, PlatformToolConfig, SourceFormat, SourceSetConfig,
        SourceSetPurpose, TestsConfig, ToolsConfig,
    };
    use tempfile::tempdir;

    #[test]
    fn rejects_platform_versions_without_four_parts() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        std::fs::create_dir_all(&source_dir).expect("source dir");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            connection: "File=/tmp/ib".to_owned(),
            source_sets: vec![SourceSetConfig {
                name: "main".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: source_dir
                    .strip_prefix(base.path())
                    .expect("relative")
                    .to_path_buf(),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig {
                platform: PlatformToolConfig {
                    path: None,
                    version: Some("8.3.25".to_owned()),
                },
                ..ToolsConfig::default()
            },
            tests: TestsConfig::default(),
        };

        let err = validate(&config).expect_err("expected invalid version");
        assert!(matches!(
            err,
            ConfigValidationError::InvalidPlatformVersion(_)
        ));
    }

    #[test]
    fn rejects_source_set_name_with_parent_traversal() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        std::fs::create_dir_all(&source_dir).expect("source dir");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            connection: "File=/tmp/ib".to_owned(),
            source_sets: vec![SourceSetConfig {
                name: "../outside".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: source_dir
                    .strip_prefix(base.path())
                    .expect("relative")
                    .to_path_buf(),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig::default(),
            tests: TestsConfig::default(),
        };

        let err = validate(&config).expect_err("expected invalid source-set name");
        assert!(matches!(
            err,
            ConfigValidationError::InvalidSourceSetName(name) if name == "../outside"
        ));
    }

    #[test]
    fn rejects_source_set_name_with_path_separator() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        std::fs::create_dir_all(&source_dir).expect("source dir");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            connection: "File=/tmp/ib".to_owned(),
            source_sets: vec![SourceSetConfig {
                name: "bad/name".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: source_dir
                    .strip_prefix(base.path())
                    .expect("relative")
                    .to_path_buf(),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig::default(),
            tests: TestsConfig::default(),
        };

        let err = validate(&config).expect_err("expected invalid source-set name");
        assert!(matches!(
            err,
            ConfigValidationError::InvalidSourceSetName(name) if name == "bad/name"
        ));
    }

    #[test]
    fn accepts_safe_source_set_name() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        std::fs::create_dir_all(&source_dir).expect("source dir");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            connection: "File=/tmp/ib".to_owned(),
            source_sets: vec![SourceSetConfig {
                name: "main-config_01".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: source_dir
                    .strip_prefix(base.path())
                    .expect("relative")
                    .to_path_buf(),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig::default(),
            tests: TestsConfig::default(),
        };

        validate(&config).expect("safe source-set name should pass");
    }

    #[test]
    fn rejects_zero_partial_load_threshold() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        std::fs::create_dir_all(&source_dir).expect("source dir");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            connection: "File=/tmp/ib".to_owned(),
            source_sets: vec![SourceSetConfig {
                name: "main".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: source_dir
                    .strip_prefix(base.path())
                    .expect("relative")
                    .to_path_buf(),
            }],
            build: BuildConfig {
                partial_load_threshold: 0,
            },
            tools: ToolsConfig::default(),
            tests: TestsConfig::default(),
        };

        let err = validate(&config).expect_err("expected invalid partial load threshold");
        assert!(matches!(
            err,
            ConfigValidationError::InvalidPartialLoadThreshold
        ));
    }

    #[test]
    fn rejects_zero_test_execution_timeout() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        std::fs::create_dir_all(&source_dir).expect("source dir");

        let mut config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            connection: "File=/tmp/ib".to_owned(),
            source_sets: vec![SourceSetConfig {
                name: "main".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: source_dir
                    .strip_prefix(base.path())
                    .expect("relative")
                    .to_path_buf(),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig::default(),
            tests: TestsConfig::default(),
        };
        config.tests.execution_timeout_seconds = 0;

        let err = validate(&config).expect_err("expected invalid timeout");
        assert!(matches!(err, ConfigValidationError::InvalidTestExecutionTimeout));
    }
}
