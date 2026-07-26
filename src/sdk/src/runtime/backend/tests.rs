//! Unit tests for the backend runtime's local fold: event mapping, the
//! optimistic-echo de-duplication, the cycle bracket, log caps, the
//! session-summary mapper, and the hub roster row mapper.

use serde_json::{json, Value};

use crate::client::{EventEnvelope as ClientEnvelope, SessionSummary};
use crate::ui::events::TuiEvent;

use super::fold::summary_from_session;
use super::types::{State, Thread, CHAT_CAP, EVENT_CAP};
use super::worker_ops::{hub_worker_to_info, label_from_patch};
use crate::hub::HubWorker;
use crate::tinyplace::WorkerSystemInfo;

fn client_env(session: &str, seq: Option<u64>, event: Value) -> ClientEnvelope {
    let mut raw = json!({
        "at": 1234u64,
        "sessionId": session,
        "event": event,
    });
    if let Some(seq) = seq {
        raw["seq"] = json!(seq);
    }
    serde_json::from_value(raw).unwrap()
}

fn state_with_thread() -> State {
    State {
        threads: vec![Thread::new("t1", "main", "sess-1".into())],
        active_id: "t1".into(),
        seq: 0,
        next_thread: 2,
        roster: Vec::new(),
        capacity: Default::default(),
    }
}

#[test]
fn folds_kinds_into_tui_events() {
    let mut s = state_with_thread();
    s.fold(
        "sess-1",
        &client_env("sess-1", Some(1), json!({"kind":"user","body":"hi"})),
    );
    s.fold(
        "sess-1",
        &client_env("sess-1", Some(2), json!({"kind":"assistant","body":"yo"})),
    );
    s.fold(
        "sess-1",
        &client_env(
            "sess-1",
            None,
            json!({"kind":"assistant_delta","delta":"y"}),
        ),
    );
    s.fold(
        "sess-1",
        &client_env(
            "sess-1",
            None,
            json!({"kind":"reasoning_delta","delta":"r"}),
        ),
    );
    s.fold(
        "sess-1",
        &client_env(
            "sess-1",
            None,
            json!({"kind":"tool_call_delta","index":2,"argsDelta":"{"}),
        ),
    );
    s.fold(
        "sess-1",
        &client_env("sess-1", Some(3), json!({"kind":"weird","x":1})),
    );

    let t = &s.threads[0];
    let kinds: Vec<&str> = t.events.iter().map(|e| e.event.kind()).collect();
    assert_eq!(
        kinds,
        vec![
            "user",
            "assistant",
            "assistant_delta",
            "reasoning_delta",
            "tool_call_delta",
            "weird"
        ]
    );
    // Chat events are the user/assistant/error subset only.
    let chat: Vec<&str> = t.chat_events.iter().map(|e| e.event.kind()).collect();
    assert_eq!(chat, vec!["user", "assistant"]);
    // Messages track user + assistant turns.
    assert_eq!(t.messages.len(), 2);
    // The tool-call delta carried its fields through.
    match &t.events[4].event {
        TuiEvent::ToolCallDelta { index, args_delta } => {
            assert_eq!(*index, 2);
            assert_eq!(args_delta, "{");
        }
        other => panic!("expected tool_call_delta, got {other:?}"),
    }
    // Envelope `at` came from the client envelope.
    assert_eq!(t.events[0].at, 1234);
}

#[test]
fn cycle_bracket_drives_running_and_last_result() {
    let mut s = state_with_thread();
    s.threads[0].running = true;
    s.fold(
        "sess-1",
        &client_env(
            "sess-1",
            Some(1),
            json!({"kind":"cycle_start","cycleId":"c1"}),
        ),
    );
    assert!(s.threads[0].running);
    s.fold(
        "sess-1",
        &client_env(
            "sess-1",
            Some(2),
            json!({"kind":"cycle_end","cycleId":"c1","passCount":3,"durationMs":42}),
        ),
    );
    assert!(!s.threads[0].running);
    let lr = s.threads[0].last_result.as_ref().unwrap();
    assert_eq!(lr.pass_count, 3);
}

#[test]
fn optimistic_user_echo_is_deduped() {
    let mut s = state_with_thread();
    s.push_local_user("t1", "hello", 10);
    assert_eq!(s.threads[0].events.len(), 1);
    assert!(s.threads[0].running);
    // The stream echoes the same user turn — it must not duplicate.
    let folded = s.fold(
        "sess-1",
        &client_env("sess-1", Some(1), json!({"kind":"user","body":"hello"})),
    );
    assert!(folded.is_none());
    assert_eq!(s.threads[0].events.len(), 1);
    assert_eq!(s.threads[0].messages.len(), 1);
    // A different user turn is not swallowed.
    let folded = s.fold(
        "sess-1",
        &client_env("sess-1", Some(2), json!({"kind":"user","body":"world"})),
    );
    assert!(folded.is_some());
    assert_eq!(s.threads[0].messages.len(), 2);
}

#[test]
fn event_logs_are_capped() {
    let mut s = state_with_thread();
    for i in 0..(EVENT_CAP + 200) {
        let body = format!("m{i}");
        // Alternate a chatty and a non-chatty event to exercise both caps.
        if i % 2 == 0 {
            s.fold(
                "sess-1",
                &client_env(
                    "sess-1",
                    Some(i as u64),
                    json!({"kind":"assistant","body":body}),
                ),
            );
        } else {
            s.fold(
                "sess-1",
                &client_env(
                    "sess-1",
                    None,
                    json!({"kind":"assistant_delta","delta":body}),
                ),
            );
        }
    }
    let t = &s.threads[0];
    assert_eq!(t.events.len(), EVENT_CAP);
    assert!(t.chat_events.len() <= CHAT_CAP);
}

#[test]
fn session_summary_maps_to_main_chat() {
    let s: SessionSummary = serde_json::from_value(json!({
        "sessionId": "sess-9",
        "title": "Auth refactor",
        "lastActiveAt": 1_700_000_000_000i64,
        "status": "active",
        "lastSeq": 7,
    }))
    .unwrap();
    let row = summary_from_session(&s);
    assert_eq!(row.session_id, "sess-9");
    assert_eq!(row.name, "Auth refactor");
    assert_eq!(row.turns, 3); // 7 / 2
    assert_eq!(row.thread_count, 1);
    assert_eq!(row.updated_at, "2023-11-14T22:13:20.000Z");
}

#[test]
fn session_summary_falls_back_to_id_for_name() {
    let s: SessionSummary = serde_json::from_value(json!({
        "sessionId": "sess-bare",
        "status": "idle",
    }))
    .unwrap();
    let row = summary_from_session(&s);
    assert_eq!(row.name, "sess-bare");
    assert_eq!(row.turns, 0);
}

fn hub_worker(address: &str) -> HubWorker {
    HubWorker {
        id: "w1".to_string(),
        address: address.to_string(),
        harness: "claude".to_string(),
        label: Some("builder".to_string()),
        selected: false,
        workspace: None,
    }
}

#[test]
fn a_plain_address_maps_to_a_row_without_a_handle() {
    let info = hub_worker_to_info(hub_worker("GRV1worker"), None);

    assert_eq!(info.id, "w1");
    assert_eq!(info.address, "GRV1worker");
    // A base58 address is not a handle, so the handle column stays empty.
    assert_eq!(info.handle, None);
    assert_eq!(info.label.as_deref(), Some("builder"));
    assert_eq!(info.harness.as_deref(), Some("claude"));
    assert_eq!(info.peer_id, None);
    assert!(!info.selected);
}

#[test]
fn an_at_handle_address_is_surfaced_as_the_handle_too() {
    // `@name` addresses are shown in both columns so the roster reads naturally.
    let info = hub_worker_to_info(hub_worker("@builder"), None);

    assert_eq!(info.address, "@builder");
    assert_eq!(info.handle.as_deref(), Some("@builder"));
}

#[test]
fn selection_carries_through_to_the_row() {
    let mut w = hub_worker("GRV1worker");
    w.selected = true;
    w.label = None;

    let info = hub_worker_to_info(w, None);
    assert!(info.selected);
    assert_eq!(info.label, None);
}

#[test]
fn captured_worker_details_map_to_the_runtime_row() {
    let details = WorkerSystemInfo {
        cpu_cores: 12,
        memory_total_bytes: Some(32 * 1024 * 1024 * 1024),
        memory_available_bytes: Some(19 * 1024 * 1024 * 1024),
        ip_address: "10.0.0.24".to_string(),
    };

    let info = hub_worker_to_info(hub_worker("GRV1worker"), Some(details));

    assert_eq!(info.cpu_cores, Some(12));
    assert_eq!(info.memory_total_bytes, Some(32 * 1024 * 1024 * 1024));
    assert_eq!(info.memory_available_bytes, Some(19 * 1024 * 1024 * 1024));
    assert_eq!(info.ip_address.as_deref(), Some("10.0.0.24"));
}

#[test]
fn worker_label_patches_require_the_supported_string_field() {
    let label = json!({"label": "builder"});
    assert_eq!(
        label_from_patch(label.as_object().unwrap()).unwrap(),
        Some("builder".into())
    );
    let clear = json!({"label": ""});
    assert_eq!(label_from_patch(clear.as_object().unwrap()).unwrap(), None);
    for patch in [json!({}), json!({"other": "value"}), json!({"label": 3})] {
        assert_eq!(
            label_from_patch(patch.as_object().unwrap())
                .unwrap_err()
                .to_string(),
            "worker update requires a string label"
        );
    }
}

// --- roster → capacity projection -------------------------------------------

/// A roster row as the backend sends it, with only the fields a test cares about.
fn roster_worker(patch: Value) -> crate::client::RosterWorker {
    let mut raw = json!({
        "registryId": "w-1",
        "label": "dev-1",
        "description": "A remote coding agent.",
        "availability": "online",
        "selected": false,
    });
    let Value::Object(patch) = patch else {
        panic!("patch must be an object")
    };
    for (k, v) in patch {
        raw[k] = v;
    }
    serde_json::from_value(raw).unwrap()
}

#[test]
fn projects_a_worker_onto_host_harness_and_agent() {
    let (agents, capacity) = super::fleet::project_roster(&[roster_worker(json!({
        "harness": "claude-code",
        "cpuCores": 8,
        "totalMemBytes": 32u64 << 30,
        "availableMemBytes": 12u64 << 30,
        "primaryIpv4": "10.0.0.4",
        "tags": ["code"],
    }))]);

    assert_eq!(capacity.hosts.len(), 1);
    assert_eq!(capacity.harnesses.len(), 1);
    let host = &capacity.hosts[0];
    assert_eq!(host.name, "dev-1");
    assert_eq!(host.address.as_deref(), Some("10.0.0.4"));
    assert_eq!(host.resources.as_ref().unwrap().cpu_cores, Some(8.0));

    let harness = &capacity.harnesses[0];
    assert_eq!(harness.host_id, host.id);
    assert_eq!(harness.kind, "claude-code");
    assert!(harness.ready);

    // No probed cwd, so the agent is placed on its host directly rather than
    // pointing at a workspace nothing declares.
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].id, "w-1");
    assert_eq!(agents[0].workspace_id, None);
    assert_eq!(agents[0].host_id.as_deref(), Some(host.id.as_str()));
    assert_eq!(agents[0].tags, vec!["code".to_string()]);
}

#[test]
fn synthesizes_a_workspace_from_a_probed_cwd() {
    let (agents, capacity) = super::fleet::project_roster(&[roster_worker(json!({
        "harness": "codex",
        "capabilities": { "cwd": "/srv/repos/medulla", "project": "medulla", "branch": "main" },
    }))]);

    assert_eq!(capacity.workspaces.len(), 1);
    let workspace = &capacity.workspaces[0];
    assert_eq!(workspace.path, "/srv/repos/medulla");
    assert_eq!(workspace.project.as_deref(), Some("medulla"));
    assert_eq!(workspace.harness_id, capacity.harnesses[0].id);

    // With a workspace to walk up from, the agent must not also claim a host.
    assert_eq!(
        agents[0].workspace_id.as_deref(),
        Some(workspace.id.as_str())
    );
    assert_eq!(agents[0].host_id, None);
    let placement = capacity.placement(&agents[0]);
    assert_eq!(placement.host, Some(&capacity.hosts[0]));
    assert_eq!(placement.harness, Some(&capacity.harnesses[0]));
}

#[test]
fn every_accessible_directory_becomes_a_workspace_not_just_the_cwd() {
    // A machine with three repositories on it has three places work could go,
    // and the orchestrator can only route by what is declared. Before this the
    // `accessibleDirs` the hub forwards reached the backend and stopped there.
    let (agents, capacity) = super::fleet::project_roster(&[roster_worker(json!({
        "harness": "claude",
        "capabilities": {
            "cwd": "/srv/repos/medulla",
            "project": "medulla",
            "accessibleDirs": [
                "/srv/repos/medulla",
                "/srv/repos/backend",
                "  ",
                "/srv/repos/backend",
                "/srv/repos/infra",
            ],
        },
    }))]);

    let paths: Vec<&str> = capacity
        .workspaces
        .iter()
        .map(|w| w.path.as_str())
        .collect();
    // The cwd leads and is not repeated; blanks and duplicates are dropped.
    assert_eq!(
        paths,
        vec![
            "/srv/repos/medulla",
            "/srv/repos/backend",
            "/srv/repos/infra"
        ]
    );
    assert!(capacity
        .workspaces
        .iter()
        .all(|w| w.harness_id == capacity.harnesses[0].id));

    // Only the cwd backs the agent's placement: that is where it actually runs.
    assert_eq!(
        agents[0].workspace_id.as_deref(),
        Some(capacity.workspaces[0].id.as_str())
    );
    // The probe reports one `project`, describing the cwd. Claiming it for a
    // sibling directory would be an invention.
    assert_eq!(capacity.workspaces[0].project.as_deref(), Some("medulla"));
    assert!(capacity.workspaces[1].project.is_none());
    assert_eq!(capacity.workspaces[1].name, "backend");
    // Ids stay distinct, or the chain would resolve two folders to one node.
    assert_eq!(
        capacity
            .workspaces
            .iter()
            .map(|w| w.id.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3
    );
}

#[test]
fn a_malformed_accessible_dirs_value_yields_no_extra_workspaces() {
    // The probe is self-reported by a harness: untrusted input, not a contract.
    let (_, capacity) = super::fleet::project_roster(&[roster_worker(json!({
        "capabilities": { "cwd": "/srv/repo", "accessibleDirs": "not-an-array" },
    }))]);
    assert_eq!(capacity.workspaces.len(), 1);
    assert_eq!(capacity.workspaces[0].path, "/srv/repo");
}

#[test]
fn accessible_directories_are_declared_even_without_a_probed_cwd() {
    // No cwd means no placement to walk up from, but the directories are still
    // real capacity the orchestrator should be able to see.
    let (agents, capacity) = super::fleet::project_roster(&[roster_worker(json!({
        "capabilities": { "accessibleDirs": ["/srv/repos/one"] },
    }))]);
    assert_eq!(capacity.workspaces.len(), 1);
    assert_eq!(capacity.workspaces[0].path, "/srv/repos/one");
    // The agent still hangs off the host directly.
    assert!(agents[0].workspace_id.is_none());
    assert!(agents[0].host_id.is_some());
}

#[test]
fn carries_budgets_onto_the_harness_and_gates_readiness_on_availability() {
    let (_, capacity) = super::fleet::project_roster(&[roster_worker(json!({
        "availability": "offline",
        "budgets": [{
            "provider": "anthropic",
            "window": "5h",
            "remainingTokens": 250_000u64,
            "limitTokens": 1_000_000u64,
            "source": "provider_reported",
        }],
    }))]);

    let harness = &capacity.harnesses[0];
    assert!(!harness.ready, "an offline worker takes no work");
    assert_eq!(harness.providers, vec!["anthropic".to_string()]);
    assert_eq!(harness.budgets[0].remaining(), Some(250_000));
    assert_eq!(harness.budgets[0].fraction_remaining(), Some(0.25));
}

#[test]
fn keeps_look_alike_workers_as_separate_hosts() {
    let (agents, capacity) = super::fleet::project_roster(&[
        roster_worker(json!({ "registryId": "w-1" })),
        roster_worker(json!({ "registryId": "w-2" })),
    ]);
    assert_eq!(agents.len(), 2);
    assert_eq!(capacity.hosts.len(), 2);
    assert_ne!(capacity.hosts[0].id, capacity.hosts[1].id);
}
