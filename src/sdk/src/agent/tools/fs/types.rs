//! The data the filesystem tools share: the checkout they are rooted at,
//! and the argument extraction every one of them repeats.

use serde_json::Value;
use tinyagents::error::{Result as TaResult, TinyAgentsError};
use tinyinference::tool::ToolCall;

use std::path::PathBuf;

/// The checkout a turn's tools are rooted at.
///
/// Cloned into each tool rather than read from ambient state: two turns can run
/// concurrently in one process against different checkouts, and a process-global
/// working directory would let one turn's `cd` decide where the other one wrote.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// Absolute path to the turn's checkout.
    pub root: PathBuf,
}

/// Pull a required string argument out of a call.
pub(super) fn arg_str(call: &ToolCall, key: &str) -> TaResult<String> {
    call.arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| TinyAgentsError::Tool(format!("`{key}` is required and must be a string")))
}
