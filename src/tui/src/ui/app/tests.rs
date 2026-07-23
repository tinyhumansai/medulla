//! Focused unit tests for the [`App`] screen: that every tab renders, the async
//! header toggle shows, and the composer/slash-command dispatch behaves.

use super::types::{tab_pos, SP_FEEDBACK};
use super::*;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn app() -> App {
    let rt: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let loaded = {
        let mut l = LoadedConfig::defaults("medulla.tui.json".into());
        l.config.tinyplace = Some(medulla::config::TinyplaceConfig::default());
        l
    };
    App::new(rt, loaded)
}

fn render(app: &mut App) -> String {
    let backend = TestBackend::new(100, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    buf.content().iter().map(|c| c.symbol()).collect::<String>()
}

#[test]
fn every_tab_renders() {
    for (i, name) in TABS.iter().enumerate() {
        let mut a = app();
        a.tab_index = i;
        let out = render(&mut a);
        assert!(out.contains("MEDULLA"), "tab {name} missing header");
    }
}

#[test]
fn header_shows_async_toggle() {
    let mut a = app();
    a.runtime.set_async_mode(true);
    a.refresh_snapshot();
    let out = render(&mut a);
    assert!(out.contains("ASYNC ON"));
}

#[test]
fn slash_help_switches_tab() {
    let mut a = app();
    a.tab_index = 1;
    let _ = a.execute("/help".into());
    assert_eq!(a.tab(), "Settings");
    assert_eq!(a.settings_subpage(), "Help");
}

#[test]
fn unknown_command_sets_status() {
    let mut a = app();
    let _ = a.execute("/bogus".into());
    assert!(a.status.contains("Unknown command"));
}

#[test]
fn plain_text_returns_submit_cmd() {
    let mut a = app();
    a.tab_index = 1;
    let cmd = a.execute("hello world".into());
    assert!(matches!(cmd, Some(Cmd::Submit(s)) if s == "hello world"));
    assert_eq!(a.status, "Cycle running…");
}

#[test]
fn typing_inserts_into_draft() {
    let mut a = app();
    a.tab_index = 1;
    for ch in "hi".chars() {
        a.on_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    assert_eq!(a.draft.text, "hi");
    a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    assert_eq!(a.draft.text, "hi\n");
}

fn repository_report() -> medulla::workspace::WorkspaceReport {
    use medulla::workspace::{
        BranchState, CommitSummary, FileChange, WorkspaceReport, WorkspaceSnapshot,
    };
    let root = std::path::PathBuf::from("/workspace/project");
    WorkspaceReport {
        root: root.clone(),
        snapshot: Some(WorkspaceSnapshot {
            root,
            branch: BranchState {
                name: "feat/ledger".into(),
                detached: false,
                ahead: 2,
                behind: 1,
            },
            files: vec![
                FileChange {
                    path: "src/ledger.rs".into(),
                    original_path: None,
                    index_status: ' ',
                    worktree_status: 'M',
                },
                FileChange {
                    path: "src/new.rs".into(),
                    original_path: None,
                    index_status: '?',
                    worktree_status: '?',
                },
            ],
            commits: vec![CommitSummary {
                id: "0123456789abcdef".into(),
                short_id: "0123456".into(),
                author: "Ada".into(),
                timestamp: 1,
                subject: "feat: add ledger".into(),
            }],
        }),
        error: None,
    }
}

#[test]
fn repo_tab_renders_branch_files_diff_and_history() {
    let mut a = app();
    a.tab_index = tab_pos("Repo");
    a.set_workspace_reports(vec![repository_report()]);
    a.set_workspace_diff(
        "/workspace/project".into(),
        "src/ledger.rs".into(),
        Ok("--- WORKTREE ---\n+ledger line".into()),
    );
    let out = render(&mut a);
    for expected in [
        "Git ledger",
        "feat/ledger",
        "↑2 ↓1",
        "src/ledger.rs",
        "WORKTREE",
        "ledger line",
        "Recent commits",
        "feat: add ledger",
    ] {
        assert!(out.contains(expected), "missing {expected}: {out}");
    }
}

#[test]
fn repo_keys_refresh_select_and_scroll_diff() {
    let mut a = app();
    a.tab_index = tab_pos("Repo");
    a.set_workspace_reports(vec![repository_report()]);

    let refresh = a.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    assert!(matches!(refresh, Some(Cmd::LoadWorkspaces(_))));
    let next = a.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert!(matches!(
        next,
        Some(Cmd::LoadWorkspaceDiff { path, .. }) if path == std::path::Path::new("src/new.rs")
    ));
    a.on_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    assert_eq!(a.repo.diff_scroll, 10);
    a.on_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
    assert_eq!(a.repo.diff_scroll, 0);
}

#[test]
fn repo_tab_keeps_typed_workspace_errors_visible() {
    let mut a = app();
    a.tab_index = tab_pos("Repo");
    a.set_workspace_reports(vec![medulla::workspace::WorkspaceReport {
        root: "/not-a-repo".into(),
        snapshot: None,
        error: Some("git repository discovery failed".into()),
    }]);
    let out = render(&mut a);
    assert!(out.contains("/not-a-repo"), "{out}");
    assert!(out.contains("repository discovery failed"), "{out}");
}

#[test]
fn repo_tab_renders_loading_clean_detached_and_fallback_states() {
    let mut loading = app();
    loading.tab_index = tab_pos("Repo");
    loading.set_workspaces_loading();
    assert!(render(&mut loading).contains("Refreshing local repositories"));

    let mut clean = repository_report();
    let snapshot = clean.snapshot.as_mut().unwrap();
    snapshot.branch.detached = true;
    snapshot.files.clear();
    let mut a = app();
    a.tab_index = tab_pos("Repo");
    a.set_workspace_reports(vec![clean]);
    let out = render(&mut a);
    assert!(out.contains("detached feat/ledger"), "{out}");
    assert!(out.contains("✓ clean"), "{out}");

    a.set_workspaces_loading();
    assert!(render(&mut a).contains("Git ledger · refreshing"),);

    a.set_workspace_diff(
        "/workspace/project".into(),
        "missing.rs".into(),
        Err("diff unavailable".into()),
    );
    assert!(render(&mut a).contains("diff unavailable"));

    a.set_workspace_reports(vec![medulla::workspace::WorkspaceReport {
        root: "/broken".into(),
        snapshot: None,
        error: None,
    }]);
    assert!(render(&mut a).contains("inspection failed"));
}

// --- Feedback subpage (Settings > GENERAL > Feedback) ------------------------

/// An app parked on the Feedback subpage with the mock board already loaded.
fn feedback_app() -> App {
    let mut a = app();
    // Enter the content pane, as a user arriving via `/feedback` or Enter does:
    // the board's letter bindings only act on a focused page.
    a.enter_settings_subpage(SP_FEEDBACK);
    let page = futures::executor::block_on(a.runtime.list_feedback(a.feedback_query())).unwrap();
    a.set_feedback_page(page);
    a
}

#[test]
fn slash_feedback_opens_the_board() {
    let mut a = app();
    let cmd = a.execute("/feedback".into());
    assert_eq!(a.tab(), "Settings");
    assert_eq!(a.settings_subpage(), "Feedback");
    assert!(matches!(cmd, Some(Cmd::LoadFeedback(_))));
}

#[test]
fn feedback_tab_renders_rows_and_controls() {
    let mut a = feedback_app();
    let out = render(&mut a);
    assert!(out.contains("Split the Trace tab"), "{out}");
    assert!(out.contains("u upvote"), "{out}");
    assert!(out.contains("sort hot"), "{out}");
}

#[test]
fn feedback_tab_without_a_board_shows_a_sign_in_hint() {
    let mut a = app();
    a.set_settings_subpage(SP_FEEDBACK);
    a.set_feedback_page(None);
    let out = render(&mut a);
    assert!(out.contains("signed-in backend connection"), "{out}");
}

#[test]
fn jk_keys_move_the_selection_and_load_comments() {
    let mut a = feedback_app();
    assert_eq!(a.feedback_index(), 0);
    // As a Settings subpage, Feedback browses with j/k — ↑↓ move the nav.
    let cmd = a.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(a.feedback_index(), 1);
    // Selecting a row whose comments are not loaded asks for them.
    assert!(matches!(cmd, Some(Cmd::LoadFeedbackDetail(id)) if id == "fb-2"));
}

#[test]
fn u_and_d_vote_and_toggle_off_when_repeated() {
    let mut a = feedback_app();
    // fb-1 leads the board and this user has already upvoted it, so `u` retracts.
    assert_eq!(a.feedback_items()[0].my_vote, 1);
    let cmd = a.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Cmd::VoteFeedback { value: 0, .. })));

    // `d` on the same row is a fresh downvote, not a toggle.
    let cmd = a.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Cmd::VoteFeedback { value: -1, .. })));
}

#[test]
fn s_cycles_sort_and_f_cycles_the_type_filter() {
    let mut a = feedback_app();
    assert_eq!(a.feedback_sort(), "hot");
    a.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    assert_eq!(a.feedback_sort(), "top");
    a.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    assert_eq!(a.feedback_sort(), "new");
    a.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    assert_eq!(a.feedback_sort(), "hot");

    // The filter cycles all → features → bugs → all, reloading each time.
    let cmd = a.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Cmd::LoadFeedback(_))));
}

#[test]
fn c_opens_a_comment_prompt_that_submits_the_typed_text() {
    let mut a = feedback_app();
    a.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
    for ch in "me too".chars() {
        a.on_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    let cmd = a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    match cmd {
        Some(Cmd::CommentFeedback { id, body }) => {
            assert_eq!(id, "fb-1");
            assert_eq!(body, "me too");
        }
        other => panic!("expected CommentFeedback, got {other:?}"),
    }
}

#[test]
fn an_empty_comment_is_cancelled_rather_than_posted() {
    let mut a = feedback_app();
    a.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
    let cmd = a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(cmd.is_none());
}

#[test]
fn n_walks_the_two_step_submit_prompt() {
    let mut a = feedback_app();
    // Step one: the title.
    a.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    for ch in "Add X".chars() {
        a.on_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    // Submitting the title must not send anything yet — it opens the body step.
    let cmd = a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(cmd.is_none());

    // Step two: the body, which submits.
    for ch in "please".chars() {
        a.on_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    let cmd = a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    match cmd {
        Some(Cmd::SubmitFeedback { kind, title, body }) => {
            assert_eq!(kind, medulla::client::FeedbackType::Feature);
            assert_eq!(title, "Add X");
            assert_eq!(body, "please");
        }
        other => panic!("expected SubmitFeedback, got {other:?}"),
    }
}

#[test]
fn b_submits_as_a_bug_report() {
    let mut a = feedback_app();
    a.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
    for ch in "Crash".chars() {
        a.on_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    for ch in "boom".chars() {
        a.on_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    let cmd = a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(
        cmd,
        Some(Cmd::SubmitFeedback { kind, .. }) if kind == medulla::client::FeedbackType::Bug
    ));
}

#[test]
fn a_vote_result_updates_the_row_in_place() {
    let mut a = feedback_app();
    let mut updated = a.feedback_items()[0].clone();
    updated.score = 99;
    updated.my_vote = 0;
    a.apply_feedback_item(updated);
    assert_eq!(a.feedback_items()[0].score, 99);
    // Applying an update must not move the cursor.
    assert_eq!(a.feedback_index(), 0);
}

#[test]
fn clicking_a_context_chunk_selects_it() {
    // Context is a Settings *subpage*, not a top-level tab, so the click router
    // has to match on the subpage — matching on the tab made this branch
    // unreachable and clicking a chunk silently did nothing.
    let mut a = app();
    let _ = a.focus_settings_subpage("Context");
    assert_eq!(a.settings_subpage(), "Context");

    a.contexts = vec![
        medulla::runtime::ContextItem {
            ref_: "a".into(),
            kind: "file".into(),
            bytes: 10,
            content: "alpha".into(),
        },
        medulla::runtime::ContextItem {
            ref_: "b".into(),
            kind: "file".into(),
            bytes: 20,
            content: "bravo".into(),
        },
    ];
    a.hit_context = Some(ratatui::layout::Rect::new(0, 5, 40, 10));

    // Second row inside the hit rect selects the second chunk.
    let _ = a.handle_click(3, 6);
    assert_eq!(a.context_index, 1);

    // A click past the last chunk leaves the selection alone.
    let _ = a.handle_click(3, 9);
    assert_eq!(a.context_index, 1);

    // A click outside the rect is ignored entirely.
    let _ = a.handle_click(3, 40);
    assert_eq!(a.context_index, 1);
}
