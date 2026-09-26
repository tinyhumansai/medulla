//! The copilot pane's conversation surviving a restart.
//!
//! A "restart" here is a second [`App`] built over the same Medulla home and
//! the same store — which is exactly what the next `medulla` invocation is,
//! minus the process boundary. Asserting through a second app rather than
//! re-reading the first one's state is the whole point: in-memory turns would
//! satisfy every assertion below without a byte reaching disk.

use medulla::ui::workflows::TurnRole;
use medulla::workflows::copilot::{Thread, Transcripts};
use medulla::workflows::{WorkflowRecord, WorkflowStore};

use super::{app_with_home, diamond};
use crate::ui::app::App;

/// Run one complete turn on `app`'s selected workflow.
fn one_turn(app: &mut App, instruction: &str, reply: &str, changes: &[&str]) {
    app.wf.draft = crate::ui::composer::insert_at("", 0, instruction);
    app.submit_copilot().expect("a turn is dispatched");
    app.copilot_finished(
        "sweep",
        reply.to_string(),
        changes.iter().map(|change| change.to_string()).collect(),
        None,
    );
}

#[test]
fn a_conversation_is_still_there_after_a_restart() {
    let home = tempfile::tempdir().expect("tempdir");
    let workflows: Vec<WorkflowRecord> = vec![diamond("sweep")];

    let mut app = app_with_home(home.path(), &workflows);
    one_turn(
        &mut app,
        "add a notify step",
        "added it",
        &["+ node notify"],
    );

    // The second app is the restart: same home, same store, nothing shared in
    // memory with the first.
    let restarted = app_with_home(home.path(), &workflows);
    let thread = restarted.copilot().expect("the thread comes back");
    let texts: Vec<&str> = thread.turns.iter().map(|turn| turn.text.as_str()).collect();
    assert_eq!(
        texts,
        vec!["add a notify step", "+ node notify", "added it"]
    );
    assert!(!thread.busy, "a restored thread is not mid-turn");
}

#[test]
fn a_restored_thread_accepts_the_next_turn_without_losing_the_last_one() {
    let home = tempfile::tempdir().expect("tempdir");
    let workflows: Vec<WorkflowRecord> = vec![diamond("sweep")];

    let mut app = app_with_home(home.path(), &workflows);
    one_turn(
        &mut app,
        "add a notify step",
        "added it",
        &["+ node notify"],
    );

    let mut restarted = app_with_home(home.path(), &workflows);
    one_turn(
        &mut restarted,
        "now remove it again",
        "removed it",
        &["− node notify"],
    );

    // And a third app sees both turns, in order — the restored history was
    // carried forward on save rather than overwritten by the new turn alone.
    let again = app_with_home(home.path(), &workflows);
    let texts: Vec<String> = again
        .copilot()
        .expect("thread")
        .turns
        .iter()
        .map(|turn| turn.text.clone())
        .collect();
    assert_eq!(
        texts,
        vec![
            "add a notify step",
            "+ node notify",
            "added it",
            "now remove it again",
            "− node notify",
            "removed it",
        ]
    );
}

#[test]
fn a_failed_turn_is_part_of_the_record_too() {
    let home = tempfile::tempdir().expect("tempdir");
    let workflows: Vec<WorkflowRecord> = vec![diamond("sweep")];

    let mut app = app_with_home(home.path(), &workflows);
    app.wf.draft = crate::ui::composer::insert_at("", 0, "add a notify step");
    app.submit_copilot().expect("a turn is dispatched");
    app.copilot_failed(
        "sweep",
        "add a notify step".to_string(),
        "the harness timed out".to_string(),
    );

    let restarted = app_with_home(home.path(), &workflows);
    let thread = restarted.copilot().expect("thread");
    assert!(
        thread
            .turns
            .iter()
            .any(|turn| turn.role == TurnRole::Error && turn.text.contains("timed out")),
        "an instruction that failed should not come back looking unanswered"
    );
}

#[test]
fn progress_chatter_does_not_survive_the_restart() {
    let home = tempfile::tempdir().expect("tempdir");
    let workflows: Vec<WorkflowRecord> = vec![diamond("sweep")];

    let mut app = app_with_home(home.path(), &workflows);
    app.wf.draft = crate::ui::composer::insert_at("", 0, "add a notify step");
    app.submit_copilot().expect("a turn is dispatched");
    app.copilot_status("sweep", "reading the graph".into());
    app.copilot_finished(
        "sweep",
        "added it".into(),
        vec!["+ node notify".into()],
        None,
    );

    // In this session the operator can see what it was doing…
    assert!(app
        .copilot()
        .expect("thread")
        .turns
        .iter()
        .any(|turn| turn.role == TurnRole::Status));
    // …and next session they see what it concluded.
    let restarted = app_with_home(home.path(), &workflows);
    assert!(!restarted
        .copilot()
        .expect("thread")
        .turns
        .iter()
        .any(|turn| turn.role == TurnRole::Status));
}

#[test]
fn deleting_a_workflow_takes_its_conversation_with_it() {
    let home = tempfile::tempdir().expect("tempdir");
    let workflows: Vec<WorkflowRecord> = vec![diamond("sweep")];

    let mut app = app_with_home(home.path(), &workflows);
    one_turn(
        &mut app,
        "add a notify step",
        "added it",
        &["+ node notify"],
    );
    app.forget_copilot("sweep");

    // Read through the store rather than a restarted app: the workflow is
    // gone, so a pane would have nothing to select and the assertion would
    // pass whether or not the file was removed.
    let cwd = std::env::current_dir().expect("cwd");
    let saved = Transcripts::under(home.path(), &cwd).load(Thread::Workflow("sweep"));
    assert!(
        saved.turns.is_empty(),
        "a deleted workflow's history would otherwise be handed to whatever \
         reused its id"
    );
}

#[test]
fn an_app_with_no_home_keeps_its_conversation_in_memory_and_writes_nothing() {
    // The shape a test fixture or an embedded use has. It must work — just
    // without history — rather than panicking or writing somewhere arbitrary.
    let runtime: std::sync::Arc<dyn medulla::runtime::Runtime> =
        std::sync::Arc::new(medulla::runtime::mock::MockRuntime::demo());
    let home = tempfile::tempdir().expect("tempdir");
    let mut app = App::new(
        runtime,
        medulla::config::LoadedConfig::defaults("medulla.tui.json".into()),
    );
    let store: std::sync::Arc<dyn WorkflowStore> =
        std::sync::Arc::new(medulla::workflows::FileWorkflowStore::new(
            vec![home.path().join("workflows")],
            home.path().join("runs"),
        ));
    store.save(&diamond("sweep")).expect("save");
    app.set_workflow_store(store);
    app.reload_workflows();

    one_turn(
        &mut app,
        "add a notify step",
        "added it",
        &["+ node notify"],
    );

    assert_eq!(
        app.copilot().expect("thread").turns.len(),
        3,
        "the pane still works"
    );
    let cwd = std::env::current_dir().expect("cwd");
    assert!(
        Transcripts::under(home.path(), &cwd)
            .load(Thread::Workflow("sweep"))
            .turns
            .is_empty(),
        "and nothing was written under a home it was never given"
    );
}
