//! Hosts subpage coverage: the `Host → Agents` tree (capacity, probe readiness
//! and budget lines, missing-detail fallbacks, pagination), the local/remote
//! capability split, the add/edit/remove selection shortcuts, and the Harnesses
//! credential/runtime view. Role assignment lives in the sibling `roles` module.

use crate::helpers::*;

#[test]
fn hosts_tab_lists_hosts_with_their_agents() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Hosts");
    app.focus_routing_subpage("Hosts");
    let out = render(&mut app, 120, 40);
    // Four hosts: this device (always present) and the three peers, carrying
    // three agents between them.
    assert!(out.contains("Hosts · 4 · agents · 3"), "tree counts: {out}");
    assert!(
        out.contains("this device · this-device · local"),
        "the local host leads, running or not: {out}"
    );
    assert!(
        out.contains("w1 label · w1.example:9000 · remote · read-only"),
        "a peer is a host, and says it is not editable here: {out}"
    );
    assert!(out.contains("CODEX"), "the agent row carries its harness");
    assert!(out.contains("a add host · r refresh"));

    // Capacity belongs to the machine, so it is the *host* row's preview.
    down(&mut app, 1);
    let out = render(&mut app, 120, 40);
    assert!(out.contains("IP 10.0.0.1"));
    assert!(out.contains("CPU 8 cores"));
    assert!(out.contains("RAM 18.0 GiB available / 32.0 GiB total"));
}

#[test]
fn the_local_host_offers_agent_creation_and_a_remote_does_not() {
    // The v1 capability split, made visible: an agent is declared on the machine
    // that owns it. Dispatch to a remote agent is unaffected — this is only
    // about what the operator can change from this terminal.
    let mut app = app_with_roster(vec![worker("w1", true)], None);
    app.focus_routing_subpage("Hosts");

    let out = render(&mut app, 130, 40);
    assert!(
        out.contains("none declared here · n declares one"),
        "the local host invites a new agent: {out}"
    );
    assert!(app.selected_host_is_local());

    down(&mut app, 1); // the remote host header
    let out = render(&mut app, 130, 40);
    assert!(!app.selected_host_is_local());
    assert!(
        out.contains("declared on that machine"),
        "the remote host says where its agents come from: {out}"
    );
    assert!(
        out.contains("this hub lists what its roster reaches"),
        "and is honest that it cannot see that machine's declarations: {out}"
    );

    // `n` refuses on a remote host rather than being silently inert.
    assert!(app.on_event(key(KeyCode::Char('n'))).is_none());
    assert!(
        app.status().contains("read-only"),
        "status: {}",
        app.status()
    );

    // On the local host it points at the flow that declares one.
    let _ = app.on_event(key(KeyCode::Up));
    assert!(app.on_event(key(KeyCode::Char('n'))).is_none());
    assert!(
        app.status().contains("New agent"),
        "status: {}",
        app.status()
    );
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
    // Readiness and budgets describe the machine, so they are the host row's.
    down(&mut app, 1);
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
    down(&mut app, 1);
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
    // Row 0 is this device, 1–2 are w1's host and agent, 3 is w2's host: a
    // refresh reads the probe of the machine the cursor is on either way.
    down(&mut app, 3);
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
    // The page is the pairing guidance — there is no kind to choose first.

    let out = render(&mut app, 120, 40);
    assert!(out.contains("Connect another machine for the orchestrator to delegate to."));
    assert!(out.contains("Example: 7Kx…9fQ"));
    assert!(!out.contains("@build-box"));

    assert!(app.on_event(key(KeyCode::Enter)).is_none());
    let (title, draft) = app.prompt_state().expect("add prompt");
    assert!(title.starts_with("Add host"));
    assert!(draft.is_empty());
}

#[test]
fn adding_a_remote_host_still_lands_on_the_tree() {
    // Read the pairing instructions, Enter for the address prompt, type it, and
    // the add op is emitted while the page returns to the list the add is about.
    let mut app = app_with_roster(Vec::new(), None);
    app.focus_routing_subpage("Add Host");
    let _ = app.on_event(key(KeyCode::Enter)); // open the address prompt
    assert!(app.prompt_state().is_some(), "the address prompt opened");
    for ch in "@build-box".chars() {
        let _ = app.on_event(key(KeyCode::Char(ch)));
    }

    match app.on_event(key(KeyCode::Enter)) {
        Some(Cmd::WorkerOp(op)) => {
            let debug = format!("{op:?}");
            assert!(debug.contains("Add"), "{debug}");
            assert!(debug.contains("build-box"), "{debug}");
        }
        other => panic!("expected an Add op, got {other:?}"),
    }
    assert_eq!(app.routing_subpage(), "Hosts");
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
fn an_empty_roster_still_shows_this_device_and_roster_actions_are_noops() {
    // "The local host is always present" is what the page is for: with nothing
    // registered the operator must still see the machine they are sitting at,
    // and that it has declared no agents — not an empty list that reads as a
    // fleet with no members.
    let mut app = app_with_roster(Vec::new(), None);
    app.focus_routing_subpage("Hosts");

    let out = render(&mut app, 120, 40);
    assert!(out.contains("this device · this-device · local · no agents"));
    assert!(out.contains("Hosts · 1 · agents · 0"));
    // Every roster mutation needs an entry to act on, and there is none.
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
    down(&mut app, 1);
    let out = render(&mut app, 120, 40);
    assert!(out.contains("details not captured"));

    down(&mut app, 2); // past w1's agent, onto w2's host row
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
    // This device, then a header and an agent per peer: the last row is the
    // twenty-fourth below the top.
    down(&mut app, 24);

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

    down(&mut app, 1);
    let out = render(&mut app, 120, 40);
    assert!(
        out.contains("Host · w1 label"),
        "preview titles the selected host: {out}"
    );
    assert!(out.contains("IP 10.0.0.1"));
    assert!(
        !out.contains("IP 10.0.0.2"),
        "an unselected host's capacity must not be drawn: {out}"
    );

    down(&mut app, 2);
    let out = render(&mut app, 120, 40);
    assert!(out.contains("Host · w2 label"));
    assert!(out.contains("IP 10.0.0.2"));
    assert!(!out.contains("IP 10.0.0.1"));
}

#[test]
fn harnesses_page_names_credentials_per_runtime_without_values() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Harness Types");
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
    assert_eq!(app.status(), "Harness types refreshed");
}

#[test]
fn hosts_up_down_walks_hosts_and_their_agents() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Hosts");
    app.focus_routing_subpage("Hosts");
    // This device, then a header and an agent for each of the three peers.
    assert_eq!(app.hosts_row_count(), 7);
    assert_eq!(app.host_index(), 0);
    assert!(app.hosts_cursor_on_host(), "row 0 is this device");
    down(&mut app, 1);
    assert_eq!(app.host_index(), 1);
    assert!(app.hosts_cursor_on_host(), "row 1 is w1's host header");
    down(&mut app, 1);
    assert_eq!(app.host_index(), 2);
    assert_eq!(
        app.selected_host_agent().map(|agent| agent.agent_id),
        Some("w1".to_string()),
        "row 2 is the agent under it"
    );
    // Clamp at the last row.
    down(&mut app, 10);
    assert_eq!(app.host_index(), 6);
    let _ = app.on_event(key(KeyCode::Up));
    assert_eq!(app.host_index(), 5);
}

#[test]
fn hosts_enter_selects_and_d_removes() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Hosts");
    app.focus_routing_subpage("Hosts");
    down(&mut app, 2); // w1's agent row
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
    down(&mut app, 2); // w1's agent row
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
    down(&mut app, 2); // w1's agent row
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
