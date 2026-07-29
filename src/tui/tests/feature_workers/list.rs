//! Hosts subpage coverage: roster rendering (capacity, probe readiness and
//! budget lines, missing-detail fallbacks, pagination), the add/edit/remove
//! selection shortcuts, and the Harnesses credential/runtime view.

use crate::helpers::*;

#[test]
fn hosts_tab_lists_registered_machines() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Hosts");
    app.focus_routing_subpage("Hosts");
    let out = render(&mut app, 120, 40);
    assert!(out.contains("Hosts · 3"), "host count in title");
    assert!(out.contains("@w1"));
    assert!(out.contains("CODEX"));
    assert!(out.contains("IP 10.0.0.1"));
    assert!(out.contains("CPU 8 cores"));
    assert!(out.contains("RAM 18.0 GiB available / 32.0 GiB total"));
    assert!(out.contains("a add · Enter/s select"));
}

#[test]
fn host_row_shows_probe_readiness_and_budget_lines() {
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
    app.focus_routing_subpage("Hosts");
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
    // Assert the drop, don't just claim it: "claude" appears once (its readiness
    // reason) and never a second time as a budget segment. A regression that
    // rendered the bare estimate would add a "claude" token to the budget line.
    assert_eq!(
        out.matches("claude").count(),
        1,
        "bare claude estimate must not render a budget segment: {out}"
    );
}

#[test]
fn host_row_sanitizes_probe_text_and_keeps_fractional_budget() {
    // Readiness reasons and budgets are untrusted peer-supplied data. Terminal
    // control/escape sequences in a reason must be stripped before rendering, and
    // sub-million headroom must keep one fractional digit (1.5k, not a rounded 2k).
    let mut w = worker("w1", true);
    w.readiness = vec![HarnessReadiness {
        provider: HarnessProvider::Claude,
        ready: false,
        // Control bytes (ESC introducer + BEL terminator) smuggled between the
        // printable letters of "denied"; stripping them leaves plain "denied".
        reason: Some("den\u{1b}i\u{7}ed".into()),
    }];
    w.budgets = vec![HarnessBudget {
        provider: HarnessProvider::Codex,
        seat: None,
        window: BudgetWindow::Daily,
        limit_tokens: Some(4_000),
        used_tokens: Some(2_500),
        remaining_tokens: Some(1_500),
        cooldown_until: None,
        source: BudgetSource::Configured,
    }];

    let mut app = app_with_roster(vec![w], None);
    app.focus_routing_subpage("Hosts");
    let out = render(&mut app, 200, 40);

    // The escape/OSC bytes are gone; only the printable tail survives.
    assert!(
        out.contains("not-ready claude (denied)"),
        "sanitized reason: {out:?}"
    );
    assert!(
        !out.contains('\u{1b}') && !out.contains('\u{7}'),
        "control bytes must not reach the terminal: {out:?}"
    );
    // Fractional thousands preserved.
    assert!(out.contains("codex 1.5k left (daily)"), "budget: {out}");
}

#[test]
fn hosts_r_refreshes_selected_machine_details() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Hosts");
    app.focus_routing_subpage("Hosts");
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
fn add_host_page_renders_guidance_and_opens_the_prompt() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Add Host");
    // Local leads the picker; choosing Remote lights up the pairing guidance.
    let _ = app.on_event(key(KeyCode::Down));
    let _ = app.on_event(key(KeyCode::Enter));

    let out = render(&mut app, 120, 40);
    assert!(out.contains("Add a host for the orchestrator to delegate to."));
    // The handle shortcut, which is the way to add a host without copying
    // anything at all.
    assert!(out.contains("@build-box"));

    assert!(app.on_event(key(KeyCode::Enter)).is_none());
    let (title, draft) = app.prompt_state().expect("add prompt");
    assert!(title.starts_with("Add host"));
    assert!(draft.is_empty());
}

#[test]
fn host_list_add_shortcut_opens_the_shared_prompt() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Hosts");

    assert!(app.on_event(key(KeyCode::Char('a'))).is_none());
    assert_eq!(app.routing_subpage(), "Add Host");
    assert!(app.prompt_state().is_some());
}

#[test]
fn empty_host_list_explains_the_state_and_roster_actions_are_noops() {
    let mut app = app_with_roster(Vec::new(), None);
    app.focus_routing_subpage("Hosts");

    let out = render(&mut app, 120, 40);
    assert!(out.contains("No hosts registered"));
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
fn host_list_formats_missing_details_and_megabytes() {
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
    app.focus_routing_subpage("Hosts");

    // Capacity is the preview's job now, so each host's line is read where the
    // cursor is — which is the point of the split: one host's detail at a time.
    let out = render(&mut app, 120, 40);
    assert!(out.contains("details not captured"));

    let _ = app.on_event(key(KeyCode::Down));
    let out = render(&mut app, 120, 40);
    assert!(out.contains("512 MiB available / 768 MiB total"));
}

#[test]
fn host_list_paginates_by_single_line_rows() {
    let workers = (1..=12)
        .map(|index| worker(&format!("w{index}"), index == 1))
        .collect();
    let mut app = app_with_roster(workers, None);
    app.focus_routing_subpage("Hosts");
    for _ in 1..12 {
        let _ = app.on_event(key(KeyCode::Down));
    }

    let out = render(&mut app, 120, 24);
    assert!(out.contains("w12"), "selected worker should remain visible");
    assert!(
        out.contains("r refresh"),
        "action footer should remain visible"
    );
}

#[test]
fn the_preview_follows_the_cursor_rather_than_repeating_every_host() {
    let mut first = worker("w1", true);
    first.ip_address = Some("10.0.0.1".into());
    let mut second = worker("w2", false);
    second.ip_address = Some("10.0.0.2".into());

    let mut app = app_with_roster(vec![first, second], None);
    app.focus_routing_subpage("Hosts");

    let out = render(&mut app, 120, 40);
    assert!(
        out.contains("Host · w1"),
        "preview titles the selected host"
    );
    assert!(out.contains("IP 10.0.0.1"));
    assert!(
        !out.contains("IP 10.0.0.2"),
        "an unselected host's capacity must not be drawn: {out}"
    );

    let _ = app.on_event(key(KeyCode::Down));
    let out = render(&mut app, 120, 40);
    assert!(out.contains("Host · w2"));
    assert!(out.contains("IP 10.0.0.2"));
    assert!(!out.contains("IP 10.0.0.1"));
}

#[test]
fn space_on_a_role_offers_the_selected_host_for_it_and_takes_it_back() {
    let mut app = app_with_roster(vec![worker("w1", true)], None);
    app.focus_routing_subpage("Hosts");

    // Roles come from the agent-template catalog, which ships built-in coding
    // roles even with nothing declared — so there is always something to toggle.
    let out = render(&mut app, 120, 44);
    assert!(
        out.contains("none assigned · offered for any role"),
        "an unassigned host reads as general, not excluded: {out}"
    );

    let _ = app.on_event(key(KeyCode::Right));
    let out = render(&mut app, 120, 44);
    assert!(out.contains("[ ]"), "the toggle list is drawn: {out}");

    let cmd = app.on_event(key(KeyCode::Char(' ')));
    let assigned = match cmd {
        Some(Cmd::WorkerOp(WorkerOp::SetRoles { id, roles })) => {
            assert_eq!(id, "w1");
            assert_eq!(roles.len(), 1, "exactly the toggled role");
            roles
        }
        other => panic!("expected a SetRoles op, got {other:?}"),
    };

    // And back off again. The op is a whole-list replacement, so removing the
    // only role must send an *empty* list — not omit the field, which would
    // read as "leave the roles alone" and make the toggle one-way.
    let mut held = app_with_roster(
        vec![{
            let mut w = worker("w1", true);
            w.roles = assigned;
            w
        }],
        None,
    );
    held.focus_routing_subpage("Hosts");
    let _ = held.on_event(key(KeyCode::Right));
    match held.on_event(key(KeyCode::Char(' '))) {
        Some(Cmd::WorkerOp(WorkerOp::SetRoles { id, roles })) => {
            assert_eq!(id, "w1");
            assert!(
                roles.is_empty(),
                "removal sends an empty list, got {roles:?}"
            );
        }
        other => panic!("expected a SetRoles op, got {other:?}"),
    }
}

#[test]
fn leaving_the_role_list_hands_the_arrows_back_to_the_host_roster() {
    let mut app = app_with_roster(vec![worker("w1", true), worker("w2", false)], None);
    app.focus_routing_subpage("Hosts");

    let _ = app.on_event(key(KeyCode::Right));
    // Down now walks roles, so the selected host must not have changed.
    let _ = app.on_event(key(KeyCode::Down));
    let out = render(&mut app, 120, 44);
    assert!(out.contains("Host · w1"), "still previewing w1: {out}");

    let _ = app.on_event(key(KeyCode::Left));
    let _ = app.on_event(key(KeyCode::Down));
    let out = render(&mut app, 120, 44);
    assert!(out.contains("Host · w2"), "back on the roster: {out}");
}

#[test]
fn harnesses_page_names_credentials_per_runtime_without_values() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Harnesses");
    let out = render(&mut app, 120, 40);
    // Credentials read per harness kind, because that is what spends them.
    assert!(out.contains("Claude Code"));
    assert!(out.contains("Claude subscription"));
    assert!(out.contains("Codex subscription"));
    assert!(out.contains("Anthropic"));
    assert!(out.contains("OpenRouter"));
    // The declared claude-code harness renders under its own kind, with the host
    // it runs on and the budget it reports.
    assert!(out.contains("workshop · ready"), "{out}");
    assert!(out.contains("anthropic 5h · 760k left"), "{out}");
    assert!(out.contains("Press r to refresh"));
    assert!(out.contains("Secret values are never rendered"));
    // `r` re-reads both halves: local credentials and declared capacity.
    assert!(matches!(
        app.on_event(key(KeyCode::Char('r'))),
        Some(Cmd::RefreshFleet)
    ));
    assert_eq!(app.status(), "Harnesses refreshed");
}

#[test]
fn hosts_up_down_moves_selection() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Hosts");
    app.focus_routing_subpage("Hosts");
    assert_eq!(app.host_index(), 0);
    let _ = app.on_event(key(KeyCode::Down));
    assert_eq!(app.host_index(), 1);
    let _ = app.on_event(key(KeyCode::Down));
    assert_eq!(app.host_index(), 2);
    // Clamp at the last worker.
    let _ = app.on_event(key(KeyCode::Down));
    assert_eq!(app.host_index(), 2);
    let _ = app.on_event(key(KeyCode::Up));
    assert_eq!(app.host_index(), 1);
}

#[test]
fn hosts_enter_selects_and_d_removes() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Hosts");
    app.focus_routing_subpage("Hosts");
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
fn hosts_s_and_x_are_select_and_remove_aliases() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Hosts");
    app.focus_routing_subpage("Hosts");
    let cmd = app.on_event(key(KeyCode::Char('s')));
    assert!(matches!(cmd, Some(Cmd::WorkerOp(_))));
    let cmd = app.on_event(key(KeyCode::Char('x')));
    assert!(matches!(cmd, Some(Cmd::WorkerOp(_))));
}

#[test]
fn hosts_e_opens_edit_label_prompt_prefilled() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Hosts");
    app.focus_routing_subpage("Hosts");
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

#[test]
fn an_assigned_role_is_summarised_and_checked() {
    let mut w = worker("w1", true);
    w.roles = vec!["code-reviewer".into()];

    let mut app = app_with_roster(vec![w], None);
    app.focus_routing_subpage("Hosts");
    let out = render(&mut app, 130, 44);

    // The summary leads the block, so what a host is offered for is readable
    // without counting checkboxes.
    assert!(
        out.contains("roles      code-reviewer"),
        "assigned roles summarised: {out}"
    );
    assert!(out.contains("[x] code-reviewer"), "and checked: {out}");
    assert!(out.contains("[ ] implementer"), "others unchecked: {out}");
    // And the roster row carries the count, so the list still says which hosts
    // have been given roles at all.
    assert!(out.contains("· 1 role"), "count on the roster row: {out}");
}

#[test]
fn a_role_below_the_fold_stays_visible_when_selected() {
    // The preview is capped at half the page, so on a short terminal the role
    // list must scroll — a role that can be selected but not seen is a toggle
    // the operator flips blind.
    let mut app = app_with_roster(vec![worker("w1", true)], None);
    app.focus_routing_subpage("Hosts");
    let _ = app.on_event(key(KeyCode::Right));

    let out = render(&mut app, 130, 26);
    let last = "repo-orchestrator";
    assert!(!out.contains(last), "the tail starts off-screen: {out}");

    for _ in 0..12 {
        let _ = app.on_event(key(KeyCode::Down));
    }
    let out = render(&mut app, 130, 26);
    assert!(
        out.contains(&format!("▸ [ ] {last}")),
        "the window follows the cursor to the last role: {out}"
    );
}

#[test]
fn a_local_host_that_could_not_be_saved_can_be_retried() {
    // The entry used to be pushed into the in-process config before the write,
    // so a failed save left a host that was never written and never started —
    // and the duplicate check then refused every retry for that directory,
    // which the operator could not clear without restarting.
    let mut app = app_with_roster(Vec::new(), None);
    let workspace = std::env::temp_dir().to_string_lossy().into_owned();

    // Drive the wizard: Local is first, one Enter settles the harness step, the
    // next opens the directory prompt. No config path is set on this app, so
    // the save cannot succeed.
    let add = |app: &mut App, workspace: &str| {
        app.focus_routing_subpage("Add Host");
        let _ = app.on_event(key(KeyCode::Enter));
        let _ = app.on_event(key(KeyCode::Enter));
        assert!(app.prompt_state().is_some(), "the directory prompt opened");
        for ch in workspace.chars() {
            let _ = app.on_event(key(KeyCode::Char(ch)));
        }
        app.on_event(key(KeyCode::Enter))
    };

    assert!(
        add(&mut app, &workspace).is_none(),
        "nothing starts when nothing was saved"
    );
    assert!(app.status().contains("No config file"), "{}", app.status());

    // The second attempt must reach the same failure, not "Already hosted".
    assert!(add(&mut app, &workspace).is_none());
    assert!(
        !app.status().contains("Already hosted"),
        "a failed save must not block the retry: {}",
        app.status()
    );
}
