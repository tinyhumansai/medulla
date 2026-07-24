//! Focused tests for local task persistence, synchronization, and recurrence.

use super::*;
use tempfile::tempdir;

fn task(source: Option<TaskSourceRef>) -> Task {
    Task {
        id: "local".into(),
        title: "local".into(),
        description: "edit".into(),
        status: TaskStatus::InProgress,
        source,
        recurrence: None,
        created_at: "1".into(),
        updated_at: "2".into(),
        last_synced_at: None,
        dispatch: serde_json::Value::Null,
    }
}

#[test]
fn round_trip_and_missing_file_are_tolerant() {
    let d = tempdir().unwrap();
    let path = d.path().join("nested/tasks.json");
    let mut r = TaskRepository::open(&path).unwrap();
    r.document_mut().tasks.push(task(None));
    r.save().unwrap();
    drop(r);
    assert_eq!(
        TaskRepository::open(path).unwrap().document().tasks.len(),
        1
    );
}

#[test]
fn repositories_serialize_the_read_mutate_save_lifecycle() {
    let d = tempdir().unwrap();
    let path = d.path().join("tasks.json");
    let mut first = TaskRepository::open(&path).unwrap();
    first.document_mut().tasks.push(task(None));

    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let second_path = path.clone();
    let writer = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let mut second = TaskRepository::open(second_path).unwrap();
        let mut second_task = task(None);
        second_task.id = "second".into();
        second.document_mut().tasks.push(second_task);
        second.save().unwrap();
        done_tx.send(()).unwrap();
    });

    started_rx.recv().unwrap();
    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err(),
        "a second repository must wait for the first lifecycle to finish"
    );
    first.save().unwrap();
    drop(first);
    done_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    writer.join().unwrap();

    assert_eq!(
        TaskRepository::open(path).unwrap().document().tasks.len(),
        2
    );
}

#[test]
fn malformed_and_unreadable_paths_are_typed() {
    let d = tempdir().unwrap();
    let malformed = d.path().join("tasks.json");
    std::fs::write(&malformed, "{").unwrap();
    assert!(matches!(
        TaskRepository::open(&malformed),
        Err(TaskRepositoryError::Parse { .. })
    ));
    assert!(matches!(
        TaskRepository::open(d.path()),
        Err(TaskRepositoryError::Read { .. })
    ));
}

#[test]
fn source_defaults_are_applied_during_deserialization() {
    let source: SourceConfig = serde_json::from_value(serde_json::json!({
        "id": "github",
        "provider": "github",
        "enabled": true,
        "repository": "tinyhumansai/medulla"
    }))
    .unwrap();
    assert_eq!(source.state, "open");
    assert!(source.labels.is_empty());
}

#[test]
fn save_reports_directory_creation_write_and_rename_failures() {
    let d = tempdir().unwrap();

    let blocker = d.path().join("blocker");
    std::fs::write(&blocker, "file").unwrap();
    let create_error = TaskRepository {
        path: blocker.join("tasks.json"),
        document: TaskDocument::default(),
        _lock: None,
    };
    assert!(matches!(
        create_error.save(),
        Err(TaskRepositoryError::Write { .. })
    ));

    let write_target = d.path().join("write.json");
    std::fs::create_dir(write_target.with_extension("json.tmp")).unwrap();
    let write_error = TaskRepository {
        path: write_target,
        document: TaskDocument::default(),
        _lock: None,
    };
    assert!(matches!(
        write_error.save(),
        Err(TaskRepositoryError::Write { .. })
    ));

    let rename_target = d.path().join("rename.json");
    std::fs::create_dir(&rename_target).unwrap();
    let rename_error = TaskRepository {
        path: rename_target,
        document: TaskDocument::default(),
        _lock: None,
    };
    assert!(matches!(
        rename_error.save(),
        Err(TaskRepositoryError::Write { .. })
    ));
}

#[test]
fn sync_deduplicates_and_preserves_local_edits() {
    let source = TaskSourceRef {
        provider: "github".into(),
        source_id: "repo#1".into(),
        url: None,
    };
    let mut r = TaskRepository {
        path: PathBuf::new(),
        document: TaskDocument {
            tasks: vec![task(Some(source.clone()))],
            sources: vec![],
        },
        _lock: None,
    };
    let mut incoming = task(Some(source));
    incoming.title = "remote".into();
    incoming.description = "remote body".into();
    assert!(!r.upsert_synced(incoming));
    assert_eq!(r.document.tasks[0].title, "local");
}

#[test]
fn sync_fills_blank_local_fields_and_inserts_new_records() {
    let source = TaskSourceRef {
        provider: "github".into(),
        source_id: "repo#1".into(),
        url: None,
    };
    let mut local = task(Some(source.clone()));
    local.title.clear();
    local.description.clear();
    let mut repository = TaskRepository {
        path: PathBuf::new(),
        document: TaskDocument {
            tasks: vec![local],
            sources: vec![],
        },
        _lock: None,
    };
    let mut incoming = task(Some(source));
    incoming.title = "remote".into();
    incoming.description = "remote body".into();
    assert!(!repository.upsert_synced(incoming));
    assert_eq!(repository.document.tasks[0].title, "remote");
    assert_eq!(repository.document.tasks[0].description, "remote body");

    assert!(repository.upsert_synced(task(None)));
    assert_eq!(repository.document.tasks.len(), 2);
}

#[test]
fn recurring_instance_is_open_and_non_recurring() {
    let mut t = task(None);
    assert!(TaskRepository::recurring_instance(&t).is_none());
    t.recurrence = Some(RecurringTask {
        recurrence: Recurrence::Daily,
        next_at: "tomorrow".into(),
    });
    let i = TaskRepository::recurring_instance(&t).unwrap();
    assert_eq!(i.status, TaskStatus::Open);
    assert!(i.recurrence.is_none());
    assert_ne!(i.id, t.id);
}

#[test]
fn timestamp_is_a_positive_epoch_value() {
    assert!(now_timestamp().parse::<u64>().unwrap() > 0);
}
