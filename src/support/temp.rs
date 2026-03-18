use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

/// Create a named temporary file with given prefix and suffix.
pub fn create_temp_file(prefix: &str, suffix: &str) -> std::io::Result<NamedTempFile> {
    tempfile::Builder::new()
        .prefix(prefix)
        .suffix(suffix)
        .tempfile()
}

/// Return the path for a named subdirectory inside `work_path`.
/// The directory is created if it does not exist.
pub fn temp_dir_for(work_path: &Path, name: &str) -> std::io::Result<PathBuf> {
    let dir = work_path.join(name);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Create a temporary JSON file for a YaXUnit run config inside `work_path`.
pub fn yaxunit_config_file(work_path: &Path) -> std::io::Result<NamedTempFile> {
    let dir = temp_dir_for(work_path, "yaxunit")?;
    tempfile::Builder::new()
        .prefix("yaxunit-config-")
        .suffix(".json")
        .tempfile_in(dir)
}

/// Create a temporary text file for a partial load list inside `work_path`.
pub fn partial_list_file(work_path: &Path) -> std::io::Result<NamedTempFile> {
    let dir = temp_dir_for(work_path, "partial-lists")?;
    tempfile::Builder::new()
        .prefix("partial-list-")
        .suffix(".txt")
        .tempfile_in(dir)
}
