//! Error type for the Medulla client.

use serde_json::Value;

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

    /// Whether this error should route a front end to the login screen rather
    /// than falling back silently.
    ///
    /// True for an expired token or an HTTP 401/403 (rejected credentials). Every
    /// front end should branch on this so authentication failures are handled
    /// identically.
    pub fn is_auth_error(&self) -> bool {
        self.is_token_expired() || matches!(self.status(), Some(401) | Some(403))
    }
}

/// Translate a [`tinyhumans_sdk::Error`] into this crate's taxonomy.
///
/// The SDK splits "the transport succeeded but the operation failed" across two
/// variants — [`tinyhumans_sdk::Error::Status`] for a non-2xx response and
/// [`tinyhumans_sdk::Error::Envelope`] for a `{success:false}` body returned
/// with a 2xx — because the backend does not reliably pair the two. Front ends
/// here branch on one thing (`is_auth_error`), so both collapse into
/// [`ClientError::Api`] and the `errorCode` is recovered from a non-2xx body
/// as well. Without that recovery a 401 carrying `TOKEN_EXPIRED` — the common
/// case — would arrive with no error code and never reach the login screen.
impl From<tinyhumans_sdk::Error> for ClientError {
    fn from(err: tinyhumans_sdk::Error) -> Self {
        use tinyhumans_sdk::Error as Sdk;
        // Rendered before the match so the socket arms can report it without
        // moving `err` out of a pattern that has already bound its fields.
        let err_message = err.to_string();
        match err {
            Sdk::Http(e) => ClientError::Transport(e),
            Sdk::Decode(e) => ClientError::Decode(e.to_string()),
            Sdk::Status { status, body } => {
                let (message, error_code, details) = describe_failure(&body, status);
                ClientError::Api {
                    status: Some(status),
                    message,
                    error_code,
                    details,
                }
            }
            Sdk::Envelope {
                error,
                error_code,
                details,
            } => ClientError::Api {
                // A `{success:false}` body that arrived with a 2xx status: the
                // operation failed without an HTTP status that says so.
                status: None,
                message: error,
                error_code,
                details: (!details.is_null()).then_some(details),
            },
            // Socket.IO transport failures. Reported as transport rather than
            // as an API failure: nothing at the other end refused the
            // operation, the connection did not carry it. They are listed by
            // name rather than swept into the arm below so that a future
            // variant is a compile error here and gets classified deliberately.
            Sdk::Socket(_)
            | Sdk::MissingSocketToken
            | Sdk::UnexpectedSocketPayload(_)
            | Sdk::SocketAckTimeout
            | Sdk::SocketAckClosed => ClientError::Api {
                status: None,
                message: err_message,
                error_code: None,
                details: None,
            },
            // Request-construction and route-gate failures never reach the
            // network, so there is no status or error code to report.
            other @ (Sdk::Url(_) | Sdk::Header(_) | Sdk::RouteNotExposed(_, _)) => {
                ClientError::Api {
                    status: None,
                    message: other.to_string(),
                    error_code: None,
                    details: None,
                }
            }
        }
    }
}

/// Pull a message, `errorCode`, and `details` out of a failed response body.
///
/// The body is whatever the backend sent: an error envelope, some other JSON
/// object, or — when the response was not JSON at all — the raw text the SDK
/// preserved as a [`Value::String`].
fn describe_failure(body: &Value, status: u16) -> (String, Option<String>, Option<Value>) {
    let fallback = || format!("request failed with status {status}");
    match body {
        Value::Object(map) => (
            map.get("error")
                .or_else(|| map.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(fallback),
            map.get("errorCode")
                .and_then(Value::as_str)
                .map(str::to_owned),
            map.get("details").cloned(),
        ),
        Value::String(text) => (text.trim().to_owned(), None, None),
        Value::Null => (fallback(), None, None),
        other => (other.to_string(), None, None),
    }
}

mod types;
pub use types::ClientError;
pub use types::Result;
