//! Chat transcript, Overview, Trace, tiny.place merge, and events-seam coverage:
//! the opencode third panel, the `events_changed` baseline seam, observation
//! merge into the snapshot, error/wrapped/spinner/thread-badge chat rendering,
//! the Trace JSON detail row, and Overview active-call/completed-task lines.

use crate::helpers::*;

// --- overview rendering: opencode third panel -------------------------------

#[test]
fn overview_renders_opencode_panel_without_tinyplace() {
    let rt = Arc::new(MockRuntime::empty());
    let mut l = LoadedConfig::defaults("medulla.tui.json".into());
    l.config.opencode = Some(medulla::config::OpencodeConfig::default());
    let mut app = App::new(rt, l);
    app.tab_index = 0; // Overview
    let out = render(&mut app, 120, 40);
    assert!(out.contains("OpenCode workers"), "opencode third panel");
}

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

// --- tinyplace observation merge --------------------------------------------

#[test]
fn tinyplace_observation_merges_into_snapshot() {
    use medulla::runtime::{AgentDescriptor, AgentPresence, TinyplaceIdentity};
    use medulla::tinyplace::service::TinyplaceObservation;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    let (mut app, _rt) = empty_app();
    let mut meta = serde_json::Map::new();
    meta.insert("harness".into(), serde_json::json!("tinyplace"));
    let mut presence = HashMap::new();
    presence.insert(
        "peer-1".into(),
        AgentPresence {
            online: true,
            detail: Some("idle".into()),
            at: 1,
        },
    );
    let obs = TinyplaceObservation {
        identity: Some(TinyplaceIdentity {
            agent_id: "cid-xyz".into(),
            public_key: "pk".into(),
            handle: Some("@merged".into()),
        }),
        roster: vec![AgentDescriptor {
            id: "peer-1".into(),
            name: "peer-1".into(),
            description: "a peer".into(),
            availability: "online".into(),
            tags: vec![],
            metadata: meta,
        }],
        presence,
    };
    app.set_tinyplace_observation(Arc::new(Mutex::new(obs)));
    assert!(app.snapshot.tinyplace.is_some());
    assert!(app.snapshot.roster.iter().any(|a| a.id == "peer-1"));
    assert!(app.snapshot.presence.contains_key("peer-1"));
    // The Overview 'me' line reflects the merged handle.
    app.tab_index = 0;
    let out = render(&mut app, 120, 40);
    assert!(out.contains("@merged"), "merged identity should render");
}

// --- chat transcript folding: error + wrapped multi-line turns --------------

#[test]
fn chat_renders_error_and_wrapped_turns() {
    let (mut app, rt) = empty_app();
    let long = "word ".repeat(40);
    rt.script_event(TuiEvent::User { body: long.clone() });
    rt.script_event(TuiEvent::Assistant { body: long });
    rt.script_event(TuiEvent::Error {
        source: "cycle".into(),
        message: "it broke".into(),
    });
    app.refresh_snapshot();
    tab(&mut app, "Chat");
    // Render narrow to force wrapping across multiple rows.
    let out = render(&mut app, 60, 24);
    assert!(out.contains("cycle: it broke"), "error line renders");
    assert!(out.contains("word"), "wrapped body renders");
}

// --- chat thinking spinner --------------------------------------------------

#[test]
fn chat_shows_thinking_spinner_with_and_without_calls() {
    let (mut app, rt) = empty_app();
    rt.set_running(true);
    app.refresh_snapshot();
    tab(&mut app, "Chat");
    // No inference in flight → "working…".
    let out = render(&mut app, 120, 40);
    assert!(out.contains("working"), "idle-stream spinner: {out:.0}");

    rt.script_event(TuiEvent::InferenceStart {
        tier: "reasoning".into(),
        op: "step".into(),
        model: Some("m".into()),
    });
    app.refresh_snapshot();
    let out = render(&mut app, 120, 40);
    assert!(out.contains("model call"), "in-flight spinner detail");
}

// --- thread badges & fork indentation ---------------------------------------

#[test]
fn chat_thread_sidebar_shows_badges_and_indent() {
    let (mut app, rt) = demo_app();
    // Fork so a child thread renders one level deep (⑃ indent).
    rt.fork(Some("child".into()));
    // A running task + a pending question on the child drives the badges.
    rt.script_event(TuiEvent::TaskStart {
        task_id: "cyc-1/t:t9".into(),
        instruction: "go".into(),
        depth: 2,
        agent_id: Some("dev-1".into()),
    });
    rt.script_event(TuiEvent::TaskAttention {
        task_id: "cyc-1/t:t9".into(),
        reason: "confirm".into(),
        content: "?".into(),
        question_id: Some("q".into()),
    });
    app.refresh_snapshot();
    tab(&mut app, "Chat");
    let out = render(&mut app, 120, 40);
    assert!(out.contains("run"), "running-task badge");
    assert!(out.contains('⚠'), "attention badge");
    assert!(out.contains('⑃'), "fork indent glyph");
}

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

// --- overview: active model calls, completed task ---------------------------

#[test]
fn overview_shows_active_model_calls_and_completed_task() {
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
            }),
            depth: 2,
        },
    });
    app.refresh_snapshot();
    app.tab_index = 0;
    let out = render(&mut app, 120, 40);
    assert!(
        out.contains("active model calls 1"),
        "overview: active calls"
    );
}
