use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::config::model::{AppConfig, SourceSetConfig, SourceSetPurpose};
use crate::platform::edt::EdtDsl;
use crate::support::edt_project;
use crate::support::error::AppError;
use crate::support::fs::{ensure_dir, remove_path_if_exists};
use crate::support::source_descriptor::{
    self, ExternalDescriptorParseError, SourceDescriptorPurpose,
};
use crate::use_cases::build_project::ensure_platform_success as ensure_build_platform_success;
use crate::use_cases::progress::log_live_stage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalArtifactKind {
    DataProcessor,
    Report,
}

impl ExternalArtifactKind {
    pub const fn root_tag(self) -> &'static str {
        match self {
            Self::DataProcessor => "ExternalDataProcessor",
            Self::Report => "ExternalReport",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalArtifactDescriptor {
    pub logical_name: String,
    pub artifact_type: ExternalArtifactKind,
    pub descriptor_xml_path: PathBuf,
    pub root_path: PathBuf,
    pub stable_id: String,
}

pub fn source_set_external_kind(source_set: &SourceSetConfig) -> Option<ExternalArtifactKind> {
    match source_set.purpose {
        SourceSetPurpose::ExternalDataProcessors => Some(ExternalArtifactKind::DataProcessor),
        SourceSetPurpose::ExternalReports => Some(ExternalArtifactKind::Report),
        _ => None,
    }
}

pub fn discover_designer_external_artifacts(
    source_set_name: &str,
    source_dir: &Path,
    expected_kind: ExternalArtifactKind,
) -> Result<Vec<ExternalArtifactDescriptor>, AppError> {
    let entries = source_descriptor::scan_designer_external_root(source_dir)
        .map_err(map_source_set_root_scan_error)?;
    let mut descriptors = Vec::new();
    for entry in entries {
        let path = entry.path;
        let parsed = parse_external_descriptor(&path)?;
        if parsed.artifact_type != expected_kind {
            return Err(AppError::Validation(format!(
                "external source-set '{source_set_name}' contains '{}', expected {}",
                path.display(),
                expected_kind.root_tag()
            )));
        }
        descriptors.push(ExternalArtifactDescriptor {
            logical_name: parsed.logical_name.clone(),
            artifact_type: parsed.artifact_type,
            descriptor_xml_path: path.clone(),
            root_path: source_dir.to_path_buf(),
            stable_id: stable_id_for_path(&parsed.logical_name, &path),
        });
    }

    if descriptors.is_empty() {
        return Err(AppError::Validation(format!(
            "external source-set '{source_set_name}' does not contain root XML descriptors"
        )));
    }

    validate_unique_publish_names(source_set_name, &descriptors)?;
    Ok(descriptors)
}

pub fn prepare_edt_external_artifacts(
    config: &AppConfig,
    source_set: &SourceSetConfig,
    dsl: &EdtDsl<'_>,
) -> Result<Vec<ExternalArtifactDescriptor>, AppError> {
    let source_dir = resolve_source_set_path(config, source_set);
    let expected_kind = source_set_external_kind(source_set).ok_or_else(|| {
        AppError::Validation(format!("source-set '{}' is not external", source_set.name))
    })?;
    let items = discover_edt_items(&source_dir, expected_kind)?;
    let mut exported = Vec::new();
    for item in items {
        let export_target = config
            .work_path
            .join("designer")
            .join(&source_set.name)
            .join(&item.stable_id);
        remove_path_if_exists(&export_target).map_err(|error| {
            AppError::Runtime(format!(
                "failed to clean external export target '{}': {error}",
                export_target.display()
            ))
        })?;
        ensure_dir(&export_target).map_err(|error| {
            AppError::Runtime(format!(
                "failed to create external export target '{}': {error}",
                export_target.display()
            ))
        })?;
        log_live_stage(
            "edt: external export",
            "[EDT] exporting external project to Designer files",
        );
        let result = dsl
            .export_project(&item.logical_name, &export_target)
            .map_err(AppError::from)?;
        ensure_build_platform_success("edt_export", source_set, &result)?;
        let mut discovered =
            discover_designer_external_artifacts(&source_set.name, &export_target, expected_kind)?;
        if discovered.len() != 1 {
            return Err(AppError::Validation(format!(
                "EDT export for '{}' must produce exactly one root XML",
                item.logical_name
            )));
        }
        let mut descriptor = discovered.remove(0);
        descriptor.stable_id = item.stable_id;
        exported.push(descriptor);
    }
    validate_unique_publish_names(&source_set.name, &exported)?;
    Ok(exported)
}

pub fn resolve_source_set_path(config: &AppConfig, source_set: &SourceSetConfig) -> PathBuf {
    if source_set.path.is_absolute() {
        source_set.path.clone()
    } else {
        config.base_path.join(&source_set.path)
    }
}

pub fn sanitize_file_stem(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            _ => ch,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_owned();
    if sanitized.is_empty() {
        "external".to_owned()
    } else {
        sanitized
    }
}

fn validate_unique_publish_names(
    source_set_name: &str,
    descriptors: &[ExternalArtifactDescriptor],
) -> Result<(), AppError> {
    let mut stems = std::collections::HashSet::new();
    for descriptor in descriptors {
        let stem = sanitize_file_stem(&descriptor.logical_name);
        if !stems.insert(stem.clone()) {
            return Err(AppError::Validation(format!(
                "external source-set '{source_set_name}' contains duplicate publish file stem '{stem}'"
            )));
        }
    }
    Ok(())
}

fn discover_edt_items(
    source_dir: &Path,
    expected_kind: ExternalArtifactKind,
) -> Result<Vec<ExternalArtifactDescriptor>, AppError> {
    let entries = source_descriptor::scan_edt_external_root(source_dir)
        .map_err(map_source_set_root_scan_error)?;
    let mut items = Vec::new();
    for entry in entries {
        let path = entry.path;
        let project =
            edt_project::validate_native_external_project(&path).map_err(AppError::Validation)?;
        let root_xml = edt_project::external_root_descriptor_path(&path);
        let artifact_type = match entry.purpose {
            Some(purpose) => map_external_purpose(purpose, &root_xml)?,
            None => parse_external_descriptor(&root_xml)?.artifact_type,
        };
        if artifact_type != expected_kind {
            return Err(AppError::Validation(format!(
                "external EDT project '{}' contains '{}', expected {}",
                path.display(),
                root_xml.display(),
                expected_kind.root_tag()
            )));
        }
        items.push(ExternalArtifactDescriptor {
            stable_id: stable_id_for_path(&project.name, &path),
            logical_name: project.name,
            artifact_type: expected_kind,
            descriptor_xml_path: root_xml,
            root_path: path,
        });
    }

    if items.is_empty() {
        return Err(AppError::Validation(
            "external EDT source-set must contain at least one child project".to_owned(),
        ));
    }
    Ok(items)
}

pub(crate) struct ParsedExternalDescriptor {
    pub logical_name: String,
    pub artifact_type: ExternalArtifactKind,
}

pub(crate) fn parse_external_descriptor(path: &Path) -> Result<ParsedExternalDescriptor, AppError> {
    let contents = std::fs::read_to_string(path).map_err(|error| {
        AppError::Runtime(format!(
            "failed to read external descriptor '{}': {error}",
            path.display()
        ))
    })?;
    let parsed = source_descriptor::parse_external_descriptor(&contents)
        .map_err(|error| map_external_descriptor_parse_error(path, error))?;
    let artifact_type = map_external_purpose(parsed.purpose, path)?;

    Ok(ParsedExternalDescriptor {
        logical_name: parsed.logical_name,
        artifact_type,
    })
}

fn map_external_purpose(
    purpose: SourceDescriptorPurpose,
    path: &Path,
) -> Result<ExternalArtifactKind, AppError> {
    match purpose {
        SourceDescriptorPurpose::ExternalDataProcessors => Ok(ExternalArtifactKind::DataProcessor),
        SourceDescriptorPurpose::ExternalReports => Ok(ExternalArtifactKind::Report),
        _ => Err(AppError::Validation(format!(
            "unsupported root XML element '{}' in '{}'",
            purpose.external_root_tag().unwrap_or("Configuration"),
            path.display()
        ))),
    }
}

fn map_external_descriptor_parse_error(
    path: &Path,
    error: ExternalDescriptorParseError,
) -> AppError {
    match error {
        ExternalDescriptorParseError::Xml(error) => AppError::Validation(format!(
            "failed to parse external descriptor '{}': {error}",
            path.display()
        )),
        ExternalDescriptorParseError::DecodeLogicalName(error) => AppError::Validation(format!(
            "failed to decode logical name in '{}': {error}",
            path.display()
        )),
        ExternalDescriptorParseError::MissingRootElement => {
            AppError::Validation(format!("missing root XML element in '{}'", path.display()))
        }
        ExternalDescriptorParseError::UnsupportedRootElement(root) => {
            AppError::Validation(format!(
                "unsupported root XML element '{root}' in '{}'",
                path.display()
            ))
        }
        ExternalDescriptorParseError::MissingLogicalName => AppError::Validation(format!(
            "external descriptor '{}' must contain Properties/Name",
            path.display()
        )),
    }
}

fn map_source_set_root_scan_error(error: source_descriptor::SourceSetRootScanError) -> AppError {
    match error {
        source_descriptor::SourceSetRootScanError::Runtime(message) => AppError::Runtime(message),
        source_descriptor::SourceSetRootScanError::Validation(message) => {
            AppError::Validation(message)
        }
    }
}

fn stable_id_for_path(logical_name: &str, path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.display().to_string().as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    format!("{}-{}", sanitize_file_stem(logical_name), &digest[..8])
}
#[cfg(test)]
mod tests {
    use super::{
        discover_designer_external_artifacts, prepare_edt_external_artifacts,
        source_set_external_kind, ExternalArtifactKind,
    };
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, SourceFormat, SourceSetConfig, SourceSetPurpose,
        TestsConfig, ToolsConfig,
    };
    use crate::platform::edt::EdtDsl;
    use crate::platform::process::ProcessExecutor;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    fn config(base: &Path, work: &Path, platform: &Path, format: SourceFormat) -> AppConfig {
        AppConfig {
            base_path: base.to_path_buf(),
            work_path: work.to_path_buf(),
            execution_timeout: 300_000,
            format,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
            source_sets: vec![
                SourceSetConfig {
                    name: "external".to_owned(),
                    purpose: SourceSetPurpose::ExternalDataProcessors,
                    path: PathBuf::from("designer/external"),
                    depends_on: Vec::new(),
                },
                SourceSetConfig {
                    name: "reports".to_owned(),
                    purpose: SourceSetPurpose::ExternalReports,
                    path: PathBuf::from("designer/reports"),
                    depends_on: Vec::new(),
                },
            ],
            build: BuildConfig::default(),
            tools: ToolsConfig {
                platform: crate::config::model::PlatformToolConfig {
                    path: Some(platform.to_path_buf()),
                    version: None,
                },
                edt_cli: crate::config::model::EdtCliConfig {
                    path: Some(platform.to_path_buf()),
                    ..Default::default()
                },
                ..ToolsConfig::default()
            },
            mcp: Default::default(),
            tests: TestsConfig::default(),
        }
    }

    #[test]
    fn source_set_external_kind_distinguishes_processors_and_reports() {
        let processor = SourceSetConfig {
            name: "external".to_owned(),
            purpose: SourceSetPurpose::ExternalDataProcessors,
            path: PathBuf::from("designer/external"),
            depends_on: Vec::new(),
        };
        let report = SourceSetConfig {
            name: "reports".to_owned(),
            purpose: SourceSetPurpose::ExternalReports,
            path: PathBuf::from("designer/reports"),
            depends_on: Vec::new(),
        };

        assert_eq!(
            source_set_external_kind(&processor),
            Some(ExternalArtifactKind::DataProcessor)
        );
        assert_eq!(
            source_set_external_kind(&report),
            Some(ExternalArtifactKind::Report)
        );
    }

    #[test]
    fn discover_designer_external_artifacts_reads_root_descriptor() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("designer/external");
        fs::create_dir_all(&source).expect("source");
        fs::write(
            source.join("Foo.xml"),
            "<ExternalDataProcessor><Properties><Name>Foo &amp; Bar</Name></Properties></ExternalDataProcessor>",
        )
        .expect("xml");

        let artifacts = discover_designer_external_artifacts(
            "external",
            &source,
            ExternalArtifactKind::DataProcessor,
        )
        .expect("discover");

        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].logical_name, "Foo & Bar");
    }

    #[test]
    fn discover_designer_external_artifacts_accepts_metadataobject_wrapper() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("designer/external");
        fs::create_dir_all(&source).expect("source");
        fs::write(
            source.join("Foo.xml"),
            r#"<MetaDataObject><ExternalDataProcessor><Properties><Name>Foo</Name></Properties></ExternalDataProcessor></MetaDataObject>"#,
        )
        .expect("xml");

        let artifacts = discover_designer_external_artifacts(
            "external",
            &source,
            ExternalArtifactKind::DataProcessor,
        )
        .expect("discover");

        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].logical_name, "Foo");
        assert_eq!(
            artifacts[0].artifact_type,
            ExternalArtifactKind::DataProcessor
        );
    }

    #[test]
    fn discover_designer_external_artifacts_accepts_uppercase_xml_extension() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("designer/external");
        fs::create_dir_all(&source).expect("source");
        fs::write(
            source.join("Foo.XML"),
            "<ExternalDataProcessor><Properties><Name>Foo</Name></Properties></ExternalDataProcessor>",
        )
        .expect("xml");

        let artifacts = discover_designer_external_artifacts(
            "external",
            &source,
            ExternalArtifactKind::DataProcessor,
        )
        .expect("discover");

        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].logical_name, "Foo");
    }

    #[cfg(unix)]
    #[test]
    fn prepare_edt_external_artifacts_handles_report_projects() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let source = base.join("designer/reports/ReportOne");
        fs::create_dir_all(source.join("DT-INF")).expect("source dt-inf");
        fs::create_dir_all(source.join("src")).expect("source src");
        fs::write(
            source.join(".project"),
            format!(
                "<projectDescription><name>Report One</name><natures><nature>{}</nature></natures></projectDescription>",
                crate::support::edt_project::V8_EXTERNAL_OBJECTS_NATURE
            ),
        )
        .expect("project");
        fs::write(
            source.join("DT-INF").join("PROJECT.PMF"),
            "Base-Project: BaseProject\nManifest-Version: 1.0\nRuntime-Version: 8.3.27\n",
        )
        .expect("manifest");
        fs::write(
            source.join("src").join("root.xml"),
            "<ExternalReport><Properties><Name>Report One</Name></Properties></ExternalReport>",
        )
        .expect("root xml");
        let edt = dir.path().join("edt");
        fs::create_dir_all(&edt).expect("edt");
        let binary = edt.join("1cedtcli");
        fs::write(&binary, "#!/bin/sh\nroot=''\nfor arg in \"$@\"; do root=\"$arg\"; done\nmkdir -p \"$root\"\nprintf '<ExternalReport><Properties><Name>Report One</Name></Properties></ExternalReport>' > \"$root/Report One.xml\"\nexit 0\n").expect("script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&binary).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&binary, perms).expect("chmod");
        }
        let config = config(&base, &work, &binary, SourceFormat::Edt);
        let dsl = EdtDsl::new(binary.clone(), work.join("edt-workspace"), &ProcessExecutor);

        let artifacts =
            prepare_edt_external_artifacts(&config, &config.source_sets[1], &dsl).expect("prepare");

        assert_eq!(artifacts[0].artifact_type, ExternalArtifactKind::Report);
        assert_eq!(artifacts[0].logical_name, "Report One");
    }
}
