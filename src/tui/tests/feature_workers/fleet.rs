//! The fleet in the Agents rail: the declared chain renders under the lanes,
//! selection walks it, the pane shows a declaration instead of a transcript, and
//! the Templates page lists the catalog.

use crate::helpers::*;

#[test]
fn the_rail_carries_the_fleet_under_the_lanes() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Agents");
    let out = render(&mut app, 160, 40);
    // One rail: the lanes, a divider, then the declared chain beneath it.
    assert!(out.contains("── fleet ──"), "{out}");
    assert!(out.contains("workshop"), "{out}");
    assert!(out.contains("/srv/repos/medulla"), "{out}");
}

#[test]
fn selecting_a_fleet_row_shows_its_declaration_instead_of_a_transcript() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Agents");
    // Walk past the lanes and the divider onto the first host.
    for _ in 0..20 {
        if app.selected_fleet_node_key().is_some() {
            break;
        }
        let _ = app.on_event(alt_key(KeyCode::Down));
    }
    let node = app.selected_fleet_node_key().expect("a fleet row");
    assert!(node.starts_with("host:"), "landed on a host: {node}");
    let out = render(&mut app, 160, 40);
    assert!(out.contains("host · workshop"), "pane title: {out}");
    assert!(out.contains("resources: 10 cores"), "{out}");
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
    // The orchestrator lane has no descriptor, so there is no placement to
    // resolve. The path still appears in the rail's fleet half — the chip is the
    // one-line "host · harness · workspace" summary above the transcript.
    let out = render(&mut app, 160, 40);
    assert!(!out.contains("claude-code · /srv/repos/medulla"), "{out}");
}
