use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::domain::config_init::{ConfigInitResult, ConfigInitSourceSet};
use crate::support::error::AppError;
use crate::support::path::is_safe_path_segment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigInitRequest {
    pub project_dir: PathBuf,
    pub output_path: PathBuf,
    pub force: bool,
    pub connection: Option<String>,
    pub format: ConfigFormatRequest,
    pub builder: ConfigBuilderRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormatRequest {
    Auto,
    Designer,
    Edt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigBuilderRequest {
    Designer,
    Ibcmd,
}

pub fn execute(request: &ConfigInitRequest) -> Result<ConfigInitResult, AppError> {
    let started = Instant::now();
    let project_dir = std::fs::canonicalize(&request.project_dir).map_err(|error| {
        AppError::Runtime(format!(
            "failed to resolve project directory '{}': {error}",
            request.project_dir.display()
        ))
    })?;
    if !project_dir.is_dir() {
        return Err(AppError::Validation(format!(
            "project directory is not a directory: {}",
            project_dir.display()
        )));
    }

    let output_path = resolve_output_path(&project_dir, &request.output_path);
    let overwritten = output_path.exists();
    if overwritten && !request.force {
        return Err(AppError::Validation(format!(
            "config file already exists: {} (use --force to overwrite)",
            output_path.display()
        )));
    }

    let detected = discover_sources(&project_dir)?;
    let format = choose_format(request.format, &detected);
    let source_sets = build_source_sets(&project_dir, &detected, format);
    let yaml = render_config(
        &project_dir,
        request.connection.as_deref(),
        format,
        request.builder,
        &source_sets,
    );

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            AppError::Runtime(format!(
                "failed to create config directory '{}': {error}",
                parent.display()
            ))
        })?;
    }
    std::fs::write(&output_path, yaml).map_err(|error| {
        AppError::Runtime(format!(
            "failed to write config file '{}': {error}",
            output_path.display()
        ))
    })?;

    Ok(ConfigInitResult {
        ok: true,
        path: output_path.display().to_string(),
        format: format.as_yaml().to_owned(),
        builder: request.builder.as_yaml().to_owned(),
        source_sets,
        overwritten,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

fn resolve_output_path(project_dir: &Path, output_path: &Path) -> PathBuf {
    if output_path.is_absolute() {
        output_path.to_path_buf()
    } else {
        project_dir.join(output_path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetectedSources {
    designer: Vec<DetectedSource>,
    edt: Vec<DetectedSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetectedSource {
    path: PathBuf,
    purpose: SourcePurpose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourcePurpose {
    Configuration,
    Extension,
}

impl SourcePurpose {
    const fn as_yaml(self) -> &'static str {
        match self {
            Self::Configuration => "CONFIGURATION",
            Self::Extension => "EXTENSION",
        }
    }
}

impl ConfigFormatRequest {
    const fn as_yaml(self) -> &'static str {
        match self {
            Self::Auto => "AUTO",
            Self::Designer => "DESIGNER",
            Self::Edt => "EDT",
        }
    }
}

impl ConfigBuilderRequest {
    const fn as_yaml(self) -> &'static str {
        match self {
            Self::Designer => "DESIGNER",
            Self::Ibcmd => "IBCMD",
        }
    }
}

fn choose_format(
    requested: ConfigFormatRequest,
    detected: &DetectedSources,
) -> ConfigFormatRequest {
    match requested {
        ConfigFormatRequest::Auto => {
            if !detected.designer.is_empty() || detected.edt.is_empty() {
                ConfigFormatRequest::Designer
            } else {
                ConfigFormatRequest::Edt
            }
        }
        explicit => explicit,
    }
}

fn discover_sources(project_dir: &Path) -> Result<DetectedSources, AppError> {
    let mut designer = Vec::new();
    let mut edt = Vec::new();
    scan_dir(project_dir, project_dir, &mut designer, &mut edt)?;
    designer.sort_by(|a, b| a.path.cmp(&b.path));
    edt.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(DetectedSources { designer, edt })
}

fn scan_dir(
    root: &Path,
    dir: &Path,
    designer: &mut Vec<DetectedSource>,
    edt: &mut Vec<DetectedSource>,
) -> Result<(), AppError> {
    if should_skip_dir(root, dir) {
        return Ok(());
    }

    let designer_marker = dir.join("Configuration.xml");
    if designer_marker.is_file() {
        designer.push(DetectedSource {
            path: dir.to_path_buf(),
            purpose: detect_designer_purpose(&designer_marker),
        });
        return Ok(());
    }

    if dir.join(".project").is_file() {
        edt.push(DetectedSource {
            path: dir.to_path_buf(),
            purpose: detect_edt_purpose(dir),
        });
        return Ok(());
    }

    let entries = std::fs::read_dir(dir).map_err(|error| {
        AppError::Runtime(format!(
            "failed to read source directory '{}': {error}",
            dir.display()
        ))
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            AppError::Runtime(format!(
                "failed to read source directory entry '{}': {error}",
                dir.display()
            ))
        })?;
        let path = entry.path();
        if path.is_dir() {
            scan_dir(root, &path, designer, edt)?;
        }
    }

    Ok(())
}

fn should_skip_dir(root: &Path, dir: &Path) -> bool {
    if dir == root {
        return false;
    }
    let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(
        name,
        ".git" | ".idea" | ".vscode" | ".v8-runner" | "target" | "node_modules" | "dist" | "build"
    )
}

fn detect_designer_purpose(configuration_xml: &Path) -> SourcePurpose {
    match std::fs::read_to_string(configuration_xml) {
        Ok(content)
            if content.contains("<ConfigurationExtensionPurpose>")
                || content.contains("<ObjectBelonging>") =>
        {
            SourcePurpose::Extension
        }
        _ => SourcePurpose::Configuration,
    }
}

fn detect_edt_purpose(project_dir: &Path) -> SourcePurpose {
    let configuration_xml = project_dir
        .join("src")
        .join("Configuration")
        .join("Configuration.xml");
    if configuration_xml.is_file() {
        detect_designer_purpose(&configuration_xml)
    } else if path_looks_like_extension(project_dir) {
        SourcePurpose::Extension
    } else {
        SourcePurpose::Configuration
    }
}

fn path_looks_like_extension(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy().to_ascii_lowercase();
        matches!(name.as_str(), "ext" | "exts" | "extension" | "extensions")
    })
}

fn build_source_sets(
    project_dir: &Path,
    detected: &DetectedSources,
    format: ConfigFormatRequest,
) -> Vec<ConfigInitSourceSet> {
    let selected = match format {
        ConfigFormatRequest::Auto | ConfigFormatRequest::Designer => &detected.designer,
        ConfigFormatRequest::Edt => &detected.edt,
    };

    let mut source_sets: Vec<_> = selected
        .iter()
        .enumerate()
        .map(|(index, source)| ConfigInitSourceSet {
            name: source_set_name(&source.path, source.purpose, index),
            source_type: source.purpose.as_yaml().to_owned(),
            path: relative_path(project_dir, &source.path),
        })
        .collect();

    if !source_sets
        .iter()
        .any(|source_set| source_set.source_type == "CONFIGURATION")
    {
        source_sets.insert(
            0,
            ConfigInitSourceSet {
                name: "main".to_owned(),
                source_type: "CONFIGURATION".to_owned(),
                path: ".".to_owned(),
            },
        );
    }

    deduplicate_names(&mut source_sets);
    source_sets
}

fn source_set_name(path: &Path, purpose: SourcePurpose, index: usize) -> String {
    let fallback = match purpose {
        SourcePurpose::Configuration => "main",
        SourcePurpose::Extension => "extension",
    };
    let raw = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(fallback);
    let normalized = normalize_name(raw);
    if normalized.is_empty() {
        format!("{fallback}-{}", index + 1)
    } else {
        normalized
    }
}

fn normalize_name(raw: &str) -> String {
    let normalized: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = normalized.trim_matches('-').to_owned();
    if is_safe_path_segment(&trimmed) {
        trimmed
    } else {
        String::new()
    }
}

fn deduplicate_names(source_sets: &mut [ConfigInitSourceSet]) {
    let mut seen = HashSet::new();
    for source_set in source_sets {
        let base = if source_set.name.is_empty() {
            "source".to_owned()
        } else {
            source_set.name.clone()
        };
        let mut name = base.clone();
        let mut suffix = 2;
        while !seen.insert(name.clone()) {
            name = format!("{base}-{suffix}");
            suffix += 1;
        }
        source_set.name = name;
    }
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|| ".".to_owned())
}

fn render_config(
    project_dir: &Path,
    connection: Option<&str>,
    format: ConfigFormatRequest,
    builder: ConfigBuilderRequest,
    source_sets: &[ConfigInitSourceSet],
) -> String {
    let connection = connection.unwrap_or("File=build/ib");
    let mut yaml = String::new();
    yaml.push_str("# Generated by v8-runner config init\n");
    yaml.push_str(&format!(
        "basePath: '{}'\n",
        escape_yaml(&project_dir.display().to_string())
    ));
    yaml.push_str("workPath: 'build'\n");
    yaml.push_str(&format!("format: {}\n", format.as_yaml()));
    yaml.push_str(&format!("builder: {}\n", builder.as_yaml()));
    yaml.push_str("infobase:\n");
    yaml.push_str(&format!("  connection: '{}'\n", escape_yaml(connection)));
    yaml.push_str("source-set:\n");
    for source_set in source_sets {
        yaml.push_str(&format!("  - name: {}\n", source_set.name));
        yaml.push_str(&format!("    type: {}\n", source_set.source_type));
        yaml.push_str(&format!("    path: '{}'\n", escape_yaml(&source_set.path)));
    }
    yaml.push_str("build:\n");
    yaml.push_str("  partialLoadThreshold: 20\n");
    yaml
}

fn escape_yaml(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::{
        execute, ConfigBuilderRequest, ConfigFormatRequest, ConfigInitRequest, SourcePurpose,
    };
    use crate::config::loader::load_config;
    use tempfile::tempdir;

    #[test]
    fn creates_config_from_designer_sources() {
        let dir = tempdir().expect("tempdir");
        let main = dir.path().join("src").join("main");
        let ext = dir.path().join("extensions").join("sales");
        std::fs::create_dir_all(&main).expect("main");
        std::fs::create_dir_all(&ext).expect("ext");
        std::fs::write(main.join("Configuration.xml"), "<Configuration/>").expect("main xml");
        std::fs::write(
            ext.join("Configuration.xml"),
            "<ConfigurationExtensionPurpose>Customization</ConfigurationExtensionPurpose>",
        )
        .expect("ext xml");

        let result = execute(&ConfigInitRequest {
            project_dir: dir.path().to_path_buf(),
            output_path: "v8project.yaml".into(),
            force: false,
            connection: None,
            format: ConfigFormatRequest::Auto,
            builder: ConfigBuilderRequest::Designer,
        })
        .expect("init config");

        assert_eq!(result.format, "DESIGNER");
        assert_eq!(result.source_sets.len(), 2);
        assert!(std::fs::read_to_string(dir.path().join("v8project.yaml"))
            .expect("config")
            .contains("type: EXTENSION"));
    }

    #[test]
    fn refuses_to_overwrite_without_force() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(dir.path().join("v8project.yaml"), "existing").expect("existing");

        let error = execute(&ConfigInitRequest {
            project_dir: dir.path().to_path_buf(),
            output_path: "v8project.yaml".into(),
            force: false,
            connection: None,
            format: ConfigFormatRequest::Designer,
            builder: ConfigBuilderRequest::Designer,
        })
        .expect_err("should fail");

        assert!(error.to_string().contains("already exists"));
    }

    #[test]
    fn extension_detection_uses_designer_xml_marker() {
        let dir = tempdir().expect("tempdir");
        let xml = dir.path().join("Configuration.xml");
        std::fs::write(&xml, "<ObjectBelonging>Adopted</ObjectBelonging>").expect("xml");

        assert_eq!(
            super::detect_designer_purpose(&xml),
            SourcePurpose::Extension
        );
    }

    #[test]
    fn generated_config_round_trips_through_loader() {
        let dir = tempdir().expect("tempdir");
        let main = dir.path().join("Configuration.xml");
        std::fs::write(&main, "<Configuration/>").expect("main xml");

        let result = execute(&ConfigInitRequest {
            project_dir: dir.path().to_path_buf(),
            output_path: "v8project.yaml".into(),
            force: false,
            connection: None,
            format: ConfigFormatRequest::Designer,
            builder: ConfigBuilderRequest::Designer,
        })
        .expect("init config");

        let config = load_config(Some(&result.path), None).expect("load config");

        assert_eq!(config.infobase.connection, format!("File={}", dir.path().join("build/ib").display()));
    }
}
