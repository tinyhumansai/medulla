//! What the server does for a connected client: hello, open, close, input,
//! resize, and who may watch or type into what.
//!
//! These start **real ptys running real POSIX shells** and speak the real wire
//! protocol to them; only the transport is swapped for an in-memory bridge.

use super::support::*;

#[tokio::test]
async fn hello_is_answered_with_what_this_host_can_do() {
    let (mut server, client) = fixture("/tmp");
    say(
        &mut server,
        &encode_remote_message(&RemoteMessage::Hello {
            client_version: "0.12.0".to_string(),
        }),
    )
    .await;

    let replies = remote_replies(&client).await;
    let capabilities = replies
        .iter()
        .find_map(|message| match message {
            RemoteMessage::Capabilities(capabilities) => Some(capabilities),
            _ => None,
        })
        .expect("hello is answered with capabilities");
    assert_eq!(capabilities.host_name, "test-host");
    assert!(
        capabilities.harnesses.iter().any(|c| c.provider == "shell"),
        "a host with a shell must offer one"
    );
    assert!(
        replies
            .iter()
            .any(|message| matches!(message, RemoteMessage::Sessions { .. })),
        "a client that says hello should learn what is already running"
    );
}

#[tokio::test]
async fn a_client_can_only_start_what_this_host_advertised() {
    // The whole authorization story for opening a session. The choice is
    // resolved by id against what this host offers, never rebuilt from what the
    // client sent — so a client cannot name an arbitrary binary or a provider
    // this machine deliberately does not serve.
    let (mut server, client) = fixture("/tmp");
    say(
        &mut server,
        &encode_remote_message(&RemoteMessage::Open {
            request_id: "r1".to_string(),
            harness: RemoteHarnessChoice {
                id: "shell:/usr/bin/evil".to_string(),
                provider: "shell".to_string(),
                preset: None,
                display_name: "evil".to_string(),
            },
            workspace: String::new(),
            cols: 80,
            rows: 24,
            name: None,
        }),
    )
    .await;

    let refused = remote_replies(&client)
        .await
        .into_iter()
        .any(|message| matches!(message, RemoteMessage::OpenFailed { .. }));
    assert!(refused, "an unadvertised harness must be refused");
    assert!(
        server.pty().rows().is_empty(),
        "nothing may have been started"
    );
}

#[tokio::test]
async fn opening_a_shell_starts_it_and_lists_it() {
    let (mut server, client) = fixture("/tmp");
    let session_id = open_shell(&mut server, &client).await;
    let rows = server.pty().rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, session_id);
    assert_eq!(rows[0].provider, HarnessProvider::Shell);
}

#[tokio::test]
async fn input_reaches_the_pty_and_the_screen_comes_back() {
    // The whole round trip in one test: the client types, the far-side shell
    // runs it, and the output arrives as a frame the client could render.
    let (mut server, client) = fixture("/tmp");
    let session_id = open_shell(&mut server, &client).await;

    say(
        &mut server,
        &encode_screen_message(&ScreenMessage::Subscribe {
            task_id: session_id.clone(),
            max_fps: 10,
            resync: true,
        }),
    )
    .await;
    assert_eq!(server.watching(), 1);

    say(
        &mut server,
        &encode_screen_message(&ScreenMessage::input(
            &session_id,
            b"echo mosh-works-here\n",
        )),
    )
    .await;

    let id = session_id.clone();
    wait_for("the shell to echo the marker", &mut server, |server| {
        screen_text(server, &id).contains("mosh-works-here")
    })
    .await;

    // And the client is actually sent a frame carrying it.
    let mut seen = false;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline && !seen {
        server.publish_screens_once().await;
        for message in client.drain_inbox(64).await {
            if let Some(ScreenMessage::Frame(frame)) =
                medulla::protocol::parse_screen_message(&message.text)
            {
                let text: String = frame
                    .rows_changed
                    .iter()
                    .flat_map(|row| row.runs.iter().map(|run| run.text.clone()))
                    .collect();
                if text.contains("mosh-works-here") {
                    seen = true;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        seen,
        "the client was never sent a frame carrying the output"
    );
}

#[tokio::test]
async fn input_is_refused_for_a_session_the_client_is_not_watching() {
    // A subscription is the one thing that says which pane a peer has open, so
    // it is what input is authorized against. Without this check any connected
    // client could type into any session on the host.
    let (mut server, client) = fixture("/tmp");
    let session_id = open_shell(&mut server, &client).await;

    say(
        &mut server,
        &encode_screen_message(&ScreenMessage::input(&session_id, b"echo never-typed\n")),
    )
    .await;

    // Give a real shell every chance to have run it, then assert it did not.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !screen_text(&server, &session_id).contains("never-typed"),
        "input without a subscription must not reach the pty"
    );
}

#[tokio::test]
async fn a_second_subscribe_replaces_the_first() {
    // Channel 1 is one grid per peer, so a client watches one session at a time.
    let (mut server, client) = fixture("/tmp");
    let first = open_shell(&mut server, &client).await;
    let second = open_shell(&mut server, &client).await;
    assert_ne!(first, second);

    for id in [&first, &second] {
        say(
            &mut server,
            &encode_screen_message(&ScreenMessage::Subscribe {
                task_id: id.clone(),
                max_fps: 10,
                resync: true,
            }),
        )
        .await;
    }
    assert_eq!(server.watching(), 1, "one client watches one screen");
    assert_eq!(server.watching_session(CLIENT), Some(second.as_str()));
}

#[tokio::test]
async fn an_unwatched_session_still_appears_in_the_session_list() {
    // The corollary of one-screen-per-client: everything else stays visible in
    // the rail, because the list rides the reliable channel rather than the
    // screen one.
    let (mut server, client) = fixture("/tmp");
    let first = open_shell(&mut server, &client).await;
    let second = open_shell(&mut server, &client).await;
    say(
        &mut server,
        &encode_screen_message(&ScreenMessage::Subscribe {
            task_id: second.clone(),
            max_fps: 10,
            resync: true,
        }),
    )
    .await;

    server.publish_sessions(CLIENT).await;
    let listed: Vec<String> = remote_replies(&client)
        .await
        .into_iter()
        .find_map(|message| match message {
            RemoteMessage::Sessions { rows } => Some(rows.iter().map(|r| r.id.clone()).collect()),
            _ => None,
        })
        .expect("a session list");
    assert!(
        listed.contains(&first),
        "the unwatched session is still listed"
    );
    assert!(listed.contains(&second));
}

#[tokio::test]
async fn closing_a_session_ends_it() {
    let (mut server, client) = fixture("/tmp");
    let session_id = open_shell(&mut server, &client).await;
    say(
        &mut server,
        &encode_remote_message(&RemoteMessage::Close {
            session_id: session_id.clone(),
        }),
    )
    .await;

    let id = session_id.clone();
    wait_for("the shell to exit", &mut server, |server| {
        !matches!(
            server.pty().row(&id).map(|row| row.state),
            Some(crate::worker::pty::PtyState::Running)
        )
    })
    .await;
    assert_eq!(server.watching(), 0);
}

#[tokio::test]
async fn an_exit_nobody_asked_for_still_updates_the_session_list() {
    // A row can go stale without any `Close`/`Kill` behind it at all: the child
    // just exits on its own, exactly as it would if the operator typed `exit`.
    // `publish_sessions_if_changed` is what a real `run()` tick calls, and this
    // is the case it exists for — the periodic tick is the only thing that will
    // ever notice this exit, since nothing else asks.
    let (mut server, client) = fixture("/tmp");
    let session_id = open_shell(&mut server, &client).await;
    say(
        &mut server,
        &encode_screen_message(&ScreenMessage::Subscribe {
            task_id: session_id.clone(),
            max_fps: 10,
            resync: true,
        }),
    )
    .await;
    // Drain the replies from opening and subscribing before asserting on what
    // the tick alone produces.
    remote_replies(&client).await;

    say(
        &mut server,
        &encode_screen_message(&ScreenMessage::input(&session_id, b"exit\n")),
    )
    .await;

    let id = session_id.clone();
    wait_for("the shell to exit on its own", &mut server, |server| {
        !matches!(
            server.pty().row(&id).map(|row| row.state),
            Some(crate::worker::pty::PtyState::Running)
        )
    })
    .await;

    server.publish_sessions_if_changed().await;
    let rows = remote_replies(&client)
        .await
        .into_iter()
        .find_map(|message| match message {
            RemoteMessage::Sessions { rows } => Some(rows),
            _ => None,
        })
        .expect("the exit must be published without any Close/Kill request");
    let row = rows
        .iter()
        .find(|row| row.id == session_id)
        .expect("the exited session stays listed");
    assert!(
        !matches!(row.state, medulla::protocol::RemoteSessionState::Running),
        "the published row must reflect that the shell exited, not stay Running forever"
    );

    // And a second call with nothing new to say publishes nothing — the whole
    // point of diffing against `last_rows`.
    server.publish_sessions_if_changed().await;
    assert!(
        remote_replies(&client).await.is_empty(),
        "an unchanged session list must not be resent"
    );
}

#[tokio::test]
async fn a_resize_reaches_the_emulator() {
    let (mut server, client) = fixture("/tmp");
    let session_id = open_shell(&mut server, &client).await;
    say(
        &mut server,
        &encode_screen_message(&ScreenMessage::Subscribe {
            task_id: session_id.clone(),
            max_fps: 10,
            resync: true,
        }),
    )
    .await;
    say(
        &mut server,
        &encode_screen_message(&ScreenMessage::Resize {
            task_id: session_id.clone(),
            cols: 100,
            rows: 40,
        }),
    )
    .await;

    let snapshot = server
        .pty()
        .screen_rows(&session_id)
        .expect("the session has a screen");
    assert_eq!(snapshot.cells.len(), 40);
    assert_eq!(snapshot.cells[0].len(), 100);
}

#[tokio::test]
async fn an_unknown_body_is_dropped_rather_than_crashing_the_loop() {
    // Inbound bodies are untrusted. A peer sending junk must cost that message
    // and nothing else.
    let (mut server, _client) = fixture("/tmp");
    for body in ["", "not json", "{}", r#"{"kind":"open"}"#] {
        say(&mut server, body).await;
    }
    assert!(server.pty().rows().is_empty());
}
