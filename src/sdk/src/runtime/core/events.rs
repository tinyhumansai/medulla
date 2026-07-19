//! Wire → view-model normalization for the core runtime: mapping a single core
//! event body onto the TUI's [`TuiEvent`] vocabulary (the §3.3 normalizations) and
//! folding a subscribe / `snapshot.get` snapshot into a replayable event log (§3.4).

use serde_json::Value;

use crate::ui::chat_store::now_millis;
use crate::ui::events::{EventEnvelope, TaskDigest, TuiEvent, Usage};

/// Compose the lane-unique task key from the envelope `cycleId` and the wire `taskId`
/// (§3.3(2)/§4.4), mirroring the library's `taskCycleId` and the core's `store.taskKey`.
pub(super) fn compose_task_id(cycle_id: &str, task_id: &str) -> String {
    if cycle_id.is_empty() {
        task_id.to_string()
    } else {
        format!("{cycle_id}/t:{task_id}")
    }
}

/// Read a non-empty string field `k` from JSON object `v`, or `None`.
pub(super) fn opt_str(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Map a core event body `{kind, ...}` onto the TUI's [`TuiEvent`], applying the §3.3
/// normalizations. `cycle_id` comes from the envelope (§3.2).
pub fn map_core_event(body: &Value, cycle_id: &str) -> TuiEvent {
    let kind = body.get("kind").and_then(Value::as_str).unwrap_or("");
    let s = |k: &str| {
        body.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let i = |k: &str| body.get(k).and_then(Value::as_i64).unwrap_or(0);
    match kind {
        "task_start" => TuiEvent::TaskStart {
            task_id: compose_task_id(cycle_id, &s("taskId")),
            instruction: s("instruction"),
            depth: i("depth"),
            agent_id: opt_str(body, "agentId"),
        },
        "task_event" => TuiEvent::TaskEvent {
            task_id: compose_task_id(cycle_id, &s("taskId")),
            event_kind: s("eventKind"),
            content: s("content"),
            harness: opt_str(body, "harness"),
        },
        "task_attention" => TuiEvent::TaskAttention {
            task_id: compose_task_id(cycle_id, &s("taskId")),
            reason: s("reason"),
            content: s("content"),
            question_id: opt_str(body, "questionId"),
        },
        "task_complete" => {
            // §3.3(1): the wire body is already flat — status/digest sit at the top
            // level, not under `digest`. Rebuild the TUI's nested `TaskDigest`.
            let status = {
                let raw = s("status");
                if raw.is_empty() {
                    "done".into()
                } else {
                    raw
                }
            };
            let usage = body
                .get("usage")
                .and_then(|u| serde_json::from_value::<Usage>(u.clone()).ok());
            TuiEvent::TaskComplete {
                digest: TaskDigest {
                    task_id: compose_task_id(cycle_id, &s("taskId")),
                    status,
                    digest: s("digest"),
                    result_ref: body.get("resultRef").cloned(),
                    usage,
                    depth: i("depth"),
                },
            }
        }
        // Everything else deserializes straight through the TuiEvent vocabulary; an
        // unknown kind rides through as `Unknown` rather than being dropped.
        _ => {
            serde_json::from_value::<TuiEvent>(body.clone()).unwrap_or_else(|_| TuiEvent::Unknown {
                kind: kind.to_string(),
                data: body.as_object().cloned().unwrap_or_default(),
            })
        }
    }
}

/// Fold a subscribe / `snapshot.get` snapshot's `{tasks[], chat[]}` into a replayable
/// event log (§3.4). Each folded task becomes a `task_start` (+ a `task_event` from
/// its `lastEvent`, + a `task_complete` when terminal); each chat entry a user /
/// assistant turn. The synthetic seqs start at `*seq` and advance it.
pub(super) fn synth_from_snapshot(snapshot: &Value, seq: &mut u64) -> Vec<EventEnvelope> {
    let mut out = Vec::new();
    let at = snapshot
        .get("at")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_millis);
    let mut push = |seq: &mut u64, event: TuiEvent| {
        *seq += 1;
        out.push(EventEnvelope {
            seq: *seq,
            at,
            event,
        });
    };
    if let Some(chat) = snapshot.get("chat").and_then(Value::as_array) {
        for c in chat {
            let body = c
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            match c.get("role").and_then(Value::as_str) {
                Some("user") => push(seq, TuiEvent::User { body }),
                Some("assistant") => push(seq, TuiEvent::Assistant { body }),
                _ => {}
            }
        }
    }
    if let Some(tasks) = snapshot.get("tasks").and_then(Value::as_array) {
        for t in tasks {
            let cycle_id = t.get("cycleId").and_then(Value::as_str).unwrap_or("");
            let task_id = compose_task_id(
                cycle_id,
                t.get("taskId").and_then(Value::as_str).unwrap_or(""),
            );
            push(
                seq,
                TuiEvent::TaskStart {
                    task_id: task_id.clone(),
                    instruction: t
                        .get("instruction")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    depth: t.get("depth").and_then(Value::as_i64).unwrap_or(0),
                    agent_id: opt_str(t, "agentId"),
                },
            );
            if let Some(le) = t.get("lastEvent") {
                push(
                    seq,
                    TuiEvent::TaskEvent {
                        task_id: task_id.clone(),
                        event_kind: le
                            .get("eventKind")
                            .and_then(Value::as_str)
                            .unwrap_or("status")
                            .to_string(),
                        content: le
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        harness: opt_str(t, "harness"),
                    },
                );
            }
            let status = t.get("status").and_then(Value::as_str).unwrap_or("running");
            if status != "running" {
                let usage = t
                    .get("usage")
                    .and_then(|u| serde_json::from_value::<Usage>(u.clone()).ok());
                push(
                    seq,
                    TuiEvent::TaskComplete {
                        digest: TaskDigest {
                            task_id,
                            status: status.to_string(),
                            digest: t
                                .get("digest")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            result_ref: None,
                            usage,
                            depth: t.get("depth").and_then(Value::as_i64).unwrap_or(0),
                        },
                    },
                );
            }
        }
    }
    out
}
