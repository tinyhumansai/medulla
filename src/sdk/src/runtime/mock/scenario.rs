//! The populated demo scenario: scripts a plausible roster, presence, a couple
//! of chat turns, and a completed delegated task so every tab has something to
//! render. Kept apart from [`super::types`] because it is scenario data rather
//! than reusable structure.

use std::collections::HashMap;

use serde_json::{json, Map};

use crate::runtime::{
    AgentDescriptor, AgentPresence, CycleResultSummary, PeerSession, TinyplaceIdentity,
};
use crate::ui::chat_store::ChatMessage;
use crate::ui::events::{TaskDigest, TuiEvent, Usage};

use super::types::{now_millis, MockRuntime};

impl MockRuntime {
    /// A populated runtime: a roster, presence, a couple of turns and a
    /// completed delegated task, for a lively demo.
    pub fn demo() -> Self {
        let rt = MockRuntime::empty();
        {
            let mut s = rt.state.lock().unwrap();
            let mut meta = Map::new();
            meta.insert("harness".into(), json!("tinyplace"));
            meta.insert("handle".into(), json!("@dev-1"));
            meta.insert("protocol".into(), json!("openhuman"));
            s.roster = vec![AgentDescriptor {
                id: "dev-1".into(),
                name: "dev-1".into(),
                description: "A remote coding agent for delegated implementation work.".into(),
                availability: "online".into(),
                tags: vec!["code".into()],
                metadata: meta,
            }];
            s.presence.insert(
                "dev-1".into(),
                AgentPresence {
                    online: true,
                    detail: Some("idle".into()),
                    at: now_millis(),
                },
            );
            s.tinyplace = Some(TinyplaceIdentity {
                agent_id: "cid-abc123".into(),
                public_key: "pk".into(),
                handle: Some("@medulla".into()),
            });
            s.tracing = true;

            s.emit(TuiEvent::CycleStart {
                cycle_id: "cyc-1".into(),
            });
            s.emit(TuiEvent::User {
                body: "Summarize the repo and delegate a refactor.".into(),
            });
            s.active_mut().messages.push(ChatMessage {
                role: "user".into(),
                content: "Summarize the repo and delegate a refactor.".into(),
            });
            s.emit(TuiEvent::InferenceEnd {
                tier: "orchestrator".into(),
                op: "orchestrate".into(),
                model: Some("gpt-4o".into()),
                duration_ms: 820,
                usage: Some(Usage {
                    input_tokens: 1200,
                    output_tokens: 90,
                }),
                content: None,
                reasoning: None,
                tool_calls: None,
            });
            s.emit(TuiEvent::TaskStart {
                task_id: "task-1".into(),
                instruction: "Refactor the auth module for clarity.".into(),
                depth: 2,
                agent_id: Some("dev-1".into()),
            });
            s.emit(TuiEvent::TaskEvent {
                task_id: "task-1".into(),
                event_kind: "text".into(),
                content: "Reading auth module…".into(),
                harness: Some("CODEX".into()),
            });
            s.emit(TuiEvent::TaskComplete {
                digest: TaskDigest {
                    task_id: "task-1".into(),
                    status: "done".into(),
                    digest: "Refactored auth into 3 focused modules.".into(),
                    result_ref: None,
                    usage: Some(Usage {
                        input_tokens: 6400,
                        output_tokens: 420,
                    }),
                    depth: 2,
                },
            });
            let reply = "Done — I mapped the repo and delegated the auth refactor to dev-1.";
            s.emit(TuiEvent::Assistant { body: reply.into() });
            s.active_mut().messages.push(ChatMessage {
                role: "assistant".into(),
                content: reply.into(),
            });
            s.emit(TuiEvent::CycleEnd {
                cycle_id: "cyc-1".into(),
                pass_count: 2,
                duration_ms: 4200,
            });
            let mut ledger = HashMap::new();
            ledger.insert(
                "task-1".into(),
                TaskDigest {
                    task_id: "task-1".into(),
                    status: "done".into(),
                    digest: "Refactored auth into 3 focused modules.".into(),
                    result_ref: None,
                    usage: Some(Usage {
                        input_tokens: 6400,
                        output_tokens: 420,
                    }),
                    depth: 2,
                },
            );
            s.active_mut().last_result = Some(CycleResultSummary {
                pass_count: 2,
                task_ledger: ledger,
            });
            s.sessions.insert(
                "dev-1".into(),
                vec![PeerSession {
                    id: "sess-9".into(),
                    state: "idle".into(),
                    harness: Some("codex".into()),
                    last_seen_at: now_millis(),
                }],
            );
        }
        rt
    }
}
