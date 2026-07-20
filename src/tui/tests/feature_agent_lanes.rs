//! Feature tests for the Agents tab's presence glyphs.
//!
//! Each lane carries a one-character marker that is the only signal of whether a
//! peer is reachable, so the distinctions between "online", "offline",
//! "announced but never seen", and "known only as a descriptor" have to survive
//! rendering — they are easy to collapse into one glyph by accident.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use medulla::config::{LoadedConfig, TinyplaceConfig};
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::{AgentDescriptor, AgentPresence, TinyplaceIdentity};
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
