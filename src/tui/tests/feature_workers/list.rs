//! List Workers subpage coverage: roster rendering (capacity, probe readiness and
//! budget lines, missing-detail fallbacks, pagination), the add/edit/remove
//! selection shortcuts, and the Manage Keys credential-source view.

use crate::helpers::*;

#[test]
fn workers_tab_lists_registered_peers() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Routing");
    app.focus_routing_subpage("List Workers");
    let out = render(&mut app, 120, 40);
    assert!(out.contains("List Workers · 3"), "worker count in title");
    assert!(out.contains("@w1"));
    assert!(out.contains("CODEX"));
    assert!(out.contains("IP 10.0.0.1"));
    assert!(out.contains("CPU 8 cores"));
    assert!(out.contains("RAM 18.0 GiB available / 32.0 GiB total"));
    assert!(out.contains("a add · Enter/s select"));
}

#[test]
fn worker_row_shows_probe_readiness_and_budget_lines() {
    // Step 7: the roster row folds the probe's per-harness readiness (ready/reason)
    // and budget headroom (remaining, window, cooldown) onto the capacity line.
    let mut w = worker("w1", true);
    w.readiness = vec![
        HarnessReadiness {
            provider: HarnessProvider::Codex,
            ready: true,
            reason: None,
        },
        HarnessReadiness {
            provider: HarnessProvider::Claude,
            ready: false,
            reason: Some("not authenticated".into()),
        },
    ];
    w.budgets = vec![
        HarnessBudget {
            provider: HarnessProvider::Codex,
            seat: None,
            window: BudgetWindow::Weekly,
            limit_tokens: Some(2_000_000),
            used_tokens: Some(800_000),
            remaining_tokens: Some(1_200_000),
            cooldown_until: None,
            source: BudgetSource::Configured,
        },
        // A bare estimate (no numbers, no window, no cooldown) is omitted.
        HarnessBudget {
            provider: HarnessProvider::Claude,
            seat: None,
            window: BudgetWindow::Unknown,
            limit_tokens: None,
            used_tokens: None,
            remaining_tokens: None,
            cooldown_until: None,
            source: BudgetSource::Estimate,
        },
    ];

    let mut app = app_with_roster(vec![w], None);
    app.focus_routing_subpage("List Workers");
    // Wide enough that the folded readiness + budget segments are not clipped.
    let out = render(&mut app, 200, 40);

    // Readiness for both harnesses, with the reason on the not-ready one.
    assert!(out.contains("ready codex"), "codex readiness: {out}");
    assert!(
        out.contains("not-ready claude (not authenticated)"),
        "claude readiness + reason: {out}"
    );
    // Budget headroom + window for codex; the bare claude estimate is dropped.
    assert!(
        out.contains("codex 1.2M left (weekly)"),
        "codex budget: {out}"
    );
}

#[test]
fn workers_r_refreshes_selected_machine_details() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Routing");
    app.focus_routing_subpage("List Workers");
    let _ = app.on_event(key(KeyCode::Down));
    let cmd = app.on_event(key(KeyCode::Char('r')));
    match cmd {
        Some(Cmd::WorkerOp(op)) => {
            let debug = format!("{op:?}");
            assert!(debug.contains("RefreshDetails"));
            assert!(debug.contains("w2"));
        }
        other => panic!("expected details refresh, got {other:?}"),
    }
}

#[test]
fn add_worker_page_renders_guidance_and_opens_the_prompt() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Add Worker");

    let out = render(&mut app, 120, 40);
    assert!(out.contains("Connect a tiny.place worker"));
    assert!(out.contains("@build-box Primary build worker"));

    assert!(app.on_event(key(KeyCode::Enter)).is_none());
    let (title, draft) = app.prompt_state().expect("add prompt");
    assert!(title.starts_with("Add worker"));
    assert!(draft.is_empty());
}

#[test]
fn worker_list_add_shortcut_opens_the_shared_prompt() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("List Workers");

    assert!(app.on_event(key(KeyCode::Char('a'))).is_none());
    assert_eq!(app.routing_subpage(), "Add Worker");
    assert!(app.prompt_state().is_some());
}

#[test]
fn empty_worker_list_explains_the_state_and_roster_actions_are_noops() {
    let mut app = app_with_roster(Vec::new(), None);
    app.focus_routing_subpage("List Workers");

    let out = render(&mut app, 120, 40);
    assert!(out.contains("No workers registered"));
    for code in [
        KeyCode::Enter,
        KeyCode::Char('d'),
        KeyCode::Char('e'),
        KeyCode::Char('r'),
    ] {
        assert!(app.on_event(key(code)).is_none());
    }
}

#[test]
fn worker_list_formats_missing_details_and_megabytes() {
    let mut missing = worker("w1", true);
    missing.ip_address = None;
    missing.cpu_cores = None;
    missing.memory_available_bytes = None;
    missing.memory_total_bytes = None;
    missing.handle = None;
    missing.label = None;
    missing.harness = None;

    let mut small = worker("w2", false);
    small.memory_available_bytes = Some(512 * 1024 * 1024);
    small.memory_total_bytes = Some(768 * 1024 * 1024);

    let mut app = app_with_roster(vec![missing, small], None);
    app.focus_routing_subpage("List Workers");
    let out = render(&mut app, 120, 40);

    assert!(out.contains("details not captured"));
    assert!(out.contains("512 MiB available / 768 MiB total"));
}

#[test]
fn worker_list_paginates_by_two_line_rows() {
    let workers = (1..=12)
        .map(|index| worker(&format!("w{index}"), index == 1))
        .collect();
    let mut app = app_with_roster(workers, None);
    app.focus_routing_subpage("List Workers");
    for _ in 1..12 {
        let _ = app.on_event(key(KeyCode::Down));
    }

    let out = render(&mut app, 120, 24);
    assert!(out.contains("w12"), "selected worker should remain visible");
    assert!(
        out.contains("refresh details"),
        "action footer should remain visible"
    );
}

#[test]
fn manage_keys_names_subscriptions_and_api_sources_without_values() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Manage Keys");
    let out = render(&mut app, 120, 40);
    assert!(out.contains("Provider subscriptions"));
    assert!(out.contains("Claude Code"));
    assert!(out.contains("Codex"));
    assert!(out.contains("Anthropic"));
    assert!(out.contains("OpenRouter"));
    assert!(out.contains("Press r to refresh"));
    assert!(out.contains("Secret values are never rendered"));
    assert!(app.on_event(key(KeyCode::Char('r'))).is_none());
    assert_eq!(app.status(), "Credential status refreshed");
}

#[test]
fn workers_up_down_moves_selection() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Routing");
    app.focus_routing_subpage("List Workers");
    assert_eq!(app.worker_index(), 0);
    let _ = app.on_event(key(KeyCode::Down));
    assert_eq!(app.worker_index(), 1);
    let _ = app.on_event(key(KeyCode::Down));
    assert_eq!(app.worker_index(), 2);
    // Clamp at the last worker.
    let _ = app.on_event(key(KeyCode::Down));
    assert_eq!(app.worker_index(), 2);
    let _ = app.on_event(key(KeyCode::Up));
    assert_eq!(app.worker_index(), 1);
}

#[test]
fn workers_enter_selects_and_d_removes() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Routing");
    app.focus_routing_subpage("List Workers");
    let _ = app.on_event(key(KeyCode::Down)); // select w2
    let cmd = app.on_event(key(KeyCode::Enter));
    match cmd {
        Some(Cmd::WorkerOp(op)) => assert!(format!("{op:?}").contains("Select")),
        other => panic!("expected Select, got {other:?}"),
    }
    let cmd = app.on_event(key(KeyCode::Char('d')));
    match cmd {
        Some(Cmd::WorkerOp(op)) => assert!(format!("{op:?}").contains("Remove")),
        other => panic!("expected Remove, got {other:?}"),
    }
}

#[test]
fn workers_s_and_x_are_select_and_remove_aliases() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Routing");
    app.focus_routing_subpage("List Workers");
    let cmd = app.on_event(key(KeyCode::Char('s')));
    assert!(matches!(cmd, Some(Cmd::WorkerOp(_))));
    let cmd = app.on_event(key(KeyCode::Char('x')));
    assert!(matches!(cmd, Some(Cmd::WorkerOp(_))));
}

#[test]
fn workers_e_opens_edit_label_prompt_prefilled() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Routing");
    app.focus_routing_subpage("List Workers");
    let _ = app.on_event(key(KeyCode::Char('e')));
    let (title, draft) = app.prompt_state().expect("edit prompt open");
    assert!(title.starts_with("Edit label"));
    // Prefilled with the current label.
    assert_eq!(draft, "w1 label");
    // Editing and submitting produces an Update WorkerOp.
    let _ = app.on_event(key(KeyCode::Backspace));
    let cmd = app.on_event(key(KeyCode::Enter));
    match cmd {
        Some(Cmd::WorkerOp(op)) => {
            let dbg = format!("{op:?}");
            assert!(dbg.contains("Update"));
            assert!(dbg.contains("label"));
        }
        other => panic!("expected Update, got {other:?}"),
    }
}
