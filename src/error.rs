use std::fmt::Display;

use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error, Serialize)]
pub enum DeploykitError {
    #[error("Failed to get field {field}: {error}")]
    GetField {
        field: String,
        error: GetFieldErrKind,
    },
    #[error("Failed to get config: {0}")]
    GetConfig(String),
}

#[derive(Debug, Serialize)]
pub enum GetFieldErrKind {
    NotSet,
    UnknownField,
}

impl Display for GetFieldErrKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GetFieldErrKind::NotSet => write!(f, "Not set"),
            GetFieldErrKind::UnknownField => write!(f, "Unknown field"),
        }
    }
}

impl DeploykitError {
    pub fn not_set(field: &str) -> Self {
        Self::GetField {
            field: field.to_string(),
            error: GetFieldErrKind::NotSet,
        }
    }

    pub fn unknown_field(field: &str) -> Self {
        Self::GetField {
            field: field.to_string(),
            error: GetFieldErrKind::UnknownField,
        }
    }

    pub fn get_config(err: serde_json::Error) -> Self {
        Self::GetConfig(err.to_string())
    }
}
