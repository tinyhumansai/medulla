//! Routing navigation and capacity-strategy coverage: the five
//! subpages and menu focus model, unbound-key passthrough, and the routing
//! strategy chooser (apply → `WorkerOp`, persistence to config, reload-highlight).

use crate::helpers::*;

#[test]
fn routing_nav_exposes_every_subpage() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Hosts");
    let out = render(&mut app, 120, 40);
    // Only Workspaces is commented out of `ROUTING_SUBPAGES`; the nav shows
    // what it can reach.
    for page in [
        "Hosts",
        "Harnesses",
        "Agent Template",
        "Add Host",
        "Strategies",
    ] {
        assert!(out.contains(page), "missing {page}: {out}");
    }
}

#[test]
fn routing_nav_has_no_redundant_fleet_heading() {
    let mut app = app_with_roster(Vec::new(), None);
    tab(&mut app, "Hosts");
    let out = render(&mut app, 120, 40);
    assert!(
        !out.contains("FLEET"),
        "single group heading is redundant: {out}"
    );
}

#[test]
fn routing_menu_enters_leaves_and_jumps_between_content_panes() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Hosts");
    assert!(!app.routing_focused());

    assert!(app.on_event(key(KeyCode::Down)).is_none());
    assert_eq!(app.routing_subpage(), "Harnesses");
    // Entering a capacity page pulls a fresh read; capacity is never streamed.
    assert!(matches!(
        app.on_event(key(KeyCode::Enter)),
        Some(Cmd::RefreshFleet)
    ));
    assert!(app.routing_focused());
    assert!(app.on_event(key(KeyCode::Esc)).is_none());
    assert!(!app.routing_focused());

    // Number keys jump positionally, so this index moves whenever a page is
    // added. Strategies is last, which is what this is really asserting.
    assert!(app.on_event(key(KeyCode::Char('5'))).is_none());
    assert_eq!(app.routing_subpage(), "Strategies");
    assert!(app.routing_focused());
}

#[test]
fn routing_pages_leave_unbound_keys_for_global_handling() {
    let mut app = app_with_workers(None);
    for page in ["Hosts", "Add Host"] {
        app.focus_routing_subpage(page);
        assert!(app.on_event(key(KeyCode::Char('z'))).is_none());
    }
}

#[test]
fn strategies_choose_and_apply_a_capacity_policy() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Strategies");
    let _ = app.on_event(key(KeyCode::Down));
    let _ = app.on_event(key(KeyCode::Down));
    let _ = app.on_event(key(KeyCode::Up));
    let _ = app.on_event(key(KeyCode::Down));
    let out = render(&mut app, 120, 40);
    assert!(out.contains("CPU First"));
    assert!(out.contains("most logical CPU cores"));
    let cmd = app.on_event(key(KeyCode::Enter));
    match cmd {
        Some(Cmd::WorkerOp(op)) => assert!(format!("{op:?}").contains("CpuFirst")),
        other => panic!("expected strategy command, got {other:?}"),
    }
}

#[test]
fn strategy_selection_persists_to_config_and_reloads_highlighted() {
    // Step 10: applying a strategy writes it to config; a fresh app built from that
    // config loads it back onto the chooser (highlighted), not always Manual.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let mut app = app_with_workers(None);
    app.set_config_path(path.clone());
    app.focus_routing_subpage("Strategies");
    // Move to "CPU First" (index 2) and apply.
    let _ = app.on_event(key(KeyCode::Down));
    let _ = app.on_event(key(KeyCode::Down));
    let cmd = app.on_event(key(KeyCode::Enter));
    assert!(
        matches!(cmd, Some(Cmd::WorkerOp(_))),
        "apply emits a WorkerOp"
    );

    // The top-level camelCase key is written to the config file.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("routingStrategy = \"cpuFirst\""),
        "strategy persisted: {text}"
    );
    assert!(
        app.status().contains("saved"),
        "status note: {}",
        app.status()
    );

    // A fresh app loaded from that config highlights CPU First on start.
    let loaded = medulla::config::load_config(
        Some(path.to_str().unwrap()),
        &Default::default(),
        dir.path(),
    )
    .expect("reload config");
    let rt: Arc<dyn Runtime> = Arc::new(FleetRuntime::new(Vec::new(), None));
    let mut reloaded = App::new(rt, loaded);
    reloaded.focus_routing_subpage("Strategies");
    let out = render(&mut reloaded, 120, 40);
    // The selection marker sits on the CPU First row.
    assert!(out.contains("▸ CPU First"), "CPU First highlighted: {out}");
}

#[test]
fn add_host_shows_the_line_to_run_on_the_machine_being_added() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Add Host");
    // The page asks which kind first; the pairing procedure belongs to Remote,
    // and only becomes the live step once that kind is confirmed.
    let _ = app.on_event(key(KeyCode::Down));
    let _ = app.on_event(key(KeyCode::Enter));
    let out = render(&mut app, 160, 44);
    // The page is a procedure, not a definition: both halves of the pairing and
    // the two keys that drive them.
    assert!(out.contains("On the machine you want to add"), "{out}");
    // The long install command may wrap between the executable and subcommand.
    assert!(out.contains("command -v medulla"), "{out}");
    assert!(out.contains("daemon"), "{out}");
    assert!(out.contains("Back here"), "{out}");
    assert!(out.contains("c copy the install line"), "{out}");
}

#[test]
fn add_host_copies_the_install_line_rather_than_asking_it_to_be_retyped() {
    let mut app = app_with_workers(None);
    let sink = app.capture_clipboard();
    app.focus_routing_subpage("Add Host");

    // Local leads the picker, and the local flow has no install line — `c`
    // there would advertise a key that copies nothing.
    assert!(app.on_event(key(KeyCode::Char('c'))).is_none());
    assert!(
        sink.lock().unwrap().is_empty(),
        "nothing to copy on the local branch: {:?}",
        sink.lock().unwrap()
    );

    // Arrowing to Remote puts the line on screen, which is exactly when the key
    // means something.
    let _ = app.on_event(key(KeyCode::Down));
    assert!(app.on_event(key(KeyCode::Char('c'))).is_none());
    let copied = sink.lock().unwrap().clone();
    assert_eq!(copied.len(), 1, "one copy: {copied:?}");
    assert_eq!(copied[0], medulla::daemon::pairing::REMOTE_JOIN_COMMAND);
    assert!(
        app.status().contains("install line"),
        "the status names what was copied: {}",
        app.status()
    );
}

#[test]
fn arrowing_after_a_confirmed_kind_does_not_carry_the_confirmation_across() {
    // Confirming Remote and then arrowing to Local used to leave
    // `add_host_kind_chosen` set, so the next Enter skipped "Choose a harness"
    // and asked for a directory for a harness nobody had picked.
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Add Host");
    let _ = app.on_event(key(KeyCode::Down)); // Remote
    let _ = app.on_event(key(KeyCode::Enter)); // confirm it

    let _ = app.on_event(key(KeyCode::Up)); // try to go back to Local
    let out = render(&mut app, 160, 44);
    assert!(
        out.contains("On the machine you want to add"),
        "a confirmed kind stays put: {out}"
    );

    // Esc is the way back, and it lands on the kind step rather than leaving.
    let _ = app.on_event(key(KeyCode::Esc));
    let _ = app.on_event(key(KeyCode::Up));
    let _ = app.on_event(key(KeyCode::Enter));
    let out = render(&mut app, 160, 44);
    assert!(
        out.contains("Which harness it runs"),
        "local now reaches its harness step: {out}"
    );
    assert!(
        app.prompt_state().is_none(),
        "and has not skipped to a prompt"
    );
}
