use std::collections::HashSet;
use std::net::SocketAddr;
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

    #[error("source-set name is reserved for internal work directories: {0}")]
    ReservedSourceSetName(String),

    #[error("source-set resolved path must be unique, duplicate: {0}")]
    DuplicateSourceSetPath(String),

    #[error("connection string is empty")]
    EmptyConnection,

    #[error("IBCMD builder requires a file-based connection string (File=... or /F <path>)")]
    IbcmdRequiresFileConnection,

    #[error("EDT format requires builder=DESIGNER")]
    EdtRequiresDesignerBuilder,

    #[error("format EDT requires at least one source-set with a valid EDT project path")]
    EdtNoProjects,

    #[error(
        "EDT source-set '{source_set}' path overlaps generated work target for '{generated_for}': source={source_path}, target={generated_path}"
    )]
    EdtSourceSetPathOverlapsGeneratedTarget {
        source_set: String,
        source_path: String,
        generated_for: String,
        generated_path: String,
    },

    #[error("platform version must use exact format major.minor.patch.build: {0}")]
    InvalidPlatformVersion(String),

    #[error("build.partialLoadThreshold must be greater than or equal to 1")]
    InvalidPartialLoadThreshold,

    #[error("tests.execution_timeout_seconds must be between 1 and 86400 seconds")]
    InvalidTestExecutionTimeout,

    #[error("mcp.http.bind_address must be a valid socket address: {0}")]
    InvalidMcpBindAddress(String),

    #[error("mcp.http.path must be a non-empty absolute path starting with '/': {0}")]
    InvalidMcpHttpPath(String),

    #[error("mcp.http.max_sessions must be greater than or equal to 1")]
    InvalidMcpMaxSessions,

    #[error("mcp.http.idle_ttl_secs must be greater than or equal to 1")]
    InvalidMcpIdleTtlSecs,

    #[error("mcp.execution.max_concurrent_calls must be greater than or equal to 1")]
    InvalidMcpMaxConcurrentCalls,

    #[error("mcp.execution.shutdown_grace_period_secs must be greater than or equal to 1")]
    InvalidMcpShutdownGracePeriodSecs,

    #[error("tools.edt_cli.startup_timeout_ms must be greater than or equal to 1")]
    InvalidEdtCliStartupTimeoutMs,

    #[error("tools.edt_cli.command_timeout_ms must be greater than or equal to 1")]
    InvalidEdtCliCommandTimeoutMs,
}

/// Validate high-level application configuration consistency and filesystem references.
pub fn validate(config: &AppConfig) -> Result<(), ConfigValidationError> {
    validate_base_path(&config.base_path)?;
    validate_work_path(&config.work_path)?;
    validate_matrix(config)?;
    validate_source_sets(config)?;
    validate_connection(config)?;
    validate_platform_version(config)?;
    validate_build_config(config)?;
    validate_test_config(config)?;
    validate_mcp_config(config)?;
    validate_edt_cli_config(config)?;
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
    if config.format == SourceFormat::Edt && config.source_sets.is_empty() {
        return Err(ConfigValidationError::EdtNoProjects);
    }

    let has_config = config
        .source_sets
        .iter()
        .any(|s| s.purpose == SourceSetPurpose::Configuration);

    if !has_config {
        return Err(ConfigValidationError::NoConfigurationSourceSet);
    }

    let mut names = HashSet::<String>::new();
    let mut resolved_paths = HashSet::<String>::new();
    let mut edt_source_paths = Vec::new();
    for ss in &config.source_sets {
        validate_source_set_name(&ss.name)?;
        if config.format == SourceFormat::Edt && is_reserved_workdir_name(&ss.name) {
            return Err(ConfigValidationError::ReservedSourceSetName(
                ss.name.clone(),
            ));
        }

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

        // DESIGNER source-set paths also serve as dump targets and may be created by the dump
        // use case on demand. EDT source-sets must still resolve to existing project paths.
        if config.format == SourceFormat::Edt && !full_path.exists() {
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

        if config.format == SourceFormat::Edt {
            edt_source_paths.push((ss.name.clone(), normalized));
        }
    }

    if config.format == SourceFormat::Edt {
        validate_edt_runtime_paths(config, &edt_source_paths)?;
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

    if config.builder == BuilderBackend::Ibcmd && config.v8_connection().file_path().is_none() {
        return Err(ConfigValidationError::IbcmdRequiresFileConnection);
    }

    Ok(())
}

fn validate_edt_runtime_paths(
    config: &AppConfig,
    edt_source_paths: &[(String, std::path::PathBuf)],
) -> Result<(), ConfigValidationError> {
    let canonical_work_path =
        std::fs::canonicalize(&config.work_path).unwrap_or_else(|_| config.work_path.clone());

    for (generated_for, _) in edt_source_paths {
        let generated_path = canonical_work_path.join("designer").join(generated_for);
        for (source_set, source_path) in edt_source_paths {
            if paths_overlap(source_path, &generated_path) {
                return Err(
                    ConfigValidationError::EdtSourceSetPathOverlapsGeneratedTarget {
                        source_set: source_set.clone(),
                        source_path: source_path.display().to_string(),
                        generated_for: generated_for.clone(),
                        generated_path: generated_path.display().to_string(),
                    },
                );
            }
        }
    }

    Ok(())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn is_reserved_workdir_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "hash-storages" | "logs" | "temp" | "edt-workspace" | "designer"
    )
}

fn validate_matrix(config: &AppConfig) -> Result<(), ConfigValidationError> {
    if config.format == SourceFormat::Edt && config.builder == BuilderBackend::Ibcmd {
        return Err(ConfigValidationError::EdtRequiresDesignerBuilder);
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

fn validate_mcp_config(config: &AppConfig) -> Result<(), ConfigValidationError> {
    if config.mcp.http.bind_address.parse::<SocketAddr>().is_err() {
        return Err(ConfigValidationError::InvalidMcpBindAddress(
            config.mcp.http.bind_address.clone(),
        ));
    }

    if config.mcp.http.path.trim().is_empty() || !config.mcp.http.path.starts_with('/') {
        return Err(ConfigValidationError::InvalidMcpHttpPath(
            config.mcp.http.path.clone(),
        ));
    }

    if config.mcp.http.max_sessions == 0 {
        return Err(ConfigValidationError::InvalidMcpMaxSessions);
    }

    if config.mcp.http.idle_ttl_secs == 0 {
        return Err(ConfigValidationError::InvalidMcpIdleTtlSecs);
    }

    if config.mcp.execution.max_concurrent_calls == 0 {
        return Err(ConfigValidationError::InvalidMcpMaxConcurrentCalls);
    }

    if config.mcp.execution.shutdown_grace_period_secs == 0 {
        return Err(ConfigValidationError::InvalidMcpShutdownGracePeriodSecs);
    }

    Ok(())
}

fn validate_edt_cli_config(config: &AppConfig) -> Result<(), ConfigValidationError> {
    if config.tools.edt_cli.startup_timeout_ms == 0 {
        return Err(ConfigValidationError::InvalidEdtCliStartupTimeoutMs);
    }

    if config.tools.edt_cli.command_timeout_ms == 0 {
        return Err(ConfigValidationError::InvalidEdtCliCommandTimeoutMs);
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
            credentials: Default::default(),
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
            mcp: Default::default(),
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
            credentials: Default::default(),
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
            mcp: Default::default(),
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
            credentials: Default::default(),
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
            mcp: Default::default(),
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
            credentials: Default::default(),
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
            mcp: Default::default(),
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
            credentials: Default::default(),
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
            mcp: Default::default(),
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
            credentials: Default::default(),
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
            mcp: Default::default(),
            tests: TestsConfig::default(),
        };
        config.tests.execution_timeout_seconds = 0;

        let err = validate(&config).expect_err("expected invalid timeout");
        assert!(matches!(
            err,
            ConfigValidationError::InvalidTestExecutionTimeout
        ));
    }

    #[test]
    fn rejects_edt_with_ibcmd_builder() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("edt-main");
        std::fs::create_dir_all(&source_dir).expect("source dir");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            format: SourceFormat::Edt,
            builder: BuilderBackend::Ibcmd,
            connection: "File=/tmp/ib".to_owned(),
            credentials: Default::default(),
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
            mcp: Default::default(),
            tests: TestsConfig::default(),
        };

        let err = validate(&config).expect_err("expected invalid edt+ibcmd matrix");
        assert!(matches!(
            err,
            ConfigValidationError::EdtRequiresDesignerBuilder
        ));
    }

    #[test]
    fn designer_format_allows_missing_source_set_path() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let missing = base.path().join("missing-src");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            connection: "File=/tmp/ib".to_owned(),
            credentials: Default::default(),
            source_sets: vec![SourceSetConfig {
                name: "main".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: missing
                    .strip_prefix(base.path())
                    .expect("relative")
                    .to_path_buf(),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig::default(),
            mcp: Default::default(),
            tests: TestsConfig::default(),
        };

        validate(&config).expect("designer config should allow missing dump target path");
    }

    #[test]
    fn ibcmd_allows_raw_f_connection() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        std::fs::create_dir_all(&source_dir).expect("source dir");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            format: SourceFormat::Designer,
            builder: BuilderBackend::Ibcmd,
            connection: "/F /tmp/ib".to_owned(),
            credentials: Default::default(),
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
            mcp: Default::default(),
            tests: TestsConfig::default(),
        };

        validate(&config).expect("raw /F connection should be accepted for IBCMD");
    }

    #[test]
    fn edt_format_rejects_missing_source_set_path() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let missing = base.path().join("missing-project");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            format: SourceFormat::Edt,
            builder: BuilderBackend::Designer,
            connection: "File=/tmp/ib".to_owned(),
            credentials: Default::default(),
            source_sets: vec![SourceSetConfig {
                name: "main".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: missing
                    .strip_prefix(base.path())
                    .expect("relative")
                    .to_path_buf(),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig::default(),
            mcp: Default::default(),
            tests: TestsConfig::default(),
        };

        let err = validate(&config).expect_err("expected missing edt source-set path");

        assert!(matches!(
            err,
            ConfigValidationError::SourceSetPathInvalid { name, .. } if name == "main"
        ));
    }

    #[test]
    fn rejects_edt_without_source_sets() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            format: SourceFormat::Edt,
            builder: BuilderBackend::Designer,
            connection: "File=/tmp/ib".to_owned(),
            credentials: Default::default(),
            source_sets: vec![],
            build: BuildConfig::default(),
            tools: ToolsConfig::default(),
            mcp: Default::default(),
            tests: TestsConfig::default(),
        };

        let err = validate(&config).expect_err("expected missing edt source sets");
        assert!(matches!(err, ConfigValidationError::EdtNoProjects));
    }

    #[test]
    fn rejects_edt_source_set_path_overlapping_generated_work_target() {
        let shared = tempdir().expect("shared");
        let source_dir = shared.path().join("designer").join("main");
        std::fs::create_dir_all(&source_dir).expect("source dir");

        let config = AppConfig {
            base_path: shared.path().to_path_buf(),
            work_path: shared.path().to_path_buf(),
            format: SourceFormat::Edt,
            builder: BuilderBackend::Designer,
            connection: "File=/tmp/ib".to_owned(),
            credentials: Default::default(),
            source_sets: vec![SourceSetConfig {
                name: "main".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: std::path::PathBuf::from("designer/main"),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig::default(),
            mcp: Default::default(),
            tests: TestsConfig::default(),
        };

        let err = validate(&config).expect_err("expected EDT overlap validation error");
        assert!(matches!(
            err,
            ConfigValidationError::EdtSourceSetPathOverlapsGeneratedTarget {
                source_set,
                generated_for,
                ..
            } if source_set == "main" && generated_for == "main"
        ));
    }

    #[test]
    fn rejects_reserved_source_set_name() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        std::fs::create_dir_all(&source_dir).expect("source dir");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            format: SourceFormat::Edt,
            builder: BuilderBackend::Designer,
            connection: "File=/tmp/ib".to_owned(),
            credentials: Default::default(),
            source_sets: vec![SourceSetConfig {
                name: "hash-storages".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: source_dir
                    .strip_prefix(base.path())
                    .expect("relative")
                    .to_path_buf(),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig::default(),
            mcp: Default::default(),
            tests: TestsConfig::default(),
        };

        let err = validate(&config).expect_err("expected reserved source-set name");
        assert!(matches!(
            err,
            ConfigValidationError::ReservedSourceSetName(name) if name == "hash-storages"
        ));
    }

    #[test]
    fn edt_ibcmd_returns_matrix_error_before_connection_check() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("edt-main");
        std::fs::create_dir_all(&source_dir).expect("source dir");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            format: SourceFormat::Edt,
            builder: BuilderBackend::Ibcmd,
            connection: "Srvr=localhost;Ref=ib".to_owned(),
            credentials: Default::default(),
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
            mcp: Default::default(),
            tests: TestsConfig::default(),
        };

        let err = validate(&config).expect_err("expected EDT matrix validation error");
        assert!(matches!(
            err,
            ConfigValidationError::EdtRequiresDesignerBuilder
        ));
    }

    #[test]
    fn rejects_reserved_source_set_name_case_insensitively_for_edt() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        std::fs::create_dir_all(&source_dir).expect("source dir");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            format: SourceFormat::Edt,
            builder: BuilderBackend::Designer,
            connection: "File=/tmp/ib".to_owned(),
            credentials: Default::default(),
            source_sets: vec![SourceSetConfig {
                name: "Logs".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: source_dir
                    .strip_prefix(base.path())
                    .expect("relative")
                    .to_path_buf(),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig::default(),
            mcp: Default::default(),
            tests: TestsConfig::default(),
        };

        let err = validate(&config).expect_err("expected reserved source-set name");
        assert!(matches!(
            err,
            ConfigValidationError::ReservedSourceSetName(name) if name == "Logs"
        ));
    }

    #[test]
    fn edt_ibcmd_matrix_error_has_priority_over_source_set_path_validation() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            format: SourceFormat::Edt,
            builder: BuilderBackend::Ibcmd,
            connection: "File=/tmp/ib".to_owned(),
            credentials: Default::default(),
            source_sets: vec![SourceSetConfig {
                name: "main".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: std::path::PathBuf::from("missing-path"),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig::default(),
            mcp: Default::default(),
            tests: TestsConfig::default(),
        };

        let err = validate(&config).expect_err("expected EDT matrix error");
        assert!(matches!(
            err,
            ConfigValidationError::EdtRequiresDesignerBuilder
        ));
    }

    #[test]
    fn rejects_invalid_mcp_bind_address() {
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
            credentials: Default::default(),
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
            mcp: Default::default(),
            tests: TestsConfig::default(),
        };
        config.mcp.http.bind_address = "localhost".to_owned();

        let err = validate(&config).expect_err("expected invalid MCP bind address");
        assert!(matches!(
            err,
            ConfigValidationError::InvalidMcpBindAddress(value) if value == "localhost"
        ));
    }

    #[test]
    fn rejects_invalid_mcp_http_limits() {
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
            credentials: Default::default(),
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
            mcp: Default::default(),
            tests: TestsConfig::default(),
        };

        config.mcp.http.path = "mcp".to_owned();
        let err = validate(&config).expect_err("expected invalid MCP path");
        assert!(matches!(
            err,
            ConfigValidationError::InvalidMcpHttpPath(value) if value == "mcp"
        ));

        config.mcp.http.path = "/mcp".to_owned();
        config.mcp.http.max_sessions = 0;
        let err = validate(&config).expect_err("expected invalid MCP max sessions");
        assert!(matches!(err, ConfigValidationError::InvalidMcpMaxSessions));

        config.mcp.http.max_sessions = 64;
        config.mcp.execution.max_concurrent_calls = 0;
        let err = validate(&config).expect_err("expected invalid MCP concurrency");
        assert!(matches!(
            err,
            ConfigValidationError::InvalidMcpMaxConcurrentCalls
        ));
    }

    #[test]
    fn rejects_zero_edt_cli_timeouts() {
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
            credentials: Default::default(),
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
            mcp: Default::default(),
            tests: TestsConfig::default(),
        };

        config.tools.edt_cli.startup_timeout_ms = 0;
        let err = validate(&config).expect_err("expected invalid EDT startup timeout");
        assert!(matches!(
            err,
            ConfigValidationError::InvalidEdtCliStartupTimeoutMs
        ));

        config.tools.edt_cli.startup_timeout_ms = 300_000;
        config.tools.edt_cli.command_timeout_ms = 0;
        let err = validate(&config).expect_err("expected invalid EDT command timeout");
        assert!(matches!(
            err,
            ConfigValidationError::InvalidEdtCliCommandTimeoutMs
        ));
    }
}
