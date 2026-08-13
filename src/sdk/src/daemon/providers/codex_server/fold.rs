//! Folding app-server notifications into Medulla's semantic event stream.
//!
//! # Scope
//!
//! Deliberately minimal: lifecycle status, the assistant's messages, token
//! usage, and repository moves that affect where the next turn executes. The
//! app-server reports far more than that — per-item reasoning deltas, command
//! output streams, patch previews — and the CLI transport's mappers turn the
//! equivalent into the rich agent-rail detail an operator watches.
//!
//! Reproducing that surface here would mean a second implementation of every
//! mapper, tracking a wire format that is still marked experimental, for a
//! transport chosen when *throughput* is what matters. So a `codex-server` lane
//! reports that it is working, what it finally said, and what it cost — and an
//! operator who wants to watch a lane work runs it on `codex`.
//!
//! The one thing this must get right regardless is the idle watchdog: every
//! notification counts as activity, including the ones that produce no event, or
//! a long silent command would look like a dead process.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use serde_json::{json, Value};

use crate::codex_app_server::Notification;
use crate::daemon::mappers::{worktree_checkout_from_output, HarnessSemanticEvent};
use crate::protocol::{HarnessEvent, TokenUsage};
use crate::sessions::WorkspaceContext;

use super::super::types::{OnEvent, OnWorkspaceContext};

/// What a finished fold reports, without the callback it folded through.
#[derive(Debug, Clone, Default)]
pub(super) struct FoldSnapshot {
    /// Assistant text, concatenated in arrival order.
    pub(super) reply: String,
    /// Count of thread items observed.
    pub(super) items: usize,
    /// Latest token usage the thread reported.
    pub(super) usage: Option<TokenUsage>,
    /// The most recent non-retryable error Codex reported.
    pub(super) error: Option<String>,
}

/// Accumulated state for one turn.
pub(super) struct FoldState {
    /// Assistant text, concatenated in arrival order.
    pub(super) reply: String,
    /// Count of thread items observed.
    pub(super) items: usize,
    /// Latest token usage the thread reported.
    pub(super) usage: Option<TokenUsage>,
    /// The most recent non-retryable error Codex reported on this turn.
    ///
    /// Retained even when the turn goes on to complete, because a `turn/completed`
    /// carrying `status: failed` names no message of its own — the message
    /// arrived earlier, on an `error` notification.
    pub(super) error: Option<String>,
    /// When the last notification arrived, for the idle watchdog.
    pub(super) last_activity: Instant,
    /// Per-event status callback.
    on_event: Option<OnEvent>,
    /// Repository position retained across turns of this thread.
    workspace_context: WorkspaceContext,
    /// Persists a newly detected worktree for the next resumed turn.
    on_workspace_context: Option<OnWorkspaceContext>,
    /// Checkout from which Git may enumerate the repository's worktrees.
    repository_cwd: Option<PathBuf>,
    /// Line counter standing in for the CLI transport's transcript offsets.
    ///
    /// There is no transcript here, but `HarnessSemanticEvent::line` is the
    /// ordering key downstream consumers dedupe on, so it must still advance
    /// monotonically.
    line: i64,
}

impl FoldState {
    /// A fold ready for one turn.
    #[cfg(test)]
    pub(super) fn new(on_event: Option<OnEvent>) -> Self {
        Self::with_workspace(on_event, WorkspaceContext::default(), None)
    }

    /// A fold seeded with repository position retained by a resumed thread.
    pub(super) fn with_workspace(
        on_event: Option<OnEvent>,
        workspace_context: WorkspaceContext,
        on_workspace_context: Option<OnWorkspaceContext>,
    ) -> Self {
        Self::with_workspace_at(on_event, workspace_context, on_workspace_context, None)
    }

    /// A fold seeded with workspace state and its configured checkout.
    pub(super) fn with_workspace_at(
        on_event: Option<OnEvent>,
        workspace_context: WorkspaceContext,
        on_workspace_context: Option<OnWorkspaceContext>,
        repository_cwd: Option<PathBuf>,
    ) -> Self {
        Self {
            reply: String::new(),
            items: 0,
            usage: None,
            error: None,
            last_activity: Instant::now(),
            on_event,
            workspace_context,
            on_workspace_context,
            repository_cwd,
            line: 0,
        }
    }

    /// Fold one notification, emitting whatever events it implies.
    ///
    /// Returns `true` once the turn is terminal, which is the caller's signal to
    /// stop reading.
    pub(super) fn fold(&mut self, notification: &Notification) -> bool {
        self.last_activity = Instant::now();
        let params = &notification.params;
        match notification.method.as_str() {
            "turn/started" => {
                self.emit("turn/started", "status", running_status("working"));
                false
            }
            "item/started" => {
                self.items += 1;
                if let Some(detail) = item_detail(params.get("item")) {
                    self.emit("item/started", "status", running_status(&detail));
                }
                false
            }
            "item/completed" => {
                self.items += 1;
                let item = params.get("item");
                if let Some(text) = agent_message_text(item) {
                    if !text.is_empty() {
                        self.reply.push_str(&text);
                        self.emit("item/completed", "agent_message", json!({ "text": text }));
                    }
                }
                self.capture_worktree(item);
                false
            }
            "thread/tokenUsage/updated" => {
                // Cumulative per thread, so latest-wins is the correct fold —
                // the same rule the CLI transport applies to codex's
                // `token_count` events.
                if let Some(usage) = token_usage(params.get("tokenUsage")) {
                    self.usage = Some(usage);
                }
                false
            }
            "turn/completed" => {
                // A turn may report its assistant text only in the terminal
                // payload — an interrupted or replayed turn does. Reading it as
                // a fallback keeps a reply from being lost, while the
                // emptiness check keeps a normal turn from duplicating text the
                // item notifications already carried.
                if self.reply.is_empty() {
                    if let Some(text) = turn_items_text(params.get("turn")) {
                        self.reply.push_str(&text);
                    }
                }
                self.emit("turn/completed", "status", idle_status());
                true
            }
            "error" => {
                // `willRetry` means Codex is handling it; surfacing it as a
                // failure would end a turn that is about to continue.
                let will_retry = params
                    .get("willRetry")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !will_retry {
                    let message = params
                        .get("error")
                        .and_then(|error| error.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("codex reported an error")
                        .to_string();
                    self.emit("error", "error", json!({ "message": message }));
                    self.error = Some(message);
                }
                false
            }
            _ => false,
        }
    }

    /// A copy of everything the caller reads out of a finished fold.
    ///
    /// Needed because the fold is shared with the idle watchdog, which may still
    /// hold a reference when the turn ends; the callback it carries is not
    /// clonable, and nothing downstream wants it.
    pub(super) fn snapshot(&self) -> FoldSnapshot {
        FoldSnapshot {
            reply: self.reply.clone(),
            items: self.items,
            usage: self.usage,
            error: self.error.clone(),
        }
    }

    /// Emit one semantic event to the status callback.
    fn emit(&mut self, record_type: &str, kind: &str, payload: Value) {
        self.line += 1;
        let event = HarnessSemanticEvent {
            line: self.line,
            timestamp_ms: crate::clock::now_millis(),
            record_type: format!("app_server:{record_type}"),
            event: HarnessEvent {
                kind: kind.to_string(),
                role: "agent".to_string(),
                payload,
                ..Default::default()
            },
        };
        if let Some(on_event) = self.on_event.as_mut() {
            on_event(&event);
        }
    }

    /// Persist a stable worktree report carried by a completed command item.
    fn capture_worktree(&mut self, item: Option<&Value>) {
        let Some(item) = item
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("commandExecution"))
        else {
            return;
        };
        if !successful_worktree_command(item) {
            return;
        }
        let output = item
            .get("aggregatedOutput")
            .or_else(|| item.get("aggregated_output"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some((cwd, branch)) = worktree_checkout_from_output(output) else {
            return;
        };
        if !self.is_registered_worktree(&cwd, &branch) {
            return;
        }
        if self.workspace_context.cwd.as_deref() != Some(&cwd)
            || self.workspace_context.branch.as_deref() != Some(&branch)
        {
            self.workspace_context.pull_request = None;
        }
        self.workspace_context.cwd = Some(cwd);
        self.workspace_context.branch = Some(branch);
        self.emit(
            "item/completed:workspace",
            crate::harness_work::kinds::SESSION_INFO,
            json!({
                "cwd": self.workspace_context.cwd,
                "branch": self.workspace_context.branch,
            }),
        );
        if let Some(callback) = self.on_workspace_context.as_ref() {
            callback(self.workspace_context.clone());
        }
    }

    /// Accept only a report for a successful helper invocation that Git says is
    /// an existing worktree of this repository on the reported branch.
    fn is_registered_worktree(&self, cwd: &str, branch: &str) -> bool {
        let Some(repository_cwd) = self.repository_cwd.as_deref() else {
            return false;
        };
        let Ok(cwd) = std::fs::canonicalize(cwd) else {
            return false;
        };
        let Ok(output) = Command::new("git")
            .args(["-C"])
            .arg(repository_cwd)
            .args(["worktree", "list", "--porcelain"])
            .output()
        else {
            return false;
        };
        if !output.status.success() {
            return false;
        }
        registered_worktrees(&String::from_utf8_lossy(&output.stdout))
            .into_iter()
            .any(|(path, registered_branch)| path == cwd && registered_branch == branch)
    }
}

/// A command item may affect retained cwd only when the worktree helper itself
/// completed successfully. Generic shell output is untrusted text.
fn successful_worktree_command(item: &Value) -> bool {
    item.get("exitCode").and_then(Value::as_i64) == Some(0)
        && item
            .get("command")
            .and_then(Value::as_str)
            .and_then(|command| command.split_whitespace().next())
            == Some("worktree")
}

/// Parse Git's porcelain worktree listing into canonical checkout/branch pairs.
fn registered_worktrees(output: &str) -> Vec<(PathBuf, String)> {
    output
        .split("\n\n")
        .filter_map(|entry| {
            let path = entry.strip_prefix("worktree ")?.lines().next()?;
            let branch = entry
                .lines()
                .find_map(|line| line.strip_prefix("branch refs/heads/"))?;
            Some((std::fs::canonicalize(Path::new(path)).ok()?, branch.to_string()))
        })
        .collect()
}

/// A `status` payload saying the lane is working, with a one-line detail.
fn running_status(detail: &str) -> Value {
    json!({ "state": "running", "detail": detail })
}

/// A `status` payload saying the lane has gone quiet.
fn idle_status() -> Value {
    json!({ "state": "idle", "detail": "idle" })
}

/// A short human-readable detail for a started thread item, or `None` for the
/// item kinds that say nothing worth a status frame.
fn item_detail(item: Option<&Value>) -> Option<String> {
    let item = item?;
    match item.get("type").and_then(Value::as_str)? {
        "commandExecution" => {
            let command = item.get("command").and_then(Value::as_str).unwrap_or("");
            Some(match first_line(command) {
                Some(line) => format!("running `{line}`"),
                None => "running a command".to_string(),
            })
        }
        "fileChange" => Some("editing files".to_string()),
        "mcpToolCall" | "dynamicToolCall" => {
            let tool = item.get("tool").and_then(Value::as_str).unwrap_or("a tool");
            Some(format!("calling {tool}"))
        }
        "webSearch" => Some("searching the web".to_string()),
        _ => None,
    }
}

/// The first line of a command, bounded so a status detail stays one line.
fn first_line(command: &str) -> Option<String> {
    let line = command.lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    Some(if line.chars().count() > 80 {
        let head: String = line.chars().take(79).collect();
        format!("{head}…")
    } else {
        line.to_string()
    })
}

/// The text of an `agentMessage` item, or `None` for any other item kind.
fn agent_message_text(item: Option<&Value>) -> Option<String> {
    let item = item?;
    if item.get("type").and_then(Value::as_str) != Some("agentMessage") {
        return None;
    }
    item.get("text").and_then(Value::as_str).map(str::to_string)
}

/// Every assistant message in a terminal turn payload, concatenated.
fn turn_items_text(turn: Option<&Value>) -> Option<String> {
    let items = turn?.get("items")?.as_array()?;
    let text = items
        .iter()
        .filter_map(|item| agent_message_text(Some(item)))
        .collect::<Vec<_>>()
        .join("");
    (!text.is_empty()).then_some(text)
}

/// Read a `ThreadTokenUsage` into Medulla's two-number shape.
///
/// The `total` breakdown is the cumulative one, which is what a task reports.
/// Missing counts read as zero rather than failing the parse: a run that
/// produced work but no usable telemetry should still report its work.
fn token_usage(usage: Option<&Value>) -> Option<TokenUsage> {
    let total = usage?.get("total")?;
    let count = |key: &str| total.get(key).and_then(Value::as_i64).unwrap_or(0);
    let input = count("inputTokens") + count("cachedInputTokens");
    let output = count("outputTokens") + count("reasoningOutputTokens");
    (input > 0 || output > 0).then_some(TokenUsage {
        input_tokens: input,
        output_tokens: output,
    })
}
