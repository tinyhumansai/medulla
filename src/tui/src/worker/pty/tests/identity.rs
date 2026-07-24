//! Which harnesses are handed a minted session id, and how one is learned back.

use super::*;

// -------------------------------------------------------------- identity ---

#[test]
fn claude_is_handed_a_minted_session_id_and_codex_is_not() {
    use super::super::launch::{accepts_preset_session_id, interactive_args, mint_session_id};

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
    use super::super::launch::mint_session_id;
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

    let error = super::super::inject::inject_prompt(&manager, &id, "ship the fix")
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
async fn codexs_trust_dialog_is_dismissed_so_the_prompt_lands() {
    // The end-to-end proof of the dismissal path: a session sitting on codex's
    // trust dialog must not be reported as a wall (as claude's are) but
    // *answered* — the worker presses Enter to confirm the highlighted "Yes,
    // continue" default and then types the prompt into the composer it was hiding.
    //
    // The child paints the trust dialog, then consumes exactly the one dismissal
    // byte (Enter = CR), clears the screen the way codex leaves the prompt, and
    // finally `cat` echoes the paste — so the prompt shows up only if the Enter
    // cleared the dialog first. `stty -echo -icanon` makes the echo
    // application-driven, matching a real TUI.
    let manager = PtyManager::new();
    let id = manager
        .open(sh("stty -echo -icanon min 1; printf '\\033[?2004h'; \
             printf 'Do you trust the contents of this directory?\\r\\n\
             1. Yes, continue\\r\\n2. No, quit\\r\\n'; \
             head -c 1 > /dev/null; printf '\\033[2J\\033[H'; cat"))
        .unwrap();
    wait_for("trust dialog painted", || {
        screen_text(&manager, &id).contains("trust the contents of this directory")
    });

    super::super::inject::inject_prompt(&manager, &id, "ship the fix please")
        .await
        .expect("the trust dialog must be dismissed, not reported");
    wait_for("prompt reached the composer", || {
        screen_text(&manager, &id).contains("ship the fix please")
    });
    assert!(
        !screen_text(&manager, &id).contains("trust the contents of this directory"),
        "the dialog must be gone once dismissed, not still on screen"
    );
    manager.close(&id);
}

#[tokio::test]
async fn codexs_update_prompt_is_skipped_so_the_prompt_lands() {
    // Proof the interactive update prompt is always *skipped*, not reported: the
    // worker sends Down, Down, Enter to land on "Skip until next version" and then
    // types into the composer behind it. The child paints the modal, consumes the
    // seven skip bytes (ESC[B ESC[B CR), clears the screen, and `cat` echoes the
    // paste — so the prompt appears only if the skip cleared the modal first.
    let manager = PtyManager::new();
    let id = manager
        .open(sh("stty -echo -icanon min 1; printf '\\033[?2004h'; \
             printf '1. Update now\\r\\n2. Not now\\r\\n3. Skip until next version\\r\\n'; \
             head -c 7 > /dev/null; printf '\\033[2J\\033[H'; cat"))
        .unwrap();
    wait_for("update prompt painted", || {
        screen_text(&manager, &id).contains("Skip until next version")
    });

    super::super::inject::inject_prompt(&manager, &id, "ship the fix please")
        .await
        .expect("the update prompt must be skipped, not reported");
    wait_for("prompt reached the composer", || {
        screen_text(&manager, &id).contains("ship the fix please")
    });
    assert!(
        !screen_text(&manager, &id).contains("Skip until next version"),
        "the modal must be gone once skipped, not still on screen"
    );
    manager.close(&id);
}

#[tokio::test]
async fn a_dismissable_dialog_that_will_not_clear_is_reported_not_typed_at() {
    // The give-up path: a dialog the worker knows how to answer, whose dismissal
    // keystrokes do not in fact clear it (a wedged or unexpected variant). After a
    // bounded number of attempts the worker reports it by name rather than typing
    // a prompt into a modal still on screen. The child paints codex's trust
    // dialog, then swallows every keystroke without ever clearing it, so the
    // dismissal never takes.
    let manager = PtyManager::new();
    let id = manager
        .open(sh("stty -echo -icanon min 1; printf '\\033[?2004h'; \
                  printf 'Do you trust the contents of this directory?\\r\\n\
                  1. Yes, continue\\r\\n'; cat > /dev/null"))
        .unwrap();
    wait_for("trust dialog painted", || {
        screen_text(&manager, &id).contains("trust the contents of this directory")
    });

    let error = super::super::inject::inject_prompt(&manager, &id, "ship the fix")
        .await
        .expect_err("a dialog the dismissal cannot clear must be reported");
    assert!(
        error.contains("dismissal did not clear it"),
        "the error must say the dismissal failed: {error}"
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

    let error = super::super::inject::inject_prompt(&manager, &id, "ship the fix")
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

    super::super::inject::inject_prompt(&manager, &id, "ship the fix please")
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

    let error = super::super::inject::inject_prompt(&manager, &id, "ship the fix please")
        .await
        .expect_err("a prompt that never lands must be reported");
    assert!(error.contains("never took the prompt"), "got: {error}");
    manager.close(&id);
}
