//! The Hooks page: what it shows about the hooks a launched harness will carry,
//! how an operator adds one, and the reports arriving back from the harnesses
//! that ran them.

use crate::helpers::*;

use medulla::harness_hooks::{HookEvent, HookEventLog, HookReport, HooksConfig};

/// An app on the Hooks page, with Medulla's own hooks resolved as a real config
/// load would leave them.
fn app_on_hooks() -> App {
    let mut app = app_with_workers(None);
    app.loaded.config.hooks = HooksConfig::default()
        .with_builtin(medulla::harness_hooks::builtin::hooks("/usr/bin/medulla"));
    tab(&mut app, "Hosts");
    app.focus_routing_subpage("Hooks");
    app
}

#[test]
fn the_page_shows_what_a_launched_harness_will_carry() {
    let mut app = app_on_hooks();
    let out = render(&mut app, 160, 40);

    assert!(out.contains("Hooks · 8"), "{out}");
    // The events, so an operator can see the session is being watched at all.
    for event in ["SessionStart", "PostToolUse", "Stop", "SessionEnd"] {
        assert!(out.contains(event), "missing {event}: {out}");
    }
    // Medulla's own are marked as such and can be switched off from here.
    assert!(out.contains("Medulla's own"), "{out}");
    assert!(out.contains("Medulla's own hooks: on"), "{out}");
}

#[test]
fn a_harness_that_cannot_run_the_hooks_says_so_once_rather_than_per_hook() {
    let mut app = app_on_hooks();
    let out = render(&mut app, 160, 40);

    assert!(
        out.contains("opencode · 7 hooks not installed"),
        "OpenCode's coverage gap should be summarized once: {out}"
    );
    assert!(
        out.contains("codex · Codex skips hooks"),
        "the trust requirement is the one warning that is actionable: {out}"
    );
}

#[test]
fn adding_a_hook_puts_it_below_medullas_own() {
    let mut app = app_on_hooks();
    let builtins = app.loaded.config.hooks.hooks.len();

    assert!(app.on_event(key(KeyCode::Char('a'))).is_none());
    let out = render(&mut app, 160, 40);
    assert!(out.contains("Add hook"), "{out}");

    // The editor opens prefilled with everything but the command, so typing the
    // command alone is the whole interaction for the common case.
    for ch in "notify-send done".chars() {
        assert!(app.on_event(key(KeyCode::Char(ch))).is_none());
    }
    assert!(app.on_event(key(KeyCode::Enter)).is_none());

    let hooks = &app.loaded.config.hooks.hooks;
    assert_eq!(hooks.len(), builtins + 1);
    let added = hooks.last().expect("the added hook");
    assert!(!added.builtin);
    assert_eq!(added.event, HookEvent::PostToolUse);
    assert_eq!(added.matcher, "Edit|Write");
    assert_eq!(added.command(), "notify-send done");

    let out = render(&mut app, 160, 40);
    assert!(out.contains("notify-send done"), "{out}");
}

#[test]
fn reports_from_a_launched_harness_appear_under_the_list() {
    let mut app = app_on_hooks();
    let log = HookEventLog::new();
    let mut report = HookReport::new("pty-1", HookEvent::PostToolUse);
    report.summary = "used Edit".into();
    log.record(report);
    app.set_hook_log(log);

    let out = render(&mut app, 160, 40);
    assert!(out.contains("Reports"), "the feed should appear: {out}");
    assert!(out.contains("used Edit"), "{out}");
}

#[test]
fn an_app_with_no_reports_gives_the_whole_page_to_the_list() {
    let mut app = app_on_hooks();
    let out = render(&mut app, 160, 40);
    assert!(
        !out.contains("Reports"),
        "an empty feed should not take space from the list: {out}"
    );
}
