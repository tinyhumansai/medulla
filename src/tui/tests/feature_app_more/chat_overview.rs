//! Chat transcript, Trace, host-link merge, and events-seam coverage:
//! the `events_changed` baseline seam, observation
//! merge into the snapshot, error/wrapped/spinner/thread-badge chat rendering,
//! the Trace JSON detail row, and the Subconscious active-signal count.

use crate::helpers::*;

// --- events_changed seam ----------------------------------------------------

#[test]
fn events_changed_flips_then_settles() {
    let (mut app, rt) = empty_app();
    // First call records the baseline (0 events) → no change reported.
    assert!(!app.events_changed());
    rt.script_event(TuiEvent::Assistant { body: "x".into() });
    app.refresh_snapshot();
    assert!(app.events_changed(), "a new event is a change");
    assert!(!app.events_changed(), "same length settles");
}

// --- host-link observation merge --------------------------------------------

#[test]
fn link_observation_merges_into_snapshot() {
    use medulla::protocol::service::LinkObservation;
    use medulla::runtime::{AgentDescriptor, AgentPresence, LinkIdentity};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    let (mut app, _rt) = empty_app();
    let mut meta = serde_json::Map::new();
    meta.insert("harness".into(), serde_json::json!("link"));
    let mut presence = HashMap::new();
    presence.insert(
        "peer-1".into(),
        AgentPresence {
            online: true,
            detail: Some("idle".into()),
            at: 1,
        },
    );
    let obs = LinkObservation {
        notice: None,
        identity: Some(LinkIdentity {
            node_name: "merged-host".into(),
            forwarder: "forwarder:41641".into(),
        }),
        roster: vec![AgentDescriptor {
            id: "peer-1".into(),
            name: "peer-1".into(),
            description: "a peer".into(),
            availability: "online".into(),
            workspace_id: None,
            host_id: None,
            template_id: None,
            tags: vec![],
            metadata: meta,
        }],
        presence,
    };
    app.set_link_observation(Arc::new(Mutex::new(obs)));
    assert!(app.snapshot.link.is_some());
    assert!(app.snapshot.roster.iter().any(|a| a.id == "peer-1"));
    assert!(app.snapshot.presence.contains_key("peer-1"));
}

// --- thread badges & fork indentation ---------------------------------------

// --- Trace tab renders the JSON detail row ----------------------------------

#[test]
fn trace_tab_renders_event_and_json() {
    use medulla_tui::ui::events::NodeTrace;
    let (mut app, rt) = empty_app();
    rt.script_event(TuiEvent::Trace {
        entry: NodeTrace {
            node: "orchestrate".into(),
            ms: 42,
            tool: None,
            op: Some("decide".into()),
        },
    });
    app.refresh_snapshot();
    tab(&mut app, "Trace");
    let out = render(&mut app, 120, 40);
    assert!(out.contains("Trace ·"), "trace header");
    assert!(out.contains("orchestrate"), "trace json detail row");
}

// --- subconscious: active model calls, completed task -----------------------

#[test]
fn subconscious_shows_active_model_calls() {
    let (mut app, rt) = empty_app();
    rt.script_event(TuiEvent::InferenceStart {
        tier: "reasoning".into(),
        op: "step".into(),
        model: Some("m".into()),
    });
    rt.script_event(TuiEvent::TaskComplete {
        digest: TaskDigest {
            task_id: "t1".into(),
            status: "done".into(),
            digest: "d".into(),
            result_ref: None,
            usage: Some(Usage {
                input_tokens: 10,
                output_tokens: 2,
                ..Default::default()
            }),
            depth: 2,
            contract: None,
            evidence: None,
        },
    });
    app.refresh_snapshot();
    app.tab_index = TABS
        .iter()
        .position(|tab| *tab == "Subconscious")
        .expect("Subconscious tab is listed");
    let out = render(&mut app, 120, 40);
    assert!(
        out.contains("1 active pulses"),
        "subconscious: active calls"
    );
}
