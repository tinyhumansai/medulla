//! The Routing › Fleet page: the declared containment chain renders as a tree,
//! selection walks it, the detail pane follows the cursor, and entering the page
//! asks the runtime for a fresh read.

use crate::helpers::*;

#[test]
fn the_fleet_page_renders_the_declared_chain_top_down() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Fleet");
    let out = render(&mut app, 160, 40);

    // Host → harness → workspace → agent, plus the template catalog beside it.
    assert!(out.contains("workshop"), "{out}");
    assert!(out.contains("claude-code"), "{out}");
    assert!(out.contains("/srv/repos/medulla"), "{out}");
    assert!(out.contains("dev-1"), "{out}");
}

#[test]
fn the_host_row_summarizes_resources_and_the_harness_row_its_budget() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Fleet");
    let out = render(&mut app, 160, 40);
    assert!(out.contains("10 cores"), "{out}");
    assert!(out.contains("760k/1.0M left"), "{out}");
}

#[test]
fn moving_the_cursor_walks_the_tree_and_swaps_the_detail_pane() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Fleet");
    assert_eq!(app.selected_fleet_key().as_deref(), Some("host:host-1"));

    let _ = app.on_event(key(KeyCode::Down));
    assert_eq!(
        app.selected_fleet_key().as_deref(),
        Some("harness:harness-1")
    );
    let out = render(&mut app, 160, 40);
    // The harness detail names its host and every budget window it declares.
    assert!(out.contains("seat seat-1"), "{out}");
    assert!(out.contains("providers: anthropic"), "{out}");
}

#[test]
fn the_agent_detail_resolves_its_placement_and_template() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Fleet");
    for _ in 0..3 {
        let _ = app.on_event(key(KeyCode::Down));
    }
    assert_eq!(app.selected_fleet_key().as_deref(), Some("agent:dev-1"));
    let out = render(&mut app, 160, 40);
    assert!(out.contains("workspace: /srv/repos/medulla"), "{out}");
    assert!(out.contains("template: Implementer"), "{out}");
}

#[test]
fn entering_the_page_and_pressing_r_both_ask_for_a_fresh_read() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Routing");
    // Fleet is the first Routing page, so the menu opens on it.
    assert_eq!(app.routing_subpage(), "Fleet");
    assert!(
        matches!(app.on_event(key(KeyCode::Enter)), Some(Cmd::RefreshFleet)),
        "entering Fleet re-reads declared capacity"
    );
    assert!(matches!(
        app.on_event(key(KeyCode::Char('r'))),
        Some(Cmd::RefreshFleet)
    ));
}

#[test]
fn a_registered_peer_is_a_host_rather_than_an_unplaced_agent() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Fleet");
    let out = render(&mut app, 160, 40);
    // `w1` is a registry peer: it belongs in the chain as a machine, and the
    // codex harness it advertises hangs off it.
    assert!(out.contains("w1 label"), "{out}");
    assert!(!out.contains("unplaced agents"), "{out}");
}

#[test]
fn the_templates_page_lists_the_catalog_and_its_usage() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Templates");
    let out = render(&mut app, 160, 40);
    assert!(out.contains("Implementer"), "{out}");
    assert!(out.contains("Implements a scoped change"), "{out}");
    assert!(out.contains("provisioned agents"), "{out}");
    assert!(out.contains("dev-1"), "{out}");
    assert!(matches!(
        app.on_event(key(KeyCode::Char('r'))),
        Some(Cmd::RefreshFleet)
    ));
}

#[test]
fn the_agents_tab_shows_where_the_selected_agent_runs() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Agents");
    // Walk off the orchestrator lane onto the placed roster agent.
    let _ = app.on_event(alt_key(KeyCode::Down));
    let out = render(&mut app, 160, 40);
    assert!(
        out.contains("workshop") && out.contains("/srv/repos/medulla"),
        "the agent lane names its host and workspace: {out}"
    );
}

#[test]
fn an_unplaced_lane_shows_no_placement_chip() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Agents");
    // The orchestrator lane has no descriptor, so nothing to resolve.
    let out = render(&mut app, 160, 40);
    assert!(!out.contains("/srv/repos/medulla"), "{out}");
}
