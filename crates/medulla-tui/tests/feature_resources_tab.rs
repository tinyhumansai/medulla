use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla_monitor::sampler::{ProcessSnapshot, ResourceSnapshot};
use medulla_tui::ui::{
    app::{App, TABS},
    resources::sampler,
};
use ratatui::{backend::TestBackend, Terminal};

fn process(pid: u32, command: &str, cpu: f64) -> ProcessSnapshot {
    ProcessSnapshot {
        pid,
        ppid: None,
        exe: Some(command.into()),
        cmd: vec![command.into()],
        cwd: Some(format!("/workspace/{command}")),
        start_time: 1,
        owner_uid: None,
        cpu_percent: cpu,
        memory_bytes: 2 * 1024 * 1024,
        disk_read_bytes: 0,
        disk_written_bytes: 0,
        threads: 4,
    }
}

#[test]
fn resources_agents_pane_renders_at_common_terminal_sizes() {
    for (width, height) in [(170, 42), (80, 24)] {
        let mut app = App::new(
            Arc::new(MockRuntime::empty()),
            LoadedConfig::defaults("test.json".into()),
        );
        app.tab_index = TABS.iter().position(|tab| *tab == "Resources").unwrap();
        app.set_resource_snapshot(Arc::new(ResourceSnapshot::default().with_processes(vec![
            process(101, "claude", 10.0),
            process(102, "codex-acp", 20.0),
            process(103, "openhuman", 30.0),
            process(104, "aider", 40.0),
        ])));
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        for label in ["Claude", "Codex-ACP", "OpenHuman", "External"] {
            assert!(
                screen.contains(label),
                "{label} missing at {width}x{height}: {screen}"
            );
        }
    }
}

#[test]
fn process_snapshot_builder_keeps_the_test_injection_path() {
    let snapshot =
        sampler::ResourceSnapshot::default().with_processes(vec![process(1, "claude", 5.0)]);
    assert_eq!(snapshot.processes.len(), 1);
    assert_eq!(snapshot.processes[0].pid, 1);
}
