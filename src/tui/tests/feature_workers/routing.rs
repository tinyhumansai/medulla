//! Routing navigation and capacity-strategy coverage: the five
//! subpages and menu focus model, unbound-key passthrough, and the routing
//! strategy chooser (apply → `WorkerOp`, persistence to config, reload-highlight).

use crate::helpers::*;

#[test]
fn routing_nav_exposes_every_subpage() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Routing");
    let out = render(&mut app, 120, 40);
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
    tab(&mut app, "Routing");
    let out = render(&mut app, 120, 40);
    assert!(
        !out.contains("FLEET"),
        "single group heading is redundant: {out}"
    );
}

#[test]
fn routing_menu_enters_leaves_and_jumps_between_content_panes() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Routing");
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

    assert!(app.on_event(key(KeyCode::Char('6'))).is_none());
    assert_eq!(app.routing_subpage(), "Strategies");
    assert!(app.routing_focused());
}

#[test]
fn routing_pages_leave_unbound_keys_for_global_handling() {
    let mut app = app_with_workers(None);
    for page in ["Add Host", "Strategies"] {
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
