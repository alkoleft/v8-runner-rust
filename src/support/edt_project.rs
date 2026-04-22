use std::collections::HashSet;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

pub const V8_CONFIGURATION_NATURE: &str = "com._1c.g5.v8.dt.core.V8ConfigurationNature";
pub const V8_EXTENSION_NATURE: &str = "com._1c.g5.v8.dt.core.V8ExtensionNature";
pub const V8_EXTERNAL_OBJECTS_NATURE: &str = "com._1c.g5.v8.dt.core.V8ExternalObjectsNature";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdtProjectDescriptor {
    pub name: String,
    pub natures: HashSet<String>,
}

impl EdtProjectDescriptor {
    pub fn kind(&self) -> Option<EdtProjectKind> {
        let has_configuration = self.natures.contains(V8_CONFIGURATION_NATURE);
        let has_extension = self.natures.contains(V8_EXTENSION_NATURE);
        let has_external_objects = self.natures.contains(V8_EXTERNAL_OBJECTS_NATURE);
        let count = has_configuration as u8 + has_extension as u8 + has_external_objects as u8;
        if count != 1 {
            return None;
        }

        if has_configuration {
            Some(EdtProjectKind::Configuration)
        } else if has_extension {
            Some(EdtProjectKind::Extension)
        } else {
            Some(EdtProjectKind::ExternalObjects)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdtProjectKind {
    Configuration,
    Extension,
    ExternalObjects,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdtProjectManifest {
    pub runtime_version: Option<String>,
    pub base_project: Option<String>,
}

pub fn parse_project_file(
    content: &str,
    source_path: &Path,
) -> Result<Option<EdtProjectDescriptor>, String> {
    let mut reader = Reader::from_str(content);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut root_tag = None::<String>;
    let mut stack = Vec::<String>::new();
    let mut project_name = None::<String>;
    let mut natures = HashSet::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let tag = xml_local_name(event.name().as_ref());
                if root_tag.is_none() {
                    root_tag = Some(tag.clone());
                }
                stack.push(tag);
            }
            Ok(Event::Empty(event)) => {
                let tag = xml_local_name(event.name().as_ref());
                if root_tag.is_none() {
                    root_tag = Some(tag.clone());
                }
                stack.push(tag);
                stack.pop();
            }
            Ok(Event::Text(text)) => {
                let value = text
                    .unescape()
                    .map_err(|error| {
                        format!(
                            "failed to decode EDT project marker '{}': {error}",
                            source_path.display()
                        )
                    })?
                    .trim()
                    .to_owned();
                if stack.len() == 2
                    && stack[0] == "projectDescription"
                    && stack[1] == "name"
                    && project_name.is_none()
                    && !value.is_empty()
                {
                    project_name = Some(value);
                } else if stack.len() == 3
                    && stack[0] == "projectDescription"
                    && stack[1] == "natures"
                    && stack[2] == "nature"
                    && !value.is_empty()
                {
                    natures.insert(value);
                }
            }
            Ok(Event::End(_event)) => {
                stack.pop();
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(format!(
                    "failed to parse EDT project marker '{}': {error}",
                    source_path.display()
                ));
            }
            _ => {}
        }
        buf.clear();
    }

    if !stack.is_empty() {
        return Err(format!(
            "failed to parse EDT project marker '{}': unexpected EOF",
            source_path.display()
        ));
    }

    if root_tag.as_deref() != Some("projectDescription") {
        return Ok(None);
    }

    Ok(project_name.map(|name| EdtProjectDescriptor { name, natures }))
}

pub fn parse_project_manifest(
    content: &str,
    source_path: &Path,
) -> Result<EdtProjectManifest, String> {
    let mut headers = std::collections::BTreeMap::<String, String>::new();
    let mut last_key = None::<String>;

    for raw_line in content.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            let Some(key) = last_key.as_ref() else {
                return Err(format!(
                    "failed to parse EDT project manifest '{}': unexpected continuation line",
                    source_path.display()
                ));
            };
            if let Some(value) = headers.get_mut(key) {
                value.push_str(line.trim_start());
            }
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!(
                "failed to parse EDT project manifest '{}': invalid header syntax",
                source_path.display()
            ));
        };
        let key = key.trim().to_owned();
        let value = value.trim().to_owned();
        last_key = Some(key.clone());
        headers.insert(key, value);
    }

    Ok(EdtProjectManifest {
        runtime_version: headers
            .get("Runtime-Version")
            .cloned()
            .filter(|value| !value.is_empty()),
        base_project: headers
            .get("Base-Project")
            .cloned()
            .filter(|value| !value.is_empty() && value != "null"),
    })
}

pub fn project_descriptor_path(project_dir: &Path) -> std::path::PathBuf {
    project_dir.join(".project")
}

pub fn project_manifest_path(project_dir: &Path) -> std::path::PathBuf {
    project_dir.join("DT-INF").join("PROJECT.PMF")
}

pub fn ordinary_root_marker_path(project_dir: &Path) -> std::path::PathBuf {
    project_dir
        .join("src")
        .join("Configuration")
        .join("Configuration.mdo")
}

pub fn external_root_descriptor_path(project_dir: &Path) -> std::path::PathBuf {
    project_dir.join("src").join("root.xml")
}

pub fn read_project_descriptor_from_dir(
    project_dir: &Path,
) -> Result<Option<EdtProjectDescriptor>, String> {
    let project_file = project_descriptor_path(project_dir);
    if !project_file.is_file() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&project_file).map_err(|error| {
        format!(
            "failed to read EDT project marker '{}': {error}",
            project_file.display()
        )
    })?;
    parse_project_file(&content, &project_file)
}

pub fn read_project_manifest_from_dir(
    project_dir: &Path,
) -> Result<Option<EdtProjectManifest>, String> {
    let manifest_path = project_manifest_path(project_dir);
    if !manifest_path.is_file() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&manifest_path).map_err(|error| {
        format!(
            "failed to read EDT project manifest '{}': {error}",
            manifest_path.display()
        )
    })?;
    parse_project_manifest(&content, &manifest_path).map(Some)
}

pub fn read_project_name_from_dir(project_dir: &Path) -> Result<String, String> {
    read_project_descriptor_from_dir(project_dir)?
        .map(|project| project.name)
        .ok_or_else(|| {
            format!(
                "EDT project directory must contain a parseable '.project': {}",
                project_dir.display()
            )
        })
}

pub fn is_valid_runtime_version(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts
            .into_iter()
            .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
}

pub fn validate_native_ordinary_project(
    project_dir: &Path,
    expected_kind: EdtProjectKind,
    expected_base_project: Option<&str>,
) -> Result<EdtProjectDescriptor, String> {
    if !matches!(
        expected_kind,
        EdtProjectKind::Configuration | EdtProjectKind::Extension
    ) {
        return Err(format!(
            "ordinary EDT validation does not support {:?}: {}",
            expected_kind,
            project_dir.display()
        ));
    }

    let Some(project) = read_project_descriptor_from_dir(project_dir)? else {
        return Err(format!(
            "EDT project directory must contain '.project': {}",
            project_dir.display()
        ));
    };
    let Some(actual_kind @ (EdtProjectKind::Configuration | EdtProjectKind::Extension)) =
        project.kind()
    else {
        return Err(format!(
            "EDT project directory must declare exactly one ordinary EDT nature ({} or {}): {}",
            V8_CONFIGURATION_NATURE,
            V8_EXTENSION_NATURE,
            project_dir.display()
        ));
    };
    if actual_kind != expected_kind {
        return Err(format!(
            "EDT project directory resolves to {:?}, expected {:?}: {}",
            actual_kind,
            expected_kind,
            project_dir.display()
        ));
    }

    let Some(manifest) = read_project_manifest_from_dir(project_dir)? else {
        return Err(format!(
            "EDT project directory must contain 'DT-INF/PROJECT.PMF': {}",
            project_dir.display()
        ));
    };
    if !manifest
        .runtime_version
        .as_deref()
        .is_some_and(is_valid_runtime_version)
    {
        return Err(format!(
            "EDT project directory must declare Runtime-Version using 'x.y.z': {}",
            project_dir.display()
        ));
    }
    if expected_kind == EdtProjectKind::Extension {
        let actual_base_project = manifest.base_project.ok_or_else(|| {
            format!(
                "EDT extension project must declare Base-Project in 'DT-INF/PROJECT.PMF': {}",
                project_dir.display()
            )
        })?;
        if let Some(expected_base_project) = expected_base_project {
            if actual_base_project != expected_base_project {
                return Err(format!(
                    "EDT extension project must declare Base-Project '{}' but found '{}': {}",
                    expected_base_project,
                    actual_base_project,
                    project_dir.display()
                ));
            }
        }
    }
    if !ordinary_root_marker_path(project_dir).is_file() {
        return Err(format!(
            "EDT project directory must contain 'src/Configuration/Configuration.mdo': {}",
            project_dir.display()
        ));
    }

    Ok(project)
}

pub fn validate_native_external_project(
    project_dir: &Path,
) -> Result<EdtProjectDescriptor, String> {
    let Some(project) = read_project_descriptor_from_dir(project_dir)? else {
        return Err(format!(
            "EDT external project directory must contain '.project': {}",
            project_dir.display()
        ));
    };
    if project.kind() != Some(EdtProjectKind::ExternalObjects) {
        return Err(format!(
            "EDT external project directory must declare {} nature: {}",
            V8_EXTERNAL_OBJECTS_NATURE,
            project_dir.display()
        ));
    }

    let Some(manifest) = read_project_manifest_from_dir(project_dir)? else {
        return Err(format!(
            "EDT external project directory must contain 'DT-INF/PROJECT.PMF': {}",
            project_dir.display()
        ));
    };
    if !manifest
        .runtime_version
        .as_deref()
        .is_some_and(is_valid_runtime_version)
    {
        return Err(format!(
            "EDT external project directory must declare Runtime-Version using 'x.y.z': {}",
            project_dir.display()
        ));
    }
    if manifest.base_project.is_none() {
        return Err(format!(
            "EDT external project directory must declare Base-Project in 'DT-INF/PROJECT.PMF': {}",
            project_dir.display()
        ));
    }
    if !external_root_descriptor_path(project_dir).is_file() {
        return Err(format!(
            "EDT external project directory must contain canonical 'src/root.xml': {}",
            project_dir.display()
        ));
    }

    Ok(project)
}

fn manifest_matches_kind(manifest: &EdtProjectManifest, kind: EdtProjectKind) -> bool {
    manifest
        .runtime_version
        .as_deref()
        .is_some_and(is_valid_runtime_version)
        && match kind {
            EdtProjectKind::Configuration => true,
            EdtProjectKind::Extension | EdtProjectKind::ExternalObjects => {
                manifest.base_project.is_some()
            }
        }
}

pub fn detect_native_ordinary_project_kind(
    project_dir: &Path,
) -> Result<Option<EdtProjectKind>, String> {
    let Some(project) = read_project_descriptor_from_dir(project_dir)? else {
        return Ok(None);
    };
    let Some(kind @ (EdtProjectKind::Configuration | EdtProjectKind::Extension)) = project.kind()
    else {
        return Ok(None);
    };
    let Some(manifest) = read_project_manifest_from_dir(project_dir)? else {
        return Ok(None);
    };
    if !manifest_matches_kind(&manifest, kind) {
        return Ok(None);
    }
    if !ordinary_root_marker_path(project_dir).is_file() {
        return Ok(None);
    }

    Ok(Some(kind))
}

pub fn has_native_external_project_layout(project_dir: &Path) -> Result<bool, String> {
    let Some(project) = read_project_descriptor_from_dir(project_dir)? else {
        return Ok(false);
    };
    if project.kind() != Some(EdtProjectKind::ExternalObjects) {
        return Ok(false);
    }
    let Some(manifest) = read_project_manifest_from_dir(project_dir)? else {
        return Ok(false);
    };
    Ok(
        manifest_matches_kind(&manifest, EdtProjectKind::ExternalObjects)
            && external_root_descriptor_path(project_dir).is_file(),
    )
}

fn xml_local_name(name: &[u8]) -> String {
    let raw = String::from_utf8_lossy(name);
    raw.rsplit(':').next().unwrap_or(raw.as_ref()).to_owned()
}

#[cfg(test)]
mod tests {
    use super::{
        detect_native_ordinary_project_kind, external_root_descriptor_path,
        has_native_external_project_layout, is_valid_runtime_version, ordinary_root_marker_path,
        parse_project_file, parse_project_manifest, read_project_name_from_dir,
        validate_native_external_project, validate_native_ordinary_project, EdtProjectKind,
        V8_CONFIGURATION_NATURE, V8_EXTENSION_NATURE, V8_EXTERNAL_OBJECTS_NATURE,
    };
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent dir");
        }
        fs::write(path, contents).expect("write file");
    }

    fn write_project(project_dir: &Path, name: &str, nature: &str, manifest: &str) {
        write_file(
            &project_dir.join(".project"),
            &format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<projectDescription>\n  <name>{name}</name>\n  <natures>\n    <nature>{nature}</nature>\n  </natures>\n</projectDescription>\n"
            ),
        );
        write_file(&project_dir.join("DT-INF").join("PROJECT.PMF"), manifest);
    }

    #[test]
    fn parses_project_natures_and_resolves_kind() {
        let descriptor = parse_project_file(
            &format!(
                "<projectDescription><name>BaseProject</name><natures><nature>{}</nature></natures></projectDescription>",
                V8_CONFIGURATION_NATURE
            ),
            Path::new("/tmp/.project"),
        )
        .expect("project descriptor")
        .expect("edt project");

        assert_eq!(descriptor.name, "BaseProject");
        assert_eq!(descriptor.kind(), Some(EdtProjectKind::Configuration));
    }

    #[test]
    fn project_kind_is_none_for_conflicting_natures() {
        let descriptor = parse_project_file(
            &format!(
                "<projectDescription><name>BaseProject</name><natures><nature>{}</nature><nature>{}</nature></natures></projectDescription>",
                V8_CONFIGURATION_NATURE, V8_EXTENSION_NATURE
            ),
            Path::new("/tmp/.project"),
        )
        .expect("project descriptor")
        .expect("edt project");

        assert_eq!(descriptor.kind(), None);
    }

    #[test]
    fn ignores_nature_outside_natures_section() {
        let descriptor = parse_project_file(
            &format!(
                "<projectDescription><name>BaseProject</name><linkedResources><nature>{}</nature></linkedResources></projectDescription>",
                V8_CONFIGURATION_NATURE
            ),
            Path::new("/tmp/.project"),
        )
        .expect("project descriptor")
        .expect("edt project");

        assert!(descriptor.natures.is_empty());
        assert_eq!(descriptor.kind(), None);
    }

    #[test]
    fn accepts_project_with_leading_comment_before_root() {
        let descriptor = parse_project_file(
            &format!(
                "<!-- comment --><projectDescription><name>BaseProject</name><natures><nature>{}</nature></natures></projectDescription>",
                V8_CONFIGURATION_NATURE
            ),
            Path::new("/tmp/.project"),
        )
        .expect("project descriptor")
        .expect("edt project");

        assert_eq!(descriptor.name, "BaseProject");
        assert_eq!(descriptor.kind(), Some(EdtProjectKind::Configuration));
    }

    #[test]
    fn parses_runtime_version_from_project_manifest() {
        let manifest = parse_project_manifest(
            "Manifest-Version: 1.0\nRuntime-Version: 8.3.24\n",
            Path::new("/tmp/DT-INF/PROJECT.PMF"),
        )
        .expect("manifest");

        assert_eq!(manifest.runtime_version.as_deref(), Some("8.3.24"));
    }

    #[test]
    fn fixture_manifest_preserves_runtime_version() {
        let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("edt")
            .join("configuration")
            .join("DT-INF")
            .join("PROJECT.PMF");
        let content = fs::read_to_string(&fixture_path).expect("fixture manifest");
        let manifest = parse_project_manifest(&content, &fixture_path).expect("manifest");

        assert_eq!(manifest.runtime_version.as_deref(), Some("8.3.27"));
    }

    #[test]
    fn validates_runtime_version_shape() {
        assert!(is_valid_runtime_version("8.3.27"));
        assert!(!is_valid_runtime_version("8.3"));
        assert!(!is_valid_runtime_version("8.3.27.beta"));
        assert!(!is_valid_runtime_version("not-a-runtime"));
    }

    #[test]
    fn native_extension_requires_base_project_in_manifest() {
        let dir = tempdir().expect("tempdir");
        write_project(
            dir.path(),
            "ExtensionProject",
            V8_EXTENSION_NATURE,
            "Manifest-Version: 1.0\nRuntime-Version: 8.3.27\n",
        );
        write_file(
            &ordinary_root_marker_path(dir.path()),
            "<Configuration />\n",
        );

        assert_eq!(
            detect_native_ordinary_project_kind(dir.path()).expect("ordinary project"),
            None
        );
    }

    #[test]
    fn native_external_layout_requires_base_project_and_root_descriptor() {
        let dir = tempdir().expect("tempdir");
        write_project(
            dir.path(),
            "ProcessorProject",
            V8_EXTERNAL_OBJECTS_NATURE,
            "Base-Project: BaseProject\nManifest-Version: 1.0\nRuntime-Version: 8.3.27\n",
        );
        write_file(
            &dir.path().join("src").join("nested").join("processor.xml"),
            "<ExternalDataProcessor />\n",
        );

        assert!(
            !has_native_external_project_layout(dir.path()).expect("external layout without root")
        );

        write_file(
            &external_root_descriptor_path(dir.path()),
            "<ExternalDataProcessor />\n",
        );
        assert!(has_native_external_project_layout(dir.path()).expect("external layout"));

        write_file(
            &dir.path().join("DT-INF").join("PROJECT.PMF"),
            "Manifest-Version: 1.0\nRuntime-Version: 8.3.27\n",
        );
        assert!(!has_native_external_project_layout(dir.path())
            .expect("external layout without base project"));
    }

    #[test]
    fn read_project_name_from_dir_requires_parseable_project_file() {
        let dir = tempdir().expect("tempdir");

        let error = read_project_name_from_dir(dir.path()).expect_err("missing .project must fail");
        assert!(error.contains(".project"));
    }

    #[test]
    fn ordinary_validation_rejects_unexpected_extension_base_project() {
        let dir = tempdir().expect("tempdir");
        write_project(
            dir.path(),
            "ExtensionProject",
            V8_EXTENSION_NATURE,
            "Base-Project: WrongBase\nManifest-Version: 1.0\nRuntime-Version: 8.3.27\n",
        );
        write_file(
            &ordinary_root_marker_path(dir.path()),
            "<Configuration />\n",
        );

        let error = validate_native_ordinary_project(
            dir.path(),
            EdtProjectKind::Extension,
            Some("ExpectedBase"),
        )
        .expect_err("base project mismatch must fail");
        assert!(error.contains("ExpectedBase"));
        assert!(error.contains("WrongBase"));
    }

    #[test]
    fn external_validation_requires_canonical_root_descriptor() {
        let dir = tempdir().expect("tempdir");
        write_project(
            dir.path(),
            "ProcessorProject",
            V8_EXTERNAL_OBJECTS_NATURE,
            "Base-Project: BaseProject\nManifest-Version: 1.0\nRuntime-Version: 8.3.27\n",
        );
        write_file(
            &dir.path().join("src").join("nested").join("processor.xml"),
            "<ExternalDataProcessor />\n",
        );

        let error = validate_native_external_project(dir.path())
            .expect_err("missing canonical root must fail");
        assert!(error.contains("src/root.xml"));
    }
}
