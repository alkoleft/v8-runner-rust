use std::path::PathBuf;

use crate::platform::process::ProcessResult;

/// DSL-level result for platform commands that may emit a platform-native `/Out` log.
///
/// `stdout_log_path` and `stderr_log_path` in [`ProcessResult`] belong to runner-level stdio
/// mirroring. `platform_log_path` below is different: it points to the file that a 1C utility
/// writes itself when the DSL passes `/Out <path>`. The DSL reads that file after process
/// completion and exposes its content as `platform_log`.
#[derive(Debug, Clone)]
pub struct PlatformCommandResult {
    /// Result returned by the runner after process completion.
    pub process: ProcessResult,
    /// Path passed to the utility via `/Out`, if the command requested one.
    pub platform_log_path: Option<PathBuf>,
    /// Contents of the `/Out` log, if the command requested and successfully read it.
    pub platform_log: Option<String>,
}
