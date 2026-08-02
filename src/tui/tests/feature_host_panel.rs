//! Feature tests for the Overview tab's "This device" strip.
//!
//! The panel is the only place the TUI says whether the machine the operator is
//! sitting at is part of the fleet and doing anything. These pin what it shows
//! against a rendered buffer, and — just as importantly — that it takes no space
//! at all when this device is not hosting.

use std::sync::Arc;

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::config::LoadedConfig;
use medulla::daemon::embedded::{EmbeddedDaemonStats, HostObservation};
use medulla::runtime::mock::MockRuntime;
use medulla::tinyplace::HarnessProvider;
use medulla_tui::ui::app::App;

/// An app parked on Overview, the tab the panel lives on.
fn overview_app() -> App {
    let rt = Arc::new(MockRuntime::demo());
    App::new(rt, LoadedConfig::defaults("medulla.tui.json".into()))
}

/// A host view with the given counters, not backed by a running host.
fn host(stats: EmbeddedDaemonStats) -> HostObservation {
    HostObservation::detached(
        "this-device",
        "/work/medulla",
        vec![HarnessProvider::Claude, HarnessProvider::Codex],
        HarnessProvider::Claude,
        stats,
    )
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect()
}

#[test]
fn an_idle_host_reports_itself_ready_with_what_it_runs_and_where() {
    let mut app = overview_app();
    app.set_host_observation(host(EmbeddedDaemonStats::default()));

    let screen = render(&mut app, 120, 40);

    assert!(screen.contains("This Device"), "missing panel: {screen}");
    assert!(screen.contains("ready"), "an idle host is ready: {screen}");
    assert!(screen.contains("this-device"), "missing address: {screen}");
    assert!(screen.contains("claude, codex"), "missing harnesses");
    assert!(screen.contains("/work/medulla"), "missing workspace");
}

#[test]
fn a_deep_workspace_keeps_its_root_and_project_within_two_lines() {
    let workspace = concat!(
        "/Users/example/work/tinyhumansai/a/very/deep/collection/of/projects/",
        "and/worktrees/that/would/not/fit/on/one/line/",
        "medulla-public"
    );
    let mut app = overview_app();
    app.set_host_observation(HostObservation::detached(
        "this-device",
        workspace,
        vec![HarnessProvider::Claude],
        HarnessProvider::Claude,
        EmbeddedDaemonStats::default(),
    ));

    let screen = render(&mut app, 120, 40);
    assert!(screen.contains("/Users/example"), "path root: {screen}");
    assert!(screen.contains("medulla-public"), "project tail: {screen}");
    assert!(screen.contains('…'), "omitted middle marker: {screen}");
    assert!(
        !screen.contains(workspace),
        "the path should wrap: {screen}"
    );
}

#[test]
fn a_host_with_work_in_flight_reports_busy_and_counts_what_it_has_done() {
    let mut app = overview_app();
    app.set_host_observation(host(EmbeddedDaemonStats {
        tasks_started: 5,
        tasks_completed: 3,
        tasks_failed: 1,
        probes_answered: 2,
        last_status: Some("editing src/main.rs".to_string()),
        ..EmbeddedDaemonStats::default()
    }));

    let screen = render(&mut app, 120, 40);

    // 5 started, 3 done, 1 failed leaves 1 still running.
    assert!(
        screen.contains("running 1"),
        "wrong running count: {screen}"
    );
    assert!(screen.contains("done 3"), "wrong done count: {screen}");
    assert!(screen.contains("failed 1"), "wrong failed count: {screen}");
    assert!(screen.contains("probes 2"), "wrong probe count: {screen}");
    assert!(
        screen.contains("busy"),
        "a host with work is busy: {screen}"
    );
    assert!(
        screen.contains("editing src/main.rs"),
        "the last status line should show: {screen}"
    );
}

#[test]
fn a_machine_that_only_orchestrates_gives_the_space_back_to_the_activity_feed() {
    let mut without = overview_app();
    let screen = render(&mut without, 120, 40);

    assert!(
        !screen.contains("This device"),
        "the panel should be absent, not empty: {screen}"
    );

    // The same screen with a host must differ, or the assertion above would pass
    // for a panel that simply never renders.
    let mut with = overview_app();
    with.set_host_observation(host(EmbeddedDaemonStats::default()));
    assert_ne!(screen, render(&mut with, 120, 40));
}
