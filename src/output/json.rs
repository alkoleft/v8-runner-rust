use chrono::Utc;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Envelope<T: Serialize> {
    pub ok: bool,
    pub command: String,
    pub duration_ms: u64,
    pub data: T,
    pub warnings: Vec<String>,
    pub steps: Vec<StepResult>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepResult {
    pub name: String,
    pub ok: bool,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl<T: Serialize> Envelope<T> {
    pub fn ok(command: impl Into<String>, duration_ms: u64, data: T) -> Self {
        Self {
            ok: true,
            command: command.into(),
            duration_ms,
            data,
            warnings: vec![],
            steps: vec![],
        }
    }

    pub fn err(command: impl Into<String>, duration_ms: u64, data: T) -> Self {
        Self {
            ok: false,
            command: command.into(),
            duration_ms,
            data,
            warnings: vec![],
            steps: vec![],
        }
    }
}

pub fn now_ms() -> u64 {
    Utc::now().timestamp_millis() as u64
}
