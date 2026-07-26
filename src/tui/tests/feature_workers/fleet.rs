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
    assert!(out.contains("agent templates"), "{out}");
    assert!(out.contains("Implementer"), "{out}");
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
    // Menu → Fleet → enter the content pane.
    let _ = app.on_event(key(KeyCode::Down));
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
fn a_heading_row_is_never_selected_as_a_detail_target() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Fleet");
    // Walk past the chain onto the `── agent templates ──` heading.
    for _ in 0..4 {
        let _ = app.on_event(key(KeyCode::Down));
    }
    assert_eq!(app.selected_fleet_key(), None);
}
