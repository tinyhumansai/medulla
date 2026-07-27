//! The populated demo scenario: scripts a plausible roster, presence, a couple
//! of chat turns, and a completed delegated task so every tab has something to
//! render. Kept apart from [`super::types`] because it is scenario data rather
//! than reusable structure.

use std::collections::HashMap;

use serde_json::{json, Map};

use crate::runtime::fleet::{
    AgentTemplate, CapacitySnapshot, HarnessBudget, HarnessDescriptor, HostDescriptor,
    HostResources, WorkspaceDescriptor,
};
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
                // Placed in the demo chain below, so the Fleet page shows the
                // agent under the workspace it is deployed into.
                workspace_id: Some("ws-1".into()),
                host_id: None,
                template_id: Some("implementer".into()),
                tags: vec!["code".into()],
                metadata: meta,
            }];
            s.capacity = demo_capacity();
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
                    // A provider that reports its cache, so the demo exercises
                    // the context meter's cache segment.
                    cache_read_tokens: Some(840),
                    cache_creation_tokens: None,
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
                contract: None,
            });
            s.emit(TuiEvent::TaskEvent {
                task_id: "task-1".into(),
                event_kind: "text".into(),
                content: "Reading auth module…".into(),
                harness: Some("CODEX".into()),
            });
            // The worker's own screen, as this one now reads it: what it plans
            // to do, what it has ticked off, who it delegated to, and what it
            // rewrote. Without these the demo shows a transcript and nothing
            // about the work behind it.
            for (kind, payload) in demo_work_events() {
                s.emit(TuiEvent::TaskEvent {
                    task_id: "task-1".into(),
                    event_kind: kind.into(),
                    content: payload.to_string(),
                    harness: Some("CODEX".into()),
                });
            }
            s.emit(TuiEvent::TaskComplete {
                digest: TaskDigest {
                    task_id: "task-1".into(),
                    status: "done".into(),
                    digest: "Refactored auth into 3 focused modules.".into(),
                    result_ref: None,
                    usage: Some(Usage {
                        input_tokens: 6400,
                        output_tokens: 420,
                        cache_read_tokens: Some(5_100),
                        cache_creation_tokens: None,
                    }),
                    depth: 2,
                    contract: None,
                    evidence: None,
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
                        cache_read_tokens: Some(5_100),
                        cache_creation_tokens: None,
                    }),
                    depth: 2,
                    contract: None,
                    evidence: None,
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

/// The demo containment chain: one host running one harness that exposes one
/// workspace, plus a template that may be provisioned onto it. Deliberately
/// small — it exists so every fleet surface has a shape to render, not to
/// exercise the caps.
fn demo_capacity() -> CapacitySnapshot {
    CapacitySnapshot {
        hosts: vec![HostDescriptor {
            id: "host-1".into(),
            name: "workshop".into(),
            availability: "online".into(),
            address: Some("10.0.0.4".into()),
            resources: Some(HostResources {
                cpu_cores: Some(10.0),
                total_memory_bytes: Some(32 * 1024 * 1024 * 1024),
                available_memory_bytes: Some(12 * 1024 * 1024 * 1024),
                disk_free_bytes: Some(220 * 1024 * 1024 * 1024),
                load_average_1m: Some(2.1),
            }),
            metadata: Map::new(),
        }],
        harnesses: vec![HarnessDescriptor {
            id: "harness-1".into(),
            host_id: "host-1".into(),
            kind: "claude-code".into(),
            availability: "online".into(),
            ready: true,
            ready_reason: None,
            providers: vec!["anthropic".into()],
            template_ids: vec!["implementer".into()],
            budgets: vec![HarnessBudget {
                provider: "anthropic".into(),
                window: "5h".into(),
                seat: Some("seat-1".into()),
                limit_tokens: Some(1_000_000),
                used_tokens: Some(240_000),
                remaining_tokens: Some(760_000),
                cooldown_until: None,
                source: "provider_reported".into(),
            }],
            metadata: Map::new(),
        }],
        workspaces: vec![WorkspaceDescriptor {
            id: "ws-1".into(),
            name: "medulla".into(),
            path: "/srv/repos/medulla".into(),
            harness_id: "harness-1".into(),
            profile: None,
            project: Some("medulla".into()),
            template_ids: Vec::new(),
            metadata: Map::new(),
        }],
        templates: vec![AgentTemplate {
            id: "implementer".into(),
            name: Some("Implementer".into()),
            description: "Implements a scoped change and reports what it did.".into(),
            instructions: None,
            tools: None,
            model: Some("reasoning".into()),
            effort: None,
            params: Map::new(),
            tags: vec!["code".into()],
            metadata: Map::new(),
            harnesses: Default::default(),
        }],
    }
}

/// The structured work a demo worker reports: a plan, a half-finished todo list,
/// a sub-agent still running, and the files it has rewritten.
fn demo_work_events() -> Vec<(&'static str, serde_json::Value)> {
    use crate::harness_work::kinds;
    vec![
        (
            kinds::SESSION_INFO,
            json!({ "model": "gpt-5-codex", "cwd": "/srv/repos/auth", "tools": ["shell", "apply_patch", "web_search"] }),
        ),
        (
            kinds::PLAN_UPDATE,
            json!({ "goal": "Split auth into focused modules" }),
        ),
        (
            kinds::TODO_UPDATE,
            json!({ "todos": [
                { "content": "map the auth surface", "status": "completed" },
                { "content": "extract the token store", "status": "completed" },
                { "content": "split the session guard", "status": "in_progress", "activeForm": "Splitting the session guard" },
                { "content": "update the callers", "status": "pending" },
            ]}),
        ),
        (
            kinds::SUBAGENT_START,
            json!({ "call_id": "sub-1", "agent_type": "code-reviewer", "description": "review the extracted token store" }),
        ),
        (
            kinds::FILE_CHANGE,
            json!({ "path": "src/auth/tokens.rs", "change": "added" }),
        ),
        (
            kinds::FILE_CHANGE,
            json!({ "path": "src/auth/mod.rs", "change": "modified" }),
        ),
    ]
}
