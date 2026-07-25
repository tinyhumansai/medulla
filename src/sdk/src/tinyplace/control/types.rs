//! Data types for the `control` module.
#[allow(unused_imports)]
use super::*;
/// A decoded control frame. `input` types `text` into the addressed session's
/// agent as a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessControlFrame {
    pub control_version: String,
    pub kind: String,
    /// Target session (wrapper or harness session id). Absent targets the primary.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session_id: Option<String>,
    pub text: String,
}
