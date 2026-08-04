//! Role assignment on the Hosts page: the toggles belong to an *agent*, they
//! are editable only on a local host, and what they change is the agent's
//! declaration rather than the live roster alone.

use crate::helpers::*;

#[test]
fn space_on_a_role_offers_the_selected_agent_for_it_and_takes_it_back() {
    // Roles belong to an *agent*: a laptop is not "the reviewer", the agent
    // working in the reviewed checkout is.
    let mut app = app_with_roster(vec![local_worker("w1", true)], None);
    app.focus_routing_subpage("Hosts");
    down(&mut app, 1);

    // Roles come from the agent-template catalog, which ships built-in coding
    // roles even with nothing declared — so there is always something to toggle.
    let out = render(&mut app, 120, 44);
    assert!(
        out.contains("none assigned · offered for any role"),
        "an unassigned agent reads as general, not excluded: {out}"
    );

    let _ = app.on_event(key(KeyCode::Right));
    let out = render(&mut app, 120, 44);
    assert!(out.contains("[ ]"), "the toggle list is drawn: {out}");

    let cmd = app.on_event(key(KeyCode::Char(' ')));
    let assigned = match cmd {
        Some(Cmd::WorkerOp(WorkerOp::SetRoles { id, roles })) => {
            assert_eq!(id, "w1");
            assert_eq!(roles.len(), 1, "exactly the toggled role");
            roles
        }
        other => panic!("expected a SetRoles op, got {other:?}"),
    };
    // This app has no config file, so the assignment cannot outlive the run —
    // and says so rather than implying it was written down.
    assert!(
        app.status().contains("this run only"),
        "status: {}",
        app.status()
    );

    // And back off again. The op is a whole-list replacement, so removing the
    // only role must send an *empty* list — not omit the field, which would
    // read as "leave the roles alone" and make the toggle one-way.
    let mut held = app_with_roster(
        vec![{
            let mut w = local_worker("w1", true);
            w.roles = assigned;
            w
        }],
        None,
    );
    held.focus_routing_subpage("Hosts");
    down(&mut held, 1);
    let _ = held.on_event(key(KeyCode::Right));
    match held.on_event(key(KeyCode::Char(' '))) {
        Some(Cmd::WorkerOp(WorkerOp::SetRoles { id, roles })) => {
            assert_eq!(id, "w1");
            assert!(
                roles.is_empty(),
                "removal sends an empty list, got {roles:?}"
            );
        }
        other => panic!("expected a SetRoles op, got {other:?}"),
    }
}

#[test]
fn a_remote_agents_roles_cannot_be_assigned_from_here() {
    let mut app = app_with_roster(vec![worker("w1", true)], None);
    app.focus_routing_subpage("Hosts");
    down(&mut app, 2); // this device, the peer's host row, then its agent

    // The toggles never open, and the preview says who owns the decision.
    let _ = app.on_event(key(KeyCode::Right));
    let out = render(&mut app, 130, 44);
    assert!(
        out.contains("read-only · assign roles on that machine"),
        "{out}"
    );
    assert!(!out.contains("[ ]"), "no checkboxes to flip: {out}");
    assert!(
        app.status().contains("assign its roles there"),
        "status: {}",
        app.status()
    );
    // Space is not a role toggle here — it falls through unhandled rather than
    // silently editing the roster.
    assert!(app.on_event(key(KeyCode::Char(' '))).is_none());
}

#[test]
fn leaving_the_role_list_hands_the_arrows_back_to_the_tree() {
    let mut app = app_with_roster(
        vec![local_worker("w1", true), local_worker("w2", false)],
        None,
    );
    app.focus_routing_subpage("Hosts");
    down(&mut app, 1);

    let _ = app.on_event(key(KeyCode::Right));
    // Down now walks roles, so the selected agent must not have changed.
    let _ = app.on_event(key(KeyCode::Down));
    let out = render(&mut app, 120, 44);
    assert!(out.contains("Agent · w1"), "still previewing w1: {out}");

    let _ = app.on_event(key(KeyCode::Left));
    let _ = app.on_event(key(KeyCode::Down));
    let out = render(&mut app, 120, 44);
    assert!(out.contains("Agent · w2"), "back on the tree: {out}");
}

#[test]
fn an_assigned_role_is_summarised_and_checked() {
    let mut w = local_worker("w1", true);
    w.roles = vec!["code-reviewer".into()];

    let mut app = app_with_roster(vec![w], None);
    app.focus_routing_subpage("Hosts");
    down(&mut app, 1);
    let out = render(&mut app, 130, 44);

    // The summary leads the block, so what an agent is offered for is readable
    // without counting checkboxes.
    assert!(
        out.contains("roles      code-reviewer"),
        "assigned roles summarised: {out}"
    );
    assert!(out.contains("[x] code-reviewer"), "and checked: {out}");
    assert!(out.contains("[ ] implementer"), "others unchecked: {out}");
    // And the agent row carries the count, so the tree still says which agents
    // have been given roles at all.
    assert!(out.contains("· 1 role"), "count on the agent row: {out}");
}

#[test]
fn a_role_below_the_fold_stays_visible_when_selected() {
    // The preview is capped at half the page, so on a short terminal the role
    // list must scroll — a role that can be selected but not seen is a toggle
    // the operator flips blind.
    let mut app = app_with_roster(vec![local_worker("w1", true)], None);
    app.focus_routing_subpage("Hosts");
    down(&mut app, 1);
    let _ = app.on_event(key(KeyCode::Right));

    let out = render(&mut app, 130, 26);
    let last = "repo-orchestrator";
    assert!(!out.contains(last), "the tail starts off-screen: {out}");

    for _ in 0..12 {
        let _ = app.on_event(key(KeyCode::Down));
    }
    let out = render(&mut app, 130, 26);
    assert!(
        out.contains(&format!("▸ [ ] {last}")),
        "the window follows the cursor to the last role: {out}"
    );
}
