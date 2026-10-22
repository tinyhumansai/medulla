use std::cmp::Ordering;

use medulla::protocol::HarnessProvider;
use medulla_monitor::sampler::{ProcessSnapshot, ResourceSnapshot};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Paragraph, Row, Table, Wrap},
    Frame,
};

use crate::ui::app::resources::types::{AgentIdentity, AgentSignal, AgentSort, ResourcesState};

#[derive(Debug, Clone, PartialEq)]
pub(in crate::ui::app) struct AgentRow {
    pub host: Option<String>,
    pub pid: u32,
    pub started: u64,
    pub kind: String,
    pub origin: String,
    pub workspace: String,
    pub threads: usize,
    pub memory: u64,
    pub cpu: f64,
    pub runtime: u64,
    pub ppid: Option<u32>,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub external: bool,
    pub remote: bool,
    pub depth: usize,
}

impl AgentRow {
    pub(in crate::ui::app) fn identity(&self) -> AgentIdentity {
        AgentIdentity {
            host: self.host.clone(),
            pid: self.pid,
            started: self.started,
        }
    }
}

pub(in crate::ui::app) fn rows(snapshot: &ResourceSnapshot) -> Vec<AgentRow> {
    let registrations = medulla_monitor::agents::agent_registry().snapshot();
    snapshot
        .processes
        .iter()
        .filter_map(|process| {
            let registered = registrations.iter().find(|entry| {
                entry.pid == process.pid && entry.started_at_secs == process.start_time
            });
            let detected = detect_kind(process);
            let kind = registered
                .map(|entry| pretty_kind(&entry.provider))
                .or(detected)?;
            let (origin, session_id, task_id, external) = match registered {
                Some(entry) if entry.session_id.is_some() => (
                    entry.session_id.clone().unwrap_or_default(),
                    entry.session_id.clone(),
                    entry.task_id.clone(),
                    false,
                ),
                Some(entry) if entry.task_id.is_some() => (
                    format!("task {}", entry.task_id.as_deref().unwrap_or_default()),
                    None,
                    entry.task_id.clone(),
                    false,
                ),
                Some(_) => ("host".into(), None, None, false),
                None => ("external".into(), None, None, true),
            };
            let remote = session_id.as_deref().is_some_and(|id| {
                matches!(
                    crate::ui::app::remote_sessions::SessionRef::parse(id),
                    crate::ui::app::remote_sessions::SessionRef::Remote { .. }
                )
            });
            Some(AgentRow {
                host: None,
                pid: process.pid,
                started: process.start_time,
                kind,
                origin,
                workspace: process.cwd.clone().unwrap_or_else(|| "—".into()),
                threads: process.threads,
                memory: process.memory_bytes,
                cpu: process.cpu_percent,
                runtime: (medulla::clock::now_millis()
                    - (process.start_time.saturating_mul(1000) as i64))
                    .max(0) as u64
                    / 1000,
                ppid: process.ppid,
                session_id,
                task_id,
                external,
                remote,
                depth: 0,
            })
        })
        .collect()
}

fn detect_kind(process: &ProcessSnapshot) -> Option<String> {
    let full = format!(
        "{} {}",
        process.exe.as_deref().unwrap_or_default(),
        process.cmd.join(" ")
    )
    .to_lowercase();
    if full.contains("codex-acp") {
        Some("Codex-ACP".into())
    } else if full.contains("claude") {
        Some("Claude".into())
    } else if full.contains("openhuman") || full.contains("medulla") {
        Some("OpenHuman".into())
    } else if full.contains("aider")
        || full.contains("gemini")
        || full.contains("goose")
        || full.contains("cursor")
        || full.contains("codex")
        || full.contains("opencode")
    {
        Some("External".into())
    } else {
        None
    }
}

fn pretty_kind(provider: &str) -> String {
    match HarnessProvider::from_wire(provider) {
        Some(HarnessProvider::Claude) => "Claude".into(),
        Some(HarnessProvider::Codex) => "Codex".into(),
        Some(HarnessProvider::Opencode) => "OpenCode".into(),
        Some(HarnessProvider::Openhuman) => "OpenHuman".into(),
        Some(_) => provider.to_owned(),
        None if provider.to_lowercase().contains("openhuman") => "OpenHuman".into(),
        None if provider.to_lowercase().contains("codex-acp") => "Codex-ACP".into(),
        None if provider.to_lowercase().contains("claude") => "Claude".into(),
        None => provider.to_owned(),
    }
}

pub(in crate::ui::app) fn visible_rows(
    snapshot: &ResourceSnapshot,
    state: &ResourcesState,
) -> Vec<AgentRow> {
    let mut rows = rows(snapshot);
    if let Some(filter) = state.agent_filter.as_deref() {
        let (regex, pattern) = filter
            .strip_prefix('!')
            .map_or((false, filter), |pattern| (true, pattern));
        if regex {
            if let Ok(pattern) = regex::Regex::new(&format!("(?i){pattern}")) {
                rows.retain(|row| pattern.is_match(&row.kind));
            } else {
                rows.clear();
            }
        } else {
            let pattern = pattern.to_lowercase();
            rows.retain(|row| {
                format!("{} {} {} {}", row.pid, row.kind, row.origin, row.workspace)
                    .to_lowercase()
                    .contains(&pattern)
            });
        }
    }
    rows.sort_by(|a, b| compare(a, b, state.agent_sort));
    if state.agent_reverse {
        rows.reverse();
    }
    if state.agent_tree {
        rows = tree_rows(snapshot, rows);
    }
    rows
}

pub(in crate::ui::app) fn visible_rows_with_remote(
    snapshot: &ResourceSnapshot,
    remote_hosts: &[(String, Vec<medulla::protocol::RemoteSessionRow>)],
    state: &ResourcesState,
) -> Vec<AgentRow> {
    let mut rows = visible_rows(snapshot, state);
    let mut hosts: Vec<_> = remote_hosts.iter().collect();
    hosts.sort_by(|(a, _), (b, _)| a.cmp(b));
    for (host, sessions) in hosts {
        let mut host_rows = Vec::new();
        for session in sessions {
            let Some(usage) = &session.usage else {
                continue;
            };
            let row = AgentRow {
                host: Some(host.clone()),
                pid: usage.pid,
                started: usage.started_at,
                kind: usage.kind.clone(),
                origin: format!("Remote{{{host}}}"),
                workspace: session.cwd.clone(),
                threads: usage.threads,
                memory: usage.rss,
                cpu: usage.cpu,
                runtime: (medulla::clock::now_millis()
                    - usage.started_at.saturating_mul(1000) as i64)
                    .max(0) as u64
                    / 1000,
                ppid: None,
                session_id: Some(crate::ui::app::remote_sessions::qualify(host, &session.id)),
                task_id: None,
                external: false,
                remote: true,
                depth: 0,
            };
            if matches_filter(&row, state.agent_filter.as_deref()) {
                host_rows.push(row);
            }
        }
        host_rows.sort_by(|a, b| compare(a, b, state.agent_sort));
        if state.agent_reverse {
            host_rows.reverse();
        }
        rows.extend(host_rows);
    }
    rows
}

fn matches_filter(row: &AgentRow, filter: Option<&str>) -> bool {
    let Some(filter) = filter else { return true };
    let (regex, pattern) = filter
        .strip_prefix('!')
        .map_or((false, filter), |pattern| (true, pattern));
    if regex {
        regex::Regex::new(&format!("(?i){pattern}"))
            .is_ok_and(|pattern| pattern.is_match(&row.kind))
    } else {
        format!("{} {} {} {}", row.pid, row.kind, row.origin, row.workspace)
            .to_lowercase()
            .contains(&pattern.to_lowercase())
    }
}

fn tree_rows(snapshot: &ResourceSnapshot, agents: Vec<AgentRow>) -> Vec<AgentRow> {
    let all_agents: std::collections::HashMap<u32, AgentRow> =
        agents.iter().cloned().map(|row| (row.pid, row)).collect();
    let mut children: std::collections::HashMap<u32, Vec<&ProcessSnapshot>> =
        std::collections::HashMap::new();
    for process in &snapshot.processes {
        if let Some(parent) = process.ppid {
            children.entry(parent).or_default().push(process);
        }
    }
    for descendants in children.values_mut() {
        descendants.sort_by_key(|process| process.pid);
    }
    let roots: Vec<AgentRow> = agents
        .iter()
        .filter(|row| {
            row.ppid
                .is_none_or(|parent| !all_agents.contains_key(&parent))
        })
        .cloned()
        .collect();
    let mut result = Vec::new();
    let mut visited = std::collections::HashSet::new();
    for root in roots {
        let pid = root.pid;
        visited.insert(pid);
        result.push(root);
        append_children(pid, 1, &children, &all_agents, &mut visited, &mut result);
    }
    // Cycles or incomplete parent chains should not make a known agent vanish.
    for row in agents {
        if visited.insert(row.pid) {
            result.push(row);
        }
    }
    result
}

fn append_children(
    parent: u32,
    depth: usize,
    children: &std::collections::HashMap<u32, Vec<&ProcessSnapshot>>,
    agents: &std::collections::HashMap<u32, AgentRow>,
    visited: &mut std::collections::HashSet<u32>,
    result: &mut Vec<AgentRow>,
) {
    if let Some(descendants) = children.get(&parent) {
        for process in descendants {
            if !visited.insert(process.pid) {
                continue;
            }
            let mut row = agents
                .get(&process.pid)
                .cloned()
                .unwrap_or_else(|| AgentRow {
                    host: None,
                    pid: process.pid,
                    started: process.start_time,
                    kind: "Process".into(),
                    origin: "child process".into(),
                    workspace: process.cwd.clone().unwrap_or_else(|| "—".into()),
                    threads: process.threads,
                    memory: process.memory_bytes,
                    cpu: process.cpu_percent,
                    runtime: (medulla::clock::now_millis()
                        - process.start_time.saturating_mul(1000) as i64)
                        .max(0) as u64
                        / 1000,
                    ppid: process.ppid,
                    session_id: None,
                    task_id: None,
                    external: true,
                    remote: false,
                    depth,
                });
            row.depth = depth;
            let pid = row.pid;
            result.push(row);
            append_children(pid, depth + 1, children, agents, visited, result);
        }
    }
}

fn compare(a: &AgentRow, b: &AgentRow, key: AgentSort) -> Ordering {
    match key {
        AgentSort::Pid => a.pid.cmp(&b.pid),
        AgentSort::Kind => a.kind.cmp(&b.kind),
        AgentSort::Origin => a.origin.cmp(&b.origin),
        AgentSort::Workspace => a.workspace.cmp(&b.workspace),
        AgentSort::Threads => a.threads.cmp(&b.threads),
        AgentSort::Memory => a.memory.cmp(&b.memory),
        AgentSort::Cpu => a.cpu.total_cmp(&b.cpu),
        AgentSort::Runtime => a.runtime.cmp(&b.runtime),
    }
}

pub(in crate::ui::app) fn draw_agents(
    frame: &mut Frame,
    area: Rect,
    snapshot: &ResourceSnapshot,
    remote_hosts: &[(String, Vec<medulla::protocol::RemoteSessionRow>)],
    state: &mut ResourcesState,
) {
    let rows = visible_rows_with_remote(snapshot, remote_hosts, state);
    if let Some(following) = state.agent_follow.clone() {
        if !snapshot.processes.iter().any(|process| {
            (process.pid, process.start_time) == (following.pid, following.started)
                && following.host.is_none()
        }) {
            state.agent_follow = None;
            if state.agent_selected.as_ref() == Some(&following) {
                state.agent_selected = None;
            }
        } else if rows.iter().any(|row| row.identity() == following) {
            state.agent_selected = Some(following.clone());
        }
    }
    let now = std::time::Instant::now();
    let sample_due = state
        .agent_last_sample
        .is_none_or(|last| now.duration_since(last).as_millis() >= u128::from(state.interval_ms));
    let current: std::collections::HashSet<_> = rows.iter().map(AgentRow::identity).collect();
    state
        .agent_cpu_history
        .retain(|identity, _| current.contains(identity));
    if sample_due {
        for row in &rows {
            let history = state.agent_cpu_history.entry(row.identity()).or_default();
            history.push(row.cpu);
            if history.len() > 24 {
                history.remove(0);
            }
        }
        state.agent_last_sample = Some(now);
    }
    if state.agent_detail {
        super::agents_detail::draw(
            frame,
            area,
            rows.iter()
                .find(|r| Some(r.identity()) == state.agent_selected),
        );
        draw_signal_confirmation(frame, area, snapshot, state);
        return;
    }
    let selected = state.agent_selected.clone();
    let compact = area.width < 105;
    let header = if compact {
        Row::new(["PID", "Agent / Origin", "CPU", "Mem", "Run"])
    } else {
        Row::new([
            "PID",
            "Agent",
            "Origin",
            "Workspace",
            "Th",
            "Mem",
            "CPU%",
            "CPU",
            "Runtime",
        ])
    };
    let agent_table_rows: Vec<Row> = rows
        .iter()
        .map(|row| {
            let graph = state
                .agent_cpu_history
                .get(&row.identity())
                .map(|history| mini_graph(history))
                .unwrap_or_default();
            let workspace = shorten(&row.workspace, if compact { 18 } else { 25 });
            let origin = if row.external {
                "external".to_owned()
            } else {
                row.origin.clone()
            };
            let cells = if compact {
                vec![
                    row.pid.to_string(),
                    format!(
                        "{}{}{} · {}",
                        "  ".repeat(row.depth),
                        if row.depth > 0 { "└─ " } else { "" },
                        row.kind,
                        origin
                    ),
                    format!("{:.1}", row.cpu),
                    bytes(row.memory),
                    duration(row.runtime),
                ]
            } else {
                vec![
                    row.pid.to_string(),
                    format!(
                        "{}{}{}",
                        "  ".repeat(row.depth),
                        if row.depth > 0 { "└─ " } else { "" },
                        row.kind
                    ),
                    origin,
                    workspace,
                    row.threads.to_string(),
                    bytes(row.memory),
                    format!("{:.1}", row.cpu),
                    graph,
                    duration(row.runtime),
                ]
            };
            let mut item = Row::new(cells);
            if Some(row.identity()) == selected {
                item = item.style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                );
            }
            item
        })
        .collect();
    let mut table_rows = Vec::new();
    let mut display_indices = Vec::with_capacity(rows.len());
    let mut last_remote_host: Option<&str> = None;
    for (row, item) in rows.iter().zip(agent_table_rows) {
        if let Some(host) = row.host.as_deref() {
            if last_remote_host != Some(host) {
                table_rows.push(
                    Row::new(vec!["Remote".to_owned(), host.to_owned()])
                        .style(Style::default().add_modifier(Modifier::BOLD)),
                );
            }
            last_remote_host = Some(host);
        } else {
            last_remote_host = None;
        }
        display_indices.push(table_rows.len());
        table_rows.push(item);
    }
    let follow = state
        .agent_follow
        .as_ref()
        .map(|identity| format!(" · FOLLOW PID {}", identity.pid))
        .unwrap_or_default();
    let title = format!(" Agents{follow} · {} · {} · CPU {:.1}% · Mem {} · ←/→ sort r reverse e tree f filter Enter detail F follow t/k/s signal ",
            rows.len(), kind_counts(&rows), rows.iter().map(|r| r.cpu).sum::<f64>(), bytes(rows.iter().map(|r| r.memory).sum::<u64>()));
    let widths = if compact {
        vec![
            ratatui::layout::Constraint::Length(7),
            ratatui::layout::Constraint::Min(17),
            ratatui::layout::Constraint::Length(6),
            ratatui::layout::Constraint::Length(8),
            ratatui::layout::Constraint::Length(6),
        ]
    } else {
        vec![
            ratatui::layout::Constraint::Length(7),
            ratatui::layout::Constraint::Length(13),
            ratatui::layout::Constraint::Length(13),
            ratatui::layout::Constraint::Min(12),
            ratatui::layout::Constraint::Length(4),
            ratatui::layout::Constraint::Length(8),
            ratatui::layout::Constraint::Length(6),
            ratatui::layout::Constraint::Length(8),
            ratatui::layout::Constraint::Length(7),
        ]
    };
    let table = Table::new(table_rows, widths)
        .header(header.style(Style::default().add_modifier(Modifier::BOLD)))
        .block(Block::default().borders(Borders::ALL).title(title))
        .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));
    let selected_index = selected.as_ref().and_then(|identity| {
        rows.iter()
            .position(|row| row.identity() == *identity)
            .map(|index| display_indices[index])
    });
    let mut table_state = ratatui::widgets::TableState::default();
    table_state.select(selected_index);
    frame.render_stateful_widget(table, area, &mut table_state);
    draw_signal_confirmation(frame, area, snapshot, state);
    if state.agent_filter_editing {
        let filter = state.agent_filter.as_deref().unwrap_or_default();
        frame.render_widget(
            Paragraph::new(format!("Filter: {filter}_   Enter apply · Esc cancel")),
            Rect::new(
                area.x + 1,
                area.bottom().saturating_sub(2),
                area.width.saturating_sub(2),
                1,
            ),
        );
    }
}

fn draw_signal_confirmation(
    frame: &mut Frame,
    area: Rect,
    snapshot: &ResourceSnapshot,
    state: &ResourcesState,
) {
    let Some(armed) = state.agent_signal_armed else {
        return;
    };
    let row = rows(snapshot)
        .into_iter()
        .find(|row| row.pid == armed.pid && row.started == armed.started);
    let process = snapshot
        .processes
        .iter()
        .find(|process| process.pid == armed.pid && process.start_time == armed.started);
    let kind = row.as_ref().map_or("Unknown", |row| row.kind.as_str());
    let origin = row.as_ref().map_or("external", |row| row.origin.as_str());
    let command = process.map_or_else(|| "unavailable".into(), display_command);
    let signal = match armed.signal {
        AgentSignal::Terminate => "terminate",
        AgentSignal::Kill => "kill",
        AgentSignal::Stop => "stop",
    };
    let external = row.as_ref().is_some_and(|row| row.external);
    let instruction = match (external, armed.stage) {
        (true, crate::ui::app::resources::types::AgentSignalConfirmStage::First) => {
            "Press y for the second confirmation; Esc cancels"
        }
        (true, crate::ui::app::resources::types::AgentSignalConfirmStage::ExternalSecond) => {
            "External target: press uppercase Y to send; Esc cancels"
        }
        (false, _) => "Press y to send; Esc cancels",
    };
    let prompt = format!(
        "Kind: {kind}\nPID: {}\nCommand: {command}\nOrigin: {origin}\nSignal: {signal}\nTarget host: local\n\n{instruction}",
        armed.pid
    );
    let popup = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2).min(10),
    );
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(prompt).wrap(Wrap { trim: true }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(if external {
                    " EXTERNAL PROCESS · Confirm signal "
                } else {
                    " Confirm process signal "
                })
                .style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
        ),
        popup,
    );
}

fn display_command(process: &ProcessSnapshot) -> String {
    let mut rendered = Vec::new();
    let mut redact_next = false;
    for argument in process
        .cmd
        .iter()
        .filter(|arg| !is_environment_assignment(arg))
    {
        if redact_next {
            redact_next = false;
            continue;
        }
        if let Some((key, _)) = argument.split_once('=') {
            if argument.starts_with('-') && is_secret_name(key) {
                rendered.push(format!("{key}=[REDACTED]"));
                continue;
            }
        }
        if argument.starts_with('-') && is_secret_name(argument) {
            rendered.push(format!("{argument}=[REDACTED]"));
            redact_next = !argument.contains('=');
        } else {
            rendered.push(argument.clone());
        }
    }
    if rendered.is_empty() {
        rendered.push(
            process
                .exe
                .as_deref()
                .map(std::path::Path::new)
                .and_then(std::path::Path::file_name)
                .and_then(|name| name.to_str())
                .unwrap_or("unavailable")
                .to_owned(),
        );
    }
    let command = rendered.join(" ");
    const LIMIT: usize = 72;
    if command.len() <= LIMIT {
        return command;
    }
    let mut end = LIMIT.saturating_sub(1);
    while !command.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &command[..end])
}

fn is_environment_assignment(argument: &str) -> bool {
    let Some((name, _)) = argument.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && !argument.starts_with('-')
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn is_secret_name(name: &str) -> bool {
    let name = name.trim_start_matches('-').to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "passwd",
        "api-key",
        "apikey",
        "auth",
        "credential",
    ]
    .iter()
    .any(|secret| name.contains(secret))
}

fn kind_counts(rows: &[AgentRow]) -> String {
    let mut counts = std::collections::BTreeMap::new();
    for row in rows {
        *counts.entry(row.kind.as_str()).or_insert(0usize) += 1;
    }
    counts
        .into_iter()
        .map(|(kind, count)| format!("{kind} {count}"))
        .collect::<Vec<_>>()
        .join(" · ")
}
fn bytes(value: u64) -> String {
    if value >= 1 << 30 {
        format!("{:.1}G", value as f64 / (1u64 << 30) as f64)
    } else if value >= 1 << 20 {
        format!("{:.0}M", value as f64 / (1u64 << 20) as f64)
    } else {
        format!("{}K", value / 1024)
    }
}
fn duration(seconds: u64) -> String {
    if seconds >= 3600 {
        format!("{}h", seconds / 3600)
    } else if seconds >= 60 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}s", seconds)
    }
}
fn shorten(value: &str, max: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max {
        value.to_owned()
    } else {
        format!(
            "…{}",
            chars[chars.len().saturating_sub(max.saturating_sub(1))..]
                .iter()
                .collect::<String>()
        )
    }
}
fn mini_graph(values: &[f64]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    values
        .iter()
        .rev()
        .take(8)
        .rev()
        .map(|v| BARS[(v.clamp(0.0, 100.0) / 100.0 * 7.0).round() as usize])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::resources::types::ResourcesState;

    fn process(pid: u32, command: &str, cpu: f64, mem: u64) -> ProcessSnapshot {
        ProcessSnapshot {
            pid,
            ppid: None,
            exe: Some(command.into()),
            cmd: vec![command.into()],
            cwd: Some(format!("/workspace/{command}")),
            start_time: 100,
            owner_uid: None,
            cpu_percent: cpu,
            memory_bytes: mem,
            disk_read_bytes: 0,
            disk_written_bytes: 0,
            threads: pid as usize,
        }
    }
    fn snapshot() -> ResourceSnapshot {
        ResourceSnapshot::default().with_processes(vec![
            process(10, "claude", 10.0, 4000),
            process(20, "codex-acp", 20.0, 3000),
            process(30, "openhuman", 30.0, 2000),
            process(40, "aider", 40.0, 1000),
        ])
    }

    #[test]
    fn remote_agents_render_beneath_each_host_heading() {
        let row = |id: &str, kind: &str, pid| medulla::protocol::RemoteSessionRow {
            id: id.into(),
            label: kind.into(),
            provider: kind.to_lowercase(),
            preset: None,
            state: medulla::protocol::RemoteSessionState::Running,
            cwd: "/work".into(),
            name: None,
            thread_name: None,
            started_at: 1_700_000_000_000,
            last_output_at: 1_700_000_000_000,
            busy: true,
            working: true,
            attention: None,
            bracketed_paste: false,
            last_error: None,
            usage: Some(medulla::protocol::AgentUsage {
                kind: kind.into(),
                pid,
                cpu: 1.0,
                rss: 1024,
                threads: 2,
                started_at: 1_700_000_000,
                cpu_history: vec![0.5, 1.0],
            }),
        };
        let remote_hosts = vec![
            ("alpha".into(), vec![row("w_a", "Claude", 101)]),
            ("beta".into(), vec![row("w_b", "Codex", 202)]),
        ];
        let backend = ratatui::backend::TestBackend::new(120, 18);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut state = ResourcesState::default();
        terminal
            .draw(|frame| {
                draw_agents(
                    frame,
                    frame.area(),
                    &ResourceSnapshot::default(),
                    &remote_hosts,
                    &mut state,
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let rendered = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "Remote",
            "alpha",
            "Remote{alpha}",
            "Claude",
            "beta",
            "Remote{beta}",
            "Codex",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected:?}:\n{rendered}"
            );
        }
        assert_eq!(
            rendered.matches("alpha").count(),
            2,
            "alpha heading and row:\n{rendered}"
        );
        assert_eq!(
            rendered.matches("beta").count(),
            2,
            "beta heading and row:\n{rendered}"
        );
        assert!(rendered.find("alpha").unwrap() < rendered.find("beta").unwrap());
    }

    #[test]
    fn remote_host_groups_preserve_local_agent_tree_order() {
        let mut parent = process(100, "claude", 1.0, 100);
        parent.ppid = None;
        let mut child = process(50, "codex-acp", 2.0, 200);
        child.ppid = Some(100);
        let snapshot = ResourceSnapshot::default().with_processes(vec![parent, child]);
        let remote = |id: &str, kind: &str, pid| medulla::protocol::RemoteSessionRow {
            id: id.into(),
            label: kind.into(),
            provider: kind.to_lowercase(),
            preset: None,
            state: medulla::protocol::RemoteSessionState::Running,
            cwd: "/work".into(),
            name: None,
            thread_name: None,
            started_at: 1_700_000_000_000,
            last_output_at: 1_700_000_000_000,
            busy: false,
            working: false,
            attention: None,
            bracketed_paste: false,
            last_error: None,
            usage: Some(medulla::protocol::AgentUsage {
                kind: kind.into(),
                pid,
                cpu: pid as f64,
                rss: 1_024,
                threads: 1,
                started_at: 1_700_000_000,
                cpu_history: vec![],
            }),
        };
        // Deliberately pass hosts and rows out of display order. Remote hosts
        // group by host, but local tree order is kept exactly as discovered.
        let hosts = vec![
            ("beta".into(), vec![remote("b", "Beta", 1)]),
            (
                "alpha".into(),
                vec![remote("a2", "Alpha", 999), remote("a1", "Alpha", 2)],
            ),
        ];
        let state = ResourcesState {
            agent_tree: true,
            ..ResourcesState::default()
        };
        let rows = visible_rows_with_remote(&snapshot, &hosts, &state);
        assert_eq!(
            rows.iter()
                .map(|row| (row.host.as_deref(), row.pid))
                .collect::<Vec<_>>(),
            [
                (None, 100),
                (None, 50),
                (Some("alpha"), 2),
                (Some("alpha"), 999),
                (Some("beta"), 1)
            ],
        );
    }

    #[test]
    fn remote_agents_share_regex_filter_semantics_and_have_host_scoped_identity() {
        let row = |id: &str, kind: &str, pid| medulla::protocol::RemoteSessionRow {
            id: id.into(),
            label: kind.into(),
            provider: kind.to_lowercase(),
            preset: None,
            state: medulla::protocol::RemoteSessionState::Running,
            cwd: "/work".into(),
            name: None,
            thread_name: None,
            started_at: 1_700_000_000_000,
            last_output_at: 1_700_000_000_000,
            busy: false,
            working: false,
            attention: None,
            bracketed_paste: false,
            last_error: None,
            usage: Some(medulla::protocol::AgentUsage {
                kind: kind.into(),
                pid,
                cpu: 1.0,
                rss: 1_024,
                threads: 2,
                started_at: 1_700_000_000,
                cpu_history: vec![],
            }),
        };
        let hosts = vec![
            ("alpha".into(), vec![row("w_same", "Claude", 55)]),
            ("beta".into(), vec![row("w_same", "Codex", 55)]),
        ];
        let state = ResourcesState {
            agent_filter: Some("!^cod".into()),
            ..ResourcesState::default()
        };
        let rows = visible_rows_with_remote(&ResourceSnapshot::default(), &hosts, &state);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "Codex");
        assert_eq!(rows[0].host.as_deref(), Some("beta"));
        assert_eq!(rows[0].session_id.as_deref(), Some("beta/w_same"));
    }

    #[test]
    fn classifies_known_and_external_agent_processes() {
        let rows = rows(&snapshot());
        assert_eq!(
            rows.iter().map(|row| row.kind.as_str()).collect::<Vec<_>>(),
            ["Claude", "Codex-ACP", "OpenHuman", "External"]
        );
        assert_eq!(
            rows.iter()
                .map(|row| row.origin.as_str())
                .collect::<Vec<_>>(),
            ["external", "external", "external", "external"]
        );
    }

    #[test]
    fn unregistered_bare_codex_process_is_visible_as_external() {
        let rows = rows(
            &ResourceSnapshot::default().with_processes(vec![process(987_105, "codex", 0.0, 1)]),
        );
        assert_eq!(rows.len(), 1, "the harness-named process must be visible");
        assert_eq!(rows[0].kind, "External");
        assert_eq!(rows[0].origin, "external");
    }

    #[test]
    fn registry_classifies_session_task_and_host_origins() {
        let registry = medulla_monitor::agents::agent_registry();
        let registrations = [
            medulla_monitor::agents::AgentRegistration {
                pid: 987_101,
                started_at_secs: 100,
                task_id: None,
                session_id: Some("session-label".into()),
                provider: "claude".into(),
            },
            medulla_monitor::agents::AgentRegistration {
                pid: 987_102,
                started_at_secs: 100,
                task_id: Some("task-7".into()),
                session_id: None,
                provider: "codex-acp".into(),
            },
            medulla_monitor::agents::AgentRegistration {
                pid: 987_103,
                started_at_secs: 100,
                task_id: None,
                session_id: None,
                provider: "openhuman".into(),
            },
        ];
        let _guards: Vec<_> = registrations
            .iter()
            .cloned()
            .map(|registration| registry.register_at(registration))
            .collect();
        let snapshot = ResourceSnapshot::default().with_processes(vec![
            process(987_101, "claude", 1.0, 1),
            process(987_102, "codex-acp", 2.0, 2),
            process(987_103, "openhuman", 3.0, 3),
            process(987_104, "aider", 4.0, 4),
        ]);
        let rows = rows(&snapshot);
        assert_eq!(
            rows.iter()
                .map(|row| row.origin.as_str())
                .collect::<Vec<_>>(),
            ["session-label", "task task-7", "host", "external"]
        );
        assert_eq!(
            rows.iter().map(|row| row.kind.as_str()).collect::<Vec<_>>(),
            ["Claude", "Codex-ACP", "OpenHuman", "External"]
        );
    }

    #[test]
    fn tree_mode_nests_child_processes_under_agent_rows() {
        let snapshot = ResourceSnapshot::default().with_processes(vec![
            process(10, "claude", 10.0, 4000),
            ProcessSnapshot {
                pid: 11,
                ppid: Some(10),
                exe: Some("node".into()),
                cmd: vec!["node".into()],
                cwd: Some("/work".into()),
                start_time: 101,
                owner_uid: None,
                cpu_percent: 2.0,
                memory_bytes: 512,
                disk_read_bytes: 0,
                disk_written_bytes: 0,
                threads: 1,
            },
        ]);
        let state = ResourcesState {
            agent_tree: true,
            ..ResourcesState::default()
        };
        let rows = visible_rows(&snapshot, &state);
        assert_eq!(
            rows.iter()
                .map(|row| (&*row.kind, row.depth))
                .collect::<Vec<_>>(),
            [("Claude", 0), ("Process", 1)]
        );
    }

    #[test]
    fn sorts_each_column_reverses_and_filters_codex_by_regex() {
        let snap = snapshot();
        let mut state = ResourcesState::default();
        for sort in [
            AgentSort::Pid,
            AgentSort::Kind,
            AgentSort::Origin,
            AgentSort::Workspace,
            AgentSort::Threads,
            AgentSort::Memory,
            AgentSort::Cpu,
            AgentSort::Runtime,
        ] {
            state.agent_sort = sort;
            let sorted = visible_rows(&snap, &state);
            assert_eq!(sorted.len(), 4);
            assert!(sorted
                .windows(2)
                .all(|w| compare(&w[0], &w[1], sort) != Ordering::Greater));
        }
        state.agent_sort = AgentSort::Cpu;
        state.agent_reverse = true;
        assert_eq!(visible_rows(&snap, &state)[0].pid, 40);
        state.agent_reverse = false;
        state.agent_filter = Some("!^cod".into());
        assert_eq!(
            visible_rows(&snap, &state)
                .iter()
                .map(|r| r.kind.as_str())
                .collect::<Vec<_>>(),
            ["Codex-ACP"]
        );
    }
}
