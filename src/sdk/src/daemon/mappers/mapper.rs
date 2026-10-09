//! The stateful per-stream fold: [`HarnessLineMapper`]'s constructor, usage
//! accessor, and the per-line dispatch to the provider mappers, including the
//! codex duplicate-message dedupe and the token-usage scan.

use serde_json::Value;

use crate::protocol::TokenUsage;

use super::claude::claude_events_from_line;
use super::codex::codex_events_from_line;
use super::opencode::opencode_events_from_line;
use super::types::{HarnessLineMapper, HarnessSemanticEvent, Provider};
use super::usage::scan_usage;

/// Codex records each assistant message twice within this window; drop the repeat.
const CODEX_DUPLICATE_WINDOW_MS: i64 = 2000;

/// How much of `current` is new since `previous`, given cumulative provider
/// counters.
///
/// A decrease means the provider reset its counter — a new sub-session
/// started counting from zero, not that usage went backwards — so the whole
/// new snapshot is the delta; `current.saturating_sub(previous)` alone would
/// floor that case to zero and silently drop it.
/// The usage to report for one Codex `token_count` record.
///
/// Codex's total is cumulative over the whole conversation, so on a mapper's
/// first record it may already include turns this mapper never saw: a resumed
/// conversation, or a PTY reused for a later turn. Subtracting a zero baseline
/// would report all of that history again. The record's `last_token_usage`
/// (`first_call`) is exactly what the latest call used, so it is reported for
/// the first record, and the total becomes the baseline. For a new
/// conversation the two are equal. Later records report the delta between
/// totals; a record with no per-call usage falls back to that delta as well.
pub(super) fn codex_reported_usage(
    first_call: Option<TokenUsage>,
    previous: TokenUsage,
    total: TokenUsage,
) -> (i64, i64) {
    match first_call {
        Some(call) => (call.input_tokens, call.output_tokens),
        None => (
            token_delta(total.input_tokens, previous.input_tokens),
            token_delta(total.output_tokens, previous.output_tokens),
        ),
    }
}

pub(super) fn token_delta(current: i64, previous: i64) -> i64 {
    if current < 0 {
        0
    } else if current < previous {
        current
    } else {
        current - previous
    }
}

impl HarnessLineMapper {
    /// Seed repository context retained by a reused interactive session.
    pub fn set_workspace_context(
        &mut self,
        cwd: Option<String>,
        branch: Option<String>,
        pull_request: Option<String>,
    ) {
        self.workspace_cwd = cwd;
        self.workspace_branch = branch;
        self.workspace_pull_request = pull_request;
    }

    /// Repository context learned from authoritative worktree reports.
    pub fn workspace_context(&self) -> (Option<String>, Option<String>, Option<String>) {
        (
            self.workspace_cwd.clone(),
            self.workspace_branch.clone(),
            self.workspace_pull_request.clone(),
        )
    }

    /// Build a mapper for `provider` (`"claude" | "codex" | "opencode"`).
    /// Unknown providers yield a mapper that emits nothing.
    pub fn new(provider: &str) -> Self {
        Self::new_with_gh_repo_override(provider, std::env::var_os("GH_REPO").is_some())
    }

    /// Build a mapper with explicit effective child `GH_REPO` state.
    ///
    /// Provider execution passes the environment installed on the child. This
    /// injection seam also keeps transcript tests deterministic without mutating
    /// the process environment shared by parallel tests.
    pub fn new_with_gh_repo_override(provider: &str, gh_repo_is_set: bool) -> Self {
        let provider = match provider {
            "claude" => Provider::Claude,
            "codex" => Provider::Codex,
            _ => Provider::Opencode,
        };
        HarnessLineMapper {
            provider,
            last_text: None,
            last_at_ms: i64::MIN,
            usage: None,
            saw_claude_call_usage: false,
            pull_request_calls: Default::default(),
            workspace_cwd: None,
            workspace_branch: None,
            workspace_pull_request: None,
            gh_repo_is_set,
        }
    }

    /// The most recent total token usage seen on the stream, if any. Claude and
    /// Codex report cumulative counters; OpenCode reports per-step counts.
    pub fn usage(&self) -> Option<TokenUsage> {
        self.usage
    }

    /// Map one raw JSONL line into zero or more semantic events.
    pub fn map_line(&mut self, raw: &str, line: i64) -> Vec<HarnessSemanticEvent> {
        // Token accounting rides on assorted records per provider; scan any
        // line that plausibly carries counts and keep the latest.
        if raw.contains("okens") {
            if let Ok(value) = serde_json::from_str::<Value>(raw) {
                // Codex `token_count` records carry the latest call's usage
                // (`last_token_usage`) beside the running total, and the scan
                // would take whichever comes first. The fold below is
                // cumulative, so it must read the total — and only the total:
                // a record whose total is present but invalid is skipped,
                // never re-read through its per-call sibling.
                let codex_total = (self.provider == Provider::Codex)
                    .then(|| value.pointer("/payload/info/total_token_usage"))
                    .flatten();
                let usage = match codex_total {
                    Some(total) => scan_usage(total, 0),
                    None => scan_usage(&value, 0),
                };
                // The same record's per-call usage, for a first snapshot.
                let codex_last = (self.provider == Provider::Codex)
                    .then(|| value.pointer("/payload/info/last_token_usage"))
                    .flatten()
                    .and_then(|last| scan_usage(last, 0));
                if let Some(usage) = usage {
                    let record_type = value.get("type").and_then(Value::as_str);
                    let duplicate_claude_result = self.provider == Provider::Claude
                        && record_type == Some("result")
                        && self.saw_claude_call_usage;
                    if !duplicate_claude_result {
                        let first_snapshot = self.usage.is_none();
                        let previous = self.usage.unwrap_or(TokenUsage {
                            input_tokens: 0,
                            output_tokens: 0,
                        });
                        // OpenCode's step-finish usage is per-step; Claude and
                        // Codex emit cumulative snapshots. Preserve total usage
                        // for callers while reporting the appropriate amount.
                        let (input, output, total) = match self.provider {
                            Provider::Claude if record_type == Some("assistant") => {
                                self.saw_claude_call_usage = true;
                                (
                                    usage.input_tokens,
                                    usage.output_tokens,
                                    TokenUsage {
                                        input_tokens: previous
                                            .input_tokens
                                            .saturating_add(usage.input_tokens),
                                        output_tokens: previous
                                            .output_tokens
                                            .saturating_add(usage.output_tokens),
                                    },
                                )
                            }
                            Provider::Opencode => (
                                usage.input_tokens,
                                usage.output_tokens,
                                TokenUsage {
                                    input_tokens: previous
                                        .input_tokens
                                        .saturating_add(usage.input_tokens),
                                    output_tokens: previous
                                        .output_tokens
                                        .saturating_add(usage.output_tokens),
                                },
                            ),
                            Provider::Codex => {
                                let (input, output) = codex_reported_usage(
                                    first_snapshot.then_some(codex_last).flatten(),
                                    previous,
                                    usage,
                                );
                                (input, output, usage)
                            }
                            Provider::Claude => (
                                token_delta(usage.input_tokens, previous.input_tokens),
                                token_delta(usage.output_tokens, previous.output_tokens),
                                usage,
                            ),
                        };
                        self.usage = Some(total);
                        if input > 0 || output > 0 {
                            crate::analytics::record_token_usage(input, output);
                        }
                    }
                }
            }
        }
        let mut events = match self.provider {
            Provider::Claude => claude_events_from_line(
                raw,
                line,
                &mut self.pull_request_calls,
                self.workspace_cwd.as_deref(),
                self.workspace_branch.as_deref(),
                self.gh_repo_is_set,
            ),
            Provider::Codex => codex_events_from_line(
                raw,
                line,
                &mut self.pull_request_calls,
                self.workspace_cwd.as_deref(),
                self.workspace_branch.as_deref(),
                self.gh_repo_is_set,
            ),
            Provider::Opencode => opencode_events_from_line(raw, line),
        };
        // A resumed Claude process starts in its launch root and repeats that
        // root in `system:init`. Retained worktree context is later, explicit
        // tool evidence, so the init cwd must not replace it in downstream
        // session snapshots.
        if self.workspace_cwd.is_some() || self.workspace_pull_request.is_some() {
            for event in &mut events {
                if event.record_type == "system:init" {
                    event
                        .event
                        .payload
                        .as_object_mut()
                        .map(|payload| payload.remove("cwd"));
                }
            }
        }
        for event in &events {
            if event.event.kind != crate::harness_work::kinds::SESSION_INFO
                || !event.record_type.ends_with(":workspace")
            {
                continue;
            }
            let cwd = event.event.payload.get("cwd").and_then(Value::as_str);
            let branch = event.event.payload.get("branch").and_then(Value::as_str);
            if cwd.is_some_and(|cwd| self.workspace_cwd.as_deref() != Some(cwd))
                || branch.is_some_and(|branch| self.workspace_branch.as_deref() != Some(branch))
            {
                self.workspace_pull_request = None;
            }
            if let Some(cwd) = cwd {
                self.workspace_cwd = Some(cwd.to_string());
            }
            if let Some(branch) = branch {
                self.workspace_branch = Some(branch.to_string());
            }
            if let Some(pull_request) = event
                .event
                .payload
                .get("pull_request")
                .and_then(Value::as_str)
            {
                self.workspace_pull_request = Some(pull_request.to_string());
            }
        }
        if self.provider != Provider::Codex {
            return events;
        }
        // Codex dedupe: an agent_message whose text equals the previous one
        // within the window is the same message re-recorded, not a new turn.
        events
            .into_iter()
            .filter(|semantic| {
                if semantic.event.kind != "agent_message" {
                    return true;
                }
                let text = semantic
                    .event
                    .payload
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let at_ms = semantic.timestamp_ms;
                let duplicate = self.last_text.as_deref() == Some(text.as_str())
                    && (at_ms - self.last_at_ms).abs() <= CODEX_DUPLICATE_WINDOW_MS;
                self.last_text = Some(text);
                self.last_at_ms = at_ms;
                !duplicate
            })
            .collect()
    }
}
