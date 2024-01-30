use std::fmt::Display;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error, Serialize, Deserialize)]
pub enum DeploykitError {
    #[error("Failed to get field {field}: {error}")]
    GetField {
        field: String,
        error: GetFieldErrKind,
    },
    #[error("Failed to set field: {0}, value: {1} is illegal")]
    SetValue(String, String),
    #[error("Failed to auto create partitions: {0}")]
    AutoPartition(String),
    #[error("Failed to install system: {0}")]
    Install(String),
    #[error("Failed to find esp partition: {0}")]
    FindEspPartition(String),
}

#[derive(Debug, Serialize, Deserialize)]
pub enum GetFieldErrKind {
    UnknownField,
}

impl Display for GetFieldErrKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GetFieldErrKind::UnknownField => write!(f, "Unknown field"),
        }
    }
}

impl DeploykitError {
    pub fn unknown_field(field: &str) -> Self {
        Self::GetField {
            field: field.to_string(),
            error: GetFieldErrKind::UnknownField,
        }
    }
}
