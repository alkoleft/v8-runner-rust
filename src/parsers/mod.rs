use crate::domain::artifact::ArtifactSet;
use crate::domain::execution::{ExecutionError, ExecutionMetrics};

pub mod designer_validation;
pub mod edt_validation;
pub mod junit;
pub mod yaxunit_log;

/// Normalized parser output shared by runner/package parsers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedParse<T> {
    pub payload: Option<T>,
    pub metrics: Option<ExecutionMetrics>,
    pub diagnostics: Vec<String>,
    pub errors: Vec<ExecutionError>,
    pub warnings: Vec<String>,
    pub artifacts: Option<ArtifactSet>,
}

impl<T> Default for NormalizedParse<T> {
    fn default() -> Self {
        Self {
            payload: None,
            metrics: None,
            diagnostics: Vec::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
            artifacts: None,
        }
    }
}

impl<T> NormalizedParse<T> {
    pub fn with_payload(mut self, payload: T) -> Self {
        self.payload = Some(payload);
        self
    }

    pub fn with_metrics(mut self, metrics: ExecutionMetrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    pub fn with_errors(mut self, errors: Vec<ExecutionError>) -> Self {
        self.errors = errors;
        self
    }
}
