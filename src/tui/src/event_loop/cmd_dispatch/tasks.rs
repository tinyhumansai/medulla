//! Dispatch local task persistence and provider synchronization commands.

use medulla_tui::ui::app::Cmd;

use super::super::AppMsg;

/// Spawn a Tasks command, returning non-Tasks commands to the main dispatcher.
pub(super) fn run_task_cmd(
    cmd: Cmd,
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>,
) -> Option<Box<Cmd>> {
    match cmd {
        Cmd::LoadTasks => {
            let home = medulla_home();
            let tx = msg_tx.clone();
            tokio::task::spawn_blocking(move || {
                let result = home.map_or_else(
                    || Err("Medulla home is unavailable".into()),
                    |home| {
                        medulla::tasks::TaskRepository::in_home(home)
                            .map(|repo| repo.document().clone())
                            .map_err(|error| error.to_string())
                    },
                );
                let message = match result {
                    Ok(document) => AppMsg::TasksLoaded(document),
                    Err(error) => AppMsg::Status(format!("Tasks · {error}")),
                };
                let _ = tx.send(message);
            });
        }
        Cmd::SaveTask(task) => {
            let tx = msg_tx.clone();
            tokio::task::spawn_blocking(move || {
                let task = *task;
                let result = medulla_home().map_or_else(
                    || Err("Medulla home is unavailable".into()),
                    |home| {
                        let mut repository = medulla::tasks::TaskRepository::in_home(home)
                            .map_err(|error| error.to_string())?;
                        repository
                            .document_mut()
                            .tasks
                            .retain(|old| old.id != task.id);
                        repository.document_mut().tasks.push(task);
                        repository.save().map_err(|error| error.to_string())
                    },
                );
                let _ = tx.send(match result {
                    Ok(()) => AppMsg::Status("Tasks · saved".into()),
                    Err(error) => AppMsg::Status(format!("Tasks · {error}")),
                });
            });
        }
        Cmd::SaveTasks(document) => {
            let tx = msg_tx.clone();
            tokio::task::spawn_blocking(move || {
                let result = medulla_home().map_or_else(
                    || Err("Medulla home is unavailable".into()),
                    |home| {
                        let mut repository = medulla::tasks::TaskRepository::in_home(home)
                            .map_err(|error| error.to_string())?;
                        *repository.document_mut() = *document;
                        repository.save().map_err(|error| error.to_string())
                    },
                );
                let _ = tx.send(match result {
                    Ok(()) => AppMsg::Status("Sources · saved".into()),
                    Err(error) => AppMsg::Status(format!("Sources · {error}")),
                });
            });
        }
        Cmd::DeleteTask(id) => {
            let tx = msg_tx.clone();
            tokio::task::spawn_blocking(move || {
                let result = medulla_home().map_or_else(
                    || Err("Medulla home is unavailable".into()),
                    |home| {
                        let mut repository = medulla::tasks::TaskRepository::in_home(home)
                            .map_err(|error| error.to_string())?;
                        repository.document_mut().tasks.retain(|task| task.id != id);
                        repository.save().map_err(|error| error.to_string())
                    },
                );
                let _ = tx.send(match result {
                    Ok(()) => AppMsg::Status("Tasks · deleted".into()),
                    Err(error) => AppMsg::Status(format!("Tasks · {error}")),
                });
            });
        }
        Cmd::SyncTasks(id) => {
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                let result = match medulla_home() {
                    Some(home) => match medulla::tasks::TaskRepository::in_home(home) {
                        Ok(mut repository) => {
                            let source = repository
                                .document()
                                .sources
                                .iter()
                                .find(|source| source.id == id)
                                .cloned();
                            match source {
                                Some(source) => {
                                    use medulla::tasks::github::TaskSourceProvider;
                                    medulla::tasks::github::GitHubProvider::default()
                                        .sync(&source, &mut repository)
                                        .await;
                                    repository
                                        .save()
                                        .map(|_| "Tasks · synchronized".to_string())
                                        .map_err(|error| error.to_string())
                                }
                                None => Err("source not found".into()),
                            }
                        }
                        Err(error) => Err(error.to_string()),
                    },
                    None => Err("Medulla home is unavailable".into()),
                };
                let _ =
                    tx.send(AppMsg::Status(result.unwrap_or_else(|error| {
                        format!("Tasks · sync failed: {error}")
                    })));
            });
        }
        other => return Some(Box::new(other)),
    }
    None
}

/// Resolve the configured Medulla home without inventing another fallback.
fn medulla_home() -> Option<std::path::PathBuf> {
    std::env::var_os("MEDULLA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|path| std::path::PathBuf::from(path).join(".medulla"))
        })
}
