//! Deterministic tests for event-loop state refresh and update checks.

use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla_tui::ui::app::App;

use super::update_checker::spawn_update_checker;
use super::{runtime_ping_needs_refresh, should_refresh_context};

#[test]
fn context_refresh_tracks_the_nested_settings_page() {
    let runtime = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));

    let _ = app.focus_settings_subpage("Usage");
    assert!(!should_refresh_context(&mut app));
    let _ = app.focus_settings_subpage("Context");
    assert!(should_refresh_context(&mut app));
    assert!(!should_refresh_context(&mut app));
}

#[test]
fn disabled_update_check_spawns_no_background_work() {
    let dir = tempfile::tempdir().unwrap();
    let env = std::collections::HashMap::new();
    let mut loaded = medulla::config::load_config(None, &env, dir.path()).unwrap();
    loaded.config.update.check = false;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    spawn_update_checker(&loaded, &tx);

    assert!(rx.try_recv().is_err());
}

/// Sinks fan out with the workflow, so the bound has to hold when several of
/// them race — the case a check-then-increment silently overshoots.
#[cfg(feature = "workflows")]
#[test]
fn concurrent_claims_never_exceed_the_bound() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Barrier;

    const LIMIT: usize = 4;
    const SINKS: usize = 16;

    let depth = Arc::new(AtomicUsize::new(0));
    let start = Arc::new(Barrier::new(SINKS));
    let granted = Arc::new(AtomicUsize::new(0));

    // Claims are held for the whole race: releasing one would free a slot the
    // next thread could legitimately take, and the count would prove nothing.
    let held = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..SINKS {
            let depth = depth.clone();
            let start = start.clone();
            let granted = granted.clone();
            let held = &held;
            scope.spawn(move || {
                start.wait();
                if let Some(frame) = super::types::PendingFrame::claim(&depth, LIMIT) {
                    granted.fetch_add(1, Ordering::Relaxed);
                    held.lock().expect("held frames").push(frame);
                }
            });
        }
    });

    assert_eq!(granted.load(Ordering::Relaxed), LIMIT);
    assert_eq!(depth.load(Ordering::Relaxed), LIMIT);

    // And every held slot comes back, so a later frame is queueable again.
    held.lock().expect("held frames").clear();
    assert_eq!(depth.load(Ordering::Relaxed), 0);
    assert!(super::types::PendingFrame::claim(&depth, LIMIT).is_some());
}

#[test]
fn a_lagged_runtime_subscription_still_refreshes_the_snapshot() {
    use tokio::sync::broadcast::error::RecvError;

    // The arm used to test `recv.is_ok()`, so the one wakeup that means the most
    // has changed — the subscription overflowed and dropped notifications — was
    // the one that redrew nothing, leaving the UI stale until an unrelated event
    // happened along.
    assert!(runtime_ping_needs_refresh(&mut false, &Ok(())));
    assert!(runtime_ping_needs_refresh(&mut false, &Err(RecvError::Lagged(7))));
}

#[test]
fn a_closed_runtime_subscription_disarms_the_refresh_arm() {
    use tokio::sync::broadcast::error::RecvError;

    // When the last sender goes away, `recv()` yields `Closed` and stays ready
    // forever. An always-ready select arm would spin at 100% CPU without this —
    // the first `Closed` must latch the arm shut, not just skip one redraw.
    let mut shut = false;
    assert!(!runtime_ping_needs_refresh(&mut shut, &Err(RecvError::Closed)));
    assert!(shut, "the arm latches shut on the first Closed");
    assert!(
        !runtime_ping_needs_refresh(&mut shut, &Ok(())),
        "later (unreachable) wakeups are ignored once shut"
    );
}
