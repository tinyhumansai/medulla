//! Unit tests for the Agents-view row helpers: the presence glyph, the backing
//! session lookup, and the human-readable state suffix.
//!
//! These are pure functions over the snapshot, so they are pinned directly here
//! rather than through a rendered buffer — the glyph is the only signal the user
//! has for "is this worker alive", and each branch means something different.

use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::{AgentPresence, PeerSession, Runtime};
use medulla::ui::agents::{AgentLane, AgentRole};

use crate::ui::app::App;

fn app() -> App {
    let rt: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    App::new(rt, LoadedConfig::defaults("medulla.tui.json".into()))
}

#[test]
fn compact_tab_labels_shorten_the_current_wide_destinations() {
    assert_eq!(super::compact_tab_label("Workflows", true), "Flows");
    assert_eq!(super::compact_tab_label("Changes", true), "Diff");
    assert_eq!(super::compact_tab_label("Feedback", true), "Feed");
    assert_eq!(super::compact_tab_label("Settings", true), "Set");
    assert_eq!(super::compact_tab_label("Hosts", true), "Hosts");
    assert_eq!(super::compact_tab_label("Changes", false), "Changes");
}

#[test]
fn harness_choice_window_keeps_the_selection_visible() {
    assert_eq!(
        super::session_modals::harness_choice_window(20, 0, 13),
        0..13
    );
    assert_eq!(
        super::session_modals::harness_choice_window(20, 10, 13),
        4..17
    );
    assert_eq!(
        super::session_modals::harness_choice_window(20, 19, 13),
        7..20
    );
    assert_eq!(super::session_modals::harness_choice_window(2, 1, 13), 0..2);
}

fn lane(role: AgentRole) -> AgentLane {
    AgentLane {
        key: "k".into(),
        label: "worker".into(),
        role,
        turns: Vec::new(),
        last_at: 0,
        tasks: Vec::new(),
        context_tokens: None,
        usage: Default::default(),
        harness_label: None,
        agent_id: None,
        session_id: None,
        parent_agent_id: None,
        descriptor: None,
        active_tasks: 0,
        work: None,
    }
}

fn presence(online: bool) -> AgentPresence {
    AgentPresence {
        online,
        detail: None,
        at: 0,
    }
}

#[test]
fn a_function_lane_is_always_marked_with_the_function_glyph() {
    let a = app();
    assert_eq!(a.lane_marker(&lane(AgentRole::Agent), true), "ƒ");
}

#[test]
fn non_worker_tiers_render_as_present() {
    let a = app();
    for role in [
        AgentRole::Orchestrator,
        AgentRole::Reasoning,
        AgentRole::Compress,
    ] {
        assert_eq!(a.lane_marker(&lane(role), false), "●");
    }
}

#[test]
fn an_unbacked_worker_is_unknown_and_a_roster_seeded_one_is_idle() {
    let a = app();
    // Nothing known at all.
    assert_eq!(a.lane_marker(&lane(AgentRole::Agent), false), "◆");
    assert_eq!(
        a.lane_state(&lane(AgentRole::Agent), &std::collections::HashSet::new()),
        " · inactive"
    );
}

#[test]
fn presence_drives_the_glyph_for_an_agent_backed_worker() {
    let mut a = app();
    a.snapshot
        .presence
        .insert("agent-1".to_string(), presence(true));
    a.snapshot
        .presence
        .insert("agent-2".to_string(), presence(false));

    let mut online = lane(AgentRole::Agent);
    online.agent_id = Some("agent-1".to_string());
    assert_eq!(a.lane_marker(&online, false), "●");

    let mut offline = lane(AgentRole::Agent);
    offline.agent_id = Some("agent-2".to_string());
    assert_eq!(a.lane_marker(&offline, false), "○");

    // Known id, no presence record yet → unknown rather than claimed-offline.
    let mut unseen = lane(AgentRole::Agent);
    unseen.agent_id = Some("agent-3".to_string());
    assert_eq!(a.lane_marker(&unseen, false), "◆");
}

#[test]
fn a_session_lane_reflects_its_peer_session_state() {
    let mut a = app();
    a.snapshot.sessions.insert(
        "machine-1".to_string(),
        vec![
            PeerSession {
                id: "s-live".to_string(),
                state: "running".to_string(),
                harness: Some("claude".to_string()),
                last_seen_at: 0,
            },
            PeerSession {
                id: "s-done".to_string(),
                state: "ended".to_string(),
                harness: None,
                last_seen_at: 0,
            },
        ],
    );

    let mut live = lane(AgentRole::Agent);
    live.session_id = Some("s-live".to_string());
    live.parent_agent_id = Some("machine-1".to_string());
    assert_eq!(a.session_state(&live).as_deref(), Some("running"));
    assert_eq!(a.lane_marker(&live, false), "●");
    assert_eq!(
        a.lane_state(&live, &std::collections::HashSet::new()),
        " · working"
    );

    // An ended session is terminal and reads as completed.
    let mut done = lane(AgentRole::Agent);
    done.session_id = Some("s-done".to_string());
    done.parent_agent_id = Some("machine-1".to_string());
    assert_eq!(a.lane_marker(&done, false), "○");
    assert_eq!(
        a.lane_state(&done, &std::collections::HashSet::new()),
        " · completed"
    );

    // A session id whose parent machine is unknown resolves to nothing, and the
    // row degrades to the pending suffix instead of claiming a state.
    let mut orphan = lane(AgentRole::Agent);
    orphan.session_id = Some("s-live".to_string());
    orphan.parent_agent_id = Some("machine-nope".to_string());
    assert_eq!(a.session_state(&orphan), None);
    assert_eq!(
        a.lane_state(&orphan, &std::collections::HashSet::new()),
        " · …"
    );

    // No parent at all → the lookup short-circuits.
    let mut parentless = lane(AgentRole::Agent);
    parentless.session_id = Some("s-live".to_string());
    assert_eq!(a.session_state(&parentless), None);
}

// ----------------------------------------------------- chat tool activity ---

/// An envelope carrying `event` at sequence/time `n`.
fn env_at(n: i64, event: medulla::ui::events::TuiEvent) -> medulla::ui::events::EventEnvelope {
    medulla::ui::events::EventEnvelope {
        seq: n as u64,
        at: n,
        event,
    }
}

/// A `tool_call_start`, which reaches the client as an unmodelled event whose
/// payload is preserved verbatim.
fn tool_start(index: i64, name: &str) -> medulla::ui::events::TuiEvent {
    let mut data = serde_json::Map::new();
    data.insert("kind".into(), "tool_call_start".into());
    data.insert("index".into(), index.into());
    data.insert("name".into(), name.into());
    medulla::ui::events::TuiEvent::Unknown {
        kind: "tool_call_start".into(),
        data,
    }
}

#[test]
fn a_tool_call_renders_as_one_line_between_the_turns_it_happened_between() {
    // The name and the arguments arrive as separate events — the name once, the
    // arguments as fragments — so this is only right if they are paired and
    // flushed in stream order.
    use medulla::ui::events::TuiEvent;
    let events = vec![
        env_at(
            1,
            TuiEvent::User {
                body: "list them".into(),
            },
        ),
        env_at(2, tool_start(0, "Bash")),
        env_at(
            3,
            TuiEvent::ToolCallDelta {
                index: 0,
                args_delta: "{\"command\":\"ls".into(),
            },
        ),
        env_at(
            4,
            TuiEvent::ToolCallDelta {
                index: 0,
                args_delta: " -la\"}".into(),
            },
        ),
        env_at(
            5,
            TuiEvent::Assistant {
                body: "here they are".into(),
            },
        ),
    ];

    let lines: Vec<String> = super::chat_lines(&events, 80)
        .into_iter()
        .map(|l| l.text)
        .collect();
    let joined = lines.join("\n");

    assert!(joined.contains("⏺ Bash(ls -la)"), "got:\n{joined}");
    let call = lines.iter().position(|l| l.contains("Bash")).expect("call");
    let user = lines
        .iter()
        .position(|l| l.contains("list them"))
        .expect("user");
    let reply = lines
        .iter()
        .position(|l| l.contains("here they are"))
        .expect("reply");
    assert!(
        user < call && call < reply,
        "must sit between the turns: {lines:?}"
    );
}

#[test]
fn a_tool_call_still_streaming_is_not_rendered_early() {
    // Flushed only when a non-tool event arrives (or the log ends), so a
    // half-arrived argument list never shows as a complete call.
    use medulla::ui::events::TuiEvent;
    let events = vec![
        env_at(1, tool_start(0, "Bash")),
        env_at(
            2,
            TuiEvent::ToolCallDelta {
                index: 0,
                args_delta: "{\"command\":\"sle".into(),
            },
        ),
    ];
    let lines: Vec<String> = super::chat_lines(&events, 80)
        .into_iter()
        .map(|l| l.text)
        .collect();
    // The log ended, so it flushes — with whatever it has, not a lie about it.
    // A half-written argument object does not parse, so the name renders alone
    // rather than showing the fragment `sle` as if it were the whole command.
    assert_eq!(lines.len(), 1, "got {lines:?}");
    assert_eq!(lines[0], "⏺ Bash", "got {lines:?}");
}

#[test]
fn concurrent_tool_calls_do_not_bleed_into_each_other() {
    use medulla::ui::events::TuiEvent;
    let events = vec![
        env_at(1, tool_start(0, "Bash")),
        env_at(2, tool_start(1, "Read")),
        env_at(
            3,
            TuiEvent::ToolCallDelta {
                index: 1,
                args_delta: "{\"path\":\"a.rs\"}".into(),
            },
        ),
        env_at(
            4,
            TuiEvent::ToolCallDelta {
                index: 0,
                args_delta: "{\"command\":\"ls\"}".into(),
            },
        ),
        env_at(
            5,
            TuiEvent::Assistant {
                body: "done".into(),
            },
        ),
    ];
    let joined = super::chat_lines(&events, 80)
        .into_iter()
        .map(|l| l.text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("⏺ Bash(ls)"), "got:\n{joined}");
    assert!(joined.contains("⏺ Read(a.rs)"), "got:\n{joined}");
}

#[test]
fn a_huge_argument_payload_is_clipped_not_dumped() {
    // The failure this exists to avoid: kilobytes of raw JSON burying the answer
    // the user is actually reading for.
    use medulla::ui::events::TuiEvent;
    let big = format!("{{\"command\":\"{}\"}}", "x".repeat(500));
    let events = vec![
        env_at(1, tool_start(0, "Bash")),
        env_at(
            2,
            TuiEvent::ToolCallDelta {
                index: 0,
                args_delta: big,
            },
        ),
        env_at(3, TuiEvent::Assistant { body: "ok".into() }),
    ];
    let lines: Vec<String> = super::chat_lines(&events, 80)
        .into_iter()
        .map(|l| l.text)
        .collect();
    let call = lines.iter().find(|l| l.contains("Bash")).expect("call");
    assert!(
        call.chars().count() <= 78,
        "got {} chars",
        call.chars().count()
    );
    // The elision marker sits inside the parentheses, which close the call.
    assert!(call.contains('…'), "clipping must be visible: {call}");
    assert!(
        !call.contains('{') && !call.contains('}'),
        "the payload must be summarised, never dumped: {call}"
    );
}

#[test]
fn leaving_the_agents_tab_takes_the_keyboard_back_from_an_attached_harness() {
    // The bug this pins: `release_session` was only reached from
    // `agents_selection`, which runs only while the Agents tab is being drawn.
    // It notices the *cursor* moving off the attached session and has nothing to
    // say once the operator has left the tab altogether — so focus stayed
    // `Attached`, and every keystroke meant for the tab now on screen was typed
    // into a harness pane the operator could no longer see, with `Ctrl-]` the
    // only way out of a mode with no visible cause.
    //
    // Only the leaving case is asserted here. "Stays attached while the pane is
    // on screen" needs a live pty session for the selection to resolve to, which
    // is covered against a real child in `ui::harness_pane`; this fixture has no
    // harnesses, so the Agents tab would release focus for the ordinary reason
    // (nothing under the cursor) and prove nothing about the tab switch.
    let mut app = app();
    app.harness_focus = crate::ui::harness_pane::HarnessFocus::Attached("w_1".to_string());
    app.tab_index = crate::ui::app::types::TABS
        .iter()
        .position(|t| *t != "Agents")
        .expect("some other tab");

    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).expect("terminal");
    terminal.draw(|f| app.draw(f)).expect("draw");

    assert_eq!(
        app.attached_session(),
        None,
        "keys must not reach a harness the operator has navigated away from"
    );
}

/// Put the Changes tab in front of a patch whose selected line is far wider
/// than the diff pane, so one rendered row wraps past the whole viewport.
fn app_on_an_oversized_diff_line() -> App {
    use crate::ui::app::changes::types::ChangedFile;

    let mut app = app();
    app.tab_index = crate::ui::app::types::TABS
        .iter()
        .position(|t| *t == "Changes")
        .expect("Changes tab");
    app.changes.root = Some(std::path::PathBuf::from("/repo"));
    app.changes.baseline = Some("baseline".to_owned());
    app.changes.files = vec![ChangedFile {
        status: "M".into(),
        path: std::path::PathBuf::from("wide.txt"),
        origins: Vec::new(),
    }];
    app.changes.patch = vec![
        "@@ -1,1 +1,1 @@".to_owned(),
        format!("+{}", "word ".repeat(800)),
        " tail".to_owned(),
    ];
    app.changes.cursor = 1;
    app
}

#[test]
fn a_wrapped_diff_bounds_the_scroll_by_the_rows_it_actually_occupies() {
    let mut app = app_on_an_oversized_diff_line();
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).expect("terminal");

    terminal.draw(|f| app.draw(f)).expect("draw");

    // The oversized line wraps to many rows, so the bound has to exceed the
    // three logical patch lines rather than counting them one row each.
    assert!(
        app.changes.max_scroll > 3,
        "wrapped rows must widen the scroll bound: {}",
        app.changes.max_scroll
    );
}

#[test]
fn a_cursor_on_an_oversized_line_holds_a_stable_scroll_offset() {
    let mut app = app_on_an_oversized_diff_line();
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).expect("terminal");

    terminal.draw(|f| app.draw(f)).expect("draw");
    let first = app.changes.scroll;
    terminal.draw(|f| app.draw(f)).expect("draw again");

    // Showing the top of the row keeps consecutive frames identical instead of
    // oscillating between the row's top and bottom edges.
    assert_eq!(first, app.changes.scroll);
    assert_eq!(app.changes.scroll, 1, "the header row precedes the cursor");
}
