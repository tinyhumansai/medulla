//! Script rendering for [`MockCli`].
//!
//! A second `impl MockCli` block that turns a mock's steps + terminal behavior
//! into an executable `/bin/sh` script — either streaming provider-shaped JSONL
//! to stdout ([`MockCli::script`]) or writing a session-log transcript for the
//! wrapper tailer ([`MockCli::session_log_script`]). Each high-level [`Step`] is
//! lowered to the provider-specific record shape via the `lower_*` helpers,
//! leaning on the record builders in the sibling `helpers` module.

#![allow(dead_code)]

use serde_json::json;

use super::helpers::*;
use super::types::*;

impl MockCli {
    /// The provider-appropriate reply line for `text` (result line for claude,
    /// an agent message for codex, a text part for opencode).
    fn reply_line(&self, text: &str) -> String {
        match self.provider {
            MockProvider::Claude => json!({ "type": "result", "result": text }).to_string(),
            MockProvider::Codex => codex_record(
                "event_msg",
                json!({ "type": "agent_message", "message": text }),
            ),
            MockProvider::Opencode => {
                opencode_record("text", json!({ "type": "text", "text": text }))
            }
        }
    }

    /// Render the executable `/bin/sh` script body.
    pub fn script(&self) -> String {
        if let Some(spec) = &self.session_log {
            return self.session_log_script(spec);
        }
        let mut out = String::from("#!/bin/sh\n");
        for step in &self.steps {
            for line in self.lower(step) {
                out.push_str(&emit_line(&line));
            }
        }
        match &self.terminal {
            Terminal::ClaudeResult(reply) => {
                let line = json!({ "type": "result", "result": reply }).to_string();
                out.push_str(&emit_line(&line));
            }
            Terminal::Exit => {}
            Terminal::Fail { code, stderr } => {
                out.push_str(&format!("printf '%s\\n' {} >&2\n", sh_quote(stderr)));
                out.push_str(&format!("exit {code}\n"));
            }
            Terminal::Hang => {
                // Sleep-loop rather than blocking on stdin: the daemon's waiter
                // closes the child's stdin, which would EOF a `read`-based hang
                // and let the script exit instead of idling until killed.
                out.push_str("while :; do sleep 1; done\n");
            }
            Terminal::StdinEcho { provider_reply } => {
                out.push_str("read line\n");
                let line = match self.provider {
                    MockProvider::Claude => {
                        r#"printf '{"type":"result","result":"got: %s"}\n' "$line""#.to_string()
                    }
                    MockProvider::Codex if *provider_reply => {
                        r#"printf '{"type":"event_msg","payload":{"type":"agent_message","message":"got: %s"}}\n' "$line""#.to_string()
                    }
                    _ => {
                        r#"printf '{"type":"text","part":{"type":"text","text":"got: %s"}}\n' "$line""#.to_string()
                    }
                };
                out.push_str(&line);
                out.push('\n');
            }
            Terminal::FlakyLock(reply) => {
                out.push_str("MARKER=\"$0.lock\"\n");
                out.push_str(
                    "if [ ! -f \"$MARKER\" ]; then : > \"$MARKER\"; printf '%s\\n' 'Error: database is locked' >&2; exit 1; fi\n",
                );
                out.push_str(&emit_line(&self.reply_line(reply)));
            }
        }
        out
    }

    /// Render a `/bin/sh` script that writes its transcript to a session-log file
    /// (for the wrapper tailer) rather than streaming to stdout.
    fn session_log_script(&self, spec: &SessionLogSpec) -> String {
        let mut out = String::from("#!/bin/sh\n");
        out.push_str(&format!("LOG={}\n", sh_quote(&spec.path.to_string_lossy())));
        out.push_str("mkdir -p \"$(dirname \"$LOG\")\"\n");
        // Head record carrying the session id + cwd so the wrapper can anchor it.
        let head = match self.provider {
            MockProvider::Codex | MockProvider::Opencode => json!({
                "type": "session_meta",
                "timestamp": "2026-07-05T00:00:00.000Z",
                "payload": { "session_id": spec.session_id, "cwd": spec.cwd },
            })
            .to_string(),
            MockProvider::Claude => json!({
                "type": "summary",
                "timestamp": "2026-07-05T00:00:00.000Z",
                "sessionId": spec.session_id,
                "cwd": spec.cwd,
            })
            .to_string(),
        };
        out.push_str(&emit_log_line(&head));
        for step in &self.steps {
            for line in self.lower(step) {
                out.push_str(&emit_log_line(&line));
            }
        }
        if spec.echo_stdin {
            out.push_str("read line\n");
            let record = match self.provider {
                MockProvider::Claude => {
                    r#"printf '{"type":"assistant","timestamp":"2026-07-05T00:00:00.000Z","message":{"role":"assistant","content":[{"type":"text","text":"got: %s"}]}}\n' "$line" >> "$LOG""#
                }
                _ => {
                    r#"printf '{"type":"event_msg","timestamp":"2026-07-05T00:00:00.000Z","payload":{"type":"agent_message","message":"got: %s"}}\n' "$line" >> "$LOG""#
                }
            };
            out.push_str(record);
            out.push('\n');
        }
        if let Terminal::Fail { code, stderr } = &self.terminal {
            out.push_str(&format!("printf '%s\\n' {} >&2\n", sh_quote(stderr)));
            out.push_str(&format!("exit {code}\n"));
        }
        out
    }

    /// Lower one high-level step to zero or more provider-specific JSONL lines.
    fn lower(&self, step: &Step) -> Vec<String> {
        match self.provider {
            MockProvider::Claude => self.lower_claude(step),
            MockProvider::Codex => self.lower_codex(step),
            MockProvider::Opencode => self.lower_opencode(step),
        }
    }

    fn lower_claude(&self, step: &Step) -> Vec<String> {
        match step {
            Step::Prompt(text) => vec![claude_record(
                "user",
                json!({ "role": "user", "content": text }),
            )],
            Step::Thinking(text) => vec![claude_record(
                "assistant",
                json!({ "role": "assistant", "content": [{ "type": "thinking", "thinking": text }] }),
            )],
            Step::Message(text) => vec![claude_record(
                "assistant",
                json!({ "role": "assistant", "content": [{ "type": "text", "text": text }] }),
            )],
            Step::Usage { input, output } => vec![claude_record(
                "assistant",
                json!({
                    "role": "assistant",
                    "content": [],
                    "usage": { "input_tokens": input, "output_tokens": output }
                }),
            )],
            Step::Tool {
                name,
                input,
                output,
                is_error,
            } => {
                let call_id = next_call_id();
                vec![
                    claude_record(
                        "assistant",
                        json!({ "role": "assistant", "content": [{
                            "type": "tool_use", "id": call_id, "name": name, "input": input,
                        }] }),
                    ),
                    claude_record(
                        "user",
                        json!({ "role": "user", "content": [{
                            "type": "tool_result", "tool_use_id": call_id,
                            "is_error": is_error, "content": output,
                        }] }),
                    ),
                ]
            }
            Step::Status { .. } => Vec::new(),
            Step::ProviderError(_) => Vec::new(),
            Step::Garbage(line) => vec![line.clone()],
            Step::Raw(line) => vec![line.clone()],
        }
    }

    fn lower_codex(&self, step: &Step) -> Vec<String> {
        match step {
            Step::Prompt(text) => vec![codex_record(
                "event_msg",
                json!({ "type": "user_message", "message": text }),
            )],
            Step::Thinking(text) => vec![codex_record(
                "response_item",
                json!({ "type": "reasoning", "summary": [{ "type": "summary_text", "text": text }] }),
            )],
            Step::Message(text) => vec![codex_record(
                "event_msg",
                json!({ "type": "agent_message", "message": text }),
            )],
            Step::Usage { input, output } => vec![codex_record(
                "event_msg",
                json!({
                    "type": "token_count",
                    "info": {
                        "total_token_usage": { "input_tokens": input, "output_tokens": output }
                    }
                }),
            )],
            Step::Tool {
                name,
                input,
                output,
                is_error,
            } => {
                let call_id = next_call_id();
                // Codex serializes arguments as a JSON *string*.
                let arguments = serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string());
                vec![
                    codex_record(
                        "response_item",
                        json!({
                            "type": "function_call", "name": name,
                            "call_id": call_id, "arguments": arguments,
                        }),
                    ),
                    codex_record(
                        "response_item",
                        json!({
                            "type": "function_call_output", "call_id": call_id,
                            "output": output, "success": !is_error,
                        }),
                    ),
                ]
            }
            Step::Status { running } => vec![codex_record(
                "event_msg",
                json!({ "type": if *running { "task_started" } else { "task_complete" } }),
            )],
            Step::ProviderError(_) => Vec::new(),
            Step::Garbage(line) => vec![line.clone()],
            Step::Raw(line) => vec![line.clone()],
        }
    }

    fn lower_opencode(&self, step: &Step) -> Vec<String> {
        match step {
            // OpenCode's flat run format carries no user echo.
            Step::Prompt(_) => Vec::new(),
            Step::Thinking(text) => vec![opencode_record(
                "reasoning",
                json!({ "type": "reasoning", "text": text }),
            )],
            Step::Message(text) => vec![opencode_record(
                "text",
                json!({ "type": "text", "text": text }),
            )],
            Step::Usage { input, output } => vec![opencode_record(
                "step-finish",
                json!({
                    "type": "step-finish",
                    "tokens": { "input_tokens": input, "output_tokens": output }
                }),
            )],
            Step::Tool {
                name,
                input,
                output,
                is_error,
            } => {
                let call_id = next_call_id();
                let status = if *is_error { "error" } else { "completed" };
                vec![
                    opencode_record(
                        "tool",
                        json!({
                            "type": "tool", "tool": name, "callID": call_id,
                            "state": { "status": "running", "input": input },
                        }),
                    ),
                    opencode_record(
                        "tool",
                        json!({
                            "type": "tool", "tool": name, "callID": call_id,
                            "state": { "status": status, "output": output },
                        }),
                    ),
                ]
            }
            Step::Status { .. } => Vec::new(),
            Step::ProviderError(message) => vec![json!({
                "type": "error",
                "error": { "name": "ProviderError", "data": { "message": message } },
            })
            .to_string()],
            Step::Garbage(line) => vec![line.clone()],
            Step::Raw(line) => vec![line.clone()],
        }
    }
}
