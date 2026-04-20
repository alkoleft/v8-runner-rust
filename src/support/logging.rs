use std::fs::{File, OpenOptions};
use std::io::{self, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use thiserror::Error;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::registry::LookupSpan;
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
    color_enabled: bool,
    work_path: &Path,
) -> Result<Option<PathBuf>, LoggingInitError> {
    let writer = ActionLogMakeWriter {
        stdout_enabled: output_format == "text",
        file: open_log_file(resolve_action_log_path(output_format, work_path).as_deref())?,
    };
    let log_path = resolve_action_log_path(output_format, work_path);
    let ansi_enabled = output_format == "text" && color_enabled;
    let env_filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));

    if output_format == "text" {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(writer)
            .with_timer(UtcTimer)
            .with_ansi(ansi_enabled)
            .with_target(false)
            .event_format(CliEventFormatter)
            .try_init()
            .map_err(|error| LoggingInitError::Install(error.to_string()))?;
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(writer)
            .with_timer(UtcTimer)
            .with_ansi(false)
            .with_target(false)
            .try_init()
            .map_err(|error| LoggingInitError::Install(error.to_string()))?;
    }

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
struct CliEventFormatter;
#[derive(Default)]
struct EventFieldVisitor {
    message: Option<String>,
    fields: Vec<String>,
    timeline_label: Option<String>,
    timeline_status: Option<String>,
    timeline_detail: Option<String>,
}

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

impl<S, N> FormatEvent<S, N> for CliEventFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        let meta = event.metadata();
        let mut visitor = EventFieldVisitor::default();
        event.record(&mut visitor);

        if let Some(label) = visitor.timeline_label.as_deref() {
            write_timeline_node(
                &mut writer,
                visitor.timeline_status.as_deref().unwrap_or("succeeded"),
            )?;
            write!(writer, " {label}")?;

            if let Some(detail) = visitor
                .timeline_detail
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                writeln!(writer)?;
                for line in detail.lines() {
                    write_timeline_pipe(&mut writer)?;
                    writeln!(writer, "   {line}")?;
                }
                return Ok(());
            }

            return writeln!(writer);
        }

        write_timeline_pipe(&mut writer)?;
        write!(writer, " ")?;
        write!(writer, "{}  ", Utc::now().format("%H:%M:%S%.3f"))?;
        write_level(&mut writer, meta.level())?;

        if let Some(message) = visitor.message.as_deref() {
            write!(writer, " ")?;
            write_message(&mut writer, message)?;
        }

        for field in &visitor.fields {
            write!(writer, " {field}")?;
        }

        writeln!(writer)
    }
}

fn write_timeline_pipe(writer: &mut Writer<'_>) -> std::fmt::Result {
    if writer.has_ansi_escapes() {
        write!(writer, "\x1b[34m│\x1b[0m")
    } else {
        write!(writer, "│")
    }
}

fn write_timeline_node(writer: &mut Writer<'_>, status: &str) -> std::fmt::Result {
    if writer.has_ansi_escapes() {
        let color = match status {
            "failed" => "31",
            "skipped" => "34",
            _ => "32",
        };
        write!(writer, "\x1b[{color}m●\x1b[0m")
    } else {
        write!(writer, "●")
    }
}

impl tracing::field::Visit for EventFieldVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_owned());
        } else if !self.record_timeline_field(field.name(), value.to_owned()) {
            self.fields.push(format!(r#"{}="{}""#, field.name(), value));
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("{value:?}"));
        } else if !self.record_timeline_field(field.name(), normalize_debug_value(value)) {
            self.fields.push(format!("{}={value:?}", field.name()));
        }
    }
}

impl EventFieldVisitor {
    fn record_timeline_field(&mut self, name: &str, value: String) -> bool {
        match name {
            "timeline_label" => self.timeline_label = Some(value),
            "timeline_status" => self.timeline_status = Some(value),
            "timeline_detail" => self.timeline_detail = Some(value),
            _ => return false,
        }
        true
    }
}

fn normalize_debug_value(value: &dyn std::fmt::Debug) -> String {
    let rendered = format!("{value:?}");
    rendered
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(&rendered)
        .to_owned()
}

fn write_level(writer: &mut Writer<'_>, level: &Level) -> std::fmt::Result {
    let label = format!("{level:>5}");
    if writer.has_ansi_escapes() {
        let code = match *level {
            Level::ERROR => "1;31",
            Level::WARN => "1;33",
            Level::INFO => "1;32",
            Level::DEBUG => "1;36",
            Level::TRACE => "1;35",
        };
        write!(writer, "\x1b[{code}m{label}\x1b[0m")
    } else {
        write!(writer, "{label}")
    }
}

fn write_message(writer: &mut Writer<'_>, message: &str) -> std::fmt::Result {
    if message == "running interactive edt command" {
        if writer.has_ansi_escapes() {
            write!(writer, "running interactive edt ")?;
            return write_highlighted_label(writer, "command", "1;96");
        }
        return write!(writer, "{message}");
    }

    if message == "interactive edt command finished" {
        if writer.has_ansi_escapes() {
            write!(writer, "interactive edt ")?;
            return write_highlighted_label(writer, "command finished", "1;96");
        }
        return write!(writer, "{message}");
    }

    let Some(prefix_end) = message.find(']') else {
        return write!(writer, "{message}");
    };

    if !message.starts_with('[') {
        return write!(writer, "{message}");
    }

    let (prefix, rest) = message.split_at(prefix_end + 1);
    if writer.has_ansi_escapes() {
        write!(writer, "\x1b[1;34m{prefix}\x1b[0m")?;
        write_stage_tail(writer, rest)
    } else {
        write!(writer, "{message}")
    }
}

fn write_stage_tail(writer: &mut Writer<'_>, rest: &str) -> std::fmt::Result {
    if let Some((before, project_name)) = split_project_name_suffix(rest) {
        write!(writer, "{before}")?;
        write_highlighted_label(writer, project_name, "1;37")
    } else {
        write!(writer, "{rest}")
    }
}

fn split_project_name_suffix(rest: &str) -> Option<(&str, &str)> {
    let (before, tail) = rest.rsplit_once(": ")?;
    let project_name = tail.trim();
    if project_name.is_empty() || project_name.contains(' ') {
        return None;
    }

    Some((&rest[..before.len() + 2], project_name))
}

fn write_highlighted_label(
    writer: &mut Writer<'_>,
    text: &str,
    ansi_code: &str,
) -> std::fmt::Result {
    if writer.has_ansi_escapes() {
        write!(writer, "\x1b[{ansi_code}m{text}\x1b[0m")
    } else {
        write!(writer, "{text}")
    }
}

impl IoWrite for ActionLogWriter {
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
