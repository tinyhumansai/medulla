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

#[test]
fn the_demo_flag_stands_a_fleet_in_only_when_nothing_is_declared() {
    // The parse rule is the contract; the surfaces read it through
    // `fleet_capacity`, which prefers anything real over the stand-in.
    use medulla::runtime::{demo_capacity, demo_requested_from};
    assert!(!demo_requested_from(None));
    assert!(demo_requested_from(Some("1")));

    // The stand-in is a complete chain, so every surface has something to draw.
    let capacity = demo_capacity();
    assert!(!capacity.is_empty());
    assert!(!capacity.hosts.is_empty() && !capacity.templates.is_empty());

    // A runtime that declares its own capacity wins: the demo app already has
    // one, so its rail shows the scripted host rather than the stand-in's.
    let mut app = app_with_workers(None);
    tab(&mut app, "Agents");
    let out = render(&mut app, 160, 40);
    assert!(out.contains("workshop"), "{out}");
    assert!(
        !out.contains("builder"),
        "stand-in must not mask a real fleet"
    );
}

#[test]
fn the_catalog_carries_the_built_in_coding_roles_beside_the_declared_ones() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Agent Templates");
    let out = render(&mut app, 160, 44);

    // The scripted runtime declares one template; the built-ins fill the rest,
    // so the page is useful before anyone writes a catalog.
    assert!(out.contains("Implementer"), "declared role: {out}");
    for role in ["Code Reviewer", "Debugger", "Doc Writer"] {
        assert!(out.contains(role), "missing built-in {role}: {out}");
    }
    assert!(
        !out.contains("No agent templates declared"),
        "the catalog is never empty now: {out}"
    );
}

#[test]
fn a_declared_template_replaces_the_built_in_of_the_same_id() {
    // The mock declares `implementer`, which is also a built-in id: the
    // declared record wins, so the catalog lists it once.
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Agent Templates");
    let out = render(&mut app, 200, 44);
    assert_eq!(
        out.matches("Implementer").count(),
        1,
        "one row per template id: {out}"
    );
    // Built-ins are additive, not privileged: the mock harness allowlists only
    // `implementer`, and the defaults respect that rather than routing around it.
    assert!(out.contains("Implementer · reasoning · 1 place"), "{out}");
    assert!(
        out.contains("Code Reviewer · reasoning · nowhere allows it"),
        "an allowlist still binds the built-ins: {out}"
    );
}

#[test]
fn the_selected_agent_shows_its_placement_and_compact_meters() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Agents");
    // Walk onto the placed roster agent.
    let _ = app.on_event(alt_key(KeyCode::Down));
    let out = render(&mut app, 160, 44);

    // Where it runs — labelled, so the machine and the working directory are
    // identifiable rather than two of several bare tokens.
    assert!(out.contains("host workshop"), "host: {out}");
    assert!(out.contains("dir /srv/repos/medulla"), "workspace: {out}");
    // …and how hard, as meters rather than bare numbers.
    assert!(out.contains("cpu"), "cpu meter: {out}");
    assert!(out.contains("2.10 load / 10 cores"), "{out}");
    assert!(out.contains("ram"), "ram meter: {out}");
    assert!(out.contains("20.0GB / 32.0GB"), "used, not free: {out}");
    assert!(out.contains("disk"), "disk: {out}");
}

#[test]
fn the_context_meter_splits_input_output_and_cache() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Agents");
    // The orchestrator lane carries the scripted cycle's usage.
    let out = render(&mut app, 160, 44);
    assert!(out.contains("ctx"), "context meter: {out}");
    assert!(out.contains("in 1k / 32k"), "{out}");
    assert!(out.contains("out "), "{out}");
    assert!(out.contains("cache 70%"), "the reported cache share: {out}");
}
