use std::path::{Path, PathBuf};

use tempfile::NamedTempFile;

/// Create a named temporary file with given prefix and suffix.
pub fn create_temp_file(prefix: &str, suffix: &str) -> std::io::Result<NamedTempFile> {
    tempfile::Builder::new()
        .prefix(prefix)
        .suffix(suffix)
        .tempfile()
}

/// Return the root temp directory inside `work_path`.
pub fn temp_root(work_path: &Path) -> std::io::Result<PathBuf> {
    let dir = work_path.join("temp");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Return the platform log directory inside `work_path`.
pub fn platform_logs_dir(work_path: &Path) -> std::io::Result<PathBuf> {
    let dir = work_path.join("logs").join("platform");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Return the temp directory for partial load/dump lists inside `work_path`.
pub fn partial_lists_dir(work_path: &Path) -> std::io::Result<PathBuf> {
    let dir = temp_root(work_path)?.join("partial-lists");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Return the temp directory for YaXUnit config files inside `work_path`.
pub fn yaxunit_dir(work_path: &Path) -> std::io::Result<PathBuf> {
    let dir = temp_root(work_path)?.join("yaxunit");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Return the reserved future EDT work directory for a source set.
pub fn reserved_source_set_dir(work_path: &Path, source_set_name: &str) -> PathBuf {
    work_path.join("designer").join(source_set_name)
}

/// Create a temporary JSON file for a YaXUnit run config inside `work_path/temp/yaxunit`.
pub fn yaxunit_config_file(work_path: &Path) -> std::io::Result<NamedTempFile> {
    tempfile::Builder::new()
        .prefix("yaxunit-config-")
        .suffix(".json")
        .tempfile_in(yaxunit_dir(work_path)?)
}

/// Create a temporary text file for a partial load list inside `work_path/temp/partial-lists`.
pub fn partial_list_file(work_path: &Path) -> std::io::Result<NamedTempFile> {
    tempfile::Builder::new()
        .prefix("partial-list-")
        .suffix(".txt")
        .tempfile_in(partial_lists_dir(work_path)?)
}

/// Create a temporary text file for a partial dump object list inside
/// `work_path/temp/partial-lists`.
pub fn dump_object_list_file(work_path: &Path) -> std::io::Result<NamedTempFile> {
    tempfile::Builder::new()
        .prefix("dump-object-list-")
        .suffix(".txt")
        .tempfile_in(partial_lists_dir(work_path)?)
}

#[cfg(test)]
mod tests {
    use super::{
        dump_object_list_file, partial_list_file, partial_lists_dir, platform_logs_dir,
        reserved_source_set_dir, yaxunit_config_file, yaxunit_dir,
    };
    use tempfile::tempdir;

    #[test]
    fn creates_new_temp_layout_under_work_path() {
        let dir = tempdir().expect("tempdir");

        let partial_dir = partial_lists_dir(dir.path()).expect("partial dir");
        let yaxunit_dir = yaxunit_dir(dir.path()).expect("yaxunit dir");
        let logs_dir = platform_logs_dir(dir.path()).expect("logs dir");

        assert!(partial_dir.ends_with("temp/partial-lists"));
        assert!(yaxunit_dir.ends_with("temp/yaxunit"));
        assert!(logs_dir.ends_with("logs/platform"));
    }

    #[test]
    fn creates_temp_files_in_new_locations() {
        let dir = tempdir().expect("tempdir");

        let partial = partial_list_file(dir.path()).expect("partial file");
        let dump_partial = dump_object_list_file(dir.path()).expect("dump partial file");
        let yaxunit = yaxunit_config_file(dir.path()).expect("yaxunit file");

        assert!(partial
            .path()
            .to_string_lossy()
            .contains("temp/partial-lists"));
        assert!(dump_partial
            .path()
            .to_string_lossy()
            .contains("temp/partial-lists"));
        assert!(yaxunit.path().to_string_lossy().contains("temp/yaxunit"));
    }

    #[test]
    fn reserved_source_set_path_is_not_created() {
        let dir = tempdir().expect("tempdir");
        let reserved = reserved_source_set_dir(dir.path(), "main");

        assert!(!reserved.exists());
        assert!(reserved.ends_with("designer/main"));
    }
}
