use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path;
use thiserror::Error;

use crate::config::model::{
    AppConfig, BuilderBackend, SourceFormat, SourceSetConfig, SourceSetPurpose,
    ToolExtensionConfig, ToolExtensionInput, ToolExtensionSourceConfig, VanessaProfileConfig,
};
use crate::platform::locator::PlatformVersionRequirement;
use crate::support::edt_project::{self, EdtProjectKind};
use crate::support::path::is_safe_path_segment;
use crate::support::source_descriptor::{self, SourceDescriptorPurpose, SourceSetRootScanError};

#[derive(Debug, Error)]
pub enum ConfigValidationError {
    #[error("{0}")]
    InvalidYamlRoot(String),

    #[error("project base path does not exist or is not a directory: {0}")]
    BasePathInvalid(String),

    #[error("workPath could not be created: {0}")]
    WorkPathInvalid(String),

    #[error("source-set must contain at least one supported entry")]
    NoSupportedSourceSet,

    #[error("EXTENSION source-set requires at least one CONFIGURATION source-set")]
    ExtensionRequiresConfiguration,

    #[error("source-set entry '{name}' path does not exist: {path}")]
    SourceSetPathInvalid { name: String, path: String },

    #[error("source-set '{name}' has invalid layout: {details}")]
    SourceSetLayoutInvalid { name: String, details: String },

    #[error("source-set name must be unique, duplicate: {0}")]
    DuplicateSourceSetName(String),

    #[error("source-set name contains unsafe path or filename characters: {0}")]
    InvalidSourceSetName(String),

    #[error("tools.client_mcp.extension.name must not duplicate project source-set name: {0}")]
    ToolExtensionNameDuplicatesSourceSet(String),

    #[error("tests.va.profile name contains unsafe path or filename characters: {0}")]
    InvalidVanessaProfileName(String),

    #[error("source-set name is reserved for internal work directories: {0}")]
    ReservedSourceSetName(String),

    #[error("source-set resolved path must be unique, duplicate: {0}")]
    DuplicateSourceSetPath(String),

    #[error("infobase.connection is empty")]
    EmptyConnection,

    #[error("legacy top-level key 'connection' is not supported; use infobase.connection")]
    LegacyTopLevelConnection,

    #[error("legacy top-level key 'credentials' is not supported; use infobase.user/password")]
    LegacyTopLevelCredentials,

    #[error("legacy key 'mcp.client' is not supported; use tools.client_mcp")]
    LegacyMcpClientConfig,

    #[error("legacy key 'tests.va.epf_path' is not supported; use tools.va.epf_path")]
    LegacyVanessaEpfPath,

    #[error(
        "top-level key 'execution_timeout_seconds' is not supported; use execution_timeout in milliseconds or tests.execution_timeout_seconds for test runs"
    )]
    LegacyTopLevelExecutionTimeoutSeconds,

    #[error("infobase.dbms is not allowed for file-based infobase.connection")]
    DbmsNotAllowedForFileConnection,

    #[error("builder=IBCMD with server-based infobase.connection requires infobase.dbms.{0}")]
    MissingIbcmdServerDbmsField(&'static str),

    #[error("format EDT requires at least one source-set with a valid EDT project path")]
    EdtNoProjects,

    #[error("external source-set '{name}' requires builder=DESIGNER")]
    ExternalSourceSetRequiresDesigner { name: String },

    #[error(
        "external EDT source-set '{name}' must contain at least one child project with .project"
    )]
    ExternalEdtSourceSetHasNoProjects { name: String },

    #[error(
        "EDT source-set '{source_set}' path overlaps generated work target for '{generated_for}': source={source_path}, target={generated_path}"
    )]
    EdtSourceSetPathOverlapsGeneratedTarget {
        source_set: String,
        source_path: String,
        generated_for: String,
        generated_path: String,
    },

    #[error("platform version must use format major.minor, major.minor.patch or major.minor.patch.build: {0}")]
    InvalidPlatformVersion(String),

    #[error("build.partialLoadThreshold must be greater than or equal to 1")]
    InvalidPartialLoadThreshold,

    #[error("execution_timeout must be between 1 and 86400000 milliseconds")]
    InvalidExecutionTimeout,

    #[error("tests.execution_timeout_seconds must be between 1 and 86400 seconds")]
    InvalidTestExecutionTimeout,

    #[error("tests.yaxunit.timeouts.{0} must be greater than or equal to 1")]
    InvalidYaxunitTimeout(&'static str),

    #[error("tools.va.epf_path is required when Vanessa Automation is configured")]
    MissingVanessaEpfPath,

    #[error("tests.va.params_path is required when Vanessa Automation is configured")]
    MissingVanessaParamsPath,

    #[error("tests.va.profile is required when Vanessa Automation is configured")]
    MissingVanessaProfile,

    #[error("tests.va.profile references unknown profile '{0}'")]
    UnknownVanessaProfile(String),

    #[error("tools.va.epf_path does not exist: {0}")]
    VanessaEpfPathInvalid(String),

    #[error("tests.va.params_path does not exist: {0}")]
    VanessaParamsPathInvalid(String),

    #[error("tests.va.profiles.{profile}.feature_path is required")]
    MissingVanessaFeaturePath { profile: String },

    #[error("tests.va.profiles.{profile}.feature_path does not exist: {path}")]
    VanessaFeaturePathInvalid { profile: String, path: String },

    #[error("tests.va.timeouts.{0} must be greater than or equal to 1")]
    InvalidVanessaTimeout(&'static str),

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

    #[error("tools.client_mcp.port must be greater than or equal to 1")]
    InvalidMcpClientPort,

    #[error("tools.client_mcp.extension.name must be a safe non-empty extension name: {0}")]
    InvalidToolExtensionName(String),

    #[error("tools.client_mcp.extension.source.path does not exist or is not a directory: {0}")]
    ToolExtensionSourcePathInvalid(String),

    #[error("tools.client_mcp.extension.source has invalid layout: {0}")]
    ToolExtensionSourceLayoutInvalid(String),

    #[error("tools.client_mcp.extension.artifact.path must point to an existing .cfe file: {0}")]
    ToolExtensionArtifactPathInvalid(String),

    #[error("tools.client_mcp.extension.artifact is supported only with builder=DESIGNER")]
    ToolExtensionArtifactRequiresDesigner,

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
    validate_execution_timeout(config)?;
    validate_test_config(config)?;
    validate_mcp_config(config)?;
    validate_client_mcp_tool_extension(config)?;
    validate_edt_cli_config(config)?;
    Ok(())
}

/// Validate only the configuration parts required to bootstrap downloaded tools.
///
/// `tools download` may be invoked specifically to create Vanessa Automation and
/// client MCP paths, so those tool-dependent checks must not block the command.
pub fn validate_tools_download_bootstrap(config: &AppConfig) -> Result<(), ConfigValidationError> {
    validate_base_path(&config.base_path)?;
    validate_work_path(&config.work_path)?;
    validate_matrix(config)?;
    validate_connection(config)?;
    validate_platform_version(config)?;
    validate_build_config(config)?;
    validate_execution_timeout(config)?;
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

    let has_supported = !config.source_sets.is_empty();
    if !has_supported {
        return Err(ConfigValidationError::NoSupportedSourceSet);
    }

    let has_config = config
        .source_sets
        .iter()
        .any(|s| s.purpose == SourceSetPurpose::Configuration);

    let has_extension = config
        .source_sets
        .iter()
        .any(|s| s.purpose == SourceSetPurpose::Extension);

    if has_extension && !has_config {
        return Err(ConfigValidationError::ExtensionRequiresConfiguration);
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

        let path_must_exist = config.format == SourceFormat::Edt || ss.purpose.is_external();
        if path_must_exist && !full_path.exists() {
            return Err(ConfigValidationError::SourceSetPathInvalid {
                name: ss.name.clone(),
                path: full_path.display().to_string(),
            });
        }
        if path_must_exist && !full_path.is_dir() {
            return Err(source_set_layout_error(
                &ss.name,
                format!("path must be a directory: {}", full_path.display()),
            ));
        }

        if ss.purpose.is_external() && config.builder != BuilderBackend::Designer {
            return Err(ConfigValidationError::ExternalSourceSetRequiresDesigner {
                name: ss.name.clone(),
            });
        }

        validate_source_set_layout(config.format, ss, &full_path)?;

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

fn validate_source_set_layout(
    format: SourceFormat,
    source_set: &SourceSetConfig,
    full_path: &Path,
) -> Result<(), ConfigValidationError> {
    match format {
        SourceFormat::Designer if source_set.purpose.is_external() => {
            validate_designer_external_source_set_layout(source_set, full_path)
        }
        SourceFormat::Edt if source_set.purpose.is_external() => {
            validate_edt_external_source_set_layout(source_set, full_path)
        }
        SourceFormat::Edt => validate_ordinary_edt_source_set_layout(source_set, full_path),
        SourceFormat::Designer => Ok(()),
    }
}

fn validate_ordinary_edt_source_set_layout(
    source_set: &SourceSetConfig,
    path: &Path,
) -> Result<(), ConfigValidationError> {
    let Some(project) = read_edt_project_descriptor(path, &source_set.name)? else {
        return Err(source_set_layout_error(
            &source_set.name,
            format!("EDT source-set must contain '.project': {}", path.display()),
        ));
    };

    let kind = project.kind();
    let detected = if let Some(mapped) = kind.and_then(map_native_edt_kind) {
        mapped
    } else if kind == Some(EdtProjectKind::ExternalObjects) {
        return Err(source_set_layout_error(
            &source_set.name,
            format!(
                "EDT project nature resolves to external objects but source-set declares {}: {}",
                source_set_type_label(source_set.purpose),
                path.display()
            ),
        ));
    } else {
        return Err(source_set_layout_error(
            &source_set.name,
            format!(
                "EDT source-set must declare exactly one ordinary EDT nature ({} or {}): {}",
                edt_project::V8_CONFIGURATION_NATURE,
                edt_project::V8_EXTENSION_NATURE,
                path.display()
            ),
        ));
    };
    if detected != source_set.purpose {
        return Err(source_set_layout_error(
            &source_set.name,
            format!(
                "EDT project nature resolves to {} but source-set declares {}: {}",
                source_set_type_label(detected),
                source_set_type_label(source_set.purpose),
                path.display()
            ),
        ));
    }
    let Some(manifest) = read_edt_project_manifest(path, &source_set.name)? else {
        return Err(source_set_layout_error(
            &source_set.name,
            format!(
                "EDT source-set must contain 'DT-INF/PROJECT.PMF': {}",
                path.display()
            ),
        ));
    };
    if !manifest
        .runtime_version
        .as_deref()
        .is_some_and(edt_project::is_valid_runtime_version)
    {
        return Err(source_set_layout_error(
            &source_set.name,
            format!(
                "EDT source-set must declare Runtime-Version in 'DT-INF/PROJECT.PMF' using 'x.y.z': {}",
                path.display()
            ),
        ));
    }
    if !has_native_ordinary_edt_root_marker(path) {
        return Err(source_set_layout_error(
            &source_set.name,
            format!(
                "EDT source-set must contain 'src/Configuration/Configuration.mdo': {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn validate_edt_external_source_set_layout(
    source_set: &SourceSetConfig,
    path: &Path,
) -> Result<(), ConfigValidationError> {
    let entries = source_descriptor::scan_edt_external_root(path).map_err(|error| {
        source_set_layout_error(&source_set.name, root_scan_error_message(error))
    })?;

    if entries.is_empty() {
        return Err(ConfigValidationError::ExternalEdtSourceSetHasNoProjects {
            name: source_set.name.clone(),
        });
    }

    for entry in entries {
        let child = entry.path;
        let detected = entry.purpose.map(map_source_descriptor_purpose);
        match detected {
            Some(detected) if detected == source_set.purpose => {}
            Some(detected) => {
                return Err(source_set_layout_error(
                    &source_set.name,
                    format!(
                        "EDT external child project '{}' resolves to {} but source-set declares {}",
                        child.display(),
                        source_set_type_label(detected),
                        source_set_type_label(source_set.purpose)
                    ),
                ));
            }
            None => {
                return Err(source_set_layout_error(
                    &source_set.name,
                    format!(
                        "EDT external child project '{}' must contain descriptors for {}",
                        child.display(),
                        source_set_type_label(source_set.purpose)
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn validate_designer_external_source_set_layout(
    source_set: &SourceSetConfig,
    path: &Path,
) -> Result<(), ConfigValidationError> {
    let entries = source_descriptor::scan_designer_external_root(path).map_err(|error| {
        source_set_layout_error(&source_set.name, root_scan_error_message(error))
    })?;

    if entries.is_empty() {
        return Err(source_set_layout_error(
            &source_set.name,
            format!(
                "Designer external source-set must contain top-level XML descriptors for {}: {}",
                source_set_type_label(source_set.purpose),
                path.display()
            ),
        ));
    }

    for entry in entries {
        let descriptor = entry.path;
        let detected = entry
            .purpose
            .map(map_source_descriptor_purpose)
            .ok_or_else(|| {
                source_set_layout_error(
                    &source_set.name,
                    format!(
                        "top-level XML descriptor '{}' is not a supported external descriptor",
                        descriptor.display()
                    ),
                )
            })?;
        if detected != source_set.purpose {
            return Err(source_set_layout_error(
                &source_set.name,
                format!(
                    "top-level XML descriptor '{}' resolves to {} but source-set declares {}",
                    descriptor.display(),
                    source_set_type_label(detected),
                    source_set_type_label(source_set.purpose)
                ),
            ));
        }
    }
    Ok(())
}

fn map_native_edt_kind(kind: EdtProjectKind) -> Option<SourceSetPurpose> {
    match kind {
        EdtProjectKind::Configuration => Some(SourceSetPurpose::Configuration),
        EdtProjectKind::Extension => Some(SourceSetPurpose::Extension),
        EdtProjectKind::ExternalObjects => None,
    }
}

fn read_edt_project_descriptor(
    project_dir: &Path,
    source_set_name: &str,
) -> Result<Option<edt_project::EdtProjectDescriptor>, ConfigValidationError> {
    edt_project::read_project_descriptor_from_dir(project_dir)
        .map_err(|error| source_set_layout_error(source_set_name, error))
}

fn read_edt_project_manifest(
    project_dir: &Path,
    source_set_name: &str,
) -> Result<Option<edt_project::EdtProjectManifest>, ConfigValidationError> {
    edt_project::read_project_manifest_from_dir(project_dir)
        .map_err(|error| source_set_layout_error(source_set_name, error))
}

fn has_native_ordinary_edt_root_marker(project_dir: &Path) -> bool {
    edt_project::ordinary_root_marker_path(project_dir).is_file()
}

fn map_source_descriptor_purpose(purpose: SourceDescriptorPurpose) -> SourceSetPurpose {
    match purpose {
        SourceDescriptorPurpose::Configuration => SourceSetPurpose::Configuration,
        SourceDescriptorPurpose::Extension => SourceSetPurpose::Extension,
        SourceDescriptorPurpose::ExternalDataProcessors => SourceSetPurpose::ExternalDataProcessors,
        SourceDescriptorPurpose::ExternalReports => SourceSetPurpose::ExternalReports,
    }
}

fn root_scan_error_message(error: SourceSetRootScanError) -> String {
    match error {
        SourceSetRootScanError::Runtime(message) | SourceSetRootScanError::Validation(message) => {
            message
        }
    }
}

fn source_set_layout_error(name: &str, details: impl Into<String>) -> ConfigValidationError {
    ConfigValidationError::SourceSetLayoutInvalid {
        name: name.to_owned(),
        details: details.into(),
    }
}

fn source_set_type_label(purpose: SourceSetPurpose) -> &'static str {
    match purpose {
        SourceSetPurpose::Configuration => "CONFIGURATION",
        SourceSetPurpose::Extension => "EXTENSION",
        SourceSetPurpose::ExternalDataProcessors => "EXTERNAL_DATA_PROCESSORS",
        SourceSetPurpose::ExternalReports => "EXTERNAL_REPORTS",
    }
}

fn validate_source_set_name(name: &str) -> Result<(), ConfigValidationError> {
    if !is_safe_path_segment(name) {
        return Err(ConfigValidationError::InvalidSourceSetName(name.to_owned()));
    }

    Ok(())
}

fn validate_connection(config: &AppConfig) -> Result<(), ConfigValidationError> {
    if config.infobase.connection.trim().is_empty() {
        return Err(ConfigValidationError::EmptyConnection);
    }

    let is_file_connection = config.v8_connection().file_path().is_some();
    if is_file_connection {
        if config.infobase.dbms.is_some() {
            return Err(ConfigValidationError::DbmsNotAllowedForFileConnection);
        }
        return Ok(());
    }

    if config.builder != BuilderBackend::Ibcmd {
        return Ok(());
    }

    let Some(dbms) = config.infobase.dbms.as_ref() else {
        return Err(ConfigValidationError::MissingIbcmdServerDbmsField("kind"));
    };
    if option_is_blank(dbms.kind.as_deref()) {
        return Err(ConfigValidationError::MissingIbcmdServerDbmsField("kind"));
    }
    if option_is_blank(dbms.server.as_deref()) {
        return Err(ConfigValidationError::MissingIbcmdServerDbmsField("server"));
    }
    if option_is_blank(dbms.name.as_deref()) {
        return Err(ConfigValidationError::MissingIbcmdServerDbmsField("name"));
    }

    Ok(())
}

fn option_is_blank(value: Option<&str>) -> bool {
    match value {
        Some(value) => value.trim().is_empty(),
        None => true,
    }
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

fn validate_matrix(_config: &AppConfig) -> Result<(), ConfigValidationError> {
    Ok(())
}

fn validate_platform_version(config: &AppConfig) -> Result<(), ConfigValidationError> {
    if let Some(version) = config.tools.platform.version.as_deref() {
        if PlatformVersionRequirement::parse(version).is_none() {
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

fn validate_execution_timeout(config: &AppConfig) -> Result<(), ConfigValidationError> {
    if !(1..=86_400_000).contains(&config.execution_timeout) {
        return Err(ConfigValidationError::InvalidExecutionTimeout);
    }

    Ok(())
}

fn validate_test_config(config: &AppConfig) -> Result<(), ConfigValidationError> {
    if !(1..=86_400).contains(&config.tests.execution_timeout_seconds) {
        return Err(ConfigValidationError::InvalidTestExecutionTimeout);
    }

    validate_timeout_block(
        &config.tests.yaxunit.timeouts,
        ConfigValidationError::InvalidYaxunitTimeout,
    )?;
    validate_timeout_block(
        &config.tests.va.timeouts,
        ConfigValidationError::InvalidVanessaTimeout,
    )?;

    let va = &config.tests.va;
    if !va.is_configured() {
        return Ok(());
    }

    let epf_path = config
        .tools
        .va
        .epf_path
        .as_ref()
        .ok_or(ConfigValidationError::MissingVanessaEpfPath)?;
    if !epf_path.exists() {
        return Err(ConfigValidationError::VanessaEpfPathInvalid(
            epf_path.display().to_string(),
        ));
    }

    let params_path = va
        .params_path
        .as_ref()
        .ok_or(ConfigValidationError::MissingVanessaParamsPath)?;
    if !params_path.exists() {
        return Err(ConfigValidationError::VanessaParamsPathInvalid(
            params_path.display().to_string(),
        ));
    }

    let profile_name = va
        .profile
        .as_deref()
        .ok_or(ConfigValidationError::MissingVanessaProfile)?;
    validate_vanessa_profile_name(profile_name)?;
    let profile = va
        .profiles
        .get(profile_name)
        .ok_or_else(|| ConfigValidationError::UnknownVanessaProfile(profile_name.to_owned()))?;
    for key in va.profiles.keys() {
        validate_vanessa_profile_name(key)?;
    }
    validate_vanessa_profile(profile_name, profile)?;

    Ok(())
}

fn validate_vanessa_profile(
    profile_name: &str,
    profile: &VanessaProfileConfig,
) -> Result<(), ConfigValidationError> {
    let feature_path = profile.feature_path.as_ref().ok_or_else(|| {
        ConfigValidationError::MissingVanessaFeaturePath {
            profile: profile_name.to_owned(),
        }
    })?;

    if !feature_path.exists() {
        return Err(ConfigValidationError::VanessaFeaturePathInvalid {
            profile: profile_name.to_owned(),
            path: feature_path.display().to_string(),
        });
    }

    Ok(())
}

fn validate_vanessa_profile_name(profile_name: &str) -> Result<(), ConfigValidationError> {
    if !is_safe_path_segment(profile_name) {
        return Err(ConfigValidationError::InvalidVanessaProfileName(
            profile_name.to_owned(),
        ));
    }

    Ok(())
}

fn validate_timeout_block(
    timeouts: &crate::domain::execution::ExecutionTimeouts,
    error_factory: fn(&'static str) -> ConfigValidationError,
) -> Result<(), ConfigValidationError> {
    for (name, value) in [
        ("startup_ms", timeouts.startup_ms),
        ("run_ms", timeouts.run_ms),
        ("total_ms", timeouts.total_ms),
    ] {
        if matches!(value, Some(0)) {
            return Err(error_factory(name));
        }
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

    if config.tools.client_mcp.port == Some(0) {
        return Err(ConfigValidationError::InvalidMcpClientPort);
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

fn validate_client_mcp_tool_extension(config: &AppConfig) -> Result<(), ConfigValidationError> {
    let Some(extension) = config.tools.client_mcp.extension.as_ref() else {
        return Ok(());
    };

    validate_tool_extension_name(&extension.name)?;
    if config
        .source_sets
        .iter()
        .any(|source_set| source_set.name == extension.name)
    {
        return Err(ConfigValidationError::ToolExtensionNameDuplicatesSourceSet(
            extension.name.clone(),
        ));
    }
    match &extension.input {
        ToolExtensionInput::Source(source) => {
            validate_tool_extension_source(config, extension, source)
        }
        ToolExtensionInput::Artifact(artifact) => {
            if config.builder != BuilderBackend::Designer {
                return Err(ConfigValidationError::ToolExtensionArtifactRequiresDesigner);
            }
            let has_cfe_extension = artifact
                .path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("cfe"));
            if !has_cfe_extension || !artifact.path.is_file() {
                return Err(ConfigValidationError::ToolExtensionArtifactPathInvalid(
                    artifact.path.display().to_string(),
                ));
            }
            Ok(())
        }
    }
}

fn validate_tool_extension_name(name: &str) -> Result<(), ConfigValidationError> {
    if !is_safe_path_segment(name) {
        return Err(ConfigValidationError::InvalidToolExtensionName(
            name.to_owned(),
        ));
    }
    Ok(())
}

fn validate_tool_extension_source(
    config: &AppConfig,
    extension: &ToolExtensionConfig,
    source: &ToolExtensionSourceConfig,
) -> Result<(), ConfigValidationError> {
    if !source.path.is_dir() {
        return Err(ConfigValidationError::ToolExtensionSourcePathInvalid(
            source.path.display().to_string(),
        ));
    }

    match source.format.unwrap_or(config.format) {
        SourceFormat::Designer => {
            let marker = source.path.join("Configuration.xml");
            if !marker.is_file() {
                return Err(ConfigValidationError::ToolExtensionSourceLayoutInvalid(
                    format!(
                        "Designer extension source must contain 'Configuration.xml': {}",
                        source.path.display()
                    ),
                ));
            }
            let descriptor = std::fs::read_to_string(&marker).map_err(|error| {
                ConfigValidationError::ToolExtensionSourceLayoutInvalid(format!(
                    "failed to read '{}': {error}",
                    marker.display()
                ))
            })?;
            match source_descriptor::classify_source_descriptor(&descriptor) {
                Ok(Some(SourceDescriptorPurpose::Extension)) => Ok(()),
                Ok(other) => Err(ConfigValidationError::ToolExtensionSourceLayoutInvalid(
                    format!(
                        "Designer extension source descriptor must describe an extension, got {other:?}: {}",
                        marker.display()
                    ),
                )),
                Err(error) => Err(ConfigValidationError::ToolExtensionSourceLayoutInvalid(
                    format!("failed to parse '{}': {error:?}", marker.display()),
                )),
            }
        }
        SourceFormat::Edt => {
            validate_tool_extension_edt_runtime_path(config, extension, source)?;
            let source_set = SourceSetConfig {
                name: extension.name.clone(),
                purpose: SourceSetPurpose::Extension,
                path: source.path.clone(),
            };
            validate_ordinary_edt_source_set_layout(&source_set, &source.path).map_err(|error| {
                ConfigValidationError::ToolExtensionSourceLayoutInvalid(error.to_string())
            })
        }
    }
}

fn validate_tool_extension_edt_runtime_path(
    config: &AppConfig,
    extension: &ToolExtensionConfig,
    source: &ToolExtensionSourceConfig,
) -> Result<(), ConfigValidationError> {
    let source_path = std::fs::canonicalize(&source.path).unwrap_or_else(|_| source.path.clone());
    let work_path =
        std::fs::canonicalize(&config.work_path).unwrap_or_else(|_| config.work_path.clone());
    let generated_path = work_path
        .join("designer")
        .join("tool-extensions")
        .join(&extension.name);
    if paths_overlap(&source_path, &generated_path) {
        return Err(ConfigValidationError::ToolExtensionSourceLayoutInvalid(
            format!(
                "EDT tool extension source overlaps generated export target: source={}, target={}",
                source_path.display(),
                generated_path.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{validate, ConfigValidationError};
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, PlatformToolConfig, SourceFormat, SourceSetConfig,
        SourceSetPurpose, TestsConfig, ToolExtensionArtifactConfig, ToolExtensionConfig,
        ToolExtensionInput, ToolExtensionSourceConfig, ToolsConfig, VanessaProfileConfig,
    };
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    fn relative_path(base: &Path, path: &Path) -> PathBuf {
        path.strip_prefix(base)
            .expect("relative path")
            .to_path_buf()
    }

    fn single_source_set_config(
        base: &Path,
        work: &Path,
        format: SourceFormat,
        builder: BuilderBackend,
        purpose: SourceSetPurpose,
        name: &str,
        path: &Path,
    ) -> AppConfig {
        AppConfig {
            base_path: base.to_path_buf(),
            work_path: work.to_path_buf(),
            execution_timeout: 300_000,
            format,
            builder,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
            source_sets: vec![SourceSetConfig {
                name: name.to_owned(),
                purpose,
                path: relative_path(base, path),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig::default(),
            mcp: Default::default(),
            tests: TestsConfig::default(),
        }
    }

    fn write_edt_project(project_dir: &Path, descriptor_xml: Option<&str>) {
        std::fs::create_dir_all(project_dir).expect("project dir");
        std::fs::write(
            project_dir.join(".project"),
            "<projectDescription><name>Project</name></projectDescription>",
        )
        .expect(".project");
        if let Some(descriptor_xml) = descriptor_xml {
            let metadata_dir = project_dir.join("metadata");
            std::fs::create_dir_all(&metadata_dir).expect("metadata dir");
            std::fs::write(metadata_dir.join("Configuration.xml"), descriptor_xml)
                .expect("descriptor xml");
        }
    }

    fn write_native_edt_project(
        project_dir: &Path,
        name: &str,
        nature: &str,
        base_project: Option<&str>,
    ) {
        std::fs::create_dir_all(project_dir.join("DT-INF")).expect("dt-inf dir");
        std::fs::create_dir_all(project_dir.join("src").join("Configuration"))
            .expect("configuration dir");
        let base_line = base_project
            .map(|value| format!("Base-Project: {value}\n"))
            .unwrap_or_default();
        std::fs::write(
            project_dir.join(".project"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<projectDescription>\n  <name>{name}</name>\n  <natures>\n    <nature>{nature}</nature>\n  </natures>\n</projectDescription>\n"
            ),
        )
        .expect(".project");
        std::fs::write(
            project_dir.join("DT-INF").join("PROJECT.PMF"),
            format!("{base_line}Manifest-Version: 1.0\nRuntime-Version: 8.3.27\n"),
        )
        .expect("pmf");
        std::fs::write(
            project_dir
                .join("src")
                .join("Configuration")
                .join("Configuration.mdo"),
            "<Configuration />",
        )
        .expect("configuration.mdo");
        std::fs::write(
            project_dir
                .join("src")
                .join("Configuration")
                .join("Module.bsl"),
            "Procedure Test()\nEndProcedure\n",
        )
        .expect("module");
    }

    fn write_native_edt_external_project(project_dir: &Path, name: &str, descriptor_xml: &str) {
        std::fs::create_dir_all(project_dir.join("DT-INF")).expect("dt-inf dir");
        std::fs::create_dir_all(project_dir.join("src")).expect("src dir");
        std::fs::write(
            project_dir.join(".project"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<projectDescription>\n  <name>{name}</name>\n  <natures>\n    <nature>{}</nature>\n  </natures>\n</projectDescription>\n",
                crate::support::edt_project::V8_EXTERNAL_OBJECTS_NATURE
            ),
        )
        .expect(".project");
        std::fs::write(
            project_dir.join("DT-INF").join("PROJECT.PMF"),
            "Base-Project: BaseProject\nManifest-Version: 1.0\nRuntime-Version: 8.3.27\n",
        )
        .expect("pmf");
        std::fs::write(project_dir.join("src").join("root.xml"), descriptor_xml)
            .expect("descriptor");
    }

    #[test]
    fn accepts_platform_version_prefix_without_build() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        std::fs::create_dir_all(&source_dir).expect("source dir");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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

        validate(&config).expect("expected valid version prefix");
    }

    #[test]
    fn accepts_platform_version_minor_prefix() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        std::fs::create_dir_all(&source_dir).expect("source dir");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
                    version: Some("8.3".to_owned()),
                },
                ..ToolsConfig::default()
            },
            mcp: Default::default(),
            tests: TestsConfig::default(),
        };

        validate(&config).expect("expected valid minor version prefix");
    }

    #[test]
    fn rejects_platform_versions_with_too_few_parts() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        std::fs::create_dir_all(&source_dir).expect("source dir");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
                    version: Some("8".to_owned()),
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
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
                dynamic_update: false,
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
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
    fn rejects_zero_global_execution_timeout() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        std::fs::create_dir_all(&source_dir).expect("source dir");

        let mut config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
        config.execution_timeout = 0;

        let err = validate(&config).expect_err("expected invalid execution timeout");
        assert!(matches!(
            err,
            ConfigValidationError::InvalidExecutionTimeout
        ));
    }

    #[test]
    fn designer_external_source_set_accepts_matching_top_level_xml_descriptors() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("external");
        std::fs::create_dir_all(&source_dir).expect("source dir");
        std::fs::write(
            source_dir.join("Alpha.xml"),
            "<ExternalDataProcessor><Properties><Name>Alpha</Name></Properties></ExternalDataProcessor>",
        )
        .expect("descriptor");

        let config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Designer,
            BuilderBackend::Designer,
            SourceSetPurpose::ExternalDataProcessors,
            "external",
            &source_dir,
        );

        validate(&config).expect("expected valid external-only designer config");
    }

    #[test]
    fn designer_external_source_set_rejects_mismatched_top_level_xml_descriptors() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("external");
        std::fs::create_dir_all(&source_dir).expect("source dir");
        std::fs::write(
            source_dir.join("Alpha.xml"),
            "<ExternalReport><Properties><Name>Alpha</Name></Properties></ExternalReport>",
        )
        .expect("descriptor");

        let config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Designer,
            BuilderBackend::Designer,
            SourceSetPurpose::ExternalDataProcessors,
            "external",
            &source_dir,
        );

        let err = validate(&config).expect_err("expected invalid designer external layout");
        assert!(matches!(
            err,
            ConfigValidationError::SourceSetLayoutInvalid { name, details }
                if name == "external" && details.contains("EXTERNAL_REPORTS")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn designer_external_source_set_ignores_symlinked_top_level_descriptors() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("external");
        std::fs::create_dir_all(&source_dir).expect("source dir");
        std::fs::write(
            source_dir.join("Alpha.xml"),
            "<ExternalDataProcessor><Properties><Name>Alpha</Name></Properties></ExternalDataProcessor>",
        )
        .expect("descriptor");

        let outside_dir = base.path().join("outside");
        std::fs::create_dir_all(&outside_dir).expect("outside dir");
        let outside_descriptor = outside_dir.join("Beta.xml");
        std::fs::write(
            &outside_descriptor,
            "<ExternalReport><Properties><Name>Beta</Name></Properties></ExternalReport>",
        )
        .expect("outside descriptor");
        std::os::unix::fs::symlink(&outside_descriptor, source_dir.join("Beta.xml"))
            .expect("symlink");

        let config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Designer,
            BuilderBackend::Designer,
            SourceSetPurpose::ExternalDataProcessors,
            "external",
            &source_dir,
        );

        validate(&config).expect("symlinked top-level descriptors should be ignored");
    }

    #[test]
    fn edt_format_rejects_non_external_source_set_without_project_marker() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("edt-main");
        std::fs::create_dir_all(&source_dir).expect("source dir");

        let config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Edt,
            BuilderBackend::Designer,
            SourceSetPurpose::Configuration,
            "main",
            &source_dir,
        );

        let err = validate(&config).expect_err("expected missing EDT marker");
        assert!(matches!(
            err,
            ConfigValidationError::SourceSetLayoutInvalid { name, details }
                if name == "main" && details.contains(".project")
        ));
    }

    #[test]
    fn edt_format_accepts_native_configuration_project_layout() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("edt-main");
        write_native_edt_project(
            &source_dir,
            "BaseProject",
            crate::support::edt_project::V8_CONFIGURATION_NATURE,
            None,
        );

        let config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Edt,
            BuilderBackend::Designer,
            SourceSetPurpose::Configuration,
            "main",
            &source_dir,
        );

        validate(&config).expect("native EDT configuration project should be valid");
    }

    #[test]
    fn edt_format_rejects_native_configuration_project_without_manifest() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("edt-main");
        write_native_edt_project(
            &source_dir,
            "BaseProject",
            crate::support::edt_project::V8_CONFIGURATION_NATURE,
            None,
        );
        std::fs::remove_file(source_dir.join("DT-INF").join("PROJECT.PMF")).expect("remove pmf");

        let config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Edt,
            BuilderBackend::Designer,
            SourceSetPurpose::Configuration,
            "main",
            &source_dir,
        );

        let err = validate(&config).expect_err("expected missing PMF validation error");
        assert!(matches!(
            err,
            ConfigValidationError::SourceSetLayoutInvalid { name, details }
                if name == "main" && details.contains("PROJECT.PMF")
        ));
    }

    #[test]
    fn edt_format_rejects_native_extension_project_when_purpose_mismatches_nature() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("edt-main");
        write_native_edt_project(
            &source_dir,
            "ExtensionProject",
            crate::support::edt_project::V8_EXTENSION_NATURE,
            Some("BaseProject"),
        );

        let config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Edt,
            BuilderBackend::Designer,
            SourceSetPurpose::Configuration,
            "main",
            &source_dir,
        );

        let err = validate(&config).expect_err("expected mismatched nature validation error");
        assert!(matches!(
            err,
            ConfigValidationError::SourceSetLayoutInvalid { name, details }
                if name == "main" && details.contains("EXTENSION")
        ));
    }

    #[test]
    fn edt_format_accepts_native_extension_project_without_base_project() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let config_dir = base.path().join("edt-main");
        let extension_dir = base.path().join("edt-ext");
        write_native_edt_project(
            &config_dir,
            "BaseProject",
            crate::support::edt_project::V8_CONFIGURATION_NATURE,
            None,
        );
        write_native_edt_project(
            &extension_dir,
            "ExtensionProject",
            crate::support::edt_project::V8_EXTENSION_NATURE,
            None,
        );

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Edt,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
            source_sets: vec![
                SourceSetConfig {
                    name: "main".to_owned(),
                    purpose: SourceSetPurpose::Configuration,
                    path: config_dir
                        .strip_prefix(base.path())
                        .expect("relative")
                        .to_path_buf(),
                },
                SourceSetConfig {
                    name: "ext".to_owned(),
                    purpose: SourceSetPurpose::Extension,
                    path: extension_dir
                        .strip_prefix(base.path())
                        .expect("relative")
                        .to_path_buf(),
                },
            ],
            build: BuildConfig::default(),
            tools: ToolsConfig::default(),
            mcp: Default::default(),
            tests: TestsConfig::default(),
        };

        validate(&config).expect("missing Base-Project should not invalidate EDT extension");
    }

    #[test]
    fn edt_format_rejects_non_external_source_set_without_supported_nature() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("edt-main");
        write_edt_project(&source_dir, Some("<Configuration />"));

        let config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Edt,
            BuilderBackend::Designer,
            SourceSetPurpose::Configuration,
            "main",
            &source_dir,
        );

        let err = validate(&config).expect_err("expected invalid EDT project layout");
        assert!(matches!(
            err,
            ConfigValidationError::SourceSetLayoutInvalid { name, details }
                if name == "main" && details.contains("V8ConfigurationNature")
        ));
    }

    #[test]
    fn edt_format_ignores_malformed_non_descriptor_xml_for_ordinary_project_validation() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("edt-main");
        write_native_edt_project(
            &source_dir,
            "BaseProject",
            crate::support::edt_project::V8_CONFIGURATION_NATURE,
            None,
        );
        std::fs::create_dir_all(source_dir.join("misc")).expect("misc dir");
        std::fs::write(source_dir.join("misc").join("broken.xml"), "<broken").expect("broken xml");

        let config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Edt,
            BuilderBackend::Designer,
            SourceSetPurpose::Configuration,
            "main",
            &source_dir,
        );

        validate(&config).expect("malformed non-descriptor XML should be ignored");
    }

    #[cfg(unix)]
    #[test]
    fn edt_format_ignores_symlinked_dirs_outside_project_root() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("edt-main");
        write_native_edt_project(
            &source_dir,
            "BaseProject",
            crate::support::edt_project::V8_CONFIGURATION_NATURE,
            None,
        );

        let outside_dir = base.path().join("outside");
        std::fs::create_dir_all(outside_dir.join("foreign")).expect("outside dir");
        std::fs::write(
            outside_dir.join("foreign").join("Configuration.xml"),
            "<Configuration><ConfigurationExtensionPurpose>Extension</ConfigurationExtensionPurpose></Configuration>",
        )
        .expect("outside descriptor");
        std::os::unix::fs::symlink(&outside_dir, source_dir.join("leak")).expect("symlink");

        let config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Edt,
            BuilderBackend::Designer,
            SourceSetPurpose::Configuration,
            "main",
            &source_dir,
        );

        validate(&config).expect("symlinked directories outside project root should be ignored");
    }

    #[test]
    fn edt_external_source_set_rejects_child_project_with_mismatched_kind() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("external");
        let child_project = source_dir.join("report-project");
        write_native_edt_external_project(
            &child_project,
            "ReportProject",
            "<ExternalReport><Properties><Name>Alpha</Name></Properties></ExternalReport>",
        );

        let config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Edt,
            BuilderBackend::Designer,
            SourceSetPurpose::ExternalDataProcessors,
            "external",
            &source_dir,
        );

        let err = validate(&config).expect_err("expected invalid EDT external layout");
        assert!(matches!(
            err,
            ConfigValidationError::SourceSetLayoutInvalid { name, details }
                if name == "external" && details.contains("EXTERNAL_REPORTS")
        ));
    }

    #[test]
    fn edt_external_source_set_rejects_child_project_without_base_project() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("external");
        let child_project = source_dir.join("processor-project");
        write_native_edt_external_project(
            &child_project,
            "ProcessorProject",
            "<ExternalDataProcessor><Properties><Name>Alpha</Name></Properties></ExternalDataProcessor>",
        );
        std::fs::write(
            child_project.join("DT-INF").join("PROJECT.PMF"),
            "Manifest-Version: 1.0\nRuntime-Version: 8.3.27\n",
        )
        .expect("rewrite pmf");

        let config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Edt,
            BuilderBackend::Designer,
            SourceSetPurpose::ExternalDataProcessors,
            "external",
            &source_dir,
        );

        let err = validate(&config).expect_err("expected missing Base-Project validation error");
        assert!(matches!(
            err,
            ConfigValidationError::SourceSetLayoutInvalid { name, details }
                if name == "external" && details.contains("must contain descriptors")
        ));
    }

    #[test]
    fn edt_external_source_set_rejects_child_project_without_canonical_root_descriptor() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("external");
        let child_project = source_dir.join("processor-project");
        write_native_edt_external_project(
            &child_project,
            "ProcessorProject",
            "<ExternalDataProcessor><Properties><Name>Alpha</Name></Properties></ExternalDataProcessor>",
        );
        std::fs::create_dir_all(child_project.join("src").join("nested")).expect("nested dir");
        std::fs::rename(
            child_project.join("src").join("root.xml"),
            child_project.join("src").join("nested").join("alpha.xml"),
        )
        .expect("move root descriptor");

        let config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Edt,
            BuilderBackend::Designer,
            SourceSetPurpose::ExternalDataProcessors,
            "external",
            &source_dir,
        );

        let err = validate(&config).expect_err("expected missing canonical root descriptor");
        assert!(matches!(
            err,
            ConfigValidationError::SourceSetLayoutInvalid { name, details }
                if name == "external" && details.contains("must contain descriptors")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn edt_external_source_set_ignores_symlinked_child_projects() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("external");
        let valid_child = source_dir.join("processor-project");
        write_native_edt_external_project(
            &valid_child,
            "ProcessorProject",
            "<ExternalDataProcessor><Properties><Name>Alpha</Name></Properties></ExternalDataProcessor>",
        );

        let outside_project = base.path().join("outside-report");
        write_native_edt_external_project(
            &outside_project,
            "OutsideReport",
            "<ExternalReport><Properties><Name>Beta</Name></Properties></ExternalReport>",
        );
        std::os::unix::fs::symlink(&outside_project, source_dir.join("leak")).expect("symlink");

        let config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Edt,
            BuilderBackend::Designer,
            SourceSetPurpose::ExternalDataProcessors,
            "external",
            &source_dir,
        );

        validate(&config).expect("symlinked child projects should be ignored");
    }

    #[test]
    fn allows_edt_with_ibcmd_builder_for_file_connection() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("edt-main");
        write_native_edt_project(
            &source_dir,
            "BaseProject",
            crate::support::edt_project::V8_CONFIGURATION_NATURE,
            None,
        );

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Edt,
            builder: BuilderBackend::Ibcmd,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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

        validate(&config).expect("EDT+IBCMD with file connection should be valid");
    }

    #[test]
    fn designer_format_allows_missing_source_set_path() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let missing = base.path().join("missing-src");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Ibcmd,
            infobase: crate::config::model::InfobaseConfig::file("/F /tmp/ib"),
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
            execution_timeout: 300_000,
            format: SourceFormat::Edt,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
            execution_timeout: 300_000,
            format: SourceFormat::Edt,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
        write_native_edt_project(
            &source_dir,
            "BaseProject",
            crate::support::edt_project::V8_CONFIGURATION_NATURE,
            None,
        );

        let config = AppConfig {
            base_path: shared.path().to_path_buf(),
            work_path: shared.path().to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Edt,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
            execution_timeout: 300_000,
            format: SourceFormat::Edt,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
        write_native_edt_project(
            &source_dir,
            "BaseProject",
            crate::support::edt_project::V8_CONFIGURATION_NATURE,
            None,
        );

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Edt,
            builder: BuilderBackend::Ibcmd,
            infobase: crate::config::model::InfobaseConfig {
                connection: "Srvr=localhost;Ref=ib".to_owned(),
                user: None,
                password: None,
                unlock_code: None,
                dbms: None,
            },
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

        let err = validate(&config).expect_err("expected IBCMD connection validation error");
        assert!(matches!(
            err,
            ConfigValidationError::MissingIbcmdServerDbmsField("kind")
        ));
    }

    #[test]
    fn ibcmd_server_connection_accepts_complete_dbms_contract() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("edt-main");
        write_native_edt_project(
            &source_dir,
            "BaseProject",
            crate::support::edt_project::V8_CONFIGURATION_NATURE,
            None,
        );

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Edt,
            builder: BuilderBackend::Ibcmd,
            infobase: crate::config::model::InfobaseConfig::server(
                "Srvr=localhost;Ref=ib",
                crate::config::model::InfobaseDbmsConfig::new("PostgreSQL", "localhost", "ib"),
            ),
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

        validate(&config).expect("server IBCMD config should be valid with dbms contract");
    }

    #[test]
    fn file_connection_rejects_dbms_contract() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        std::fs::create_dir_all(&source_dir).expect("source dir");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Ibcmd,
            infobase: crate::config::model::InfobaseConfig::server(
                "File=/tmp/ib",
                crate::config::model::InfobaseDbmsConfig::new("PostgreSQL", "localhost", "ib"),
            ),
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

        let err = validate(&config).expect_err("file connection must reject dbms contract");
        assert!(matches!(
            err,
            ConfigValidationError::DbmsNotAllowedForFileConnection
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
            execution_timeout: 300_000,
            format: SourceFormat::Edt,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
    fn edt_ibcmd_still_validates_source_set_paths() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");

        let config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Edt,
            builder: BuilderBackend::Ibcmd,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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

        let err = validate(&config).expect_err("expected source-set path error");
        assert!(matches!(
            err,
            ConfigValidationError::SourceSetPathInvalid { .. }
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
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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

        config.mcp.execution.max_concurrent_calls = 1;
        config.tools.client_mcp.port = Some(0);
        let err = validate(&config).expect_err("expected invalid MCP client port");
        assert!(matches!(err, ConfigValidationError::InvalidMcpClientPort));
    }

    #[test]
    fn validates_client_mcp_extension_source_and_artifact_contract() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        let tool_source = base.path().join("tools").join("client-mcp");
        let artifact = base.path().join("tools").join("client-mcp.cfe");
        std::fs::create_dir_all(&source_dir).expect("source dir");
        std::fs::create_dir_all(&tool_source).expect("tool source dir");
        std::fs::write(
            tool_source.join("Configuration.xml"),
            "<Configuration><ConfigurationExtensionPurpose>Extension</ConfigurationExtensionPurpose></Configuration>",
        )
        .expect("tool source marker");
        std::fs::write(&artifact, "cfe").expect("artifact");

        let mut config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Designer,
            BuilderBackend::Designer,
            SourceSetPurpose::Configuration,
            "main",
            &source_dir,
        );
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Source(ToolExtensionSourceConfig {
                path: tool_source.clone(),
                format: None,
            }),
        });
        validate(&config).expect("designer source extension should be valid");

        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Artifact(ToolExtensionArtifactConfig {
                path: artifact.clone(),
            }),
        });
        validate(&config).expect("cfe artifact extension should be valid");
    }

    #[test]
    fn rejects_invalid_client_mcp_extension_inputs() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        let tool_source = base.path().join("tools").join("client-mcp");
        let artifact = base.path().join("tools").join("client-mcp.cf");
        std::fs::create_dir_all(&source_dir).expect("source dir");
        std::fs::create_dir_all(&tool_source).expect("tool source dir");
        std::fs::write(
            tool_source.join("Configuration.xml"),
            "<Configuration><ConfigurationExtensionPurpose>Extension</ConfigurationExtensionPurpose></Configuration>",
        )
        .expect("tool source marker");
        std::fs::write(&artifact, "cf").expect("artifact");

        let mut config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Designer,
            BuilderBackend::Designer,
            SourceSetPurpose::Configuration,
            "main",
            &source_dir,
        );
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "bad/name".to_owned(),
            input: ToolExtensionInput::Source(ToolExtensionSourceConfig {
                path: tool_source.clone(),
                format: None,
            }),
        });
        let err = validate(&config).expect_err("unsafe name should fail");
        assert!(matches!(
            err,
            ConfigValidationError::InvalidToolExtensionName(name) if name == "bad/name"
        ));
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Artifact(ToolExtensionArtifactConfig { path: artifact }),
        });
        let err = validate(&config).expect_err("non-cfe artifact should fail");
        assert!(matches!(
            err,
            ConfigValidationError::ToolExtensionArtifactPathInvalid(_)
        ));
    }

    #[test]
    fn rejects_client_mcp_designer_source_without_extension_descriptor() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        let tool_source = base.path().join("tools").join("client-mcp");
        std::fs::create_dir_all(&source_dir).expect("source dir");
        std::fs::create_dir_all(&tool_source).expect("tool source dir");
        std::fs::write(tool_source.join("Configuration.xml"), "<Configuration />")
            .expect("tool source marker");

        let mut config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Designer,
            BuilderBackend::Designer,
            SourceSetPurpose::Configuration,
            "main",
            &source_dir,
        );
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Source(ToolExtensionSourceConfig {
                path: tool_source,
                format: Some(SourceFormat::Designer),
            }),
        });

        let err = validate(&config).expect_err("configuration descriptor should fail");
        assert!(matches!(
            err,
            ConfigValidationError::ToolExtensionSourceLayoutInvalid(details)
                if details.contains("must describe an extension")
        ));
    }

    #[test]
    fn rejects_client_mcp_edt_source_overlapping_generated_export_target() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        let tool_source = work
            .path()
            .join("designer")
            .join("tool-extensions")
            .join("client_mcp");
        write_native_edt_project(
            &source_dir,
            "main",
            crate::support::edt_project::V8_CONFIGURATION_NATURE,
            None,
        );
        write_native_edt_project(
            &tool_source,
            "client_mcp",
            crate::support::edt_project::V8_EXTENSION_NATURE,
            Some("main"),
        );

        let mut config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Edt,
            BuilderBackend::Ibcmd,
            SourceSetPurpose::Configuration,
            "main",
            &source_dir,
        );
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Source(ToolExtensionSourceConfig {
                path: tool_source,
                format: Some(SourceFormat::Edt),
            }),
        });

        let err = validate(&config).expect_err("overlapping export target should fail");
        assert!(matches!(
            err,
            ConfigValidationError::ToolExtensionSourceLayoutInvalid(details)
                if details.contains("overlaps generated export target")
        ));
    }

    #[test]
    fn rejects_client_mcp_cfe_artifact_with_ibcmd_builder() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        let artifact = base.path().join("client-mcp.cfe");
        std::fs::create_dir_all(&source_dir).expect("source dir");
        std::fs::write(&artifact, "cfe").expect("artifact");

        let mut config = single_source_set_config(
            base.path(),
            work.path(),
            SourceFormat::Designer,
            BuilderBackend::Ibcmd,
            SourceSetPurpose::Configuration,
            "main",
            &source_dir,
        );
        config.tools.client_mcp.extension = Some(ToolExtensionConfig {
            name: "client_mcp".to_owned(),
            input: ToolExtensionInput::Artifact(ToolExtensionArtifactConfig { path: artifact }),
        });

        let err = validate(&config).expect_err("ibcmd artifact should fail");
        assert!(matches!(
            err,
            ConfigValidationError::ToolExtensionArtifactRequiresDesigner
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
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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

    #[test]
    fn rejects_configured_vanessa_without_known_profile() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        let epf = base.path().join("runner.epf");
        let params = base.path().join("params.json");
        std::fs::create_dir_all(&source_dir).expect("source dir");
        std::fs::write(&epf, "epf").expect("epf");
        std::fs::write(&params, "{}").expect("params");

        let mut config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
        config.tools.va.epf_path = Some(epf);
        config.tests.va.params_path = Some(params);
        config.tests.va.profile = Some("smoke".to_owned());

        let err = validate(&config).expect_err("expected invalid profile");
        assert!(matches!(
            err,
            ConfigValidationError::UnknownVanessaProfile(name) if name == "smoke"
        ));
    }

    #[test]
    fn rejects_unsafe_vanessa_profile_name() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        let epf = base.path().join("runner.epf");
        let params = base.path().join("params.json");
        let feature = base.path().join("features");
        std::fs::create_dir_all(&source_dir).expect("source dir");
        std::fs::create_dir_all(&feature).expect("feature dir");
        std::fs::write(&epf, "epf").expect("epf");
        std::fs::write(&params, "{}").expect("params");

        let mut config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
        config.tools.va.epf_path = Some(epf);
        config.tests.va.params_path = Some(params);
        config.tests.va.profile = Some("bad/name".to_owned());
        config.tests.va.profiles.insert(
            "bad/name".to_owned(),
            VanessaProfileConfig {
                feature_path: Some(feature),
                ..Default::default()
            },
        );

        let err = validate(&config).expect_err("expected invalid profile name");
        assert!(matches!(
            err,
            ConfigValidationError::InvalidVanessaProfileName(name) if name == "bad/name"
        ));
    }

    #[test]
    fn rejects_zero_vanessa_timeout() {
        let base = tempdir().expect("base");
        let work = tempdir().expect("work");
        let source_dir = base.path().join("src");
        let features = base.path().join("features");
        let epf = base.path().join("runner.epf");
        let params = base.path().join("params.json");
        std::fs::create_dir_all(&source_dir).expect("source dir");
        std::fs::create_dir_all(&features).expect("features dir");
        std::fs::write(&epf, "epf").expect("epf");
        std::fs::write(&params, "{}").expect("params");

        let mut config = AppConfig {
            base_path: base.path().to_path_buf(),
            work_path: work.path().to_path_buf(),
            execution_timeout: 300_000,
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
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
        config.tools.va.epf_path = Some(epf);
        config.tests.va.params_path = Some(params);
        config.tests.va.profile = Some("smoke".to_owned());
        config.tests.va.timeouts.total_ms = Some(0);
        config.tests.va.profiles.insert(
            "smoke".to_owned(),
            VanessaProfileConfig {
                feature_path: Some(features),
                ..Default::default()
            },
        );

        let err = validate(&config).expect_err("expected invalid timeout");
        assert!(matches!(
            err,
            ConfigValidationError::InvalidVanessaTimeout("total_ms")
        ));
    }
}
