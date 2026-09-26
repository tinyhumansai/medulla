//! Writing a file into the checkout, creating parents as needed.

use serde_json::json;
use tinyagents::error::{Result as TaResult, TinyAgentsError};
use tinyagents::tool::{Tool, ToolResult};
use tinyinference::tool::{ToolCall, ToolFormat, ToolSchema};

use super::types::{arg_str, Workspace};
use crate::agent::tools::guard;

/// Write a file into the checkout, creating parent directories as needed.
pub struct WriteFileTool {
    workspace: Workspace,
}

impl WriteFileTool {
    /// Root a writer at `workspace`.
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

#[async_trait::async_trait]
impl Tool<()> for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write a UTF-8 text file into the workspace, replacing it if it exists. \
         `path` is relative to the workspace root."
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
                        "description": "Workspace-relative path to write.",
                    },
                    "content": {
                        "type": "string",
                        "description": "Full file contents to write.",
                    },
                },
                "required": ["path", "content"],
            }),
            format: ToolFormat::default(),
        }
    }

    async fn call(&self, _state: &(), call: ToolCall) -> TaResult<ToolResult> {
        let path = arg_str(&call, "path")?;
        let content = arg_str(&call, "content")?;
        let resolved = guard::contained(&self.workspace.root, &path)
            .map_err(|e| TinyAgentsError::Tool(e.to_string()))?;
        // The guard resolved the parent, so it exists in the escape sense —
        // but a nested create still needs the intermediate directories made.
        if let Some(parent) = resolved.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| TinyAgentsError::Tool(format!("could not create {path}: {e}")))?;
        }
        tokio::fs::write(&resolved, content.as_bytes())
            .await
            .map_err(|e| TinyAgentsError::Tool(format!("could not write {path}: {e}")))?;
        Ok(ToolResult {
            call_id: call.id,
            name: self.name().to_string(),
            content: format!("wrote {} bytes to {path}", content.len()),
            raw: None,
            error: None,
            elapsed_ms: 0,
        })
    }
}
