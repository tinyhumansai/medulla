//! Tests for the saved copilot conversation.
//!
//! The interesting cases are all about what a *restart* sees, so every one of
//! them writes through one `Transcripts` and reads back through a second —
//! never through the same in-memory value, which would pass even if nothing
//! reached the disk.

use super::*;

/// A transcripts store over a fresh temporary directory, and the directory,
/// which the caller must keep alive for the store to have anywhere to write.
fn store() -> (tempfile::TempDir, Transcripts) {
    let root = tempfile::tempdir().expect("tempdir");
    let store = Transcripts::new(root.path().join("copilot"));
    (root, store)
}

fn turn(role: TurnRole, text: &str) -> CopilotTurn {
    CopilotTurn::new(role, text)
}

#[test]
fn a_thread_that_was_never_saved_loads_empty_rather_than_failing() {
    let (_root, store) = store();
    let loaded = store.load(Thread::Workflow("sweep"));
    assert_eq!(loaded.thread, "sweep");
    assert!(loaded.turns.is_empty());
    assert_eq!(loaded.recap(), None);
}

#[test]
fn a_saved_conversation_comes_back_through_a_second_store() {
    let (root, store) = store();
    store
        .save(
            Thread::Workflow("sweep"),
            &[
                turn(TurnRole::User, "add a lint step"),
                turn(TurnRole::Agent, "added it after the build"),
                turn(TurnRole::Change, "+ node lint"),
            ],
        )
        .expect("save");

    // A second store over the same directory is what a restart is.
    let reopened = Transcripts::new(root.path().join("copilot"));
    let loaded = reopened.load(Thread::Workflow("sweep"));
    assert_eq!(loaded.turns.len(), 3);
    assert_eq!(loaded.turns[0].role, TurnRole::User);
    assert_eq!(loaded.turns[0].text, "add a lint step");
    assert!(loaded.updated_at > 0, "a saved transcript is timestamped");
}

#[test]
fn progress_chatter_is_not_persisted() {
    let (_root, store) = store();
    store
        .save(
            Thread::Workflow("sweep"),
            &[
                turn(TurnRole::User, "add a lint step"),
                turn(TurnRole::Status, "thinking · reading the graph"),
                turn(TurnRole::Tool, "workflow_get"),
                turn(TurnRole::Agent, "added it"),
            ],
        )
        .expect("save");

    let loaded = store.load(Thread::Workflow("sweep"));
    let roles: Vec<TurnRole> = loaded.turns.iter().map(|turn| turn.role).collect();
    assert_eq!(
        roles,
        vec![TurnRole::User, TurnRole::Tool, TurnRole::Agent],
        "status lines age out; what the turn did does not"
    );
}

#[test]
fn a_long_conversation_keeps_its_recent_end() {
    let (_root, store) = store();
    let turns: Vec<CopilotTurn> = (0..MAX_PERSISTED_TURNS + 25)
        .map(|index| turn(TurnRole::User, &format!("instruction {index}")))
        .collect();
    store.save(Thread::Workflow("sweep"), &turns).expect("save");

    let loaded = store.load(Thread::Workflow("sweep"));
    assert_eq!(loaded.turns.len(), MAX_PERSISTED_TURNS);
    assert_eq!(
        loaded.turns.last().expect("a turn").text,
        format!("instruction {}", MAX_PERSISTED_TURNS + 24),
        "trimming drops the oldest, not the newest"
    );
}

#[test]
fn a_recap_names_who_said_what_and_skips_the_tool_calls() {
    let (_root, store) = store();
    store
        .save(
            Thread::Workflow("sweep"),
            &[
                turn(TurnRole::User, "add a lint step"),
                turn(TurnRole::Tool, "workflow_apply_ops"),
                turn(TurnRole::Agent, "added it after the build"),
            ],
        )
        .expect("save");

    let recap = store
        .load(Thread::Workflow("sweep"))
        .recap()
        .expect("a recap");
    assert!(recap.contains("operator: add a lint step"));
    assert!(recap.contains("you: added it after the build"));
    assert!(
        !recap.contains("workflow_apply_ops"),
        "a recap is what was decided, not the calls that did it"
    );
}

#[test]
fn a_recap_carries_only_the_last_few_turns() {
    let (_root, store) = store();
    let turns: Vec<CopilotTurn> = (0..RECAP_TURNS * 3)
        .map(|index| turn(TurnRole::User, &format!("instruction {index}")))
        .collect();
    store.save(Thread::Workflow("sweep"), &turns).expect("save");

    let recap = store
        .load(Thread::Workflow("sweep"))
        .recap()
        .expect("a recap");
    assert_eq!(recap.lines().count(), RECAP_TURNS);
    assert!(recap.contains(&format!("instruction {}", RECAP_TURNS * 3 - 1)));
    assert!(!recap.contains("instruction 0\n"));
}

#[test]
fn a_conversation_that_only_reported_progress_has_nothing_to_recap() {
    let (_root, store) = store();
    store
        .save(
            Thread::Workflow("sweep"),
            &[turn(TurnRole::Tool, "workflow_get")],
        )
        .expect("save");
    assert_eq!(
        store.load(Thread::Workflow("sweep")).recap(),
        None,
        "a heading with nothing under it is worse than no heading"
    );
}

#[test]
fn renaming_a_thread_moves_its_conversation_with_it() {
    let (_root, store) = store();
    store
        .save(Thread::Pending, &[turn(TurnRole::User, "build me a sweep")])
        .expect("save");

    store.rename(Thread::Pending, Thread::Workflow("sweep"));

    assert_eq!(store.load(Thread::Workflow("sweep")).turns.len(), 1);
    assert!(
        store.load(Thread::Pending).turns.is_empty(),
        "the sentinel thread is not left holding a second copy"
    );
}

#[test]
fn forgetting_a_thread_removes_it_from_disk() {
    let (_root, store) = store();
    store
        .save(
            Thread::Workflow("sweep"),
            &[turn(TurnRole::User, "add a lint step")],
        )
        .expect("save");
    store.forget(Thread::Workflow("sweep"));
    assert!(store.load(Thread::Workflow("sweep")).turns.is_empty());
}

#[test]
fn an_id_that_is_not_a_filename_is_declined_rather_than_sanitized() {
    let (root, store) = store();
    // A traversal that a sanitizing store would rewrite into some neighbouring
    // filename instead of refusing.
    store
        .save(
            Thread::Workflow("../escape"),
            &[turn(TurnRole::User, "first")],
        )
        .expect("a refused name is not an error, only a no-op");

    assert!(store.load(Thread::Workflow("../escape")).turns.is_empty());
    assert!(
        !root.path().join("copilot").exists(),
        "a name with no safe spelling writes nothing at all"
    );
    assert!(
        !root.path().join("escape.json").exists(),
        "and certainly not outside the store's own directory"
    );
}

#[test]
fn the_pending_thread_cannot_collide_with_a_workflow_that_shares_its_name() {
    let (_root, store) = store();
    store
        .save(Thread::Pending, &[turn(TurnRole::User, "build me one")])
        .expect("save");
    store
        .save(
            // The id an operator would have to choose to collide with the
            // pending thread's own file, if the two shared a directory.
            Thread::Workflow("pending"),
            &[turn(TurnRole::User, "unrelated")],
        )
        .expect("save");

    assert_eq!(store.load(Thread::Pending).turns[0].text, "build me one");
    assert_eq!(
        store.load(Thread::Workflow("pending")).turns[0].text,
        "unrelated"
    );
}

#[test]
fn a_corrupt_file_reads_as_an_empty_conversation() {
    let (root, store) = store();
    let dir = root.path().join("copilot").join("workflows");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("sweep.json"), "{ not json").expect("write");

    // The pane opens; it just has no history to show.
    assert!(store.load(Thread::Workflow("sweep")).turns.is_empty());
}
