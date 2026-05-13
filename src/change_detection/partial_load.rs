use std::path::{Component, Path, PathBuf};

use crate::change_detection::analyzer::{ChangeKind, FileChange};

/// Default maximum number of changed files before forcing a full load.
#[cfg(test)]
pub const DEFAULT_PARTIAL_LOAD_THRESHOLD: usize = 20;

/// The name of the root configuration descriptor — if changed, partial load is forbidden.
const CONFIGURATION_XML: &str = "Configuration.xml";

/// Top-level metadata-type directory names in the 1C Designer Hierarchical layout.
///
/// When a `.bsl` change is located under `<root>/<MetadataType>/<ObjectName>/...`, the
/// owning XML descriptor lives at `<root>/<MetadataType>/<ObjectName>.xml` (one level
/// above the object directory) and the related files for the object live under
/// `<root>/<MetadataType>/<ObjectName>/`. Without this whitelist we cannot reliably
/// reconstruct the owning object from a deeply nested BSL path (think
/// `Documents/<doc>/Forms/<form>/Ext/Form/Module.bsl`).
const METADATA_TYPES: &[&str] = &[
    "AccountingRegisters",
    "AccumulationRegisters",
    "BusinessProcesses",
    "CalculationRegisters",
    "Catalogs",
    "ChartsOfAccounts",
    "ChartsOfCalculationTypes",
    "ChartsOfCharacteristicTypes",
    "CommandGroups",
    "CommonAttributes",
    "CommonCommands",
    "CommonForms",
    "CommonModules",
    "CommonPictures",
    "CommonTemplates",
    "Constants",
    "DataProcessors",
    "DefinedTypes",
    "DocumentJournals",
    "Documents",
    "Enums",
    "EventSubscriptions",
    "ExchangePlans",
    "ExternalDataSources",
    "FilterCriteria",
    "FunctionalOptions",
    "FunctionalOptionsParameters",
    "HTTPServices",
    "InformationRegisters",
    "Interfaces",
    "Languages",
    "Reports",
    "Roles",
    "ScheduledJobs",
    "Sequences",
    "SessionParameters",
    "SettingsStorages",
    "StyleItems",
    "Styles",
    "Subsystems",
    "Tasks",
    "WSReferences",
    "WebServices",
    "XDTOPackages",
];

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

/// Write a partial-load list file (UTF-8, one path per line, no empty lines).
///
/// Paths are written relative to `source_root` as required by Designer's
/// `-listFile` parameter.
pub fn write_list_file(paths: &[PathBuf], source_root: &Path, dest: &Path) -> std::io::Result<()> {
    let rel_paths = relative_paths(paths, source_root)?;
    let lines = rel_paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    std::fs::write(dest, lines.join("\r\n"))
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
        // The change itself always goes into the list (the platform needs to see the
        // file whose contents actually moved).
        push_if_safe(&mut paths, &change.path, source_root, &root_real)?;

        // For .bsl files Designer cannot identify the owning metadata object from the
        // module path alone — the type/name is encoded in the XML descriptor that
        // lives next to the object directory. Without that descriptor in the list
        // Designer parses the BSL path as a property of Configuration and fails with
        // "Свойство <name> не входит в состав объекта метаданных Configuration".
        // So for every changed BSL we resolve <root>/<MetadataType>/<ObjectName>/
        // and add the XML descriptor plus everything inside the object directory.
        if !is_bsl(&change.path) {
            continue;
        }

        let Some(owner) = locate_metadata_object(&change.path, source_root) else {
            continue;
        };

        push_if_safe_if_exists(&mut paths, &owner.xml, source_root, &root_real)?;

        if owner.dir.is_dir() {
            collect_object_directory(&mut paths, &owner.dir, source_root, &root_real)?;
        }
    }

    paths.sort();
    paths.dedup();
    Some(paths)
}

/// Resolved location of the owning metadata object for a `.bsl` path under the
/// Hierarchical Designer layout.
struct MetadataOwner {
    /// `<root>/<MetadataType>/<ObjectName>.xml` — the XML descriptor Designer
    /// uses to recognise the object's type and name.
    xml: PathBuf,
    /// `<root>/<MetadataType>/<ObjectName>/` — contents recurse here for forms,
    /// templates, manager/object modules, etc.
    dir: PathBuf,
}

/// Walks the relative path components looking for the first known metadata-type
/// directory followed by an object name. Returns the XML descriptor + object
/// directory pair, or `None` if the file does not sit inside a recognised
/// metadata container.
fn locate_metadata_object(bsl: &Path, source_root: &Path) -> Option<MetadataOwner> {
    let relative = bsl.strip_prefix(source_root).ok()?;
    let components: Vec<&str> = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect();

    // Need at least <MetadataType>/<ObjectName>/<something>.bsl to make sense:
    // a `.bsl` directly inside <MetadataType>/ is not a real Hierarchical layout.
    for (idx, component) in components.iter().enumerate() {
        if !is_metadata_type(component) {
            continue;
        }
        if idx + 2 > components.len() {
            // No object name after the metadata-type directory.
            continue;
        }
        let object_name = components[idx + 1];
        // Sanity: the object name must not be a file (e.g. nested BSL right under
        // <MetadataType>/); skip and keep searching.
        if object_name.ends_with(".bsl") || object_name.ends_with(".xml") {
            continue;
        }

        let mut xml = source_root.to_path_buf();
        let mut dir = source_root.to_path_buf();
        for prefix in &components[..=idx] {
            xml.push(prefix);
            dir.push(prefix);
        }
        xml.push(format!("{object_name}.xml"));
        dir.push(object_name);
        return Some(MetadataOwner { xml, dir });
    }

    None
}

fn is_metadata_type(name: &str) -> bool {
    METADATA_TYPES.iter().any(|known| *known == name)
}

/// Recursively adds every regular file under `dir` (relative to `source_root`)
/// into `paths`. Unreadable entries are silently skipped — the goal is best
/// effort enumeration of the owning object's tree, not a strict invariant.
fn collect_object_directory(
    paths: &mut Vec<PathBuf>,
    dir: &Path,
    source_root: &Path,
    root_real: &Path,
) -> Option<()> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Some(());
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_object_directory(paths, &path, source_root, root_real)?;
        } else if path.is_file() {
            push_if_safe(paths, &path, source_root, root_real)?;
        }
    }
    Some(())
}

fn push_if_safe(
    paths: &mut Vec<PathBuf>,
    candidate: &Path,
    source_root: &Path,
    root_real: &Path,
) -> Option<()> {
    let relative = safe_relative_path(candidate, source_root, root_real)?;
    paths.push(source_root.join(relative));
    Some(())
}

fn push_if_safe_if_exists(
    paths: &mut Vec<PathBuf>,
    candidate: &Path,
    source_root: &Path,
    root_real: &Path,
) -> Option<()> {
    if !candidate.exists() {
        return Some(());
    }

    push_if_safe(paths, candidate, source_root, root_real)
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tempfile::tempdir;

    use super::{
        decide, locate_metadata_object, relative_paths, write_list_file, LoadDecision,
        DEFAULT_PARTIAL_LOAD_THRESHOLD,
    };
    use crate::change_detection::analyzer::{ChangeKind, FileChange};

    /// Helper: ensure that the file is created on disk so that `canonicalize` in the
    /// production code succeeds. Returns the absolute path.
    fn touch(root: &Path, relative: &str) -> PathBuf {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, b"").expect("write");
        path
    }

    #[test]
    fn locate_metadata_object_for_common_module_designer_layout() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let bsl = root.join("CommonModules/MyModule/Ext/Module.bsl");

        let owner = locate_metadata_object(&bsl, root).expect("owner");

        assert_eq!(owner.xml, root.join("CommonModules/MyModule.xml"));
        assert_eq!(owner.dir, root.join("CommonModules/MyModule"));
    }

    #[test]
    fn locate_metadata_object_for_document_object_module() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let bsl = root.join("Documents/Order/Ext/ObjectModule.bsl");

        let owner = locate_metadata_object(&bsl, root).expect("owner");

        assert_eq!(owner.xml, root.join("Documents/Order.xml"));
        assert_eq!(owner.dir, root.join("Documents/Order"));
    }

    #[test]
    fn locate_metadata_object_for_form_module_walks_up_to_document() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let bsl = root.join("Documents/Order/Forms/MainForm/Ext/Form/Module.bsl");

        let owner = locate_metadata_object(&bsl, root).expect("owner");

        // Form's owning object is the document itself — Designer needs the document
        // XML, not the form XML, to recognise the target during partial load.
        assert_eq!(owner.xml, root.join("Documents/Order.xml"));
        assert_eq!(owner.dir, root.join("Documents/Order"));
    }

    #[test]
    fn locate_metadata_object_returns_none_when_no_metadata_type_in_path() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let bsl = root.join("Misc/something/Module.bsl");

        assert!(locate_metadata_object(&bsl, root).is_none());
    }

    #[test]
    fn decide_partial_load_for_common_module_adds_xml_and_object_dir_recursively() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();

        let xml = touch(root, "CommonModules/MyModule.xml");
        let module = touch(root, "CommonModules/MyModule/Ext/Module.bsl");

        let decision = decide(
            &[FileChange {
                path: module.clone(),
                kind: ChangeKind::Modified,
            }],
            root,
            DEFAULT_PARTIAL_LOAD_THRESHOLD,
        );

        // Sorted, deduped: the BSL itself (picked up both as the change and via the
        // recursive walk of the object directory) + the XML descriptor. PathBuf
        // ordering is component-wise, so "MyModule/Ext/Module.bsl" sorts before
        // "MyModule.xml" — the shorter `MyModule` component beats `MyModule.xml`.
        assert_eq!(decision, LoadDecision::Partial(vec![module, xml]));
    }

    #[test]
    fn decide_partial_load_for_document_object_module_includes_all_object_files() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();

        let doc_xml = touch(root, "Documents/Order.xml");
        let object_module = touch(root, "Documents/Order/Ext/ObjectModule.bsl");
        let manager_module = touch(root, "Documents/Order/Ext/ManagerModule.bsl");
        let form_xml = touch(root, "Documents/Order/Forms/MainForm.xml");
        let form_descriptor = touch(root, "Documents/Order/Forms/MainForm/Ext/Form.xml");
        let form_module = touch(root, "Documents/Order/Forms/MainForm/Ext/Form/Module.bsl");

        let decision = decide(
            &[FileChange {
                path: object_module.clone(),
                kind: ChangeKind::Modified,
            }],
            root,
            DEFAULT_PARTIAL_LOAD_THRESHOLD,
        );

        let LoadDecision::Partial(mut paths) = decision else {
            panic!("expected Partial decision, got {decision:?}");
        };
        paths.sort();

        let mut expected = vec![
            doc_xml,
            object_module,
            manager_module,
            form_xml,
            form_descriptor,
            form_module,
        ];
        expected.sort();

        assert_eq!(paths, expected);
    }

    #[test]
    fn decide_partial_load_for_form_module_pulls_in_owning_document() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();

        let doc_xml = touch(root, "Documents/Order.xml");
        let object_module = touch(root, "Documents/Order/Ext/ObjectModule.bsl");
        let form_xml = touch(root, "Documents/Order/Forms/MainForm.xml");
        let form_module = touch(root, "Documents/Order/Forms/MainForm/Ext/Form/Module.bsl");

        let decision = decide(
            &[FileChange {
                path: form_module.clone(),
                kind: ChangeKind::Modified,
            }],
            root,
            DEFAULT_PARTIAL_LOAD_THRESHOLD,
        );

        let LoadDecision::Partial(mut paths) = decision else {
            panic!("expected Partial decision, got {decision:?}");
        };
        paths.sort();

        // Editing one form module must still bring the document XML and sibling
        // files (other modules, other forms) along so Designer can load the
        // whole object coherently.
        let mut expected = vec![doc_xml, object_module, form_xml, form_module];
        expected.sort();

        assert_eq!(paths, expected);
    }

    #[test]
    fn decide_bsl_outside_metadata_whitelist_adds_only_the_file() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let module = touch(root, "Misc/SomeFolder/Module.bsl");

        let decision = decide(
            &[FileChange {
                path: module.clone(),
                kind: ChangeKind::Modified,
            }],
            root,
            DEFAULT_PARTIAL_LOAD_THRESHOLD,
        );

        assert_eq!(decision, LoadDecision::Partial(vec![module]));
    }

    #[test]
    fn write_list_file_skips_empty_relative_paths() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let list_file = root.join("partial.lst");

        write_list_file(&[root.to_path_buf()], root, &list_file).expect("write list");

        assert_eq!(std::fs::read_to_string(list_file).expect("read list"), "");
    }

    #[test]
    fn relative_paths_returns_relative_entries_for_safe_paths() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let module = touch(root, "CommonModules/MyModule/Ext/Module.bsl");

        let rels = relative_paths(&[module.clone()], root).expect("relative paths");

        assert_eq!(
            rels,
            vec![PathBuf::from("CommonModules/MyModule/Ext/Module.bsl")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn relative_paths_rejects_path_outside_root() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("src");
        let outside = temp.path().join("outside");
        let link = root.join("CommonModules");
        let escaped = outside.join("Module.bsl");

        std::fs::create_dir_all(&root).expect("root");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::write(&escaped, "module").expect("escaped");
        symlink(&outside, &link).expect("link");

        let err = relative_paths(&[link.join("Module.bsl")], &root).expect_err("expected error");

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn write_list_file_fails_for_paths_outside_root() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("src");
        let outside = temp.path().join("outside");
        let link = root.join("CommonModules");
        let escaped = outside.join("Module.bsl");
        let list_file = temp.path().join("partial.lst");

        std::fs::create_dir_all(&root).expect("root");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::write(&escaped, "module").expect("escaped");
        symlink(&outside, &link).expect("link");

        let err = write_list_file(&[link.join("Module.bsl")], &root, &list_file)
            .expect_err("expected invalid path");

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn decide_forces_full_when_configuration_xml_changed() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let config_xml = touch(root, "Configuration.xml");

        let decision = decide(
            &[FileChange {
                path: config_xml,
                kind: ChangeKind::Modified,
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
        // Even though the path itself does not need to exist for a deletion event,
        // the Hierarchical layout is preserved for readability.
        let removed = root.join("CommonModules/MyModule/Ext/Module.bsl");

        let decision = decide(
            &[FileChange {
                path: removed,
                kind: ChangeKind::Deleted,
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

        // Each module sits in its own object directory but without an XML descriptor
        // on disk — that keeps the per-change file count to exactly one and lets us
        // count past the threshold predictably.
        for index in 0..=DEFAULT_PARTIAL_LOAD_THRESHOLD {
            let path = touch(
                root,
                &format!("CommonModules/Module{index}/Ext/Module.bsl"),
            );
            changes.push(FileChange {
                path,
                kind: ChangeKind::Modified,
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
        let link_dir = root.join("CommonModules");
        let escaped = outside_dir.join("Module.bsl");

        std::fs::create_dir_all(&outside_dir).expect("outside");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(&escaped, "module").expect("write escaped");
        symlink(&outside_dir, &link_dir).expect("create symlink");

        let decision = decide(
            &[FileChange {
                path: link_dir.join("Module.bsl"),
                kind: ChangeKind::Modified,
            }],
            &root,
            DEFAULT_PARTIAL_LOAD_THRESHOLD,
        );

        assert_eq!(decision, LoadDecision::Full);
    }
}
