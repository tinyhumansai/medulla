//! The `MEDULLA_DEMO_FLEET` stand-in fleet, end to end.
//!
//! Its own test binary because the flag is process-global: setting it inside the
//! shared binary would leak into every other test's view of the world. One test
//! here, one process, one env var.

use std::sync::Arc;

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla_tui::ui::app::{App, TABS};

#[test]
fn the_stand_in_fleet_renders_when_the_runtime_declares_nothing() {
    // An empty runtime: no capacity, no roster, no registered peers — the state
    // an operator is in before anything is wired up.
    std::env::set_var(medulla::runtime::DEMO_FLEET_ENV, "1");
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::empty());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    app.tab_index = TABS.iter().position(|t| *t == "Agents").unwrap();

    let mut terminal = Terminal::new(TestBackend::new(160, 44)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let out: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();

    // The whole chain, in the rail, under the lanes.
    assert!(out.contains("── fleet ──"), "{out}");
    assert!(out.contains("workshop"), "host: {out}");
    assert!(out.contains("claude-code"), "harness: {out}");
    assert!(out.contains("/srv/repos/medulla"), "workspace: {out}");
    assert!(out.contains("dev-1"), "agent lane and placement: {out}");
    // The nearly-spent seat is what makes the budget rendering worth having.
    assert!(out.contains("60k/1.0M left"), "budget: {out}");
    // And the harness that cannot take work says why.
    assert!(out.contains("cli not authenticated"), "readiness: {out}");

    std::env::remove_var(medulla::runtime::DEMO_FLEET_ENV);
}
