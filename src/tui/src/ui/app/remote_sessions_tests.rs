//! Tests for id resolution and the routed session lookups.
//!
//! The property under test throughout is that a *local* id behaves exactly as it
//! always has. Every routed accessor added here is on a path the local case
//! already used, so a mistake in the parsing would not fail loudly — it would
//! quietly send a local keystroke nowhere.

use medulla::protocol::{RemoteAttention, RemoteSessionRow, RemoteSessionState};

use super::remote_sessions::{qualify, session_row, SessionRef};

fn remote_row(id: &str) -> RemoteSessionRow {
    RemoteSessionRow {
        id: id.to_string(),
        label: "you:shell:zsh".to_string(),
        provider: "shell".to_string(),
        preset: None,
        state: RemoteSessionState::Running,
        cwd: "/work".to_string(),
        name: Some("build".to_string()),
        thread_name: None,
        started_at: 1_700_000_000_000,
        last_output_at: 1_700_000_001_000,
        busy: false,
        working: true,
        attention: None,
        bracketed_paste: true,
        last_error: None,
    }
}

#[test]
fn a_bare_id_is_local() {
    // The rule every existing caller depends on. A pty id has no separator in
    // it, so nothing that worked before can start resolving as remote.
    assert_eq!(SessionRef::parse("w_7"), SessionRef::Local("w_7"));
    assert_eq!(SessionRef::parse(""), SessionRef::Local(""));
}

#[test]
fn a_prefixed_id_names_its_host_and_session() {
    assert_eq!(
        SessionRef::parse("tower/w_7"),
        SessionRef::Remote {
            host: "tower",
            session: "w_7"
        }
    );
}

#[test]
fn a_malformed_prefix_falls_back_to_local_rather_than_half_resolving() {
    // Half a remote id is not a remote session. Treating it as one would send a
    // keystroke to a host named "" — better to fail the local lookup, which says
    // so.
    assert_eq!(SessionRef::parse("/w_7"), SessionRef::Local("/w_7"));
    assert_eq!(SessionRef::parse("tower/"), SessionRef::Local("tower/"));
}

#[test]
fn qualify_and_parse_are_inverses() {
    let id = qualify("tower", "w_7");
    assert_eq!(id, "tower/w_7");
    assert_eq!(
        SessionRef::parse(&id),
        SessionRef::Remote {
            host: "tower",
            session: "w_7"
        }
    );
}

#[test]
fn a_remote_row_becomes_a_session_row_the_rail_can_place() {
    let row = session_row("tower", &remote_row("w_1"), 0);
    assert_eq!(row.id, "tower/w_1");
    assert_eq!(row.cwd, "/work");
    assert_eq!(row.name.as_deref(), Some("build"));
    assert!(row.working);
    assert_eq!(row.provider, medulla::protocol::HarnessProvider::Shell);
    assert!(row.state.is_running());
}

#[test]
fn a_remote_session_is_always_the_operators() {
    // Nothing dispatches into a remote session — it exists because somebody
    // asked for it in the picker. The orchestrator must never see one as
    // available, so both fields are fixed rather than carried.
    let row = session_row("tower", &remote_row("w_1"), 0);
    assert_eq!(row.control, crate::worker::pty::SessionControl::User);
    assert!(row.origin.is_user());
}

#[test]
fn a_remote_row_claims_no_checkout_it_cannot_see() {
    // The Changes view reads these. A fabricated checkout would have it claim to
    // know a repository on a filesystem this machine cannot reach.
    let row = session_row("tower", &remote_row("w_1"), 0);
    assert!(row.launch_root.is_none());
    assert!(row.launch_commit.is_none());
    assert!(row.mcp_grant_session.is_none());
    assert_eq!(row.checkout, medulla::ui::checkout::Checkout::default());
}

#[test]
fn an_exited_remote_session_keeps_its_code() {
    let mut remote = remote_row("w_1");
    remote.state = RemoteSessionState::Exited { code: Some(130) };
    let row = session_row("tower", &remote, 0);
    assert!(matches!(
        row.state,
        crate::worker::pty::PtyState::Exited { code: Some(130) }
    ));
}

#[test]
fn a_named_attention_kind_survives_the_wire() {
    let mut remote = remote_row("w_1");
    remote.attention = Some(RemoteAttention {
        summary: "allow this command?".to_string(),
        kind: "approval".to_string(),
    });
    let row = session_row("tower", &remote, 0);
    let attention = row.attention.expect("the cue should cross");
    assert_eq!(attention.kind, crate::worker::pty::AttentionKind::Approval);
    assert_eq!(attention.what, "allow this command?");
}

#[test]
fn an_unknown_attention_kind_still_raises_a_cue() {
    // A newer peer naming a kind this build has never heard of is still a
    // harness waiting on somebody. Dropping the cue would hide a stuck session,
    // which is the one thing the rail exists to surface.
    let mut remote = remote_row("w_1");
    remote.attention = Some(RemoteAttention {
        summary: "something new".to_string(),
        kind: "teleport-consent".to_string(),
    });
    let row = session_row("tower", &remote, 0);
    assert!(
        row.attention.is_some(),
        "an unrecognized kind must not silence the cue"
    );
}

/// An app with one connected remote host holding `rows`.
fn app_with_host(rows: Vec<RemoteSessionRow>) -> crate::ui::app::App {
    let rt: std::sync::Arc<dyn medulla::runtime::Runtime> =
        std::sync::Arc::new(medulla::runtime::mock::MockRuntime::demo());
    let mut loaded = medulla::config::LoadedConfig::defaults("medulla.tui.json".into());
    loaded.config.remote_hosts = vec![medulla::config::RemoteHostSection {
        id: "tower".to_string(),
        host: "tower.local".to_string(),
        enabled: true,
        ..medulla::config::RemoteHostSection::default()
    }];
    let mut app = crate::ui::app::App::new(rt, loaded);
    app.remote_host_sessions("tower", rows);
    app
}

#[test]
fn remote_sessions_reach_the_rail_even_when_this_device_hosts_nothing() {
    // A remote session runs somewhere else, so the local host's absence says
    // nothing about it. Before this, `own_session_rows` returned early on a
    // device that was not hosting and every remote session vanished with it.
    let app = app_with_host(vec![remote_row("w_1"), remote_row("w_2")]);
    assert!(
        app.local_sessions_for_test().is_none(),
        "this fixture deliberately has no local host"
    );
    let ids: Vec<String> = app
        .own_session_rows_for_test()
        .into_iter()
        .map(|row| row.id)
        .collect();
    assert!(ids.contains(&"tower/w_1".to_string()));
    assert!(ids.contains(&"tower/w_2".to_string()));
}

#[test]
fn a_remote_screen_is_only_shown_for_the_session_being_watched() {
    // The host may already have had a frame in flight when the pane moved.
    // Drawing it under the new session's title would be a lie the operator has
    // no way to notice.
    let mut app = app_with_host(vec![remote_row("w_1"), remote_row("w_2")]);
    app.session_watch_for_test("tower/w_1");
    app.remote_host_screen("tower", "w_2", blank_snapshot());
    assert!(
        app.session_screen_for_test("tower/w_1").is_none(),
        "a frame for another session must not fill this pane"
    );
    app.remote_host_screen("tower", "w_1", blank_snapshot());
    assert!(app.session_screen_for_test("tower/w_1").is_some());
}

#[test]
fn watching_a_second_session_clears_the_first_ones_screen() {
    let mut app = app_with_host(vec![remote_row("w_1"), remote_row("w_2")]);
    app.session_watch_for_test("tower/w_1");
    app.remote_host_screen("tower", "w_1", blank_snapshot());
    app.session_watch_for_test("tower/w_2");
    assert!(
        app.session_screen_for_test("tower/w_2").is_none(),
        "the held grid belongs to the session we stopped watching"
    );
}

#[test]
fn writing_to_a_disconnected_host_says_so_rather_than_appearing_to_work() {
    let mut app = app_with_host(vec![remote_row("w_1")]);
    let error = app
        .session_write_for_test("tower/w_1", b"ls\r")
        .expect_err("a host with no connection cannot accept input");
    assert!(error.contains("not connected"), "unhelpful error: {error}");
}

/// A 1×1 blank screen, enough to stand in for a frame.
fn blank_snapshot() -> crate::worker::pty::ScreenSnapshot {
    crate::worker::pty::ScreenSnapshot {
        cells: vec![vec![crate::worker::pty::ScreenCell {
            text: crate::worker::pty::CellText::from(" "),
            fg: vt100::Color::Default,
            bg: vt100::Color::Default,
            bold: false,
            italic: false,
            underline: false,
            inverse: false,
        }]],
        cursor: (0, 0),
        hide_cursor: false,
    }
}

#[test]
fn a_remote_harness_keeps_the_name_and_id_its_host_gave_it() {
    // Two shells on the far side are two different rows there, and must be two
    // different rows here. Deriving the label from the provider instead collapsed
    // both to "Shell", and sent back an id the host could not resolve — which is
    // how a picker ends up offering the same thing twice and starting neither.
    let rt: std::sync::Arc<dyn medulla::runtime::Runtime> =
        std::sync::Arc::new(medulla::runtime::mock::MockRuntime::demo());
    let mut loaded = medulla::config::LoadedConfig::defaults("medulla.tui.json".into());
    loaded.config.remote_hosts = vec![medulla::config::RemoteHostSection {
        id: "tower".to_string(),
        host: "tower.local".to_string(),
        enabled: true,
        ..medulla::config::RemoteHostSection::default()
    }];
    let mut app = crate::ui::app::App::new(rt, loaded);
    app.remote_host_connected(
        "tower",
        medulla::protocol::RemoteCapabilities {
            version: "0.12.0".to_string(),
            harnesses: vec![
                medulla::protocol::RemoteHarnessChoice {
                    id: "bash".to_string(),
                    provider: "shell".to_string(),
                    preset: None,
                    display_name: "bash".to_string(),
                },
                medulla::protocol::RemoteHarnessChoice {
                    id: "sh".to_string(),
                    provider: "shell".to_string(),
                    preset: None,
                    display_name: "sh".to_string(),
                },
            ],
            workspace: "/work".to_string(),
            workspaces: Vec::new(),
            host_name: "tower".to_string(),
        },
    );

    let choices = app.remote_choices_for_test("tower");
    let labels: Vec<&str> = choices.iter().map(|c| c.display_name()).collect();
    assert_eq!(labels, ["bash", "sh"], "each shell keeps its own name");
    let ids: Vec<&str> = choices.iter().map(|c| c.id()).collect();
    assert_eq!(
        ids,
        ["bash", "sh"],
        "the id sent back must be the one the host offered"
    );
}

#[test]
fn a_running_remote_session_can_be_attached_to() {
    // `is_running` asked only the local pty manager, which has never heard of a
    // remote session — so attaching to one reported "that session has exited"
    // and silently did nothing, while the session ran perfectly well.
    let mut app = app_with_host(vec![remote_row("w_1")]);
    app.attach_to_session_for_test("tower/w_1");
    assert_eq!(
        app.attached_session(),
        Some("tower/w_1"),
        "a running remote session must be typeable: {}",
        app.status()
    );
}

#[test]
fn an_exited_remote_session_cannot_be_attached_to() {
    let mut exited = remote_row("w_1");
    exited.state = RemoteSessionState::Exited { code: Some(0) };
    let mut app = app_with_host(vec![exited]);
    app.attach_to_session_for_test("tower/w_1");
    assert_eq!(app.attached_session(), None);
    assert!(app.status().contains("exited"), "got: {}", app.status());
}

#[test]
fn closing_a_remote_session_sends_a_close_request_to_its_host() {
    // Before this, `close_session` only ever closed
    // `self.local_sessions` — a qualified remote id such as `tower/w_1`
    // reached `harnesses.sessions.close(session)`, which is the *local* pty
    // manager, with the qualified id itself as the key. It has never heard of
    // that id, so nothing closed and the child kept running until something
    // else killed it.
    let mut app = app_with_host(vec![remote_row("w_1")]);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.remote_host_dialling("tower", tx);
    app.pane_session = Some("tower/w_1".to_string());

    app.close_pane_session_prompt();
    assert_eq!(
        app.harness_close_armed.as_deref(),
        Some("tower/w_1"),
        "a running remote session must be closeable, not reported as already exited: {}",
        app.status()
    );

    app.close_session("tower/w_1");
    assert_eq!(app.status(), "Closed the harness");
    let sent = rx.try_recv().expect("a Close request must reach the host");
    assert!(
        matches!(sent, crate::remote::client::RemoteRequest::Close(id) if id == "w_1"),
        "the request must carry the host's own (unprefixed) session id"
    );
}

#[test]
fn closing_a_remote_session_on_a_disconnected_host_says_so() {
    // A host with rows but no live connection (bootstrap in flight, or the link
    // dropped) must not silently no-op and claim success.
    let mut app = app_with_host(vec![remote_row("w_1")]);
    app.close_session("tower/w_1");
    assert!(
        app.status().contains("not connected"),
        "got: {}",
        app.status()
    );
}

#[test]
fn an_attention_cue_is_timestamped_by_this_client_not_the_sender() {
    // `HarnessAttention::label` subtracts `since` from *our* clock, so carrying
    // the sender's would make a fresh prompt on a skewed host read as stuck for
    // hours — or pinned at zero forever.
    let mut waiting = remote_row("w_1");
    waiting.attention = Some(RemoteAttention {
        summary: "allow this command?".to_string(),
        kind: "approval".to_string(),
    });
    // A sender timestamp far in the past; if it crossed, the cue would look
    // ancient.
    waiting.last_output_at = 1_000;

    let mut app = app_with_host(vec![waiting.clone()]);
    let before = medulla::clock::now_millis();
    app.remote_host_sessions("tower", vec![waiting.clone()]);

    let row = app
        .session_row_for_test("tower/w_1")
        .expect("the session is listed");
    let since = row.attention.expect("the cue crosses").since;
    assert!(
        since >= before - 1_000,
        "the cue should be stamped locally, got {since} against a local clock near {before}"
    );

    // And the stamp holds while the same cue does, rather than resetting on
    // every session-list update.
    app.remote_host_sessions("tower", vec![waiting.clone()]);
    let again = app
        .session_row_for_test("tower/w_1")
        .expect("still listed")
        .attention
        .expect("still asking")
        .since;
    assert_eq!(
        since, again,
        "an unchanged cue must keep its first-seen moment"
    );

    // A different cue starts again.
    let mut changed = waiting.clone();
    changed.attention = Some(RemoteAttention {
        summary: "something else".to_string(),
        kind: "approval".to_string(),
    });
    app.remote_host_sessions("tower", vec![changed]);
    let restamped = app
        .session_row_for_test("tower/w_1")
        .expect("still listed")
        .attention
        .expect("still asking")
        .since;
    assert!(restamped >= since, "a new cue should restamp, not inherit");
}

#[test]
fn an_exited_remote_session_reports_that_it_cannot_be_dismissed_here() {
    // The local half of `k` forgets an exited row. A remote one cannot be:
    // `remote_session_rows` is rebuilt from what the host reports, so a local
    // forget would be undone by the next update. Nothing pretends otherwise —
    // an earlier version of this branch claimed the host would retire the row,
    // which is false for a headless host (`sweep_finished_sessions` runs only
    // from the interactive frame loop, and `Serve::current_rows` publishes
    // `PtyManager::rows()` verbatim). See #307.
    let mut exited = remote_row("w_1");
    exited.state = RemoteSessionState::Exited { code: Some(1) };
    let mut app = app_with_host(vec![exited]);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.remote_host_dialling("tower", tx);
    app.pane_session = Some("tower/w_1".to_string());

    app.close_pane_session_prompt();

    assert!(
        app.harness_close_armed.is_none(),
        "a settled child has nothing to confirm: {}",
        app.status()
    );
    assert!(
        app.status().contains("cannot be dismissed"),
        "got: {}",
        app.status()
    );
    assert!(
        rx.try_recv().is_err(),
        "nothing is sent to the host: it has no request that forgets a row"
    );
}
