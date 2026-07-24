//! Feature tests for the Agents tab's presence glyphs.
//!
//! Each lane carries a one-character marker that is the only signal of whether a
//! peer is reachable, so the distinctions between "online", "offline",
//! "announced but never seen", and "known only as a descriptor" have to survive
//! rendering — they are easy to collapse into one glyph by accident.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use medulla::config::{LoadedConfig, TinyplaceConfig};
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::{AgentDescriptor, AgentPresence, TinyplaceIdentity, WorkerInfo};
use medulla::tinyplace::service::TinyplaceObservation;
use medulla_tui::ui::app::{App, TABS};

use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect()
}

/// A tiny.place peer descriptor named `id`.
fn peer(id: &str) -> AgentDescriptor {
    let mut metadata = serde_json::Map::new();
    metadata.insert("harness".into(), serde_json::json!("tinyplace"));
    AgentDescriptor {
        id: id.into(),
        name: id.into(),
        description: "a peer".into(),
        availability: "online".into(),
        tags: vec![],
        metadata,
    }
}

/// An app on the Agents tab whose roster is `ids`, with presence for whichever
/// of them appear in `online` (mapped to their online flag).
fn agents_app(ids: &[&str], online: &[(&str, bool)]) -> App {
    let rt = Arc::new(MockRuntime::empty());
    let mut l = LoadedConfig::defaults("medulla.tui.json".into());
    l.config.tinyplace = Some(TinyplaceConfig::default());
    let mut app = App::new(rt, l);

    let mut presence = HashMap::new();
    for (id, is_online) in online {
        presence.insert(
            (*id).to_string(),
            AgentPresence {
                online: *is_online,
                detail: None,
                at: 1,
            },
        );
    }
    app.set_tinyplace_observation(Arc::new(Mutex::new(TinyplaceObservation {
        notice: None,
        identity: Some(TinyplaceIdentity {
            agent_id: "me".into(),
            public_key: "pk".into(),
            handle: Some("@me".into()),
        }),
        roster: ids.iter().map(|id| peer(id)).collect(),
        presence,
    })));
    app.tab_index = TABS.iter().position(|t| *t == "Agents").unwrap();
    app
}

fn dirty_report(paths: &[&str]) -> medulla::workspace::WorkspaceReport {
    use medulla::workspace::{BranchState, FileChange, WorkspaceReport, WorkspaceSnapshot};
    let root = std::path::PathBuf::from("/workspace/project");
    WorkspaceReport {
        root: root.clone(),
        snapshot: Some(WorkspaceSnapshot {
            root,
            branch: BranchState {
                name: "feat/claims".into(),
                detached: false,
                ahead: 0,
                behind: 0,
            },
            files: paths
                .iter()
                .map(|path| FileChange {
                    path: (*path).into(),
                    original_path: None,
                    index_status: ' ',
                    worktree_status: 'M',
                })
                .collect(),
            commits: vec![],
        }),
        error: None,
    }
}

fn key(app: &mut App, code: KeyCode) {
    let _ = app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn claim_selected_lane(app: &mut App, claim: &str) {
    key(app, KeyCode::Char('C'));
    for ch in claim.chars() {
        key(app, KeyCode::Char(ch));
    }
    key(app, KeyCode::Enter);
}

#[test]
fn an_online_peer_and_an_offline_peer_render_different_glyphs() {
    // The filled/hollow distinction is the whole signal — if both rendered the
    // same, an unreachable peer would look healthy.
    let mut app = agents_app(&["up", "down"], &[("up", true), ("down", false)]);
    let out = render(&mut app, 140, 40);
    assert!(out.contains("up"), "the online peer is listed: {out}");
    assert!(out.contains("down"), "the offline peer is listed: {out}");
    assert!(out.contains('●'), "an online peer is filled: {out}");
    assert!(out.contains('○'), "an offline peer is hollow: {out}");
}

#[test]
fn a_peer_with_no_presence_reading_is_marked_as_merely_announced() {
    // Never having heard from a peer is not the same as hearing it is offline,
    // and the glyph has to say so rather than guessing.
    let mut app = agents_app(&["silent"], &[]);
    let out = render(&mut app, 140, 40);
    assert!(out.contains("silent"), "the peer is listed: {out}");
    assert!(
        out.contains('◌') || out.contains('◆'),
        "a peer with no reading is neither online nor offline: {out}"
    );
}

/// A worker as the local registry holds it: an address, a harness, no backend
/// descriptor behind it.
fn local_worker(address: &str, label: Option<&str>) -> WorkerInfo {
    WorkerInfo {
        id: address.into(),
        address: address.into(),
        handle: None,
        label: label.map(str::to_string),
        harness: Some("claude".into()),
        peer_id: None,
        selected: true,
    }
}

/// An app on the Agents tab whose *registry* holds `workers` while the snapshot
/// roster stays empty — the shape of a tiny.place worker added at runtime.
fn workers_app(workers: Vec<WorkerInfo>) -> App {
    let rt = MockRuntime::empty();
    rt.set_workers(workers);
    let mut app = App::new(
        Arc::new(rt),
        LoadedConfig::defaults("medulla.tui.json".into()),
    );
    app.tab_index = TABS.iter().position(|t| *t == "Agents").unwrap();
    app
}

#[test]
fn a_locally_registered_worker_is_listed_even_with_an_empty_backend_roster() {
    // The registry is what actually resolves a delegated task's address, so a
    // worker in it is live and dispatchable. Reading only the backend-advertised
    // roster left it invisible here while tasks were running on it.
    let mut app = workers_app(vec![local_worker("Wk3Hob1FxUwsy1K2rweppbmCkuPef", None)]);
    let out = render(&mut app, 140, 40);
    assert!(
        out.contains("Wk3Hob1FxUwsy1K2rweppbmCkuPef"),
        "the registered worker has a lane: {out}"
    );
    assert!(out.contains("CLAUDE"), "its harness tags the lane: {out}");
    // Registered is not the same as heard from: the registry knows the peer is
    // dispatchable, not that it is up, and the glyph must not claim otherwise.
    assert!(
        out.contains('◌'),
        "a registered worker with no presence reading is announced, not online: {out}"
    );
}

#[test]
fn a_registered_worker_prefers_its_label_and_is_not_listed_twice() {
    // The same peer reached the roster from the backend and the local registry;
    // one lane, and the operator's own label names it.
    let mut app = workers_app(vec![
        local_worker("addr-1", Some("build box")),
        local_worker("addr-1", Some("build box")),
    ]);
    let out = render(&mut app, 140, 40);
    assert!(out.contains("build box"), "the label names the lane: {out}");
    assert_eq!(
        out.matches("build box").count(),
        1,
        "one peer, one lane: {out}"
    );
}

#[test]
fn the_lane_list_survives_a_roster_that_changes_under_it() {
    // Presence arrives separately from the roster, so a reading for an agent
    // that is no longer listed must not panic or resurrect the lane.
    let mut app = agents_app(&["kept"], &[("kept", true), ("gone", false)]);
    let out = render(&mut app, 140, 40);
    assert!(out.contains("kept"), "the live peer renders: {out}");
    assert!(
        !out.contains("gone"),
        "a stale presence reading must not create a lane: {out}"
    );
}

#[test]
fn manual_claim_overlap_badges_both_lanes_and_the_overview() {
    let mut app = agents_app(&["up", "down"], &[]);
    app.set_workspace_reports(vec![dirty_report(&["src/shared.rs"])]);

    claim_selected_lane(&mut app, "src/**");
    key(&mut app, KeyCode::Down);
    claim_selected_lane(&mut app, "src/**/*.rs");

    let agents = render(&mut app, 180, 44);
    assert!(agents.contains("lanes 2/4"), "{agents}");
    assert!(agents.matches("⚠ overlap").count() >= 2, "{agents}");
    assert!(agents.contains("claim src/**/*.rs"), "{agents}");

    app.tab_index = TABS.iter().position(|tab| *tab == "Overview").unwrap();
    let overview = render(&mut app, 140, 40);
    assert!(
        overview.contains("⚠ lane overlap · 1 path(s)"),
        "{overview}"
    );
}

#[test]
fn shared_path_claims_and_invalid_patterns_are_visible() {
    let mut app = agents_app(&["up"], &[]);
    app.set_workspace_reports(vec![dirty_report(&["Cargo.lock"])]);
    claim_selected_lane(&mut app, "Cargo.lock");
    let out = render(&mut app, 180, 44);
    assert!(out.contains("⚠ shared-path"), "{out}");

    claim_selected_lane(&mut app, "[");
    assert!(app.status().contains("invalid lane-claim pattern"));

    key(&mut app, KeyCode::Char('C'));
    for _ in 0..32 {
        key(&mut app, KeyCode::Backspace);
    }
    key(&mut app, KeyCode::Enter);
    assert!(app.status().contains("lane claim cleared"));
    assert!(!render(&mut app, 180, 44).contains("claim Cargo.lock"));
}
