use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use thiserror::Error;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::EnvFilter;

const ACTION_LOG_FILE_ENV: &str = "V8TR_ACTION_LOG_FILE";

#[derive(Debug, Error)]
pub enum LoggingInitError {
    #[error("failed to open action log file '{path}': {source}")]
    OpenFile { path: PathBuf, source: io::Error },

    #[error("failed to initialize action logger: {0}")]
    Install(String),
}

pub fn init_action_logging(
    level: &str,
    output_format: &str,
    work_path: &Path,
) -> Result<Option<PathBuf>, LoggingInitError> {
    let writer = ActionLogMakeWriter {
        stdout_enabled: output_format == "text",
        file: open_log_file(resolve_action_log_path(output_format, work_path).as_deref())?,
    };
    let log_path = resolve_action_log_path(output_format, work_path);

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(writer)
        .with_timer(UtcTimer)
        .with_ansi(false)
        .with_target(false)
        .try_init()
        .map_err(|error| LoggingInitError::Install(error.to_string()))?;

    Ok(log_path)
}

fn resolve_action_log_path(output_format: &str, work_path: &Path) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(ACTION_LOG_FILE_ENV) {
        return Some(PathBuf::from(path));
    }

    if output_format == "json" {
        return Some(work_path.join("logs").join("mcp").join("actions.log"));
    }

    None
}

fn open_log_file(path: Option<&Path>) -> Result<Option<Arc<Mutex<File>>>, LoggingInitError> {
    let Some(path) = path else {
        return Ok(None);
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| LoggingInitError::OpenFile {
            path: path.to_path_buf(),
            source,
        })?;
    }

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| LoggingInitError::OpenFile {
            path: path.to_path_buf(),
            source,
        })?;

    Ok(Some(Arc::new(Mutex::new(file))))
}

#[derive(Clone)]
struct ActionLogMakeWriter {
    stdout_enabled: bool,
    file: Option<Arc<Mutex<File>>>,
}

struct ActionLogWriter {
    stdout_enabled: bool,
    file: Option<Arc<Mutex<File>>>,
}

struct UtcTimer;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for ActionLogMakeWriter {
    type Writer = ActionLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        ActionLogWriter {
            stdout_enabled: self.stdout_enabled,
            file: self.file.clone(),
        }
    }
}

impl FormatTime for UtcTimer {
    fn format_time(
        &self,
        writer: &mut tracing_subscriber::fmt::format::Writer<'_>,
    ) -> std::fmt::Result {
        write!(writer, "{}", Utc::now().format("%H:%M:%S%.3f"))
    }
}

impl Write for ActionLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.stdout_enabled {
            io::stdout().write_all(buf)?;
        }

        if let Some(file) = &self.file {
            let mut file = file
                .lock()
                .map_err(|_| io::Error::other("action log mutex poisoned"))?;
            file.write_all(buf)?;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.stdout_enabled {
            io::stdout().flush()?;
        }

        if let Some(file) = &self.file {
            let mut file = file
                .lock()
                .map_err(|_| io::Error::other("action log mutex poisoned"))?;
            file.flush()?;
        }

        Ok(())
    }
}
