//! Native tool declarations delegate to the same grant-checked handlers as MCP.

use crate::mcp::McpSession;
use openhuman_embed::{HostTurnTools, Tool, ToolResult};
use serde_json::{json, Value};
use std::sync::Arc;

pub(super) fn belt(session: Arc<McpSession>) -> HostTurnTools {
    HostTurnTools::advertised(
        crate::mcp::tools::tool_definitions(&session)
            .into_iter()
            .filter_map(|schema| {
                Some(Box::new(NativeTool {
                    name: schema.get("name")?.as_str()?.to_owned(),
                    description: schema
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    parameters: schema.get("inputSchema")?.clone(),
                    session: session.clone(),
                }) as Box<dyn Tool>)
            })
            .collect(),
    )
}

struct NativeTool {
    name: String,
    description: String,
    parameters: Value,
    session: Arc<McpSession>,
}
#[async_trait::async_trait]
impl Tool for NativeTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn parameters_schema(&self) -> Value {
        self.parameters.clone()
    }
    async fn execute(&self, arguments: Value) -> anyhow::Result<ToolResult> {
        let reply = crate::mcp::handle_request(
            &self.session,
            &json!({"jsonrpc":"2.0", "id":1,
            "method":"tools/call", "params":{"name":self.name,"arguments":arguments}}),
        )
        .await
        .ok_or_else(|| anyhow::anyhow!("native tool handler returned no response"))?;
        if let Some(error) = reply.get("error") {
            anyhow::bail!("{error}");
        }
        let result = &reply["result"];
        let text = result["content"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_else(|| result.to_string());
        if result["isError"].as_bool().unwrap_or(false) {
            Ok(ToolResult::error(text))
        } else {
            Ok(ToolResult::success(text))
        }
    }
}
