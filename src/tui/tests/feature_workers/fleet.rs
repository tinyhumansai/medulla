//! The declared fleet: where it is surfaced now that the Agents rail no longer
//! carries it, and the Templates page that lists the catalog.
//!
//! The rail used to hang the whole chain under the lanes. Its agents *were*
//! those lanes, so a worker that was both connected and declared appeared
//! twice; its hosts and harnesses are the Routing tab's Harnesses page, which
//! reads the same capacity. These now assert the absence rather than the
//! duplication.
//!
//! Both pages these render — Agent Templates and the Harnesses capacity view —
//! are reachable from `ROUTING_SUBPAGES`. Only Workspaces is hidden, and its
//! tests are the ignored ones.

use crate::helpers::*;

#[test]
fn the_rail_carries_only_what_is_running() {
    // The fleet half was a third rendering of things with two homes already:
    // its agents are the lanes themselves, and its hosts and harnesses are the
    // Routing tab's Harnesses page.
    let mut app = app_with_workers(None);
    tab(&mut app, "Agents");
    let out = render(&mut app, 160, 40);
    assert!(!out.contains("── fleet ──"), "no fleet divider: {out}");
}

#[test]
fn the_hosts_and_harnesses_are_still_reachable_on_routing() {
    // Removing the rail half must not remove the information — only the second
    // copy of it.
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Harnesses");
    let out = render(&mut app, 160, 44);
    assert!(out.contains("workshop"), "the declared host: {out}");
}

#[test]
fn the_agents_tab_shows_where_the_selected_agent_runs() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Agents");
    // Walk off the orchestrator lane onto the placed roster agent.
    let _ = app.on_event(alt_key(KeyCode::Down));
    let out = render(&mut app, 160, 40);
    assert!(
        out.contains("workshop") && out.contains("/srv/repos/auth"),
        "the agent lane names its host and workspace: {out}"
    );
}

#[test]
fn an_unplaced_lane_shows_no_placement_chip() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Agents");
    // The orchestrator lane has no descriptor, so there is no placement to
    // resolve. The chip is the one-line "host · harness · workspace" summary
    // above the transcript.
    let out = render(&mut app, 160, 40);
    assert!(!out.contains("claude-code · /srv/repos/medulla"), "{out}");
}

#[test]
fn the_demo_flag_stands_a_fleet_in_only_when_nothing_is_declared() {
    // The parse rule is the contract; the surfaces read it through
    // `fleet_capacity`, which prefers anything real over the stand-in.
    use medulla::runtime::{demo_capacity, demo_requested_from};
    assert!(!demo_requested_from(None));
    assert!(demo_requested_from(Some("1")));

    // The stand-in is a complete chain, so every surface has something to draw.
    let capacity = demo_capacity();
    assert!(!capacity.is_empty());
    assert!(!capacity.hosts.is_empty() && !capacity.templates.is_empty());

    // A runtime that declares its own capacity wins: the demo app already has
    // one, so Routing shows the scripted host rather than the stand-in's.
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Harnesses");
    let out = render(&mut app, 160, 44);
    assert!(out.contains("workshop"), "{out}");
    assert!(
        !out.contains("builder"),
        "stand-in must not mask a real fleet"
    );
}

#[test]
fn the_catalog_carries_the_built_in_coding_roles_beside_the_declared_ones() {
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Agent Templates");
    let out = render(&mut app, 160, 44);

    // The scripted runtime declares one template; the built-ins fill the rest,
    // so the page is useful before anyone writes a catalog.
    assert!(out.contains("Implementer"), "declared role: {out}");
    for role in ["Code Reviewer", "Debugger", "Doc Writer"] {
        assert!(out.contains(role), "missing built-in {role}: {out}");
    }
    assert!(
        !out.contains("No agent templates declared"),
        "the catalog is never empty now: {out}"
    );
}

#[test]
fn the_template_popup_does_not_follow_you_off_the_page_that_owns_it() {
    // `Tab` still switches tabs while the popup is up — the popup binds only
    // Esc, Enter and the page keys, and only on this page. It used to keep
    // drawing over whatever tab you landed on, where none of its dismissal keys
    // are bound, so the pane behind it took every keystroke and every paste
    // while being invisible.
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Agent Templates");
    let _ = app.on_event(key(KeyCode::Enter));
    let open = render(&mut app, 160, 44);
    assert!(
        open.contains("agent template ·"),
        "the popup is open on its own page: {open}"
    );

    tab(&mut app, "Agents");

    let elsewhere = render(&mut app, 160, 44);
    assert!(
        !elsewhere.contains("agent template ·"),
        "the popup belongs to Agent Templates, not to every tab: {elsewhere}"
    );
}

#[test]
fn a_declared_template_replaces_the_built_in_of_the_same_id() {
    // The mock declares `implementer`, which is also a built-in id: the
    // declared record wins, so the catalog lists it once.
    let mut app = app_with_workers(None);
    app.focus_routing_subpage("Agent Templates");
    let out = render(&mut app, 200, 44);
    assert_eq!(
        out.matches("Implementer").count(),
        1,
        "one row per template id: {out}"
    );
    // Built-ins are additive, not privileged: the mock harness allowlists only
    // `implementer`, and the defaults respect that rather than routing around it.
    assert!(out.contains("Implementer · reasoning · 1 place"), "{out}");
    assert!(
        out.contains("Code Reviewer · reasoning · nowhere allows it"),
        "an allowlist still binds the built-ins: {out}"
    );
}

#[test]
fn installing_the_defaults_writes_an_editable_store_and_reloads_it() {
    let home = std::env::temp_dir().join("medulla-tui-template-install");
    let _ = std::fs::remove_dir_all(&home);

    let mut app = app_with_workers(None);
    app.set_medulla_home(home.clone());
    app.focus_routing_subpage("Agent Templates");
    // `i` on the catalog installs the built-in roles as files under the home.
    assert!(app.on_event(key(KeyCode::Char('i'))).is_none());

    let store = home.join("agents");
    let reviewer = store.join("code-reviewer.toml");
    assert!(reviewer.is_file(), "expected {}", reviewer.display());
    let text = std::fs::read_to_string(&reviewer).expect("read");
    assert!(text.contains("id = \"code-reviewer\""), "{text}");
    assert!(
        text.contains("instructions = '''"),
        "editable prose: {text}"
    );
    assert!(
        app.status().contains("Installed"),
        "status: {}",
        app.status()
    );
    assert!(
        app.status().contains("agents"),
        "names the dir: {}",
        app.status()
    );

    // An edit to the store reaches the running catalog on the next read.
    std::fs::write(
        store.join("code-reviewer.toml"),
        "id = \"code-reviewer\"\nname = \"House Reviewer\"\ndescription = \"ours\"\n",
    )
    .expect("edit");
    app.reload_templates();
    let out = render(&mut app, 200, 44);
    assert!(out.contains("House Reviewer"), "{out}");

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn the_selected_agent_shows_its_placement_and_compact_meters() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Agents");
    // Walk onto the placed roster agent.
    let _ = app.on_event(alt_key(KeyCode::Down));
    let out = render(&mut app, 160, 44);

    // Where it runs — labelled, so the machine and the working directory are
    // identifiable rather than two of several bare tokens.
    assert!(out.contains("host workshop"), "host: {out}");
    assert!(out.contains("dir /srv/repos/auth"), "workspace: {out}");
    // …and how hard, as meters rather than bare numbers.
    assert!(out.contains("cpu"), "cpu meter: {out}");
    assert!(out.contains("2.10 load / 10 cores"), "{out}");
    assert!(out.contains("ram"), "ram meter: {out}");
    assert!(out.contains("20.0GB / 32.0GB"), "used, not free: {out}");
    assert!(out.contains("disk"), "disk: {out}");
}

#[test]
fn the_context_meter_splits_input_output_and_cache() {
    let mut app = app_with_workers(None);
    tab(&mut app, "Agents");
    // The orchestrator lane carries the scripted cycle's usage.
    let out = render(&mut app, 160, 44);
    assert!(out.contains("ctx"), "context meter: {out}");
    // The window is named — a bar at 0% means nothing without knowing 0% of
    // what — and the breakdown is one bracket. `1M`, not `1000k`.
    assert!(out.contains("1M window"), "the window is named: {out}");
    assert!(
        out.contains("(in 1k / out 90 / cached 70%)"),
        "in/out/cached breakdown: {out}"
    );
    // The rail row agrees with the meter about the window's size.
    assert!(out.contains("ctx 1.2k/1M"), "rail row: {out}");
}

#[test]
fn a_store_that_cannot_be_written_or_read_says_so_rather_than_failing_quietly() {
    let dir = std::env::temp_dir().join("medulla-tui-template-errors");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    // A file where the home directory should be: the store can be neither
    // created nor read, and both paths have to reach the status line.
    let blocker = dir.join("home");
    std::fs::write(&blocker, "not a directory").expect("write");

    let mut app = app_with_workers(None);
    app.set_medulla_home(blocker.clone());
    app.focus_routing_subpage("Agent Templates");
    assert!(app.on_event(key(KeyCode::Char('i'))).is_none());
    assert!(
        app.status().contains("Cannot install templates"),
        "status: {}",
        app.status()
    );

    // Reading a store that is a file, not a directory, is reported too — and
    // leaves the catalog it could not replace alone.
    let before = app.template_row_count();
    app.reload_templates();
    assert!(
        app.status().contains("Template store"),
        "status: {}",
        app.status()
    );
    assert_eq!(app.template_row_count(), before, "catalog survives");

    let _ = std::fs::remove_dir_all(&dir);
}
