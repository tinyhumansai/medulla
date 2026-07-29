//! Tests for the session-facing half of [`LocalHarnesses`], against a real
//! child on a real pseudo-terminal.
//!
//! `/bin/sh` stands in for a coding agent: it is a genuine pty client with a
//! genuine terminal, so reads, resizes, writes and exit detection are exercised
//! exactly as they will be against `claude`, while staying fast, offline, and
//! deterministic.
//!
//! Unix-only, for the same reason the pty layer's own tests are: Windows has no
//! `/bin/sh` to drive.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use medulla::tinyplace::HarnessProvider;

use crate::worker::pty::{LaunchSpec, PtyManager};

use super::LocalHarnesses;

/// A spec that runs `sh -c <script>` on a pty.
///
/// Codex rather than Claude: Claude's interactive argv carries a minted
/// `--session-id`, which `/bin/sh` would reject as an unknown option. Codex
/// takes no preset id, so its argv is empty and the script is the whole command.
fn sh(script: &str) -> LaunchSpec {
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    LaunchSpec {
        provider: HarnessProvider::Codex,
        bin: "/bin/sh".to_string(),
        cwd: "/".to_string(),
        env,
        extra_args: vec!["-c".to_string(), script.to_string()],
        skip_permissions: false,
        label: "test".to_string(),
        session_id: None,
    }
}

/// A [`LocalHarnesses`] over `sessions`, with a runtime that serves no tasks.
///
/// Task resolution needs a live host and is covered by the daemon's own screen
/// e2e; everything here is about what the pane does *once* a session is named,
/// so the runtime is inert on purpose.
fn harnesses(sessions: PtyManager) -> LocalHarnesses {
    let config = medulla::daemon::DaemonConfig {
        providers: vec![HarnessProvider::Codex],
        default_provider: HarnessProvider::Codex,
        workspace: "/".to_string(),
        accessible_dirs: Vec::new(),
        env: HashMap::new(),
        task_timeout_ms: 1_000,
        capability_timeout_ms: None,
        concurrency: 1,
        status_throttle_ms: 1_000,
        max_pending: 1,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        router: None,
        budget: None,
    };
    let run_task: medulla::daemon::providers::RunTaskFn =
        std::sync::Arc::new(|_| Box::pin(async { Err("not used in these tests".to_string()) }));
    let send: medulla::daemon::SendFn = std::sync::Arc::new(|_, _| {
        Box::pin(async {}) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    });
    LocalHarnesses {
        sessions,
        runtime: medulla::daemon::DaemonRuntime::new(config, run_task, send),
        hub_address: "medulla-orchestrator".to_string(),
    }
}

/// Spin until `check` passes or the deadline expires.
///
/// The budget is far larger than these conditions actually need: real children
/// on real ptys are at the mercy of machine load, and a tight deadline turns
/// "the box was busy" into a red test.
fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out after 30s waiting for: {what}");
}

/// The whole screen as one string.
fn text(harnesses: &LocalHarnesses, id: &str) -> String {
    harnesses
        .screen(id)
        .expect("the session has a screen")
        .cells
        .iter()
        .map(|row| {
            row.iter()
                .map(|c| c.text.as_str())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_running_harness_screen_is_readable_from_the_pane() {
    let sessions = PtyManager::new();
    let harnesses = harnesses(sessions.clone());
    let id = sessions.open(sh("echo pane-sees-this; sleep 30")).unwrap();

    wait_for("the child's output to reach the emulator", || {
        text(&harnesses, &id).contains("pane-sees-this")
    });
    assert!(harnesses.is_running(&id));
    sessions.close(&id);
}

#[test]
fn fitting_the_pane_reflows_the_child_to_the_pane_size() {
    let sessions = PtyManager::new();
    let harnesses = harnesses(sessions.clone());
    // `tput cols` asks the terminal how wide it is, so the child's own answer
    // proves the resize reached the pty and not only the emulator.
    let id = sessions.open(sh("sleep 0.4; tput cols; sleep 30")).unwrap();

    harnesses.fit(&id, 100, 24);
    wait_for("the child to report the pane's width", || {
        text(&harnesses, &id).contains("100")
    });

    let snapshot = harnesses.screen(&id).expect("a screen");
    assert_eq!(snapshot.cells.len(), 24, "the emulator moved with the pty");
    assert_eq!(snapshot.cells[0].len(), 100);
    sessions.close(&id);
}

#[test]
fn a_zero_sized_pane_is_ignored_rather_than_collapsing_the_child() {
    let sessions = PtyManager::new();
    let harnesses = harnesses(sessions.clone());
    let id = sessions.open(sh("sleep 30")).unwrap();

    harnesses.fit(&id, 90, 20);
    // A pane can momentarily measure zero mid-layout; resizing a pty to it
    // would tell the harness it has no screen at all.
    harnesses.fit(&id, 0, 0);

    let snapshot = harnesses.screen(&id).expect("a screen");
    assert_eq!(snapshot.cells.len(), 20);
    assert_eq!(snapshot.cells[0].len(), 90);
    sessions.close(&id);
}

#[test]
fn typing_reaches_the_child_as_the_bytes_a_terminal_would_have_sent() {
    let sessions = PtyManager::new();
    let harnesses = harnesses(sessions.clone());
    let id = sessions
        .open(sh("read line; echo got:$line; sleep 30"))
        .unwrap();

    wait_for("the shell to be reading", || harnesses.is_running(&id));
    // Carriage return, not newline: this is what the encoder emits for Enter,
    // and it is what a raw-mode reader accepts as end-of-line.
    harnesses
        .write(&id, b"hello\r")
        .expect("the pty accepts input");

    wait_for("the child to echo what was typed", || {
        text(&harnesses, &id).contains("got:hello")
    });
    sessions.close(&id);
}

#[test]
fn an_exited_harness_stops_being_attachable_but_keeps_its_last_screen() {
    let sessions = PtyManager::new();
    let harnesses = harnesses(sessions.clone());
    let id = sessions.open(sh("echo last-words")).unwrap();

    wait_for("the child to exit", || !harnesses.is_running(&id));
    // The screen survives the child: an operator usually wants to read how it
    // ended, and a pane that blanks on exit throws that away.
    assert!(text(&harnesses, &id).contains("last-words"));
    // Writing to it fails rather than silently vanishing, which is what tells
    // the attached pane to release the keyboard.
    assert!(harnesses.write(&id, b"x").is_err());
}

#[test]
fn an_unknown_session_resolves_to_nothing_rather_than_panicking() {
    let harnesses = harnesses(PtyManager::new());
    assert!(harnesses.screen("w_nope").is_none());
    assert!(!harnesses.is_running("w_nope"));
    assert!(harnesses.write("w_nope", b"x").is_err());
    // A resize against a session that is gone is a no-op, not a crash: the
    // render pass fits every frame and the child can exit between frames.
    harnesses.fit("w_nope", 80, 24);
}

#[test]
fn a_child_that_asks_for_the_mouse_has_wheel_notches_forwarded_to_it() {
    let sessions = PtyManager::new();
    let harnesses = harnesses(sessions.clone());
    // Turn on SGR mouse reporting (1000 = report presses, 1006 = SGR encoding)
    // the way a real harness does, then echo whatever arrives on stdin.
    let id = sessions
        .open(sh(
            "printf '\\033[?1000h\\033[?1006h'; sleep 0.3; cat -v; sleep 30",
        ))
        .unwrap();

    wait_for("the child to enable mouse reporting", || {
        matches!(
            harnesses.sessions.mouse_protocol(&id),
            Some((mode, _)) if mode != vt100::MouseProtocolMode::None
        )
    });

    harnesses.scroll(&id, 3, 4, true, 3);

    // `cat -v` prints control bytes visibly, so the report the child received is
    // readable straight off its own screen. 1-based: pane (3,4) is 4;5.
    wait_for("the child to receive the wheel report", || {
        text(&harnesses, &id).contains("[<64;4;5M")
    });
    sessions.close(&id);
}

#[test]
fn a_child_that_never_asked_for_the_mouse_gets_our_scrollback_instead() {
    let sessions = PtyManager::new();
    let harnesses = harnesses(sessions.clone());
    // Enough lines to push history off a 30-row screen, and no mouse reporting.
    let id = sessions
        .open(sh(
            "i=1; while [ $i -le 200 ]; do echo line-$i; i=$((i+1)); done; sleep 30",
        ))
        .unwrap();

    wait_for("the child to scroll its output off the screen", || {
        text(&harnesses, &id).contains("line-200")
    });
    assert!(
        matches!(
            harnesses.sessions.mouse_protocol(&id),
            Some((vt100::MouseProtocolMode::None, _))
        ),
        "a plain shell asks for no mouse reporting"
    );

    // Nothing is written to the child — the wheel moves our own emulator. The
    // 40 exceeds both the screen height and what vt100 can safely offset by; it
    // must clamp rather than panic inside the crate's `visible_rows`.
    harnesses.scroll(&id, 0, 0, true, 40);
    let scrolled = text(&harnesses, &id);
    assert!(
        !scrolled.contains("line-200"),
        "the view should have moved back into history:\n{scrolled}"
    );

    // And typing snaps it back, so a harness answering below is not missed.
    harnesses.scroll_to_live(&id);
    assert!(text(&harnesses, &id).contains("line-200"));
    sessions.close(&id);
}

#[test]
fn scrolling_down_at_the_live_edge_stays_put_rather_than_underflowing() {
    let sessions = PtyManager::new();
    let harnesses = harnesses(sessions.clone());
    let id = sessions.open(sh("echo hello; sleep 30")).unwrap();

    wait_for("the child to paint", || {
        text(&harnesses, &id).contains("hello")
    });
    // Already at the bottom; scrolling further down must not wrap into a huge
    // offset and blank the pane.
    harnesses.scroll(&id, 0, 0, false, 500);
    assert!(text(&harnesses, &id).contains("hello"));
    sessions.close(&id);
}
