//! Shared control-request parameter parsing.

use serde_json::Value;

use super::super::super::types::{ControlFailure, ErrorKind};

/// Read a required non-blank string parameter.
pub(super) fn required_str(params: &Value, key: &str) -> Result<String, ControlFailure> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            ControlFailure::new(
                ErrorKind::BadRequest,
                format!("`{key}` is required and must be a non-empty string"),
            )
        })
}

/// Read an optional string parameter, treating blank as absent.
pub(super) fn optional_str(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
