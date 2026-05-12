use std::path::Path;
use std::process::{Command, Stdio};

/// Returns Git's effective ignore decision for `path`.
///
/// `None` means Git is unavailable, `path` is outside a worktree, or Git reported
/// an execution error that should fall back to local `.gitignore` editing.
pub fn check_ignored(path: &Path) -> Option<bool> {
    let workdir = path.parent().unwrap_or_else(|| Path::new("."));
    let status = Command::new("git")
        .arg("-C")
        .arg(workdir)
        .args(["check-ignore", "--quiet", "--"])
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;

    match status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}
