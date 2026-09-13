use std::borrow::Cow;

use rmcp::ErrorData;
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("path is outside the repository root")]
    PathOutsideRoot,
    #[error("path was not found: {0}")]
    NotFound(String),
    #[error("I/O operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("input exceeds the configured limit: {0}")]
    LimitExceeded(String),
}

impl ToolError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "invalid_input",
            Self::PathOutsideRoot => "path_outside_root",
            Self::NotFound(_) => "not_found",
            Self::Io(_) => "io_error",
            Self::LimitExceeded(_) => "limit_exceeded",
        }
    }
}

impl From<ToolError> for ErrorData {
    fn from(error: ToolError) -> Self {
        let code = error.code();
        let message = error.to_string();
        Self::invalid_params(
            Cow::Owned(message.clone()),
            Some(json!({ "toolz_code": code, "message": message })),
        )
    }
}
