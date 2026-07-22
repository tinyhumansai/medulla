//! Pty allocation, the reader thread, emulator parsing, resize, and reaping.

use super::*;

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
    super::super::inject::inject_prompt(&manager, &id, "ping")
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
    super::super::inject::inject_prompt(&manager, &id, "ping")
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
    super::super::inject::inject_prompt(&manager, &id, "ping")
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
    let error = super::super::inject::inject_prompt(&manager, &id, "hello")
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
fn state_labels_and_glyphs_cover_every_variant() {
    // Pure projections the list pane reads: each variant maps to its own label
    // and glyph, a clean exit reads the same as no reported code, and only a
    // running child counts as alive.
    assert_eq!(PtyState::Running.as_str(), "running");
    assert_eq!(PtyState::Exited { code: Some(0) }.as_str(), "exited");
    assert_eq!(PtyState::Failed.as_str(), "failed");

    assert_eq!(PtyState::Running.glyph(), '●');
    assert_eq!(PtyState::Exited { code: Some(0) }.glyph(), '✓');
    assert_eq!(PtyState::Exited { code: None }.glyph(), '✓');
    assert_eq!(
        PtyState::Exited { code: Some(3) }.glyph(),
        '✕',
        "a non-zero exit is failure"
    );
    assert_eq!(PtyState::Failed.glyph(), '✕');

    assert!(PtyState::Running.is_running());
    assert!(!(PtyState::Exited { code: Some(0) }).is_running());
    assert!(!PtyState::Failed.is_running());
}

#[test]
fn resizing_an_unknown_session_is_a_no_op() {
    // The render pass resizes whichever session is selected; a stale id (a
    // session forgotten between selection and draw) must be ignored, not panic.
    let manager = PtyManager::new();
    manager.resize("w_nope", 80, 24);
    assert!(manager.rows().is_empty());
}

#[test]
fn writing_to_an_unknown_session_names_the_missing_id() {
    let manager = PtyManager::new();
    let error = manager
        .write("w_ghost", b"x")
        .expect_err("there is no such session to write to");
    assert!(error.contains("no session"), "got: {error}");
    assert!(error.contains("w_ghost"), "the error names the id: {error}");
}

#[test]
fn writing_raw_bytes_to_an_exited_session_is_refused() {
    // `inject_prompt` refuses at the readiness gate, but the raw write path has
    // its own guard: a dead session must be rejected here too rather than
    // writing into a closed pty.
    let manager = PtyManager::new();
    let id = manager.open(sh("exit 0")).unwrap();
    wait_for("exit recorded", || {
        !manager.row(&id).unwrap().state.is_running()
    });
    let error = manager
        .write(&id, b"x")
        .expect_err("a dead session cannot be written to");
    assert!(error.contains("has exited"), "got: {error}");
}

#[test]
fn closing_or_recording_against_an_unknown_session_is_harmless() {
    // Both are called from paths that may race a session's removal; neither may
    // panic or invent a session when the id is gone.
    let manager = PtyManager::new();
    assert!(
        !manager.close("w_missing"),
        "nothing to close returns false"
    );
    manager.record_session_id("w_missing", "sess-x");
    assert!(
        manager.rows().is_empty(),
        "recording an id for no session changes nothing"
    );
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
