//! Error type for the Medulla client.

use serde_json::Value;

/// Errors returned by [`crate::MedullaClient`].
///
/// API-level failures preserve the backend `errorCode` (for example
/// `TOKEN_EXPIRED`) so callers can react to specific conditions.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The underlying HTTP transport failed (connect, TLS, timeout, ...).
    #[error("http transport error: {0}")]
    Transport(#[from] reqwest::Error),

    /// A response body could not be decoded into the expected shape.
    #[error("failed to decode response: {0}")]
    Decode(String),

    /// The backend returned `{"success": false, ...}` or a non-2xx status.
    #[error("api error{}: {message}", .error_code.as_deref().map(|c| format!(" [{c}]")).unwrap_or_default())]
    Api {
        /// HTTP status code, when the error originated from a transport response.
        status: Option<u16>,
        /// Human-readable error message from the `error` field.
        message: String,
        /// Machine-readable `errorCode`, when present (e.g. `TOKEN_EXPIRED`).
        error_code: Option<String>,
        /// Optional structured `details` payload.
        details: Option<Value>,
    },

    /// A recoverable failure while reading the SSE stream.
    #[error("sse stream error: {0}")]
    Sse(String),
}

impl ClientError {
    /// The backend `errorCode`, when this is an [`ClientError::Api`] error.
    pub fn error_code(&self) -> Option<&str> {
        match self {
            ClientError::Api { error_code, .. } => error_code.as_deref(),
            _ => None,
        }
    }

    /// HTTP status code, when available.
    pub fn status(&self) -> Option<u16> {
        match self {
            ClientError::Api { status, .. } => *status,
            _ => None,
        }
    }

    /// Whether the backend reported an expired token (`TOKEN_EXPIRED`).
    pub fn is_token_expired(&self) -> bool {
        self.error_code() == Some("TOKEN_EXPIRED")
    }
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, ClientError>;
