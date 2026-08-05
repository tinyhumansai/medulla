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
use medulla_tui::ui::app::App;

// Harnesses is where the fleet is read now: PR #86 took it out of the Agents
// rail on the grounds that this page shows the same hosts, budgets and
// readiness, so it is the only place that information appears.
#[test]
fn the_stand_in_fleet_renders_when_the_runtime_declares_nothing() {
    // An empty runtime: no capacity, no roster, no registered peers — the state
    // an operator is in before anything is wired up.
    std::env::set_var(medulla::runtime::DEMO_FLEET_ENV, "1");
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::empty());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    app.focus_routing_subpage("Harness Types");

    let mut terminal = Terminal::new(TestBackend::new(160, 44)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let out: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();

    // The stand-in is surfaced on Routing › Harnesses. It used to hang in the
    // Agents rail as well, which made every declared worker appear twice —
    // once as its lane and once as its host — so the rail now carries only
    // what is running and this is the one place the declaration renders.
    assert!(out.contains("workshop"), "host: {out}");
    assert!(out.contains("Claude Code"), "harness: {out}");
    // The nearly-spent seat is what makes the budget rendering worth having.
    assert!(out.contains("60k left"), "budget: {out}");
    // And the harness that cannot take work still says so.
    assert!(out.contains("not ready"), "readiness: {out}");

    std::env::remove_var(medulla::runtime::DEMO_FLEET_ENV);
}
