use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::domain::artifact::ArtifactSet;

/// A transport-neutral execution step shared by CLI envelopes and use-case payloads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StepResult {
    pub name: String,
    pub ok: bool,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Shared execution status used by runner and package-like flows.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Succeeded,
    Failed,
    TimedOut,
    InvalidOutput,
}

impl ExecutionStatus {
    pub const fn is_ok(self) -> bool {
        matches!(self, Self::Succeeded)
    }
}

/// Shared counters emitted by parsers and execution adapters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ExecutionMetrics {
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub errors: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, u64>,
}

/// Shared timeout budget for execution scenarios.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ExecutionTimeouts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_ms: Option<u64>,
}

/// Structured execution error that can point to related artifacts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<crate::domain::artifact::ArtifactRef>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub retryable: bool,
}

impl ExecutionError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: Vec::new(),
            artifact: None,
            retryable: false,
        }
    }

    pub fn with_details(mut self, details: Vec<String>) -> Self {
        self.details = details;
        self
    }
}

/// Shared execution envelope for runner-like flows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionOutcome<T> {
    pub status: ExecutionStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ExecutionError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<ExecutionMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<ArtifactSet>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<T>,
}

impl<T> Default for ExecutionOutcome<T> {
    fn default() -> Self {
        Self::new(ExecutionStatus::Succeeded)
    }
}

impl<T> ExecutionOutcome<T> {
    pub fn new(status: ExecutionStatus) -> Self {
        Self {
            status,
            diagnostics: Vec::new(),
            errors: Vec::new(),
            metrics: None,
            artifacts: None,
            payload: None,
        }
    }

    pub const fn is_ok(&self) -> bool {
        self.status.is_ok()
    }

    pub fn with_diagnostics(mut self, diagnostics: Vec<String>) -> Self {
        self.diagnostics = diagnostics;
        self
    }

    pub fn with_errors(mut self, errors: Vec<ExecutionError>) -> Self {
        self.errors = errors;
        self
    }

    pub fn with_metrics(mut self, metrics: ExecutionMetrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    pub fn with_artifacts(mut self, artifacts: ArtifactSet) -> Self {
        self.artifacts = Some(artifacts);
        self
    }

    pub fn with_payload(mut self, payload: T) -> Self {
        self.payload = Some(payload);
        self
    }
}
