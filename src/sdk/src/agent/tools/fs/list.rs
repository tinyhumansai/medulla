//! Listing a checkout directory, bounded during the scan.

use serde_json::{json, Value};
use tinyagents::error::{Result as TaResult, TinyAgentsError};
use tinyagents::tool::{Tool, ToolResult};
use tinyinference::tool::{ToolCall, ToolFormat, ToolSchema};

use super::types::Workspace;
use crate::agent::tools::guard;

use super::MAX_ENTRIES;

/// List a directory in the checkout.
pub struct ListDirTool {
    workspace: Workspace,
}

impl ListDirTool {
    /// Root a lister at `workspace`.
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

#[async_trait::async_trait]
impl Tool<()> for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List the entries of a workspace directory. `path` is relative to the \
         workspace root and defaults to the root itself."
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
                        "description": "Workspace-relative directory to list.",
                    },
                },
            }),
            format: ToolFormat::default(),
        }
    }

    async fn call(&self, _state: &(), call: ToolCall) -> TaResult<ToolResult> {
        let path = call
            .arguments
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or(".")
            .to_string();
        let resolved = guard::contained(&self.workspace.root, &path)
            .map_err(|e| TinyAgentsError::Tool(e.to_string()))?;
        let mut entries = tokio::fs::read_dir(&resolved)
            .await
            .map_err(|e| TinyAgentsError::Tool(format!("could not list {path}: {e}")))?;

        // The scan stops once truncation is established, not at end of directory.
        // Bounding retained memory was half the fix: a generated or cache
        // directory with millions of entries still cost millions of
        // `next_entry()` calls before returning 500 names, so the cap bounded
        // the answer and not the work.
        //
        // The cost is the exact total, and with it some determinism. Reading
        // everything and sorting returned the lexicographically first
        // `MAX_ENTRIES` — stable across runs regardless of filesystem order.
        // Stopping early returns the first `MAX_ENTRIES` in *readdir* order,
        // which is filesystem-dependent, so which names come back from an
        // oversized directory can differ between runs. Sorting still applies to
        // what was read, so the listing itself is ordered either way.
        //
        // That trade is worth taking because the two cases that matter are not
        // affected: a directory at or under the cap is read whole and is exactly
        // as deterministic as before, and one far over it was never going to be
        // usefully summarised by 500 arbitrary names anyway. What the model
        // needs there is to know the listing is incomplete, which the notice
        // says.
        let mut names: Vec<String> = Vec::new();
        let mut truncated = false;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| TinyAgentsError::Tool(format!("could not list {path}: {e}")))?
        {
            if names.len() == MAX_ENTRIES {
                // One entry past the cap is what proves there are more; nothing
                // beyond it is read.
                truncated = true;
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let suffix = match entry.file_type().await {
                Ok(t) if t.is_dir() => "/",
                _ => "",
            };
            names.push(format!("{name}{suffix}"));
        }
        // Sorted so a directory read whole produces the same transcript twice;
        // `read_dir` order is filesystem-dependent and not stable.
        names.sort();
        let mut content = names.join("\n");
        if truncated {
            // No exact total: establishing it is the scan this avoids. The
            // model needs to know the listing is incomplete, not how incomplete.
            content.push_str(&format!(
                "\n[truncated: showing {MAX_ENTRIES} entries; more were omitted]"
            ));
        }
        Ok(ToolResult {
            call_id: call.id,
            name: self.name().to_string(),
            content,
            raw: None,
            error: None,
            elapsed_ms: 0,
        })
    }
}
