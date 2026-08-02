//! Reading a pane's tail for a handoff brief.
//!
//! The brief is captured out from under an operator who is still looking at the
//! pane, so the interesting property is not what comes back — it is what does
//! *not* move while it does.

use super::*;

/// Paint `lines` numbered rows, then hold the session open.
fn painting(lines: usize) -> LaunchSpec {
    sh(&format!(
        "i=1; while [ $i -le {lines} ]; do printf 'line %s\\r\\n' $i; i=$((i+1)); done; sleep 30"
    ))
}

#[test]
fn tail_lines_returns_the_last_lines_oldest_first() {
    let manager = PtyManager::new();
    let id = manager.open(painting(6)).unwrap();
    wait_for("output painted", || {
        manager.tail_lines(&id, 50).iter().any(|l| l == "line 6")
    });

    let tail = manager.tail_lines(&id, 50);
    let numbered: Vec<&String> = tail.iter().filter(|l| l.starts_with("line ")).collect();
    assert_eq!(
        numbered,
        vec!["line 1", "line 2", "line 3", "line 4", "line 5", "line 6"],
        "a brief reads top to bottom, like the pane it came from"
    );

    manager.close(&id);
}

#[test]
fn tail_lines_keeps_only_the_last_max_lines() {
    let manager = PtyManager::new();
    let id = manager.open(painting(12)).unwrap();
    wait_for("output painted", || {
        manager.tail_lines(&id, 50).iter().any(|l| l == "line 12")
    });

    let tail = manager.tail_lines(&id, 3);
    assert_eq!(tail.len(), 3);
    assert_eq!(tail.last().unwrap(), "line 12");

    manager.close(&id);
}

#[test]
fn tail_lines_drops_the_blank_space_below_the_prompt() {
    // A harness pane is mostly empty rows. Without trimming them the brief is a
    // screenful of nothing with two lines of content at the top.
    let manager = PtyManager::new();
    let id = manager.open(painting(2)).unwrap();
    wait_for("output painted", || {
        manager.tail_lines(&id, 50).iter().any(|l| l == "line 2")
    });

    let tail = manager.tail_lines(&id, 50);
    assert_eq!(
        tail.last().map(String::as_str),
        Some("line 2"),
        "trailing blank rows must not pad the brief"
    );

    manager.close(&id);
}

#[test]
fn tail_lines_leaves_the_operators_scroll_offset_where_it_was() {
    // The regression that would otherwise be invisible: capturing a brief reads
    // at offset 0, and if it does not put the offset back, handing a harness
    // over yanks the pane the operator is reading down to the live screen.
    let manager = PtyManager::new();
    let id = manager.open(painting(200)).unwrap();
    wait_for("output painted", || {
        manager.tail_lines(&id, 200).iter().any(|l| l == "line 200")
    });

    let scrolled = manager
        .scroll_history(&id, 5, true)
        .expect("session exists");
    assert!(
        scrolled > 0,
        "the test needs a non-zero offset to be about anything"
    );

    let _ = manager.tail_lines(&id, 50);

    assert_eq!(
        manager.scroll_history(&id, 0, true),
        Some(scrolled),
        "capturing a brief moved the pane the operator was reading"
    );

    manager.close(&id);
}

#[test]
fn tail_lines_of_an_unknown_session_is_empty() {
    let manager = PtyManager::new();
    assert!(manager.tail_lines("w_nope", 50).is_empty());
}
