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
                let prefix = if last {
                    self.timeline_tail()
                } else {
                    self.timeline_pipe()
                };
                for line in detail.lines() {
                    println!("{prefix}   {line}");
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
                TimelineStatus::Skipped => "34",
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

    fn timeline_tail(&self) -> String {
        " ".to_owned()
    }
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
