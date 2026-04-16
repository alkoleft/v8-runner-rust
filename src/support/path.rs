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
        matches!(
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

    let mut existing = absolute.as_path();
    while !existing.exists() {
        existing = existing.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no existing ancestor for path '{}'", path.display()),
            )
        })?;
    }

    let existing_canonical = std::fs::canonicalize(existing)?;
    if existing == absolute {
        return Ok(existing_canonical);
    }

    let suffix = absolute
        .strip_prefix(existing)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let suffix =
        suffix
            .components()
            .try_fold(PathBuf::new(), |mut acc, component| match component {
                Component::Normal(part) => {
                    acc.push(part);
                    Ok(acc)
                }
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "path '{}' contains unsupported component '{}'",
                        path.display(),
                        component.as_os_str().to_string_lossy()
                    ),
                )),
            })?;

    Ok(existing_canonical.join(suffix))
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

#[cfg(test)]
mod tests {
    use super::{
        hashed_lock_path, is_filesystem_root, nearest_existing_canonical_path, stable_path_identity,
    };
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn nearest_existing_canonical_path_uses_existing_ancestor() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("root");
        fs::create_dir_all(&root).expect("root");

        let resolved =
            nearest_existing_canonical_path(&root.join("nested").join("target")).expect("resolved");

        assert_eq!(resolved, root.join("nested").join("target"));
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
}
