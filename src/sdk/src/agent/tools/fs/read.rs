//! Reading a file from the checkout, bounded at the source.

use serde_json::json;
use tinyagents::error::{Result as TaResult, TinyAgentsError};
use tinyagents::tool::{Tool, ToolResult};
use tinyinference::tool::{ToolCall, ToolFormat, ToolSchema};

use super::types::{arg_str, Workspace};
use crate::agent::tools::guard;

use tokio::io::AsyncReadExt;

use super::MAX_READ_BYTES;

/// Read a file from the checkout.
pub struct ReadFileTool {
    workspace: Workspace,
}

impl ReadFileTool {
    /// Root a reader at `workspace`.
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

#[async_trait::async_trait]
impl Tool<()> for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read a UTF-8 text file from the workspace. `path` is relative to the workspace root."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative path to read.",
                    },
                },
                "required": ["path"],
            }),
            format: ToolFormat::default(),
        }
    }

    async fn call(&self, _state: &(), call: ToolCall) -> TaResult<ToolResult> {
        let path = arg_str(&call, "path")?;
        let resolved = guard::contained(&self.workspace.root, &path)
            .map_err(|e| TinyAgentsError::Tool(e.to_string()))?;
        let mut file = tokio::fs::File::open(&resolved)
            .await
            .map_err(|e| TinyAgentsError::Tool(format!("could not read {path}: {e}")))?;
        // The total is taken from metadata rather than by reading to the end,
        // so the truncation notice can name the real size without the read
        // costing it.
        let total = file.metadata().await.map(|m| m.len()).unwrap_or_default();

        // One byte past the cap, so a file sitting exactly on it is not reported
        // as truncated.
        let mut bytes = Vec::new();
        tokio::io::AsyncReadExt::take(&mut file, MAX_READ_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| TinyAgentsError::Tool(format!("could not read {path}: {e}")))?;

        let truncated = bytes.len() > MAX_READ_BYTES;
        bytes.truncate(MAX_READ_BYTES);
        // Lossy rather than a hard error: a file with one stray byte is still
        // worth showing, and refusing it sends the model looking for a problem
        // that is not the one it is solving. Truncating at a byte offset can
        // also split a multi-byte character, which is exactly what lossy
        // decoding is for.
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        if truncated {
            text.push_str(&format!(
                "\n\n[truncated: showing the first {MAX_READ_BYTES} of {total} bytes]"
            ));
        }
        Ok(ToolResult {
            call_id: call.id,
            name: self.name().to_string(),
            content: text,
            raw: None,
            error: None,
            elapsed_ms: 0,
        })
    }
}
