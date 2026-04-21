use std::path::{Path, PathBuf};

use quick_xml::events::Event;
use quick_xml::Reader;
use sha2::{Digest, Sha256};

use crate::config::model::{AppConfig, SourceSetConfig, SourceSetPurpose};
use crate::platform::edt::EdtDsl;
use crate::support::error::AppError;
use crate::support::fs::{ensure_dir, remove_path_if_exists};
use crate::use_cases::build_project::ensure_platform_success as ensure_build_platform_success;

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
    let mut descriptors = Vec::new();
    for entry in std::fs::read_dir(source_dir).map_err(|error| {
        AppError::Runtime(format!(
            "failed to read external source-set '{source_set_name}': {error}"
        ))
    })? {
        let entry = entry.map_err(|error| {
            AppError::Runtime(format!(
                "failed to read external source-set entry for '{source_set_name}': {error}"
            ))
        })?;
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|value| value.to_str()) != Some("xml") {
            continue;
        }
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
        let result = dsl
            .export_project(&item.logical_name, &export_target)
            .map_err(|error| AppError::Platform(error.to_string()))?;
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
    let mut items = Vec::new();
    for entry in std::fs::read_dir(source_dir).map_err(|error| {
        AppError::Runtime(format!("failed to read EDT external source-set: {error}"))
    })? {
        let entry = entry.map_err(|error| {
            AppError::Runtime(format!(
                "failed to read EDT external source-set entry: {error}"
            ))
        })?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let project_file = path.join(".project");
        if !project_file.is_file() {
            continue;
        }
        let logical_name = extract_xml_tag_text(
            &std::fs::read_to_string(&project_file).map_err(|error| {
                AppError::Runtime(format!(
                    "failed to read EDT project '{}': {error}",
                    project_file.display()
                ))
            })?,
            "name",
        )
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::Validation(format!(
                "EDT external project '{}' must contain non-empty <name>",
                project_file.display()
            ))
        })?;
        items.push(ExternalArtifactDescriptor {
            stable_id: stable_id_for_path(&logical_name, &path),
            logical_name,
            artifact_type: expected_kind,
            descriptor_xml_path: project_file,
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

struct ParsedExternalDescriptor {
    logical_name: String,
    artifact_type: ExternalArtifactKind,
}

fn parse_external_descriptor(path: &Path) -> Result<ParsedExternalDescriptor, AppError> {
    let contents = std::fs::read_to_string(path).map_err(|error| {
        AppError::Runtime(format!(
            "failed to read external descriptor '{}': {error}",
            path.display()
        ))
    })?;
    let mut reader = Reader::from_str(&contents);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut root_tag = None;
    let mut artifact_root_tag = None;
    let mut seen_properties = false;
    let mut seen_name = false;
    let mut logical_name = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let tag = String::from_utf8_lossy(event.name().as_ref()).into_owned();
                if root_tag.is_none() {
                    root_tag = Some(tag.clone());
                }
                if artifact_root_tag.is_none() {
                    match tag.as_str() {
                        "MetaDataObject" => {}
                        "ExternalDataProcessor" | "ExternalReport" => {
                            artifact_root_tag = Some(tag.clone());
                        }
                        _ if root_tag.as_deref() != Some("MetaDataObject") => {
                            artifact_root_tag = Some(tag.clone());
                        }
                        _ => {}
                    }
                }
                if tag == "Properties" {
                    seen_properties = true;
                } else if seen_properties && tag == "Name" {
                    seen_name = true;
                }
            }
            Ok(Event::Text(text)) if seen_name && logical_name.is_none() => {
                logical_name = Some(
                    text.unescape()
                        .map_err(|error| {
                            AppError::Validation(format!(
                                "failed to decode logical name in '{}': {error}",
                                path.display()
                            ))
                        })?
                        .into_owned(),
                );
            }
            Ok(Event::End(event)) => {
                let tag = String::from_utf8_lossy(event.name().as_ref()).into_owned();
                if tag == "Name" {
                    seen_name = false;
                } else if tag == "Properties" {
                    seen_properties = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(AppError::Validation(format!(
                    "failed to parse external descriptor '{}': {error}",
                    path.display()
                )));
            }
            _ => {}
        }
        buf.clear();
    }

    let artifact_type = match artifact_root_tag.as_deref().or(root_tag.as_deref()) {
        Some("ExternalDataProcessor") => ExternalArtifactKind::DataProcessor,
        Some("ExternalReport") => ExternalArtifactKind::Report,
        Some(other) => {
            return Err(AppError::Validation(format!(
                "unsupported root XML element '{other}' in '{}'",
                path.display()
            )));
        }
        None => {
            return Err(AppError::Validation(format!(
                "missing root XML element in '{}'",
                path.display()
            )));
        }
    };
    let logical_name = logical_name
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::Validation(format!(
                "external descriptor '{}' must contain Properties/Name",
                path.display()
            ))
        })?;

    Ok(ParsedExternalDescriptor {
        logical_name,
        artifact_type,
    })
}

fn stable_id_for_path(logical_name: &str, path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.display().to_string().as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    format!("{}-{}", sanitize_file_stem(logical_name), &digest[..8])
}

fn extract_xml_tag_text(contents: &str, tag_name: &str) -> Option<String> {
    let open_tag = format!("<{tag_name}>");
    let close_tag = format!("</{tag_name}>");
    let start = contents.find(&open_tag)? + open_tag.len();
    let rest = &contents[start..];
    let end = rest.find(&close_tag)?;
    Some(rest[..end].trim().to_owned())
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
            format,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
            source_sets: vec![
                SourceSetConfig {
                    name: "external".to_owned(),
                    purpose: SourceSetPurpose::ExternalDataProcessors,
                    path: PathBuf::from("designer/external"),
                },
                SourceSetConfig {
                    name: "reports".to_owned(),
                    purpose: SourceSetPurpose::ExternalReports,
                    path: PathBuf::from("designer/reports"),
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
        };
        let report = SourceSetConfig {
            name: "reports".to_owned(),
            purpose: SourceSetPurpose::ExternalReports,
            path: PathBuf::from("designer/reports"),
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

    #[cfg(unix)]
    #[test]
    fn prepare_edt_external_artifacts_handles_report_projects() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let source = base.join("designer/reports/ReportOne");
        fs::create_dir_all(&source).expect("source");
        fs::write(
            source.join(".project"),
            "<projectDescription><name>Report One</name></projectDescription>",
        )
        .expect("project");
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
