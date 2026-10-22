use super::{AgentRegistration, AgentRegistry};

fn origin(task_id: Option<&str>, session_id: Option<&str>, provider: &str) -> AgentRegistration {
    AgentRegistration {
        pid: 17,
        started_at_secs: 100,
        task_id: task_id.map(str::to_owned),
        session_id: session_id.map(str::to_owned),
        provider: provider.to_owned(),
    }
}

#[test]
fn registration_is_visible_with_its_task_session_and_provider_origin() {
    let registry = AgentRegistry::default();
    let guard = registry.register_at(origin(Some("task-1"), Some("session-2"), "claude"));

    assert_eq!(
        registry.snapshot(),
        vec![origin(Some("task-1"), Some("session-2"), "claude")]
    );
    drop(guard);
    assert!(registry.snapshot().is_empty());
}

#[test]
fn stale_pid_registration_is_not_a_known_root_after_pid_reuse() {
    let registry = AgentRegistry::default();
    let _guard = registry.register_at(origin(Some("task-1"), None, "codex"));

    assert!(registry.current_for(17, 101).is_none());
    assert_eq!(
        registry.current_for(17, 100),
        Some(origin(Some("task-1"), None, "codex"))
    );
}

#[test]
fn dropping_an_old_guard_does_not_remove_a_newer_pid_identity() {
    let registry = AgentRegistry::default();
    let old = registry.register_at(origin(Some("old"), None, "claude"));
    let mut replacement = origin(Some("new"), None, "codex");
    replacement.started_at_secs = 101;
    let _new = registry.register_at(replacement.clone());

    drop(old);

    assert_eq!(registry.snapshot(), vec![replacement]);
}

#[test]
fn dropping_an_old_guard_does_not_remove_a_reregistered_process_identity() {
    let registry = AgentRegistry::default();
    let old = registry.register_at(origin(Some("old"), None, "claude"));
    let mut replacement = origin(Some("new"), Some("session"), "codex");
    replacement.started_at_secs = 100;
    let _new = registry.register_at(replacement.clone());

    drop(old);

    assert_eq!(registry.snapshot(), vec![replacement]);
}

#[test]
fn a_live_registration_handle_attaches_a_late_session_id() {
    let registry = AgentRegistry::default();
    let guard = registry.register_at(origin(None, None, "claude"));
    let handle = guard.handle();

    assert!(handle.set_session_id("session-arrived-later"));
    assert_eq!(
        registry.snapshot(),
        vec![origin(None, Some("session-arrived-later"), "claude")]
    );
}

#[test]
fn a_stale_registration_handle_cannot_update_a_replacement() {
    let registry = AgentRegistry::default();
    let old = registry.register_at(origin(None, None, "claude"));
    let old_handle = old.handle();
    let replacement = registry.register_at(origin(Some("task-2"), None, "codex"));

    assert!(!old_handle.set_session_id("stale-session"));
    assert_eq!(
        registry.snapshot(),
        vec![origin(Some("task-2"), None, "codex")]
    );
    drop(replacement);
}

#[test]
fn spawn_idle_child_is_registered_with_origin_and_removed_when_guard_drops() {
    let mut child = spawn_idle_child();
    let registry = AgentRegistry::default();
    let guard = registry
        .register(
            child.id(),
            Some("task-child".into()),
            Some("session-child".into()),
            "claude",
        )
        .expect("the spawned child has a process start time");

    let registration = registry.snapshot().pop().expect("child is registered");
    assert_eq!(registration.pid, child.id());
    assert_eq!(registration.task_id.as_deref(), Some("task-child"));
    assert_eq!(registration.session_id.as_deref(), Some("session-child"));
    assert_eq!(registration.provider, "claude");

    drop(guard);
    assert!(registry.snapshot().is_empty());
    let _ = child.kill();
    let _ = child.wait();
}

fn spawn_idle_child() -> std::process::Child {
    #[cfg(windows)]
    {
        std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
            .spawn()
            .expect("start idle child")
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("start idle child")
    }
}
