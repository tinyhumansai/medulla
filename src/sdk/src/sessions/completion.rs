//! Turn-completion detection for a harness driven through its **interactive**
//! interface.
//!
//! The headless path knows a turn ended because `claude -p --output-format
//! stream-json` prints a `result` frame. An interactive session prints nothing —
//! it paints a screen. But it still writes its JSONL transcript, and that
//! transcript carries an explicit, authoritative marker:
//!
//! ```jsonc
//! { "type": "assistant",
//!   "message": { "stop_reason": "tool_use",  … } }   // mid-turn, will continue
//! { "type": "assistant",
//!   "message": { "stop_reason": "end_turn",  … } }   // ← the turn is complete
//! ```
//!
//! Verified across 575 local transcripts (~60k assistant records): `tool_use`
//! 54,837 · `end_turn` 4,819 · `stop_sequence` 45 · absent 48 (0.08%).
//!
//! So completion is **observed, not inferred**. That matters: inferring it from
//! silence would end a turn every time a model paused or a build ran long.
//! Quiescence survives here only as a backstop for the 0.08% of records that
//! carry no `stop_reason` at all — see [`TurnWatcher::stalled_for`].
//!
//! Sub-agents are not a hazard by construction: `isSidechain` is never true in
//! any observed transcript, because a sub-agent writes its own file. Tailing the
//! session's own transcript therefore sees only top-level turns.
//!
//! # Codex
//!
//! Codex marks completion even more directly, in its `~/.codex/sessions/**`
//! rollout:
//!
//! ```jsonc
//! { "type": "event_msg", "payload": { "type": "task_started", … } }
//! { "type": "event_msg", "payload": { "type": "task_complete",
//!     "turn_id": "…", "last_agent_message": "…" } }   // ← done, with the answer
//! { "type": "event_msg", "payload": { "type": "turn_aborted",
//!     "turn_id": "…", "reason": "…" } }
//! ```
//!
//! `last_agent_message` carries the final answer on the completion event itself,
//! so the codex path needs no text accumulation at all.

use serde_json::Value;

use crate::tinyplace::HarnessProvider;

/// A claude `stop_reason` meaning the assistant will continue working.
const CONTINUES: &str = "tool_use";

/// A terminal `stop_reason` seen, held until its message is fully written.
///
/// Claude Code writes **one transcript record per content block**, repeating the
/// message-level `stop_reason` on every one. A final `[thinking, text]` message
/// therefore lands as two `end_turn` records — the thinking one first, carrying
/// no reply text at all. Settling on the first would answer with whatever
/// narration preceded it, or with nothing.
#[derive(Debug, Clone)]
struct PendingTerminal {
    /// `message.id`, shared by every record of the same message. `None` when the
    /// record carried no id, in which case the next record of any kind closes it.
    message_id: Option<String>,
    /// The `stop_reason` that ended the turn.
    stop_reason: String,
}

/// What one transcript line said about the turn in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnSignal {
    /// The assistant produced text but is not finished.
    Progress {
        /// The text of this record, for a live status line.
        text: String,
    },
    /// The assistant invoked a tool; it will continue.
    Tool {
        /// The tool's name.
        name: String,
    },
    /// The turn is over.
    Complete {
        /// Everything the assistant said this turn, oldest first.
        reply: String,
        /// The `stop_reason` that ended it.
        stop_reason: String,
    },
}

/// Folds transcript lines into turn-completion signals.
///
/// One watcher tracks one turn: construct it when a prompt is injected, feed it
/// every appended transcript line, and stop at the first
/// [`TurnSignal::Complete`].
#[derive(Debug, Clone)]
pub struct TurnWatcher {
    /// Which transcript dialect this watcher folds.
    provider: HarnessProvider,
    /// Assistant text seen so far this turn, oldest first.
    said: Vec<String>,
    /// Whether a tool call is outstanding — used only by the stall backstop, so
    /// a long build is never mistaken for a finished turn.
    tool_outstanding: bool,
    /// A terminal record seen, waiting for the rest of its message.
    pending: Option<PendingTerminal>,
    /// Whether the turn has already been settled.
    done: bool,
}

impl Default for TurnWatcher {
    fn default() -> Self {
        TurnWatcher::for_provider(HarnessProvider::Claude)
    }
}

impl TurnWatcher {
    /// A watcher for a freshly injected claude turn.
    pub fn new() -> Self {
        TurnWatcher::default()
    }

    /// A watcher for a freshly injected turn on `provider`.
    ///
    /// `opencode` has no rollout this module can read, so it folds nothing and
    /// settles only via the stall backstop — which is why it is not offered as a
    /// task target.
    pub fn for_provider(provider: HarnessProvider) -> Self {
        TurnWatcher {
            provider,
            said: Vec::new(),
            tool_outstanding: false,
            pending: None,
            done: false,
        }
    }

    /// Whether this turn has already completed.
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Whether a tool call is currently outstanding.
    pub fn tool_outstanding(&self) -> bool {
        self.tool_outstanding
    }

    /// Everything the assistant has said this turn.
    pub fn reply(&self) -> String {
        self.said.join("\n").trim().to_string()
    }

    /// Fold one raw transcript line.
    ///
    /// Returns `None` for a line that says nothing about this turn — a
    /// non-assistant record, a malformed line, or anything after completion.
    pub fn observe(&mut self, raw: &str) -> Option<TurnSignal> {
        if self.done {
            return None;
        }
        let record: Value = serde_json::from_str(raw).ok()?;
        match self.provider {
            HarnessProvider::Claude => self.observe_claude(&record),
            HarnessProvider::Codex => self.observe_codex(&record),
            // No rollout format to read.
            HarnessProvider::Opencode => None,
        }
    }

    /// Fold one codex rollout record.
    ///
    /// Simpler than claude's: the completion event states the turn is over *and*
    /// carries the final answer, so nothing has to be accumulated across records.
    fn observe_codex(&mut self, record: &Value) -> Option<TurnSignal> {
        if record.get("type").and_then(Value::as_str) != Some("event_msg") {
            return None;
        }
        let payload = record.get("payload")?;
        match payload.get("type").and_then(Value::as_str)? {
            "task_complete" => {
                self.done = true;
                self.tool_outstanding = false;
                // `last_agent_message` is authoritative; fall back to whatever
                // was streamed if a build ever omits it.
                let reply = payload
                    .get("last_agent_message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or_else(|| self.reply());
                Some(TurnSignal::Complete {
                    reply,
                    stop_reason: "task_complete".to_string(),
                })
            }
            "turn_aborted" => {
                self.done = true;
                self.tool_outstanding = false;
                Some(TurnSignal::Complete {
                    reply: self.reply(),
                    stop_reason: format!(
                        "aborted: {}",
                        payload
                            .get("reason")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                    ),
                })
            }
            "agent_message" => {
                let text = payload.get("message").and_then(Value::as_str)?.to_string();
                if text.is_empty() {
                    return None;
                }
                self.said.push(text.clone());
                Some(TurnSignal::Progress { text })
            }
            // A turn is running: keep the stall backstop from settling it.
            "task_started" => {
                self.tool_outstanding = true;
                None
            }
            _ => None,
        }
    }

    /// Fold one claude transcript record.
    fn observe_claude(&mut self, record: &Value) -> Option<TurnSignal> {
        // A terminal record does not end the turn on its own — the rest of its
        // message may still be unwritten, and the reply usually lives there. The
        // turn ends when something arrives that is not part of that message.
        if self.pending.is_some() && !self.continues_pending(record) {
            return Some(self.close_pending());
        }
        if record.get("type").and_then(Value::as_str) != Some("assistant") {
            return None;
        }
        // A sub-agent's records live in their own transcript, but guard anyway:
        // if that ever changes, a sub-agent's `end_turn` must not settle ours.
        if record.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return None;
        }
        let message = record.get("message")?;
        let blocks = message.get("content").and_then(Value::as_array);

        let mut text = String::new();
        let mut tool: Option<String> = None;
        for block in blocks.into_iter().flatten() {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(chunk) = block.get("text").and_then(Value::as_str) {
                        if !chunk.is_empty() {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(chunk);
                        }
                    }
                }
                Some("tool_use") => {
                    tool = Some(
                        block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_string(),
                    );
                }
                // `thinking` is deliberately not part of the reply: it is the
                // model's scratch work, not its answer to the peer.
                _ => {}
            }
        }
        if !text.is_empty() {
            self.said.push(text.clone());
        }

        match message.get("stop_reason").and_then(Value::as_str) {
            // Still working. The outstanding-tool flag is what keeps the stall
            // backstop from settling a turn that is running a long build.
            Some(CONTINUES) => {
                self.tool_outstanding = true;
                Some(match tool {
                    Some(name) => TurnSignal::Tool { name },
                    None => TurnSignal::Progress { text },
                })
            }
            // Any other *stated* reason is terminal: `end_turn` normally,
            // `stop_sequence` occasionally. Hold it until the message closes, so
            // the text blocks still to come are part of the reply.
            Some(reason) => {
                self.tool_outstanding = false;
                self.pending = Some(PendingTerminal {
                    message_id: message
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    stop_reason: reason.to_string(),
                });
                (!text.is_empty()).then_some(TurnSignal::Progress { text })
            }
            // No stated reason (0.08% of records). Not terminal — say nothing
            // and let the stall backstop decide, rather than guessing either way.
            None => {
                self.tool_outstanding = false;
                (!text.is_empty()).then_some(TurnSignal::Progress { text })
            }
        }
    }

    /// Whether `record` is another block of the pending terminal message.
    ///
    /// Requires a *stated* matching id: a record with no `message.id` — every
    /// `system`, `user`, and attachment record — is not a continuation, so the
    /// message closes on the next line rather than absorbing unrelated ones.
    fn continues_pending(&self, record: &Value) -> bool {
        let Some(id) = self.pending.as_ref().and_then(|p| p.message_id.as_deref()) else {
            return false;
        };
        record.get("type").and_then(Value::as_str) == Some("assistant")
            && record
                .get("message")
                .and_then(|m| m.get("id"))
                .and_then(Value::as_str)
                == Some(id)
    }

    /// Settle the held terminal, replying with everything the message said.
    fn close_pending(&mut self) -> TurnSignal {
        let stop_reason = self
            .pending
            .take()
            .map(|p| p.stop_reason)
            .unwrap_or_else(|| "end_turn".to_string());
        self.done = true;
        self.tool_outstanding = false;
        TurnSignal::Complete {
            reply: self.reply(),
            stop_reason,
        }
    }

    /// Whether a terminal record is held pending the rest of its message.
    ///
    /// The turn is over; only the reply text may still be incomplete. A caller
    /// that sees this stay true across a short quiet period should
    /// [`settle_pending`](Self::settle_pending) — nothing more is coming.
    pub fn terminal_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Close a held terminal without waiting for a following record.
    ///
    /// Returns `None` when nothing is pending.
    pub fn settle_pending(&mut self) -> Option<TurnSignal> {
        self.pending.is_some().then(|| self.close_pending())
    }

    /// Whether the turn should be given up on after `idle_ms` of silence.
    ///
    /// The backstop for a turn whose terminal record carried no `stop_reason`.
    /// It refuses while a tool is outstanding, because a long-running build is
    /// silence that means the opposite of "finished".
    pub fn stalled_for(&self, idle_ms: i64, budget_ms: i64) -> bool {
        !self.done && !self.tool_outstanding && idle_ms >= budget_ms
    }

    /// Settle the turn from the backstop rather than from a stated reason.
    pub fn settle_stalled(&mut self) -> TurnSignal {
        self.done = true;
        TurnSignal::Complete {
            reply: self.reply(),
            stop_reason: "stalled".to_string(),
        }
    }
}
