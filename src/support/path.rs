use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

/// Returns true when `value` can be safely used as a single file/path segment.
pub fn is_safe_path_segment(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }

    let mut components = Path::new(value).components();
    let is_single_normal_component =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    if !is_single_normal_component {
        return false;
    }

    !value.chars().any(|ch| {
        ch.is_control()
            || matches!(
                ch,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '\0'
            )
    })
}

pub fn nearest_existing_canonical_path(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => resolved.push(prefix.as_os_str()),
            Component::RootDir => resolved.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !resolved.pop() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("path escapes filesystem root: '{}'", path.display()),
                    ));
                }
                if resolved.try_exists()? {
                    resolved = std::fs::canonicalize(&resolved)?;
                }
            }
            Component::Normal(part) => {
                let candidate = resolved.join(part);
                resolved = if candidate.try_exists()? {
                    std::fs::canonicalize(candidate)?
                } else {
                    candidate
                };
            }
        }
    }
    Ok(resolved)
}

pub fn stable_path_identity(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn hashed_lock_path(path: &Path, prefix: &str) -> std::io::Result<PathBuf> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("path has no parent: {}", path.display()),
        )
    })?;
    Ok(parent.join(format!(".{prefix}-{}.lock", stable_path_identity(path))))
}

pub fn is_filesystem_root(path: &Path) -> bool {
    path.parent().is_none()
}

pub fn strip_windows_verbatim_prefix(value: &str) -> String {
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }

    if let Some(rest) = value.strip_prefix(r"\\?\") {
        return rest.to_owned();
    }

    value.to_owned()
}

pub fn normalize_windows_verbatim_path(path: &Path) -> PathBuf {
    PathBuf::from(strip_windows_verbatim_prefix(&path.display().to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        hashed_lock_path, is_filesystem_root, is_safe_path_segment,
        nearest_existing_canonical_path, normalize_windows_verbatim_path, stable_path_identity,
        strip_windows_verbatim_prefix,
    };
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn safe_path_segment_rejects_control_characters() {
        assert!(!is_safe_path_segment("Sales\nAddon"));
        assert!(!is_safe_path_segment("Sales\tAddon"));
    }

    #[test]
    fn nearest_existing_canonical_path_uses_existing_ancestor() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("root");
        fs::create_dir_all(&root).expect("root");

        let resolved =
            nearest_existing_canonical_path(&root.join("nested").join("target")).expect("resolved");

        let canonical_root = fs::canonicalize(&root).expect("canonical root");
        assert_eq!(resolved, canonical_root.join("nested").join("target"));
    }

    #[cfg(unix)]
    #[test]
    fn nearest_existing_canonical_path_propagates_lookup_errors() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("loop");
        std::os::unix::fs::symlink(&path, &path).expect("self symlink");

        assert!(nearest_existing_canonical_path(&path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn stable_path_identity_is_canonical_for_symlinked_paths() {
        let dir = tempdir().expect("tempdir");
        let real = dir.path().join("real");
        let link = dir.path().join("link");
        fs::create_dir_all(&real).expect("real");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        let real = std::fs::canonicalize(&real).expect("canonical real");
        let link = std::fs::canonicalize(&link).expect("canonical link");

        assert_eq!(stable_path_identity(&real), stable_path_identity(&link));
    }

    #[test]
    fn hashed_lock_path_uses_parent_directory() {
        let dir = tempdir().expect("tempdir");
        let target = dir.path().join("main");
        let lock_path = hashed_lock_path(&target, "dump").expect("lock path");

        assert_eq!(lock_path.parent(), Some(dir.path()));
        assert!(lock_path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.starts_with(".dump-") && name.ends_with(".lock")));
    }

    #[test]
    fn filesystem_root_detection_matches_non_root_paths() {
        let dir = tempdir().expect("tempdir");
        assert!(!is_filesystem_root(dir.path()));
    }

    #[test]
    fn strips_windows_verbatim_drive_prefix() {
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\E:\Git_reps\MDM\src\cf"),
            r"E:\Git_reps\MDM\src\cf"
        );
    }

    #[test]
    fn strips_windows_verbatim_unc_prefix() {
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\UNC\server\share\ib"),
            r"\\server\share\ib"
        );
    }

    #[test]
    fn normalize_windows_verbatim_path_leaves_regular_paths_unchanged() {
        let path = PathBuf::from("/tmp/project");

        assert_eq!(normalize_windows_verbatim_path(&path), path);
    }
}
