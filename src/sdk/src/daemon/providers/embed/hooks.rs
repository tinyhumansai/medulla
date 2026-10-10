//! Scoped operator command hooks. Post-tool commands finish before the next call.

use crate::harness_hooks::{HookEvent, HookHandler, HookSpec, HooksConfig};
use crate::protocol::HarnessProvider;
use openhuman_embed::seams::{ToolHook, ToolHookContext, ToolHookDecision};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub(super) struct Hooks {
    hooks: Vec<HookSpec>,
    env: HashMap<String, String>,
    cwd: PathBuf,
}

impl Hooks {
    pub fn new(config: &HooksConfig, env: HashMap<String, String>, cwd: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            hooks: config
                .for_provider(HarnessProvider::Openhuman)
                .into_iter()
                .cloned()
                .collect(),
            env,
            cwd,
        })
    }

    async fn run(
        &self,
        event: HookEvent,
        tool: &str,
        payload: Value,
    ) -> anyhow::Result<Vec<Value>> {
        let mut replies = Vec::new();
        for hook in &self.hooks {
            if hook.event != event
                || (matches!(event, HookEvent::PreToolUse | HookEvent::PostToolUse)
                    && !matches_tool(&hook.matcher, tool))
            {
                continue;
            }
            let HookHandler::Command { command, timeout } = &hook.handler;
            let mut cmd = command_for_platform(command);
            cmd.current_dir(&self.cwd).env_clear().envs(&self.env);
            let output = openhuman_embed::process::command_output(
                &mut cmd,
                serde_json::to_vec(&payload)?,
                Duration::from_secs(timeout.unwrap_or(60)),
            )
            .await?;
            if !output.status.success() {
                anyhow::bail!("{} hook exited {}", event.as_str(), output.status);
            }
            if !output.stdout.is_empty() {
                if let Ok(reply) = serde_json::from_slice(&output.stdout) {
                    replies.push(reply);
                }
            }
        }
        Ok(replies)
    }

    pub async fn stop(&self, session: &str, reply: &str) -> anyhow::Result<()> {
        self.run(
            HookEvent::Stop,
            "",
            json!({"hook_event_name":"Stop", "session_id":session,
            "cwd":self.cwd, "last_assistant_message":reply, "stop_hook_active":false}),
        )
        .await?;
        Ok(())
    }
}

fn command_for_platform(command: &str) -> tokio::process::Command {
    #[cfg(unix)]
    {
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.args(["-c", command]);
        cmd
    }
    #[cfg(windows)]
    {
        let mut cmd = tokio::process::Command::new("cmd.exe");
        cmd.args(["/D", "/S", "/C", command]);
        cmd
    }
}

fn matches_tool(matcher: &str, tool: &str) -> bool {
    matcher.split('|').any(|pattern| {
        pattern == "*"
            || pattern == tool
            || regex::Regex::new(&format!("^(?:{pattern})$"))
                .is_ok_and(|pattern| pattern.is_match(tool))
    })
}

fn payload(ctx: &ToolHookContext, event: HookEvent) -> Value {
    json!({"hook_event_name":event.as_str(), "session_id":ctx.session_id, "agent_id":ctx.agent_id,
        "cwd":ctx.cwd, "tool_use_id":ctx.call_id, "tool_name":ctx.tool_name,
        "tool_input":ctx.arguments, "tool_response":ctx.output, "error":ctx.error})
}

#[async_trait::async_trait]
impl ToolHook for Hooks {
    fn name(&self) -> &str {
        "medulla.harness-hooks"
    }
    async fn before_tool(&self, _: &ToolHookContext) -> anyhow::Result<()> {
        Ok(())
    }
    async fn after_tool(&self, ctx: &ToolHookContext) -> anyhow::Result<()> {
        self.run(
            HookEvent::PostToolUse,
            &ctx.tool_name,
            payload(ctx, HookEvent::PostToolUse),
        )
        .await?;
        Ok(())
    }
    async fn before_tool_decision(&self, ctx: &ToolHookContext) -> ToolHookDecision {
        let replies = match self
            .run(
                HookEvent::PreToolUse,
                &ctx.tool_name,
                payload(ctx, HookEvent::PreToolUse),
            )
            .await
        {
            Ok(replies) => replies,
            Err(error) => return ToolHookDecision::Deny(error.to_string()),
        };
        let mut decision = ToolHookDecision::Proceed;
        for reply in replies {
            let specifics = &reply["hookSpecificOutput"];
            let reason = specifics["permissionDecisionReason"]
                .as_str()
                .unwrap_or("operator hook refused the tool")
                .to_owned();
            if reply["decision"] == "block" || specifics["permissionDecision"] == "deny" {
                return ToolHookDecision::Deny(reason);
            }
            if specifics["permissionDecision"] == "ask" {
                return ToolHookDecision::Deny(
                    "operator hook requires approval; this call was not approved".into(),
                );
            }
            if let Some(input) = specifics.get("updatedInput") {
                decision = ToolHookDecision::ProceedWith(input.clone());
            }
        }
        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[tokio::test]
    async fn stop_hooks_run_even_with_a_tool_matcher() {
        let directory = tempfile::tempdir().unwrap();
        let config: HooksConfig = serde_json::from_value(json!([{
            "event":"Stop", "matcher":"write_file", "type":"command",
            "command":"cat > stop-payload.json", "harnesses":["openhuman"]
        }]))
        .unwrap();
        let hooks = Hooks::new(&config, HashMap::new(), directory.path().to_owned());
        hooks.stop("native-session", "done").await.unwrap();
        let payload: Value = serde_json::from_str(
            &std::fs::read_to_string(directory.path().join("stop-payload.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(payload["hook_event_name"], "Stop");
        assert_eq!(payload["last_assistant_message"], "done");
    }

    #[test]
    fn matchers_are_tool_scoped() {
        assert!(matches_tool("shell|write_file", "shell"));
        assert!(matches_tool("workflow_.*", "workflow_run"));
        assert!(!matches_tool("shell", "shell_extra"));
    }
}
