//! LIVE smoke test of an interactive **codex TUI** session (the real `codex`
//! binary running in a pseudo-terminal), as opposed to the fake-harness tests in
//! the sibling modules.
//!
//! It proves the thing the PTY worker exists for: once codex finishes a turn, a
//! second prompt can be pushed into the *same* live session, and codex answers it
//! with the first turn's context still in mind. Turn 1 states a secret; turn 2
//! (which reuses the idle session) must recall it — something a fresh process
//! could not do, so a pass is real end-to-end evidence.
//!
//! Ignored by default: it needs an authed `codex` on `PATH` and network, and it
//! costs tokens. Run it explicitly (single-threaded, so the two turns are
//! ordered):
//!
//! ```text
//! cargo test -p medulla-tui --lib -- --ignored --nocapture \
//!     interactive_codex_session_accepts_a_second_turn
//! ```
//!
//! It self-skips (rather than failing) when no `codex` binary is found.
//!
//! This is the acceptance test for the codex startup-dialog fix. It used to fail
//! with "the harness never took the prompt (tried 3 times)": the
//! [`diagnose_codex_paste_rendering`] probe classified it as NOT a rendering or
//! kitty-protocol gap — medulla's screen parser renders codex's TUI perfectly —
//! but codex sat on an unhandled startup modal the composer was hiding behind,
//! and the old `pty/dialog.rs` only knew claude's modals.
//!
//! What that modal actually is, verified live against codex-cli 0.142.5 by
//! [`experiment_codex_startup_dialog_dismissal`]: on a fresh workspace codex
//! opens on its **trust dialog** ("Do you trust the contents of this directory?",
//! default `› 1. Yes, continue`), shown even under
//! `--dangerously-bypass-approvals-and-sandbox`. `pty::dialog` now recognizes it
//! and dismisses it with a single **Enter** (confirming the highlighted default —
//! never Esc, which was found to *cancel* the dialog and exit codex), and
//! `pty::inject` presses it and waits for the modal to clear before typing. The
//! offline end-to-end proof is
//! `pty::tests::identity::codexs_trust_dialog_is_dismissed_so_the_prompt_lands`.
//!
//! Codex's "update available" notice turned out to be a passive *banner* here,
//! not a blocking modal — it does not gate the composer. Its interactive variant
//! (`1. Update now` / `3. Skip until next version`) is always **skipped**: the
//! dismissal arrows the cursor onto the Skip row before pressing Enter, so an
//! unattended worker never risks confirming "Update now" and running the
//! installer.

use std::collections::HashMap;
use std::time::Duration;

use medulla::daemon::providers::{Abort, RunTaskOptions};
use medulla::tinyplace::HarnessProvider;

use super::super::executor::PtySessionExecutor;
use super::super::pty::{LaunchSpec, PtyManager};

/// Render a session's medulla-parsed screen as plain text (mirrors the private
/// `pty::inject::screen_text`), for the paste-gap diagnostic below.
fn dump_screen(sessions: &PtyManager, id: &str) -> String {
    match sessions.screen_rows(id) {
        Some(snapshot) => snapshot
            .cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| cell.text.as_str())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n"),
        None => "<no screen>".to_string(),
    }
}

/// Resolve a real `codex` binary on `PATH`, or `None` if not installed.
fn find_codex() -> Option<String> {
    let path = std::env::var("PATH").ok()?;
    path.split(':')
        .map(|dir| std::path::Path::new(dir).join("codex"))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

/// Options for one interactive codex turn on conversation `peer` in `cwd`.
///
/// `skip_permissions` launches `codex --dangerously-bypass-approvals-and-sandbox`
/// (unattended sessions cannot answer permission prompts); no `extra_args`, so
/// codex paints its normal TUI and the prompt is typed in.
fn live_options(
    env: &HashMap<String, String>,
    peer: &str,
    prompt: &str,
    cwd: &str,
) -> RunTaskOptions {
    RunTaskOptions {
        conversation: peer.to_string(),
        resume_session_id: None,
        provider: HarnessProvider::Codex,
        prompt: prompt.to_string(),
        cwd: cwd.to_string(),
        env: env.clone(),
        timeout_ms: 120_000,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: true,
        abort: Abort::new(),
        on_event: None,
        on_stdin: None,
    }
}

#[tokio::test]
#[ignore = "live: needs an authed codex CLI + network; costs tokens"]
async fn interactive_codex_session_accepts_a_second_turn() {
    let Some(bin) = find_codex() else {
        eprintln!("skipping interactive-codex test: no `codex` binary on PATH");
        return;
    };
    let home = std::env::var("HOME").expect("HOME is set");
    eprintln!("using real codex at {bin}");

    // A unique temp workspace: codex records this cwd on its rollout, so the
    // tailer's cwd filter matches only THIS session's transcript among all the
    // rollouts under ~/.codex/sessions.
    let workspace = tempfile::tempdir().unwrap();
    let cwd = workspace.path().to_string_lossy().into_owned();

    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.insert("TINYPLACE_CODEX_BIN".to_string(), bin);
    // Real codex writes rollouts under $CODEX_HOME/sessions (default ~/.codex);
    // point the transcript tailer at that tree (it recurses the date subdirs).
    env.insert(
        "TINYPLACE_CODEX_SESSIONS_DIR".to_string(),
        format!("{home}/.codex/sessions"),
    );
    env.insert("TERM".to_string(), "xterm-256color".to_string());

    let executor = PtySessionExecutor::new(PtyManager::new(), env.clone(), cwd.clone());
    let sessions = executor.sessions_for_test();

    // Turn 1: open the interactive session and state a secret.
    let turn1 = tokio::time::timeout(
        Duration::from_secs(150),
        executor.clone().run_for_test(live_options(
            &env,
            "live-peer",
            "Remember this fact for later: the secret code is PLUM7. Reply with just: OK",
            &cwd,
        )),
    )
    .await
    .expect("turn 1 settles within the deadline")
    .expect("turn 1 succeeds");
    eprintln!("turn 1 reply: {:?}", turn1.reply);
    assert!(!turn1.reply.trim().is_empty(), "turn 1 returned no reply");

    // The interactive codex session stays alive, idle, ready for the next input.
    let rows = sessions.rows();
    assert_eq!(rows.len(), 1, "exactly one live codex session");
    assert!(
        rows[0].state.is_running(),
        "the session must stay open between turns, got {:?}",
        rows[0].state
    );

    // Turn 2: push a NEW input into the SAME session; it must recall turn 1.
    let turn2 = tokio::time::timeout(
        Duration::from_secs(150),
        executor.clone().run_for_test(live_options(
            &env,
            "live-peer",
            "What was the secret code I told you earlier? Reply with just the code.",
            &cwd,
        )),
    )
    .await
    .expect("turn 2 settles within the deadline")
    .expect("turn 2 succeeds");
    eprintln!("turn 2 reply: {:?}", turn2.reply);

    assert!(
        turn2.reply.to_uppercase().contains("PLUM7"),
        "the reused session must remember the first turn's secret, got: {:?}",
        turn2.reply
    );

    sessions.shutdown();
}

#[tokio::test]
#[ignore = "live experiment: find a safe keystroke to dismiss codex's update modal"]
async fn experiment_codex_dialog_dismissal() {
    // Navigate the update modal SAFELY: never press Enter while "Update now" is
    // highlighted (it would run `npm install`). Down-arrow to a Skip option first,
    // dumping medulla's parsed screen after each key so we can see what worked.
    let Some(bin) = find_codex() else {
        eprintln!("skipping: no `codex`");
        return;
    };
    let workspace = tempfile::tempdir().unwrap();
    let cwd = workspace.path().to_string_lossy().into_owned();
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.insert("TINYPLACE_CODEX_BIN".to_string(), bin.clone());
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    let sessions = PtyManager::new();
    let id = sessions
        .open(LaunchSpec {
            provider: HarnessProvider::Codex,
            bin,
            cwd,
            env,
            extra_args: Vec::new(),
            skip_permissions: true,
            label: "exp".to_string(),
            session_id: None,
        })
        .expect("open");

    let step = |label: &str, s: &PtyManager| {
        eprintln!("\n========== {label} ==========\n{}", dump_screen(s, &id));
    };
    tokio::time::sleep(Duration::from_secs(5)).await;
    step("startup", &sessions);

    // Down twice → move the cursor from "1. Update now" to "3. Skip until next
    // version" (safe: arrows only move, they never confirm).
    sessions.write(&id, b"\x1b[B").unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    step("after Down #1", &sessions);
    sessions.write(&id, b"\x1b[B").unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    step(
        "after Down #2 (cursor should be on a Skip option)",
        &sessions,
    );

    // Now Enter is safe — the cursor is on a Skip row, not Update.
    sessions.write(&id, b"\r").unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    step("after Enter (composer should appear)", &sessions);

    // Prove the composer now takes a paste.
    const MARKER: &str = "ZZ_INJECT_MARKER_9137";
    sessions.write(&id, b"\x1b[200~").unwrap();
    sessions.write(&id, MARKER.as_bytes()).unwrap();
    sessions.write(&id, b"\x1b[201~").unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    let after = dump_screen(&sessions, &id);
    step("after paste", &sessions);
    eprintln!("=== marker landed? {} ===", after.contains(MARKER));

    sessions.shutdown();
}

#[tokio::test]
#[ignore = "live experiment: how codex's startup dialogs dismiss (trust vs update)"]
async fn experiment_codex_startup_dialog_dismissal() {
    // Findings against codex-cli 0.142.5, recorded here so the dismissal keys in
    // `pty::dialog` are grounded in what codex actually paints, not a guess:
    //
    //  - A fresh workspace opens on the TRUST dialog ("Do you trust the contents
    //    of this directory?"), NOT the update notice. Its default is `› 1. Yes,
    //    continue`, so Enter confirms trust — safe and desired for an unattended
    //    worker, which launched with the bypass flag precisely to run here.
    //  - `--dangerously-bypass-approvals-and-sandbox` does NOT skip that trust
    //    prompt; the worker still has to answer it.
    //  - Esc is UNSAFE: on the trust dialog it cancels and codex EXITS. So the
    //    dismissal must be Enter (confirm the highlighted default), never Esc.
    //  - The update notice appears only when a newer codex exists; its default is
    //    `1. Update now`, where Enter would run the updater — so that dialog needs
    //    the arrow-to-Skip path, kept in `experiment_codex_dialog_dismissal` below.
    //
    // This probe presses Enter on whatever startup dialog is up and checks the
    // composer beneath it then takes a paste (no Enter after → no turn, no tokens).
    let Some(bin) = find_codex() else {
        eprintln!("skipping: no `codex`");
        return;
    };
    let workspace = tempfile::tempdir().unwrap();
    let cwd = workspace.path().to_string_lossy().into_owned();
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.insert("TINYPLACE_CODEX_BIN".to_string(), bin.clone());
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    let sessions = PtyManager::new();
    let id = sessions
        .open(LaunchSpec {
            provider: HarnessProvider::Codex,
            bin,
            cwd,
            env,
            extra_args: Vec::new(),
            skip_permissions: true,
            label: "trust".to_string(),
            session_id: None,
        })
        .expect("open");

    tokio::time::sleep(Duration::from_secs(5)).await;
    eprintln!(
        "\n========== startup ==========\n{}",
        dump_screen(&sessions, &id)
    );

    // Enter confirms the highlighted default ("Yes, continue" on the trust
    // dialog). Guarded, because a wrong keystroke could have exited codex.
    sessions
        .write(&id, b"\r")
        .expect("session still alive for Enter");
    tokio::time::sleep(Duration::from_secs(1)).await;
    eprintln!(
        "\n========== after Enter ==========\n{}",
        dump_screen(&sessions, &id)
    );
    assert!(
        sessions.row(&id).is_some_and(|r| r.state.is_running()),
        "Enter on the trust dialog must confirm it, not exit codex"
    );

    // Prove the composer beneath the dialog now takes a paste.
    const MARKER: &str = "ZZ_TRUST_MARKER_5521";
    if sessions.write(&id, b"\x1b[200~").is_ok() {
        sessions.write(&id, MARKER.as_bytes()).ok();
        sessions.write(&id, b"\x1b[201~").ok();
        tokio::time::sleep(Duration::from_secs(1)).await;
        let pasted = dump_screen(&sessions, &id);
        eprintln!("\n========== after paste ==========\n{pasted}");
        eprintln!(
            "=== marker reached the composer? {} ===",
            pasted.contains(MARKER)
        );
    }

    sessions.shutdown();
}

#[tokio::test]
#[ignore = "live diagnostic: dumps codex 0.142's parsed screen to classify the paste gap"]
async fn diagnose_codex_paste_rendering() {
    // Classifies the injection failure: open real codex, dump medulla's parsed
    // screen, send a bracketed paste (no Enter → no turn, no tokens), dump again.
    //   - blank screen while codex runs  → RENDERING gap (parser vs alt-screen/kitty)
    //   - codex TUI shown but no marker  → INSERTION gap (codex ignored the paste)
    //   - marker shown                   → parser + insertion OK (needle logic at fault)
    let Some(bin) = find_codex() else {
        eprintln!("skipping: no `codex` on PATH");
        return;
    };
    let workspace = tempfile::tempdir().unwrap();
    let cwd = workspace.path().to_string_lossy().into_owned();
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.insert("TINYPLACE_CODEX_BIN".to_string(), bin.clone());
    env.insert("TERM".to_string(), "xterm-256color".to_string());

    let sessions = PtyManager::new();
    let id = sessions
        .open(LaunchSpec {
            provider: HarnessProvider::Codex,
            bin,
            cwd,
            env,
            extra_args: Vec::new(),
            skip_permissions: true,
            label: "diag".to_string(),
            session_id: None,
        })
        .expect("open codex session");

    // Let the TUI paint.
    tokio::time::sleep(Duration::from_secs(5)).await;
    eprintln!(
        "=== bracketed_paste mode: {:?} ===",
        sessions.bracketed_paste(&id)
    );
    let before = dump_screen(&sessions, &id);
    eprintln!(
        "=== SCREEN BEFORE PASTE ({} non-space chars) ===\n{}",
        before.chars().filter(|c| !c.is_whitespace()).count(),
        before
    );

    // Bracketed paste of a distinctive marker — no submit.
    const MARKER: &str = "ZZ_INJECT_MARKER_9137";
    sessions.write(&id, b"\x1b[200~").unwrap();
    sessions.write(&id, MARKER.as_bytes()).unwrap();
    sessions.write(&id, b"\x1b[201~").unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;

    let after = dump_screen(&sessions, &id);
    eprintln!("=== SCREEN AFTER PASTE ===\n{}", after);
    eprintln!(
        "=== marker on medulla's parsed screen? {} ===",
        after.contains(MARKER)
    );
    eprintln!(
        "=== 'pasted' placeholder present? {} ===",
        after
            .to_lowercase()
            .replace(char::is_whitespace, "")
            .contains("pastedtext")
    );

    sessions.shutdown();
}
