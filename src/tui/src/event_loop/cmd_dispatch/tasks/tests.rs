//! Tests for serialized task persistence command dispatch.

use medulla::tasks::{SourceConfig, Task, TaskDocument};

use super::{run_task_cmd_at, AppMsg, Cmd};

fn task(id: &str) -> Task {
    Task {
        id: id.into(),
        title: id.into(),
        description: String::new(),
        status: Default::default(),
        source: None,
        recurrence: None,
        created_at: "1".into(),
        updated_at: "1".into(),
        last_synced_at: None,
        dispatch: serde_json::Value::Null,
    }
}

async fn next(receiver: &mut tokio::sync::mpsc::UnboundedReceiver<AppMsg>) -> AppMsg {
    tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
        .await
        .expect("task command timed out")
        .expect("task command dropped its response channel")
}

#[tokio::test]
async fn task_commands_save_load_and_delete_without_losing_other_data() {
    let directory = tempfile::tempdir().unwrap();
    let home = Some(directory.path().to_path_buf());
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();

    assert!(run_task_cmd_at(Cmd::SaveTask(Box::new(task("one"))), home.clone(), &sender).is_none());
    assert!(
        matches!(next(&mut receiver).await, AppMsg::Status(status) if status == "Tasks · saved")
    );

    let source = SourceConfig {
        id: "github".into(),
        provider: "github".into(),
        enabled: true,
        repository: "tinyhumansai/medulla".into(),
        state: "open".into(),
        labels: Vec::new(),
        filter: None,
        token: None,
    };
    assert!(run_task_cmd_at(
        Cmd::SaveTasks(Box::new(TaskDocument {
            tasks: Vec::new(),
            sources: vec![source],
        })),
        home.clone(),
        &sender,
    )
    .is_none());
    assert!(
        matches!(next(&mut receiver).await, AppMsg::Status(status) if status == "Sources · saved")
    );

    run_task_cmd_at(Cmd::LoadTasks, home.clone(), &sender);
    assert!(matches!(
        next(&mut receiver).await,
        AppMsg::TasksLoaded(document)
            if document.tasks.len() == 1 && document.sources.len() == 1
    ));

    run_task_cmd_at(Cmd::DeleteTask("one".into()), home, &sender);
    assert!(
        matches!(next(&mut receiver).await, AppMsg::Status(status) if status == "Tasks · deleted")
    );
}

#[tokio::test]
async fn task_commands_surface_missing_home_and_source_errors() {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    run_task_cmd_at(Cmd::LoadTasks, None, &sender);
    assert!(
        matches!(next(&mut receiver).await, AppMsg::Status(status) if status.contains("home is unavailable"))
    );

    let directory = tempfile::tempdir().unwrap();
    run_task_cmd_at(
        Cmd::SyncTasks("missing".into()),
        Some(directory.path().to_path_buf()),
        &sender,
    );
    assert!(
        matches!(next(&mut receiver).await, AppMsg::Status(status) if status.contains("source not found"))
    );

    assert!(matches!(
        run_task_cmd_at(Cmd::Quit, None, &sender),
        Some(command) if matches!(*command, Cmd::Quit)
    ));
}
