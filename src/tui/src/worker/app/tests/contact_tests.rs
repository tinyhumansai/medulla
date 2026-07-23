//! The Contacts and Requests tabs: decisions, the badge, and the policy cycle.

use crossterm::event::KeyCode;

use super::super::super::pty::PtyManager;
use super::helpers::{app_with, key, render};
use medulla::contacts::ContactDecision;

use super::super::types::{Confirm, WorkerCmd, TAB_CONTACTS, TAB_REQUESTS};
use super::helpers::desk_with;

// --------------------------------------------------------------- contacts ---

#[tokio::test]
async fn pending_requests_drive_the_requests_tab_and_the_badge() {
    let desk = desk_with(&["alice", "bob"]).await;
    let mut app = app_with(PtyManager::new(), Some(desk));

    assert_eq!(app.pending_requests().len(), 2);
    let out = render(&mut app, 110, 16);
    assert!(out.contains("Requests (2)"), "badge in the tab bar: {out}");
}

#[tokio::test]
async fn accept_and_decline_emit_contact_ops() {
    for (code, expected) in [
        (KeyCode::Char('a'), ContactDecision::Accept),
        (KeyCode::Char('x'), ContactDecision::Decline),
    ] {
        let desk = desk_with(&["alice"]).await;
        let mut app = app_with(PtyManager::new(), Some(desk));
        app.set_tab(TAB_REQUESTS);
        match app.on_key(key(code)) {
            Some(WorkerCmd::ContactOp { agent_id, decision }) => {
                assert_eq!(agent_id, "alice");
                assert_eq!(decision, expected);
            }
            other => panic!("expected {expected:?}, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn blocking_asks_before_it_emits() {
    let desk = desk_with(&["spammer"]).await;
    let mut app = app_with(PtyManager::new(), Some(desk));
    app.set_tab(TAB_REQUESTS);

    assert!(app.on_key(key(KeyCode::Char('B'))).is_none(), "asks first");
    assert_eq!(app.confirm(), Some(&Confirm::BlockPeer("spammer".into())));

    match app.on_key(key(KeyCode::Char('y'))) {
        Some(WorkerCmd::ContactOp { decision, .. }) => {
            assert_eq!(decision, ContactDecision::Block);
        }
        other => panic!("expected a Block op, got {other:?}"),
    }
}

#[tokio::test]
async fn the_contacts_tab_shows_accepted_peers_only() {
    let desk = desk_with(&["alice", "bob"]).await;
    desk.book()
        .settle("alice", ContactDecision::Accept, false, 2);
    let mut app = app_with(PtyManager::new(), Some(desk));

    assert_eq!(app.accepted_contacts().len(), 1);
    assert_eq!(app.accepted_contacts()[0].agent_id, "alice");
    assert_eq!(app.pending_requests().len(), 1, "bob is still waiting");

    app.set_tab(TAB_CONTACTS);
    let out = render(&mut app, 110, 16);
    assert!(out.contains("alice"));
    assert!(out.contains("Contacts · 1"));
}

#[tokio::test]
async fn p_cycles_the_admission_policy() {
    let desk = desk_with(&["alice"]).await;
    let mut app = app_with(PtyManager::new(), Some(desk));
    app.set_tab(TAB_REQUESTS);

    app.on_key(key(KeyCode::Char('p')));
    assert!(app.status().contains("allowlist"), "got {:?}", app.status());
    app.on_key(key(KeyCode::Char('p')));
    assert!(app.status().contains("all"), "got {:?}", app.status());
    app.on_key(key(KeyCode::Char('p')));
    assert!(app.status().contains("manual"), "got {:?}", app.status());
}

#[tokio::test]
async fn enter_accepts_the_selected_request() {
    // Enter is the same decision as 'a' — the muscle-memory "yes" for a queued
    // request — and must emit the same accept op.
    let desk = desk_with(&["alice"]).await;
    let mut app = app_with(PtyManager::new(), Some(desk));
    app.set_tab(TAB_REQUESTS);

    match app.on_key(key(KeyCode::Enter)) {
        Some(WorkerCmd::ContactOp { agent_id, decision }) => {
            assert_eq!(agent_id, "alice");
            assert_eq!(decision, ContactDecision::Accept);
        }
        other => panic!("expected an Accept op, got {other:?}"),
    }
}

#[tokio::test]
async fn a_decision_key_with_an_empty_queue_says_nothing_is_selected() {
    // 'p' still cycles policy on an empty queue, but a/x/B have no target and
    // must say so rather than silently doing nothing.
    let desk = desk_with(&[]).await;
    let mut app = app_with(PtyManager::new(), Some(desk));
    app.set_tab(TAB_REQUESTS);

    for code in [KeyCode::Char('a'), KeyCode::Char('x'), KeyCode::Char('B')] {
        assert!(app.on_key(key(code)).is_none());
        assert!(
            app.status().contains("No pending request selected"),
            "for {code:?} got {:?}",
            app.status()
        );
    }
}

#[tokio::test]
async fn the_contacts_tab_also_cycles_the_policy() {
    // Policy is reachable from Contacts too, not only Requests.
    let desk = desk_with(&["alice"]).await;
    let mut app = app_with(PtyManager::new(), Some(desk));
    app.set_tab(TAB_CONTACTS);

    app.on_key(key(KeyCode::Char('p')));
    assert!(app.status().contains("allowlist"), "got {:?}", app.status());
}

#[tokio::test]
async fn an_unbound_key_on_the_contacts_tab_does_nothing() {
    // Contacts is policy-only; a key it does not handle must fall through
    // quietly rather than emit a command or change state.
    let desk = desk_with(&["alice"]).await;
    let mut app = app_with(PtyManager::new(), Some(desk));
    app.set_tab(TAB_CONTACTS);
    let before = app.status().to_string();
    assert!(app.on_key(key(KeyCode::Char('z'))).is_none());
    assert_eq!(app.status(), before, "an unhandled key changes nothing");
}

#[tokio::test]
async fn an_unbound_key_on_the_requests_tab_with_a_selection_does_nothing() {
    // With a request selected but a key that is not a decision, the handler
    // returns nothing rather than acting on the selection.
    let desk = desk_with(&["alice"]).await;
    let mut app = app_with(PtyManager::new(), Some(desk));
    app.set_tab(TAB_REQUESTS);
    assert!(app.selected_request().is_some(), "a request is selected");
    assert!(
        app.on_key(key(KeyCode::Char('z'))).is_none(),
        "an unbound key emits no contact op"
    );
    assert!(app.confirm().is_none(), "and arms no confirmation");
}

#[tokio::test]
async fn the_contacts_tab_with_no_accepted_peers_says_how_to_get_one() {
    // A desk with pending requests but nothing accepted must read as an empty
    // contact list with guidance, not as a missing identity.
    let desk = desk_with(&["alice"]).await;
    let mut app = app_with(PtyManager::new(), Some(desk));
    app.set_tab(TAB_CONTACTS);
    let out = render(&mut app, 110, 16);
    assert!(out.contains("No accepted contacts"), "got: {out}");
    assert!(
        out.contains("Requests tab"),
        "it points at where to accept: {out}"
    );
}

#[test]
fn the_requests_tab_without_an_identity_explains_the_absence() {
    let mut app = app_with(PtyManager::new(), None);
    app.set_tab(TAB_REQUESTS);
    let out = render(&mut app, 110, 16);
    assert!(
        out.contains("No tiny.place identity is configured"),
        "got: {out}"
    );
}

#[tokio::test]
async fn the_requests_tab_with_an_empty_queue_shows_nothing_waiting() {
    let desk = desk_with(&[]).await;
    let mut app = app_with(PtyManager::new(), Some(desk));
    app.set_tab(TAB_REQUESTS);
    let out = render(&mut app, 110, 16);
    assert!(out.contains("Nothing waiting"), "the list is empty: {out}");
    assert!(
        out.contains("No request selected"),
        "and the detail pane has nothing to describe: {out}"
    );
}

#[test]
fn cycling_policy_without_an_identity_explains_the_absence() {
    // No tiny.place identity means no admission policy to cycle; the key must
    // explain rather than appear to do nothing.
    let mut app = app_with(PtyManager::new(), None);
    app.set_tab(TAB_REQUESTS);

    app.on_key(key(KeyCode::Char('p')));
    assert!(
        app.status().contains("No tiny.place identity"),
        "got {:?}",
        app.status()
    );
}

#[tokio::test]
async fn r_forces_a_relay_check_rather_than_waiting_out_the_interval() {
    let desk = desk_with(&["alice"]).await;
    let mut app = app_with(PtyManager::new(), Some(desk));
    app.set_tab(TAB_REQUESTS);
    assert_eq!(
        app.on_key(key(KeyCode::Char('r'))),
        Some(WorkerCmd::Refresh)
    );
}

#[tokio::test]
async fn the_requests_header_says_how_the_last_poll_went() {
    // An empty queue and an unreachable relay look identical without this.
    let desk = desk_with(&["alice"]).await;
    desk.refresh().await;
    let mut app = app_with(PtyManager::new(), Some(desk));
    app.set_tab(TAB_REQUESTS);

    let out = render(&mut app, 120, 16);
    assert!(out.contains("checked"), "poll health in the header: {out}");
}

#[tokio::test]
async fn the_contacts_tab_lists_peers_this_process_never_saw_ask() {
    // Regression. Contacts came from filtering the incoming-request queue, so
    // the tab only ever showed peers whose request arrived *and* was accepted
    // while this process was running: empty on every restart, and never showing
    // a peer this agent had requested and who accepted. They are now read from
    // the relay's own contact list, which is the thing that actually governs
    // who may dispatch work here.
    let desk = super::helpers::desk_with_contacts(&["bob"], &["alice"]).await;
    let mut app = app_with(PtyManager::new(), Some(desk));

    assert_eq!(app.accepted_contacts().len(), 1);
    assert_eq!(app.accepted_contacts()[0].agent_id, "alice");
    assert_eq!(
        app.pending_requests().len(),
        1,
        "an established contact must not also sit in the Requests queue"
    );
    assert_eq!(app.pending_requests()[0].agent_id, "bob");

    app.set_tab(TAB_CONTACTS);
    let out = render(&mut app, 110, 16);
    assert!(out.contains("alice"), "got: {out}");
    assert!(out.contains("Contacts · 1"), "got: {out}");
}
