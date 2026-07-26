use std::path::{Path, PathBuf};

use crate::change_detection::analyzer::{ChangeKind, FileChange};

/// Default maximum number of changed files before forcing a full load.
#[cfg(test)]
pub const DEFAULT_PARTIAL_LOAD_THRESHOLD: usize = 20;

/// The name of the root configuration descriptor — if changed, partial load is forbidden.
const CONFIGURATION_XML: &str = "Configuration.xml";

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// Decision made by [`decide`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadDecision {
    /// Load only the listed files.
    Partial(Vec<PathBuf>),
    /// Load the entire source-set directory.
    Full,
}

/// Decide whether a partial or full load is appropriate for `changes`.
pub fn decide(changes: &[FileChange], source_root: &Path, threshold: usize) -> LoadDecision {
    if threshold == 0 {
        return LoadDecision::Full;
    }

    if changes
        .iter()
        .any(|change| is_configuration_xml(&change.path))
    {
        return LoadDecision::Full;
    }

    if changes
        .iter()
        .any(|change| change.kind == ChangeKind::Deleted)
    {
        return LoadDecision::Full;
    }

    let Some(expanded) = expand_files(changes, source_root) else {
        return LoadDecision::Full;
    };

    if expanded.is_empty() || expanded.len() > threshold {
        LoadDecision::Full
    } else {
        LoadDecision::Partial(expanded)
    }
}

/// Write a partial-load list file as UTF-8 with BOM and CRLF-separated paths.
///
/// Paths are written relative to `source_root` as required by Designer's
/// `-listFile` parameter when running in agent mode. Path component separators
/// remain native to the current operating system.
pub fn write_list_file(paths: &[PathBuf], source_root: &Path, dest: &Path) -> std::io::Result<()> {
    let rel_paths = relative_paths(paths, source_root)?;
    let lines = rel_paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let contents = lines.join("\r\n");
    let mut payload = Vec::with_capacity(UTF8_BOM.len() + contents.len());
    payload.extend_from_slice(UTF8_BOM);
    payload.extend_from_slice(contents.as_bytes());
    std::fs::write(dest, payload)
}

/// Convert safe absolute paths into relative paths under `source_root`.
pub fn relative_paths(paths: &[PathBuf], source_root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let root_real = canonicalize_existing(source_root).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "source root does not exist or is not canonicalizable: {}",
                source_root.display()
            ),
        )
    })?;
    let mut rel_paths = Vec::new();

    for path in paths {
        let rel = safe_relative_path(path, source_root, &root_real).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "path cannot be safely represented in partial list: {}",
                    path.display()
                ),
            )
        })?;
        if !rel.as_os_str().is_empty() {
            rel_paths.push(rel);
        }
    }

    Ok(rel_paths)
}

fn expand_files(changes: &[FileChange], source_root: &Path) -> Option<Vec<PathBuf>> {
    let root_real = canonicalize_existing(source_root)?;
    let mut paths = Vec::new();

    for change in changes {
        push_file_if_safe(&mut paths, &change.path, source_root, &root_real)?;

        if is_bsl(&change.path) {
            if let Some(xml) = sibling_xml(&change.path) {
                push_file_if_safe_if_exists(&mut paths, &xml, source_root, &root_real)?;
            }

            for descriptor in ancestor_xml_descriptors(&change.path, source_root) {
                push_file_if_safe_if_exists(&mut paths, &descriptor, source_root, &root_real)?;
            }
        }
    }

    paths.sort();
    paths.dedup();
    Some(paths)
}

fn push_file_if_safe(
    paths: &mut Vec<PathBuf>,
    candidate: &Path,
    source_root: &Path,
    root_real: &Path,
) -> Option<()> {
    if !candidate.is_file() {
        return None;
    }

    let relative = safe_relative_path(candidate, source_root, root_real)?;
    paths.push(source_root.join(relative));
    Some(())
}

fn push_file_if_safe_if_exists(
    paths: &mut Vec<PathBuf>,
    candidate: &Path,
    source_root: &Path,
    root_real: &Path,
) -> Option<()> {
    if !candidate.exists() {
        return Some(());
    }

    push_file_if_safe(paths, candidate, source_root, root_real)
}

fn safe_relative_path(path: &Path, source_root: &Path, root_real: &Path) -> Option<PathBuf> {
    let candidate_real = canonicalize_existing(path)?;
    if !candidate_real.starts_with(root_real) {
        return None;
    }

    if let Ok(relative) = path.strip_prefix(source_root) {
        return Some(relative.to_path_buf());
    }

    candidate_real
        .strip_prefix(root_real)
        .ok()
        .map(Path::to_path_buf)
}

fn canonicalize_existing(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

fn is_configuration_xml(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|name| name == CONFIGURATION_XML)
        .unwrap_or(false)
}

fn is_bsl(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("bsl"))
        .unwrap_or(false)
}

/// Return the XML descriptor alongside a `.bsl` file (same name, `.xml` ext).
fn sibling_xml(bsl: &Path) -> Option<PathBuf> {
    let parent = bsl.parent()?;
    let stem = bsl.file_stem()?.to_str()?;
    Some(parent.join(format!("{stem}.xml")))
}

fn ancestor_xml_descriptors(bsl: &Path, source_root: &Path) -> Vec<PathBuf> {
    let mut descriptors = Vec::new();
    let mut current = bsl.parent();

    while let Some(dir) = current {
        if dir == source_root {
            break;
        }

        descriptors.push(dir.with_extension("xml"));
        current = dir.parent();
    }

    descriptors
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{
        decide, relative_paths, write_list_file, LoadDecision, DEFAULT_PARTIAL_LOAD_THRESHOLD,
    };
    use crate::change_detection::analyzer::{ChangeKind, FileChange};

    #[test]
    fn write_list_file_skips_empty_relative_paths() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let list_file = root.join("partial.lst");

        write_list_file(&[root.to_path_buf()], root, &list_file).expect("write list");

        assert_eq!(
            std::fs::read(list_file).expect("read list"),
            b"\xEF\xBB\xBF"
        );
    }

    #[test]
    fn write_list_file_uses_utf8_bom_and_crlf_for_unicode_relative_paths() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("src");
        let first = root.join("CommonModules").join("ОбщийМодуль1.xml");
        let second = root
            .join("CommonModules")
            .join("ОбщийМодуль1")
            .join("Ext")
            .join("Module.bsl");
        let list_file = temp.path().join("partial.lst");

        std::fs::create_dir_all(first.parent().expect("first parent")).expect("first parent dir");
        std::fs::create_dir_all(second.parent().expect("second parent"))
            .expect("second parent dir");
        std::fs::write(&first, "<xml />").expect("write first");
        std::fs::write(&second, "procedure Test()\nendprocedure").expect("write second");

        write_list_file(&[first.clone(), second.clone()], &root, &list_file).expect("write list");

        let relative_payload = [first, second]
            .iter()
            .map(|path| {
                path.strip_prefix(&root)
                    .expect("relative path")
                    .display()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\r\n");
        let mut expected = b"\xEF\xBB\xBF".to_vec();
        expected.extend_from_slice(relative_payload.as_bytes());

        assert_eq!(std::fs::read(list_file).expect("read list"), expected);
    }

    #[test]
    fn relative_paths_returns_relative_entries_for_safe_paths() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let nested = root.join("Catalogs.Items");
        std::fs::create_dir_all(&nested).expect("mkdir");
        let file = nested.join("ObjectModule.bsl");
        std::fs::write(&file, "module").expect("write");

        let rels = relative_paths(&[file.clone()], root).expect("relative paths");

        assert_eq!(rels, vec![PathBuf::from("Catalogs.Items/ObjectModule.bsl")]);
    }

    #[cfg(unix)]
    #[test]
    fn relative_paths_rejects_path_outside_root() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("src");
        let outside = temp.path().join("outside");
        let link = root.join("Catalogs.Items");
        let escaped = outside.join("ObjectModule.bsl");

        std::fs::create_dir_all(&root).expect("root");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::write(&escaped, "module").expect("escaped");
        symlink(&outside, &link).expect("link");

        let err =
            relative_paths(&[link.join("ObjectModule.bsl")], &root).expect_err("expected error");

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn write_list_file_fails_for_paths_outside_root() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("src");
        let outside = temp.path().join("outside");
        let link = root.join("Catalogs.Items");
        let escaped = outside.join("ObjectModule.bsl");
        let list_file = temp.path().join("partial.lst");

        std::fs::create_dir_all(&root).expect("root");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::write(&escaped, "module").expect("escaped");
        symlink(&outside, &link).expect("link");

        let err = write_list_file(&[link.join("ObjectModule.bsl")], &root, &list_file)
            .expect_err("expected invalid path");

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn decide_expands_bsl_to_existing_xml_files_only() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let object_dir = root.join("Catalogs.Items");
        let module = object_dir.join("ObjectModule.bsl");
        let xml = object_dir.join("ObjectModule.xml");

        std::fs::create_dir_all(&object_dir).expect("create object dir");
        std::fs::write(&module, "module").expect("write module");
        std::fs::write(&xml, "<xml />").expect("write xml");

        let decision = decide(
            &[FileChange {
                path: module.clone(),
                rel_path: "Catalogs.Items/ObjectModule.bsl".to_owned(),
                kind: ChangeKind::Modified,
                pre_hash: Some("old".to_owned()),
                post_hash: Some("new".to_owned()),
            }],
            root,
            DEFAULT_PARTIAL_LOAD_THRESHOLD,
        );

        assert_eq!(decision, LoadDecision::Partial(vec![module, xml]));
    }

    #[test]
    fn decide_expands_nested_bsl_to_ancestor_xml_files_without_directories() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let module = root.join("Catalogs/Items/Forms/ItemForm/Ext/Form/Module.bsl");
        let form_xml = root.join("Catalogs/Items/Forms/ItemForm.xml");
        let form_ext_xml = root.join("Catalogs/Items/Forms/ItemForm/Ext/Form.xml");
        let object_xml = root.join("Catalogs/Items.xml");

        std::fs::create_dir_all(module.parent().expect("module parent")).expect("module dir");
        std::fs::write(&module, "module").expect("write module");
        std::fs::write(&form_xml, "<form />").expect("write form xml");
        std::fs::write(&form_ext_xml, "<form ext />").expect("write form ext xml");
        std::fs::write(&object_xml, "<object />").expect("write object xml");

        let decision = decide(
            &[FileChange {
                path: module.clone(),
                rel_path: "Catalogs/Items/Forms/ItemForm/Ext/Form/Module.bsl".to_owned(),
                kind: ChangeKind::Modified,
                pre_hash: Some("old".to_owned()),
                post_hash: Some("new".to_owned()),
            }],
            root,
            DEFAULT_PARTIAL_LOAD_THRESHOLD,
        );

        let LoadDecision::Partial(paths) = decision else {
            panic!("expected partial decision");
        };

        assert_eq!(paths, vec![module, form_ext_xml, form_xml, object_xml]);
        assert!(paths.iter().all(|path| path.is_file()));
    }

    #[test]
    fn decide_forces_full_when_changed_path_is_directory() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let directory = root.join("CommonModules");
        std::fs::create_dir_all(&directory).expect("directory");

        let decision = decide(
            &[FileChange {
                path: directory,
                rel_path: "CommonModules".to_owned(),
                kind: ChangeKind::Modified,
                pre_hash: Some("old".to_owned()),
                post_hash: Some("new".to_owned()),
            }],
            root,
            DEFAULT_PARTIAL_LOAD_THRESHOLD,
        );

        assert_eq!(decision, LoadDecision::Full);
    }

    #[test]
    fn decide_forces_full_when_configuration_xml_changed() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let config_xml = root.join("Configuration.xml");
        std::fs::write(&config_xml, "<xml />").expect("write config");

        let decision = decide(
            &[FileChange {
                path: config_xml,
                rel_path: "Configuration.xml".to_owned(),
                kind: ChangeKind::Modified,
                pre_hash: Some("old".to_owned()),
                post_hash: Some("new".to_owned()),
            }],
            root,
            DEFAULT_PARTIAL_LOAD_THRESHOLD,
        );

        assert_eq!(decision, LoadDecision::Full);
    }

    #[test]
    fn decide_forces_full_when_deleted_files_exist() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let removed = root.join("Catalogs.Items").join("ObjectModule.bsl");

        let decision = decide(
            &[FileChange {
                path: removed,
                rel_path: "Catalogs.Items/ObjectModule.bsl".to_owned(),
                kind: ChangeKind::Deleted,
                pre_hash: Some("old".to_owned()),
                post_hash: None,
            }],
            root,
            DEFAULT_PARTIAL_LOAD_THRESHOLD,
        );

        assert_eq!(decision, LoadDecision::Full);
    }

    #[test]
    fn decide_forces_full_when_threshold_is_exceeded() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let mut changes = Vec::new();

        for index in 0..=DEFAULT_PARTIAL_LOAD_THRESHOLD {
            let path = root.join(format!("CommonModules/Module{index}.bsl"));
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(&path, "module").expect("write");
            changes.push(FileChange {
                rel_path: format!("CommonModules/Module{index}.bsl"),
                path,
                kind: ChangeKind::Modified,
                pre_hash: Some("old".to_owned()),
                post_hash: Some("new".to_owned()),
            });
        }

        let decision = decide(&changes, root, DEFAULT_PARTIAL_LOAD_THRESHOLD);
        assert_eq!(decision, LoadDecision::Full);
    }

    #[cfg(unix)]
    #[test]
    fn traversal_or_symlink_escape_forces_full() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("src");
        let outside_dir = temp.path().join("outside");
        let link_dir = root.join("Catalogs.Items");
        let escaped = outside_dir.join("ObjectModule.bsl");

        std::fs::create_dir_all(&outside_dir).expect("outside");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(&escaped, "module").expect("write escaped");
        symlink(&outside_dir, &link_dir).expect("create symlink");

        let decision = decide(
            &[FileChange {
                path: link_dir.join("ObjectModule.bsl"),
                rel_path: "Catalogs.Items/ObjectModule.bsl".to_owned(),
                kind: ChangeKind::Modified,
                pre_hash: Some("old".to_owned()),
                post_hash: Some("new".to_owned()),
            }],
            &root,
            DEFAULT_PARTIAL_LOAD_THRESHOLD,
        );

        assert_eq!(decision, LoadDecision::Full);
    }
}
