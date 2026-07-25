//! Master and workspace controls added by the daemon setup surface.

use crossterm::event::KeyCode;

use super::super::super::pty::PtyManager;
use super::super::types::{Prompt, WorkerCmd, TAB_MASTER, TAB_WORKSPACES};
use super::helpers::{app_with, key, render};

#[test]
fn master_add_prompt_emits_a_connect_command() {
    let mut app = app_with(PtyManager::new(), None);
    app.set_tab(TAB_MASTER);

    assert_eq!(app.on_key(key(KeyCode::Char('a'))), None);
    assert_eq!(app.prompt, Some(Prompt::ConnectMaster(String::new())));
    for ch in "@boss".chars() {
        app.on_key(key(KeyCode::Char(ch)));
    }

    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Some(WorkerCmd::ConnectMaster("@boss".into()))
    );
    assert!(app.prompt.is_none());
}

#[test]
fn selected_master_can_be_messaged_from_the_same_screen() {
    let mut app = app_with(PtyManager::new(), None);
    app.add_master("master-address".into(), Some("@boss".into()));
    app.set_tab(TAB_MASTER);

    app.on_key(key(KeyCode::Char('i')));
    for ch in "status?".chars() {
        app.on_key(key(KeyCode::Char(ch)));
    }

    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Some(WorkerCmd::MessageMaster {
            address: "master-address".into(),
            text: "status?".into(),
        })
    );
}

#[test]
fn workspace_prompt_and_removal_preserve_the_active_root() {
    let mut app = app_with(PtyManager::new(), None);
    app.set_tab(TAB_WORKSPACES);
    app.on_key(key(KeyCode::Char('a')));
    for ch in "/another".chars() {
        app.on_key(key(KeyCode::Char(ch)));
    }
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Some(WorkerCmd::AddWorkspace("/another".into()))
    );

    assert!(!app.remove_workspace("/workspace"));
    assert!(app.workspaces().iter().any(|path| path == "/workspace"));
}

#[test]
fn daemon_subset_renders_agents_master_workspaces_and_requests() {
    let mut app = app_with(PtyManager::new(), None);
    let out = render(&mut app, 120, 18);

    for tab in ["Agents", "Master", "Workspaces", "Requests"] {
        assert!(out.contains(tab), "missing {tab}: {out}");
    }
}
