use crate::output::json::Envelope;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineStatus {
    Succeeded,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineItem {
    pub status: TimelineStatus,
    pub label: String,
    pub detail: Option<String>,
}

impl TimelineItem {
    pub fn new(status: TimelineStatus, label: impl Into<String>) -> Self {
        Self {
            status,
            label: label.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

pub struct TextPresenter {
    pub no_color: bool,
}

impl TextPresenter {
    pub fn print_ok(&self, msg: &str) {
        println!(
            "{} OK: {msg}",
            self.timeline_node(TimelineStatus::Succeeded)
        );
    }

    pub fn print_error(&self, msg: &str) {
        if self.no_color {
            eprintln!("ERROR: {msg}");
        } else {
            eprintln!("\x1b[31mERROR\x1b[0m: {msg}");
        }
    }

    pub fn print_info(&self, msg: &str) {
        for line in msg.lines() {
            println!("{} {line}", self.timeline_pipe());
        }
    }

    pub fn print_success_item(&self, msg: &str) {
        println!("{} {msg}", self.timeline_node(TimelineStatus::Succeeded));
    }

    pub fn print_timeline(&self, items: &[TimelineItem]) {
        for (index, item) in items.iter().enumerate() {
            let last = index + 1 == items.len();
            println!("{} {}", self.timeline_node(item.status), item.label);

            if let Some(detail) = item.detail.as_deref().filter(|value| !value.is_empty()) {
                let prefix = self.timeline_pipe();
                for line in detail.lines() {
                    println!("{prefix}   {}", self.timeline_detail(line));
                }
            }

            if !last {
                println!("{}", self.timeline_pipe());
            }
        }
    }

    fn timeline_node(&self, status: TimelineStatus) -> String {
        let glyph = "●";
        if self.no_color {
            glyph.to_owned()
        } else {
            let color = match status {
                TimelineStatus::Succeeded => "32",
                TimelineStatus::Failed => "31",
                TimelineStatus::Skipped => "90",
            };
            format!("\x1b[{color}m{glyph}\x1b[0m")
        }
    }

    fn timeline_pipe(&self) -> String {
        if self.no_color {
            "│".to_owned()
        } else {
            "\x1b[34m│\x1b[0m".to_owned()
        }
    }

    fn timeline_detail(&self, detail: &str) -> String {
        if self.no_color {
            return detail.to_owned();
        }

        if let Some((prefix, rest)) = bracketed_prefix(detail) {
            return format!("\x1b[1;34m{prefix}\x1b[0m{rest}");
        }

        if let Some(rest) = detail.strip_prefix("Изменения:") {
            return format!("\x1b[1;34mИзменения\x1b[0m:{rest}");
        }

        if let Some(rest) = detail.strip_prefix("✓ ") {
            return format!("\x1b[1;32m✓\x1b[0m {rest}");
        }

        if let Some(rest) = detail.strip_prefix("✗ ") {
            return format!("\x1b[1;31m✗\x1b[0m {rest}");
        }

        if let Some(rest) = detail.strip_prefix("○ ") {
            return format!("\x1b[90m○\x1b[0m {rest}");
        }

        detail.to_owned()
    }
}

fn bracketed_prefix(value: &str) -> Option<(&str, &str)> {
    if !value.starts_with('[') {
        return None;
    }
    let prefix_end = value.find(']')? + 1;
    Some(value.split_at(prefix_end))
}

pub struct JsonPresenter;

impl JsonPresenter {
    pub fn print<T: Serialize>(&self, envelope: &Envelope<T>) {
        match serde_json::to_string_pretty(envelope) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("JSON serialization error: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TextPresenter;

    #[test]
    fn timeline_detail_highlights_status_markers() {
        let presenter = TextPresenter { no_color: false };

        assert_eq!(
            presenter.timeline_detail("✓ completed"),
            "\x1b[1;32m✓\x1b[0m completed"
        );
        assert_eq!(
            presenter.timeline_detail("✗ failed"),
            "\x1b[1;31m✗\x1b[0m failed"
        );
        assert_eq!(
            presenter.timeline_detail("○ skipped"),
            "\x1b[90m○\x1b[0m skipped"
        );
    }

    #[test]
    fn timeline_detail_keeps_no_color_plain() {
        let presenter = TextPresenter { no_color: true };

        assert_eq!(presenter.timeline_detail("✓ completed"), "✓ completed");
    }
}
