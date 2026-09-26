//! A harness's own clipboard writes reaching the operator's terminal.
//!
//! The child is `/bin/sh` printing the escape a coding agent emits when it
//! copies something. What matters is that it survives the trip: OSC 52 arrives
//! on a pty whose only reader parses it into a screen grid, so without the
//! forwarding path the copy would be decoded, dropped, and never mentioned
//! again.

use std::sync::{Arc, Mutex};

use super::*;
use crate::worker::pty::manager::ClipboardSink;

/// A manager whose forwarded copies land in a vector instead of the terminal.
fn recording() -> (PtyManager, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink: ClipboardSink = {
        let seen = seen.clone();
        Arc::new(move |text: &str| seen.lock().expect("sink").push(text.to_string()))
    };
    (
        PtyManager::with_now_and_clipboard(Arc::new(medulla::clock::now_millis), sink),
        seen,
    )
}

/// Everything forwarded so far.
fn copies(seen: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    seen.lock().expect("sink").clone()
}

#[test]
fn a_harness_copy_is_forwarded_out_of_the_pane() {
    let (manager, seen) = recording();
    // base64("hello from the harness")
    let id = manager
        .open(sh(
            "printf '\\033]52;c;aGVsbG8gZnJvbSB0aGUgaGFybmVzcw==\\007'; sleep 30",
        ))
        .unwrap();
    wait_for("the copy to be forwarded", || !copies(&seen).is_empty());
    assert_eq!(copies(&seen), vec!["hello from the harness".to_string()]);
    manager.close(&id);
}

#[test]
fn the_escape_never_reaches_the_rendered_screen() {
    // It is a clipboard instruction, not text. Leaking it into the grid would
    // put base64 in front of the operator and, worse, into anything they then
    // selected out of the pane.
    let (manager, seen) = recording();
    let id = manager
        .open(sh("printf '\\033]52;c;aGk=\\007after'; sleep 30"))
        .unwrap();
    wait_for("the trailing text", || {
        screen_text(&manager, &id).contains("after")
    });
    wait_for("the copy to be forwarded", || !copies(&seen).is_empty());
    let screen = screen_text(&manager, &id);
    assert!(!screen.contains("aGk="), "{screen:?}");
    assert!(!screen.contains('\u{1b}'), "{screen:?}");
    manager.close(&id);
}

#[test]
fn a_clipboard_query_is_not_forwarded_as_a_copy() {
    // `\033]52;c;?\007` asks the terminal to *report* the clipboard back.
    // Forwarding it as a copy would put a literal "?" on the operator's
    // clipboard, replacing whatever they had.
    let (manager, seen) = recording();
    let id = manager
        .open(sh("printf '\\033]52;c;?\\007marker'; sleep 30"))
        .unwrap();
    wait_for("the trailing text", || {
        screen_text(&manager, &id).contains("marker")
    });
    assert!(copies(&seen).is_empty(), "{:?}", copies(&seen));
    manager.close(&id);
}

#[test]
fn each_copy_is_forwarded_once() {
    // The reader polls after every drain, so a copy that was not taken would be
    // re-sent on every subsequent read — and each send spawns `pbcopy`/`tmux`
    // in the real sink.
    let (manager, seen) = recording();
    let id = manager
        .open(sh(
            "printf '\\033]52;c;Zmlyc3Q=\\007'; sleep 0.2; printf 'noise'; \
             sleep 0.2; printf '\\033]52;c;c2Vjb25k\\007'; sleep 30",
        ))
        .unwrap();
    wait_for("both copies", || copies(&seen).len() >= 2);
    // A third read carrying no escape must not repeat either of them.
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(
        copies(&seen),
        vec!["first".to_string(), "second".to_string()]
    );
    manager.close(&id);
}
