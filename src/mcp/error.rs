use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::use_cases::result::{UseCaseError, UseCaseErrorKind};

/// Stable machine-readable code for MCP-facing service failures.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpErrorCode {
    InvalidArgument,
    UnsupportedValue,
    RuntimeFailure,
    PlatformFailure,
    Internal,
}

/// High-level business error class surfaced by the MCP service layer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpBusinessErrorKind {
    Validation,
    Runtime,
    Platform,
}

impl From<UseCaseErrorKind> for McpBusinessErrorKind {
    fn from(value: UseCaseErrorKind) -> Self {
        match value {
            UseCaseErrorKind::Validation => Self::Validation,
            UseCaseErrorKind::Runtime => Self::Runtime,
            UseCaseErrorKind::Platform => Self::Platform,
        }
    }
}

/// Structured business error metadata returned by the MCP service layer.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpBusinessError {
    pub code: McpErrorCode,
    pub kind: McpBusinessErrorKind,
    pub message: String,
}

impl McpBusinessError {
    /// Maps a use-case error into a stable MCP-facing business error.
    pub fn from_use_case(error: &UseCaseError) -> Self {
        let kind = error.kind();
        let code = match kind {
            UseCaseErrorKind::Validation => McpErrorCode::InvalidArgument,
            UseCaseErrorKind::Runtime => McpErrorCode::RuntimeFailure,
            UseCaseErrorKind::Platform => McpErrorCode::PlatformFailure,
        };
        Self {
            code,
            kind: kind.into(),
            message: error.message().to_owned(),
        }
    }
}

/// Structured business failure with a failure-shaped MCP response payload.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpBusinessFailure<T> {
    pub error: McpBusinessError,
    pub response: T,
}

impl<T> McpBusinessFailure<T> {
    /// Creates a new business failure for the specified response payload.
    pub fn new(error: McpBusinessError, response: T) -> Self {
        Self { error, response }
    }
}

/// Non-business service error that must not be surfaced as a business response.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpInternalError {
    pub code: McpErrorCode,
    pub message: String,
}

impl McpInternalError {
    /// Creates a new internal MCP service error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            code: McpErrorCode::Internal,
            message: message.into(),
        }
    }
}

/// Top-level MCP service failure contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServiceError<T> {
    Business(McpBusinessFailure<T>),
    Internal(McpInternalError),
}

/// Top-level MCP service result contract.
pub type McpServiceResult<T> = Result<T, McpServiceError<T>>;
