//! Mint a per-run host capability. Its secret never enters the model tool env.

use crate::mcp::McpSession;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

pub(super) async fn create(
    env: &HashMap<String, String>,
    cwd: &Path,
    run: &str,
) -> Result<Option<Arc<McpSession>>, String> {
    if crate::harness_tools::withheld(env) {
        return Ok(None);
    }
    let mut private = env.clone();
    private.remove(crate::control_socket::MCP_SOCKET_ENV);
    private.remove(crate::control_socket::MCP_GRANT_ENV);
    private.insert(crate::mcp::attach::ATTACHED_ENV.into(), run.into());
    let workflows = crate::mcp::attach::workflows_enabled(env);
    let mode = env.get(crate::mcp::TOOL_MODE_ENV).map(String::as_str);
    if let Some((socket, token)) = crate::mcp::attach::local_fleet_grant(run, env, mode, workflows)
    {
        private.insert(
            crate::control_socket::MCP_SOCKET_ENV.into(),
            socket.to_string_lossy().into_owned(),
        );
        private.insert(crate::control_socket::MCP_GRANT_ENV.into(), token);
    }
    crate::mcp::session_for_env(&private, cwd, None)
        .await
        .map(Some)
        .map_err(|error| format!("native tools: {error}"))
}
