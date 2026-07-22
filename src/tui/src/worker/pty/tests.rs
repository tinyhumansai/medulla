//! Tests for the PTY session layer.
//!
//! These drive a real child on a real pseudo-terminal — `/bin/sh`, not a coding
//! agent, so they stay fast, offline, and deterministic while still exercising
//! the parts that actually break: pty allocation, the reader thread, emulator
//! parsing, resize, input, and reaping.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla::tinyplace::HarnessProvider;

use super::manager::PtyManager;
use super::types::{LaunchSpec, PtyState};

/// A spec that runs `sh -c <script>` on a pty.
fn sh(script: &str) -> LaunchSpec {
    let mut env = HashMap::new();
    // A pty child with no PATH cannot exec anything useful.
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    LaunchSpec {
        // Codex, not Claude: claude now gets a minted `--session-id`, which
        // `/bin/sh` would reject as an unknown option. Codex takes no preset id,
        // so its interactive argv is empty and the script is the whole command.
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

/// Spin until `check` passes or the deadline expires.
///
/// The budget is deliberately far larger than the milliseconds these conditions
/// actually need. Real children on real ptys are at the mercy of machine load,
/// and a tight deadline turns "the box was busy" into a red test — which is
/// worse than useless, because it trains you to re-run rather than read.
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
fn screen_text(manager: &PtyManager, id: &str) -> String {
    manager
        .screen_rows(id)
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
fn a_childs_output_reaches_the_emulator_screen() {
    let manager = PtyManager::new();
    // `interactive_args` prepends nothing for codex (see `sh`), so extra_args is
    // the whole argv.
    let id = manager.open(sh("echo hello-from-pty; sleep 30")).unwrap();
    wait_for("output on screen", || {
        screen_text(&manager, &id).contains("hello-from-pty")
    });
    manager.close(&id);
}

#[test]
fn ansi_colour_is_parsed_into_cell_attributes() {
    // The whole point of the emulator: a harness paints with escape sequences,
    // and they must survive as attributes rather than as literal text.
    let manager = PtyManager::new();
    let id = manager
        .open(sh("printf '\\033[1;31mRED\\033[0m'; sleep 30"))
        .unwrap();
    wait_for("red text", || screen_text(&manager, &id).contains("RED"));

    let snapshot = manager.screen_rows(&id).unwrap();
    let cell = &snapshot.cells[0][0];
    assert_eq!(cell.text, "R");
    assert!(cell.bold, "bold attribute must survive parsing");
    assert_eq!(cell.fg, vt100::Color::Idx(1), "red must survive parsing");
    assert!(
        !screen_text(&manager, &id).contains("\u{1b}"),
        "escape sequences must not leak through as literal text"
    );
    manager.close(&id);
}

#[tokio::test]
async fn typed_input_reaches_the_child() {
    let manager = PtyManager::new();
    let id = manager
        .open(sh("read line; echo GOT:$line; sleep 30"))
        .unwrap();
    super::inject::inject_prompt(&manager, &id, "ping")
        .await
        .expect("input is accepted");
    wait_for("echoed input", || {
        screen_text(&manager, &id).contains("GOT:ping")
    });
    manager.close(&id);
}

#[tokio::test]
async fn a_child_that_never_asked_for_bracketed_paste_is_not_sent_the_markers() {
    // Regression. The markers used to go out unconditionally. `/bin/sh` never
    // enables the mode, so `read` received `ESC[200~ping ESC[201~` as the line —
    // and a real harness in the same state takes the `ESC` as a keypress that
    // can clear its composer, which is why a prompt could arrive looking typed
    // but never run. The assertion below is on the *bytes the child read*, not
    // on the rendered screen: our own emulator swallows the escapes on echo, so
    // a screen-level check passes either way and proves nothing.
    let manager = PtyManager::new();
    let id = manager
        .open(sh(
            "read line; case $line in ping) echo CLEAN;; *) echo DIRTY;; esac; sleep 30",
        ))
        .unwrap();
    assert_eq!(
        manager.bracketed_paste(&id),
        Some(false),
        "sh never enables the mode"
    );
    super::inject::inject_prompt(&manager, &id, "ping")
        .await
        .expect("input is accepted");
    wait_for("the child reported what it read", || {
        let text = screen_text(&manager, &id);
        text.contains("CLEAN") || text.contains("DIRTY")
    });
    assert!(
        screen_text(&manager, &id).contains("CLEAN"),
        "the child must read exactly `ping`, with no paste markers around it"
    );
    manager.close(&id);
}

#[test]
fn resizing_moves_the_emulator_and_is_idempotent() {
    let manager = PtyManager::new();
    let id = manager.open(sh("sleep 30")).unwrap();
    manager.resize(&id, 100, 20);
    let snapshot = manager.screen_rows(&id).unwrap();
    assert_eq!(snapshot.cells.len(), 20, "rows follow the pane");
    assert_eq!(snapshot.cells[0].len(), 100, "cols follow the pane");
    // A repeat resize is a no-op rather than another SIGWINCH.
    manager.resize(&id, 100, 20);
    assert_eq!(manager.screen_rows(&id).unwrap().cells.len(), 20);
    manager.close(&id);
}

#[test]
fn a_zero_sized_resize_is_ignored() {
    // ratatui hands out zero-height rects while a pane is collapsed; passing
    // that to the pty would wedge the child.
    let manager = PtyManager::new();
    let id = manager.open(sh("sleep 30")).unwrap();
    manager.resize(&id, 0, 0);
    assert_eq!(manager.screen_rows(&id).unwrap().cells.len(), 30);
    manager.close(&id);
}

#[test]
fn an_exited_child_is_reaped_and_keeps_its_last_screen() {
    let manager = PtyManager::new();
    let id = manager.open(sh("echo done-here")).unwrap();
    wait_for("exit recorded", || {
        !manager.row(&id).unwrap().state.is_running()
    });
    assert!(
        matches!(manager.row(&id).unwrap().state, PtyState::Exited { .. }),
        "the child must be reaped, not left running"
    );
    assert!(
        screen_text(&manager, &id).contains("done-here"),
        "the last screen stays readable after exit"
    );
    manager.close(&id);
}

#[test]
fn a_failed_spawn_reports_rather_than_panicking() {
    let manager = PtyManager::new();
    let mut spec = sh("true");
    spec.bin = "/nonexistent/harness".to_string();
    let error = manager.open(spec).expect_err("a missing binary must fail");
    assert!(error.contains("/nonexistent/harness"), "got: {error}");
    assert!(
        manager.rows().is_empty(),
        "no half-open session is left behind"
    );
}

#[tokio::test]
async fn a_child_that_enables_the_mode_is_sent_the_markers_and_waited_for() {
    // The complement of the test above: honouring the child's mode has to cut
    // both ways, or "never bracket" would pass one test and quietly break every
    // multi-line prompt, whose embedded newlines would each submit a fragment.
    // The script sets the mode before reading, so `await_ready` must observe it
    // and take the bracketed branch.
    let manager = PtyManager::new();
    let id = manager
        .open(sh(
            "printf '\\033[?2004h'; read line; case $line in *200~*) echo BRACKETED;; \
             *) echo BARE;; esac; sleep 30",
        ))
        .unwrap();
    super::inject::inject_prompt(&manager, &id, "ping")
        .await
        .expect("input is accepted");
    wait_for("the child reported what it read", || {
        let text = screen_text(&manager, &id);
        text.contains("BRACKETED") || text.contains("BARE")
    });
    assert!(
        screen_text(&manager, &id).contains("BRACKETED"),
        "a child that turned the mode on must receive the paste markers"
    );
    manager.close(&id);
}

#[tokio::test]
async fn writing_to_an_exited_session_is_refused() {
    let manager = PtyManager::new();
    let id = manager.open(sh("exit 0")).unwrap();
    wait_for("exit recorded", || {
        !manager.row(&id).unwrap().state.is_running()
    });
    // Refused immediately rather than waited out: a dead session will never
    // become ready, so spending the readiness budget on it only delays the
    // error the peer is owed.
    let error = super::inject::inject_prompt(&manager, &id, "hello")
        .await
        .expect_err("a dead session cannot be typed at");
    assert!(error.contains("not running"), "got: {error}");
}

#[test]
fn a_running_session_cannot_be_forgotten() {
    // Dropping the record while the child lives would orphan it holding a pty.
    let manager = PtyManager::new();
    let id = manager.open(sh("sleep 30")).unwrap();
    assert!(!manager.forget(&id));
    assert_eq!(manager.running_count(), 1);

    manager.close(&id);
    assert!(manager.forget(&id), "a closed session can be dropped");
    assert!(manager.rows().is_empty());
}

#[test]
fn sessions_keep_open_order_so_the_cursor_does_not_jump() {
    let manager = PtyManager::new();
    let a = manager.open(sh("sleep 30")).unwrap();
    let b = manager.open(sh("sleep 30")).unwrap();
    let rows = manager.rows();
    assert_eq!(rows[0].id, a);
    assert_eq!(rows[1].id, b);
    assert_eq!(manager.running_count(), 2);
    manager.shutdown();
}

#[test]
fn the_clock_is_injectable() {
    let manager = PtyManager::with_now(Arc::new(|| 4_242));
    let id = manager.open(sh("sleep 30")).unwrap();
    assert_eq!(manager.row(&id).unwrap().started_at, 4_242);
    manager.close(&id);
}

#[test]
fn many_sessions_exiting_at_once_are_all_reaped_without_stalling_reads() {
    // Regression: the reaper used to hold the manager's lock across a blocking
    // `child.wait()`. EOF on the pty master and the child's exit are not
    // simultaneous, so that serialized every reader — in the TUI it showed up as
    // the whole screen freezing whenever a session ended.
    let manager = PtyManager::new();
    let ids: Vec<String> = (0..8)
        .map(|i| manager.open(sh(&format!("echo bye-{i}"))).unwrap())
        .collect();

    // Reads must keep answering while the children are exiting.
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let call = Instant::now();
        let rows = manager.rows();
        assert!(
            call.elapsed() < Duration::from_millis(500),
            "rows() blocked for {:?} — the reaper is holding the lock again",
            call.elapsed()
        );
        if rows.iter().all(|r| !r.state.is_running()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    for id in &ids {
        assert!(
            matches!(manager.row(id).unwrap().state, PtyState::Exited { .. }),
            "{id} was never reaped"
        );
    }
}

// -------------------------------------------------------------- identity ---

#[test]
fn claude_is_handed_a_minted_session_id_and_codex_is_not() {
    use super::launch::{accepts_preset_session_id, interactive_args, mint_session_id};

    // Claude writes its transcript under the id it is given, which turns
    // attribution from a guess into a fact.
    assert!(accepts_preset_session_id(HarnessProvider::Claude));
    let minted = mint_session_id(HarnessProvider::Claude).expect("claude accepts one");
    let args = interactive_args(HarnessProvider::Claude, Some(&minted), false, &[]);
    assert_eq!(args, vec!["--session-id".to_string(), minted.clone()]);

    // Codex has no flag to choose an id for a fresh session; handing it one
    // would be an unknown option, so it must not be offered.
    assert!(!accepts_preset_session_id(HarnessProvider::Codex));
    assert_eq!(mint_session_id(HarnessProvider::Codex), None);
    assert!(interactive_args(HarnessProvider::Codex, Some("ignored"), false, &[]).is_empty());
}

#[test]
fn minted_ids_are_unique_per_session() {
    use super::launch::mint_session_id;
    // Two sessions sharing an id would collide on one transcript — the exact
    // ambiguity minting exists to remove.
    let a = mint_session_id(HarnessProvider::Claude).unwrap();
    let b = mint_session_id(HarnessProvider::Claude).unwrap();
    assert_ne!(a, b);
}

#[test]
fn a_claude_session_records_the_id_it_was_launched_with() {
    let manager = PtyManager::new();
    let mut spec = sh("sleep 30");
    // A real claude would receive `--session-id`; /bin/sh would reject it, so
    // the id is supplied explicitly and the argv left alone.
    spec.session_id = Some("pinned-abc".to_string());
    let id = manager.open(spec).unwrap();

    assert_eq!(
        manager.row(&id).unwrap().session_id.as_deref(),
        Some("pinned-abc"),
        "the pin must be visible to whatever tails this session's transcript"
    );
    manager.shutdown();
}

#[test]
fn a_codex_session_learns_its_id_from_the_rollout_and_keeps_the_first() {
    // Codex cannot be told an id, so its own is only knowable once it has
    // written line one. The first reading wins: a later re-read must not
    // re-point a tailer that is already following a file.
    let manager = PtyManager::new();
    let id = manager.open(sh("sleep 30")).unwrap();
    assert_eq!(
        manager.row(&id).unwrap().session_id,
        None,
        "unknown at spawn"
    );

    manager.record_session_id(&id, "rollout-read-id");
    assert_eq!(
        manager.row(&id).unwrap().session_id.as_deref(),
        Some("rollout-read-id")
    );

    manager.record_session_id(&id, "a-different-id");
    assert_eq!(
        manager.row(&id).unwrap().session_id.as_deref(),
        Some("rollout-read-id"),
        "the first reading is authoritative"
    );
    manager.shutdown();
}

#[tokio::test]
async fn a_session_showing_a_startup_dialog_is_not_typed_at() {
    // The end-to-end half: recognising the dialog is only useful if injection
    // actually consults it. The child echoes whatever it reads, so a prompt
    // typed into the dialog would show up as TYPED: — and the bare Return that
    // used to follow it would have answered a question nobody chose.
    let manager = PtyManager::new();
    let id = manager
        .open(sh("printf '1. Yes, I trust this folder\\r\\n'; \
                  read line; printf 'TYPED:%s\\r\\n' \"$line\"; sleep 30"))
        .unwrap();
    wait_for("dialog painted", || {
        screen_text(&manager, &id).contains("I trust this folder")
    });

    let error = super::inject::inject_prompt(&manager, &id, "ship the fix")
        .await
        .expect_err("a session on a startup dialog must not be typed at");
    assert!(
        error.contains("approve this workspace"),
        "the error must name what is in the way: {error}"
    );
    assert!(
        !screen_text(&manager, &id).contains("TYPED:"),
        "nothing may be typed at a modal — it discards the paste and reads the \
         Return as an answer"
    );
    manager.close(&id);
}

#[tokio::test]
async fn a_dialog_painted_after_the_terminal_modes_is_still_caught() {
    // Regression, measured against a real cold start: Claude Code sets
    // bracketed-paste mode ~0.3s in and paints its first screen *after* that.
    // Readiness used to return the moment the mode bit appeared, so the screen
    // was inspected while still blank, no dialog was recognised, and the prompt
    // was typed into a modal that discarded it — after which the bare Return
    // answered the modal instead. This child reproduces that ordering: modes
    // first, dialog later.
    let manager = PtyManager::new();
    let id = manager
        .open(sh("printf '\\033[?2004h'; sleep 0.4; \
                  printf '1. Yes, I trust this folder\\r\\n'; \
                  read line; printf 'TYPED:%s\\r\\n' \"$line\"; sleep 30"))
        .unwrap();

    let error = super::inject::inject_prompt(&manager, &id, "ship the fix")
        .await
        .expect_err("readiness must outlast the harness's first paint");
    assert!(
        error.contains("approve this workspace"),
        "the dialog must be seen even though it painted late: {error}"
    );
    assert!(
        !screen_text(&manager, &id).contains("TYPED:"),
        "nothing may be typed at a modal"
    );
    manager.close(&id);
}

#[tokio::test]
async fn a_paste_the_harness_drops_is_sent_again() {
    // Regression, reproduced against the real CLI. Claude Code sets its
    // terminal modes and paints a welcome screen seconds before it will accept
    // input — it is still loading MCP servers, and goes *quiet* while it does.
    // Every timing heuristic therefore had a window where the paste was
    // silently discarded and Enter submitted an empty composer, which is
    // exactly what a peer saw: a session that started and then did nothing.
    //
    // This child reproduces that: it ignores everything typed at it for the
    // first stretch, then starts echoing. Only re-sending gets the prompt in.
    // The `stty` matters twice over. `-echo`, because a real harness draws its
    // own composer and text reaches the screen only if the *application* put it
    // there — with the line discipline echoing, a discarded paste still appears
    // and the check passes without the child taking anything. `-icanon`,
    // because a paste carries no newline, and a canonical-mode reader is handed
    // nothing at all until one arrives.
    //
    // `head -c 31` then swallows exactly the first paste — 19 prompt characters
    // plus the two six-byte bracket markers — and discards it, which is what
    // Claude Code does to anything typed before it is ready. `cat` echoes
    // whatever arrives after, so only a re-send can be observed to land.
    let manager = PtyManager::new();
    let id = manager
        .open(sh("stty -echo -icanon min 1; printf '\\033[?2004h'; \
             head -c 31 > /dev/null; cat"))
        .unwrap();

    super::inject::inject_prompt(&manager, &id, "ship the fix please")
        .await
        .expect("a dropped paste must be re-sent, not given up on");
    assert!(
        screen_text(&manager, &id).contains("ship the fix please"),
        "the re-sent paste must be the one that reached the child"
    );
    manager.close(&id);
}

#[tokio::test]
async fn a_harness_that_never_takes_the_prompt_is_reported_not_hoped_at() {
    // Pressing Enter on an empty composer does nothing, and the turn then dies
    // half a minute later against a transcript that was never going to exist.
    // Saying so here names the failure while the session is still on screen.
    let manager = PtyManager::new();
    // Enables the mode, so the bracketed path is taken, then reads nothing back
    // to the screen — the paste can never be observed to land.
    let id = manager
        .open(sh(
            "stty -echo -icanon min 1; printf '\\033[?2004h'; cat > /dev/null",
        ))
        .unwrap();

    let error = super::inject::inject_prompt(&manager, &id, "ship the fix please")
        .await
        .expect_err("a prompt that never lands must be reported");
    assert!(error.contains("never took the prompt"), "got: {error}");
    manager.close(&id);
}
