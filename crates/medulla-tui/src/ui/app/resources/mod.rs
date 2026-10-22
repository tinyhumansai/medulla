//! State, layout, keys, and rendering for the host Resources tab.

mod keys;
pub(super) mod layout;
#[cfg(test)]
mod tests;
pub(super) mod types;

pub(super) use types::ResourcesState;

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use super::types::App;

impl App {
    pub(super) fn draw_resources(&mut self, f: &mut Frame, area: Rect) {
        let remote_agents: Vec<_> = self
            .remote_hosts
            .iter()
            .map(|(host, state)| (host.clone(), state.rows.clone()))
            .collect();
        let state = &mut self.resources;
        let rects = layout::calc_sizes(area, state.shown, &state.opts);
        state.rects = rects;
        if let Some(required) = rects.too_small {
            if state.shown.agents && area.width >= 44 && area.height >= 10 {
                state.rects.agents = area;
                crate::ui::app::render::resources::draw_agents(
                    f,
                    area,
                    &self.resource_snapshot,
                    &remote_agents,
                    state,
                );
                return;
            }
            f.render_widget(
                Paragraph::new(format!(
                    "Terminal too small for Resources (minimum {required}x8)"
                ))
                .alignment(ratatui::layout::Alignment::Center)
                .style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                area,
            );
            return;
        }
        state.hits = types::ResourceHits::default();
        for (shown, rect, title, color) in [
            (
                state.shown.cpu,
                rects.cpu,
                "CPU",
                self.theme.resources.cpu_box,
            ),
            (
                state.shown.mem,
                rects.mem,
                "Mem",
                self.theme.resources.mem_box,
            ),
            (
                state.shown.net,
                rects.net,
                "Net",
                self.theme.resources.net_box,
            ),
            (
                state.shown.agents,
                rects.agents,
                "Agents",
                self.theme.resources.proc_box,
            ),
        ] {
            if shown && rect.width > 0 && rect.height > 0 {
                let title = if title == "CPU" {
                    format!(
                        " CPU [1] [m] [p] [P] [-] [+] {} preset {} {}ms ",
                        clock_text(),
                        state.preset,
                        state.interval_ms
                    )
                } else {
                    let key = match title {
                        "Mem" => 2,
                        "Net" => 3,
                        _ => 4,
                    };
                    if title == "Mem" {
                        format!(" Mem [{key}] [d] [i] ")
                    } else if title == "Net" {
                        let selected = state.net_iface.as_deref().or_else(|| {
                            crate::ui::resources::net::default_interface(
                                &self.resource_snapshot.networks,
                            )
                        });
                        let address = selected
                            .and_then(|name| {
                                self.resource_snapshot
                                    .networks
                                    .iter()
                                    .find(|entry| entry.interface == name)
                            })
                            .and_then(|entry| entry.ip_addresses.first())
                            .map(String::as_str)
                            .unwrap_or("no IP");
                        format!(
                            " Net [{key}] [b] [n] [z] [a{}] [y{}] {} {} ",
                            if state.net_auto { "✓" } else { " " },
                            if state.net_sync { "✓" } else { " " },
                            selected.unwrap_or("no iface"),
                            address
                        )
                    } else {
                        format!(" {title} [{key}] ")
                    }
                };
                if title.starts_with(" CPU ") {
                    state.hits.cpu_toggle = Rect::new(rect.x + 6, rect.y, 3, 1);
                    state.hits.menu = Rect::new(rect.x + 10, rect.y, 3, 1);
                    state.hits.preset_prev = Rect::new(rect.x + 14, rect.y, 3, 1);
                    state.hits.preset_next = Rect::new(rect.x + 18, rect.y, 3, 1);
                    state.hits.interval_down = Rect::new(rect.x + 22, rect.y, 3, 1);
                    state.hits.interval_up = Rect::new(rect.x + 26, rect.y, 3, 1);
                } else {
                    let offset = if title.contains("Agents") { 9 } else { 6 };
                    let hit = Rect::new(rect.x + offset, rect.y, 3, 1);
                    match title.chars().next() {
                        Some(' ') if title.contains("Mem") => state.hits.mem_toggle = hit,
                        Some(' ') if title.contains("Net") => state.hits.net_toggle = hit,
                        _ => state.hits.agents_toggle = hit,
                    }
                    if title.contains("Net") {
                        state.hits.net_prev = Rect::new(rect.x + 10, rect.y, 3, 1);
                        state.hits.net_next = Rect::new(rect.x + 14, rect.y, 3, 1);
                        state.hits.net_zero = Rect::new(rect.x + 18, rect.y, 3, 1);
                        state.hits.net_auto = Rect::new(rect.x + 22, rect.y, 3, 1);
                        state.hits.net_sync = Rect::new(rect.x + 26, rect.y, 3, 1);
                    }
                }
                let block = Block::default()
                    .borders(Borders::ALL)
                    .title(title.clone())
                    .border_style(Style::default().fg(color));
                if title.starts_with(" CPU ") {
                    let inner = block.inner(rect);
                    f.render_widget(Paragraph::new(Line::default()).block(block), rect);
                    draw_cpu_panel(
                        f,
                        inner,
                        &self.resource_snapshot,
                        &self.loaded.config.resources,
                        self.theme.resources.cpu,
                        self.theme.resources.temp,
                    );
                } else {
                    let inner = block.inner(rect);
                    f.render_widget(Paragraph::new(Line::default()).block(block), rect);
                    if title.contains("Mem") {
                        draw_mem_panel(
                            f,
                            inner,
                            &self.resource_snapshot,
                            &self.loaded.config.resources,
                        );
                        state.hits.disks_toggle = Rect::new(rect.x + 9, rect.y, 3, 1);
                        state.hits.io_toggle = Rect::new(rect.x + 13, rect.y, 3, 1);
                    } else if title.contains("Net") {
                        draw_net_panel(
                            f,
                            inner,
                            &self.resource_snapshot,
                            &self.loaded.config.resources,
                            state.net_iface.as_deref(),
                            state
                                .net_zero_offsets
                                .get(
                                    state
                                        .net_iface
                                        .as_deref()
                                        .or_else(|| {
                                            crate::ui::resources::net::default_interface(
                                                &self.resource_snapshot.networks,
                                            )
                                        })
                                        .unwrap_or_default(),
                                )
                                .copied()
                                .unwrap_or_default(),
                            if state.net_auto {
                                state.net_download_scaler.max()
                            } else {
                                state.net_download as f64 * 1024.0
                            },
                            if state.net_auto {
                                state.net_upload_scaler.max()
                            } else {
                                state.net_upload as f64 * 1024.0
                            },
                            state.net_sync,
                        );
                    } else if title.contains("Agents") {
                        crate::ui::app::render::resources::draw_agents(
                            f,
                            inner,
                            &self.resource_snapshot,
                            &remote_agents,
                            state,
                        );
                    }
                }
            }
        }
        if state.menu_open {
            let menu = Rect::new(
                area.right().saturating_sub(34),
                area.y + 1,
                33.min(area.width),
                3.min(area.height.saturating_sub(1)),
            );
            f.render_widget(
                Paragraph::new("Resources · 1–4 boxes · p/P presets · +/- interval")
                    .block(Block::default().borders(Borders::ALL).title(" Menu ")),
                menu,
            );
        }
        if state.help_open {
            let help = Rect::new(
                area.x.saturating_add(area.width.saturating_sub(62) / 2),
                area.y.saturating_add(area.height.saturating_sub(11) / 2),
                62.min(area.width),
                11.min(area.height),
            );
            f.render_widget(
                Paragraph::new("1–4 show or hide boxes\np/P change layout preset\n+/- change sample interval\na/y control network scaling\nb/n select network interface\nz reset network totals\no open Resources settings\n←/→ change a setting · Esc close")
                    .block(Block::default().borders(Borders::ALL).title(" Resources key help ")),
                help,
            );
        }
    }

    pub(in crate::ui::app) fn resources_key(&mut self, key: crossterm::event::KeyCode) -> bool {
        if key == crossterm::event::KeyCode::Char('o') {
            self.tab_index = crate::ui::app::TABS
                .iter()
                .position(|tab| *tab == "Settings")
                .unwrap_or(0);
            self.settings_index = crate::ui::app::types::SP_RESOURCES;
            self.settings_focused = true;
            self.set_status("Settings · Resources");
            return true;
        }
        if key == crossterm::event::KeyCode::Char('?') {
            self.resources.help_open = !self.resources.help_open;
            return true;
        }
        if self.resources.help_open {
            if key == crossterm::event::KeyCode::Esc {
                self.resources.help_open = false;
            }
            return true;
        }
        if self.agent_key(key) {
            return true;
        }
        if !self.resources.key(key) {
            return false;
        }
        if key == crossterm::event::KeyCode::Char('z') {
            let current = self.resources.net_iface.as_deref().or_else(|| {
                crate::ui::resources::net::default_interface(&self.resource_snapshot.networks)
            });
            if let Some(interface) = current.and_then(|name| {
                self.resource_snapshot
                    .networks
                    .iter()
                    .find(|entry| entry.interface == name)
            }) {
                self.resources.net_zero_offsets.insert(
                    interface.interface.clone(),
                    (
                        interface.total_received_bytes,
                        interface.total_transmitted_bytes,
                    ),
                );
                self.resources.net_iface = Some(interface.interface.clone());
            }
        }
        if matches!(key, crossterm::event::KeyCode::Char('b' | 'n')) {
            let current = self.resources.net_iface.as_deref().or_else(|| {
                crate::ui::resources::net::default_interface(&self.resource_snapshot.networks)
            });
            let interfaces: Vec<_> = self
                .resource_snapshot
                .networks
                .iter()
                .map(|interface| (interface.interface.as_str(), interface.up))
                .collect();
            self.resources.net_iface = crate::ui::resources::net::cycle_interface(
                current,
                &interfaces,
                key == crossterm::event::KeyCode::Char('n'),
            );
        }
        let state = &self.resources;
        let shown = [
            (state.shown.cpu, "cpu"),
            (state.shown.mem, "mem"),
            (state.shown.net, "net"),
            (state.shown.agents, "agents"),
        ]
        .into_iter()
        .filter_map(|(shown, name)| shown.then_some(name))
        .collect::<Vec<_>>()
        .join(",");
        let config = &mut self.loaded.config.resources;
        config.update_ms = state.interval_ms;
        config.shown_boxes = shown;
        config.preset = state.preset;
        config.cpu_bottom = Some(state.opts.cpu_bottom);
        config.mem_below_net = Some(state.opts.mem_below_net);
        config.agents_left = Some(state.opts.agents_left);
        config.show_disks = state.show_disks;
        config.io_mode = state.io_mode;
        config.net_auto = state.net_auto;
        config.net_sync = state.net_sync;
        config.net_download = state.net_download;
        config.net_upload = state.net_upload;
        config.net_iface = state.net_iface.clone();
        config.swap_upload_download = state.swap_upload_download;
        if let Some(path) = self.config_path.as_deref() {
            let settings = [
                ("updateMs", ratatui_toml_value_u64(config.update_ms)),
                (
                    "shownBoxes",
                    toml::Value::String(config.shown_boxes.clone()),
                ),
                ("preset", toml::Value::Integer(config.preset as i64)),
                (
                    "cpuBottom",
                    toml::Value::Boolean(config.cpu_bottom.unwrap_or(false)),
                ),
                (
                    "memBelowNet",
                    toml::Value::Boolean(config.mem_below_net.unwrap_or(false)),
                ),
                (
                    "agentsLeft",
                    toml::Value::Boolean(config.agents_left.unwrap_or(false)),
                ),
                ("showDisks", toml::Value::Boolean(config.show_disks)),
                ("ioMode", toml::Value::Boolean(config.io_mode)),
                ("netAuto", toml::Value::Boolean(config.net_auto)),
                ("netSync", toml::Value::Boolean(config.net_sync)),
                ("netDownload", ratatui_toml_value_u64(config.net_download)),
                ("netUpload", ratatui_toml_value_u64(config.net_upload)),
                (
                    "netIface",
                    config
                        .net_iface
                        .clone()
                        .map_or(toml::Value::String(String::new()), toml::Value::String),
                ),
                (
                    "swapUploadDownload",
                    toml::Value::Boolean(config.swap_upload_download),
                ),
            ];
            for (key, value) in settings {
                if let Err(error) = medulla::config::persist_setting(path, "resources", key, value)
                {
                    self.set_status(format!("Resources save failed: {error}"));
                    return true;
                }
            }
            self.set_status("Resources settings saved");
        } else {
            self.set_status("Resources settings changed (not persisted)");
        }
        true
    }

    fn agent_key(&mut self, key: crossterm::event::KeyCode) -> bool {
        use crossterm::event::KeyCode;
        use types::{AgentSignal, AgentSignalConfirmStage, AgentSignalConfirmation, AgentSort};
        if !self.resources.shown.agents {
            self.resources.agent_signal_armed = None;
            return false;
        }
        if let Some(armed) = self.resources.agent_signal_armed.take() {
            let confirmation = match armed.stage {
                AgentSignalConfirmStage::First => key == KeyCode::Char('y'),
                AgentSignalConfirmStage::ExternalSecond => key == KeyCode::Char('Y'),
            };
            if !confirmation {
                self.set_status("Process signal cancelled");
                return true;
            }
            let process = self
                .resource_snapshot
                .processes
                .iter()
                .find(|process| process.pid == armed.pid && process.start_time == armed.started)
                .cloned();
            let Some(process) = process else {
                self.set_status("Process exited before signal was confirmed");
                return true;
            };
            let row = crate::ui::app::render::resources::rows(&self.resource_snapshot)
                .into_iter()
                .find(|row| row.pid == armed.pid && row.started == armed.started);
            let Some(row) = row else {
                self.set_status("Process is no longer a classified local agent");
                return true;
            };
            if row.remote {
                self.set_status("Signals to remote processes are disabled");
                return true;
            }
            if !(self.resources.agent_process_owned_fn)(
                armed.pid,
                armed.started,
                process.owner_uid.as_deref(),
            ) {
                self.set_status("Cannot signal a process not owned by the current user");
                return true;
            }
            if row.external && armed.stage == AgentSignalConfirmStage::First {
                self.resources.agent_signal_armed = Some(AgentSignalConfirmation {
                    stage: AgentSignalConfirmStage::ExternalSecond,
                    ..armed
                });
                self.set_status("External process: press uppercase Y to confirm the signal");
                return true;
            }
            match (self.resources.agent_signal_fn)(
                armed.pid,
                armed.started,
                process.owner_uid.as_deref(),
                armed.signal,
            ) {
                Ok(()) => self.set_status(format!("Signal sent to PID {}", armed.pid)),
                Err(error) => self.set_status(format!("Signal failed: {error}")),
            }
            return true;
        }
        if self.resources.agent_filter_editing {
            match key {
                KeyCode::Esc => {
                    self.resources.agent_filter_editing = false;
                }
                KeyCode::Enter => {
                    self.resources.agent_filter_editing = false;
                    let remote_hosts: Vec<_> = self
                        .remote_hosts
                        .iter()
                        .map(|(id, host)| (id.clone(), host.rows.clone()))
                        .collect();
                    let rows = crate::ui::app::render::resources::visible_rows_with_remote(
                        &self.resource_snapshot,
                        &remote_hosts,
                        &self.resources,
                    );
                    self.resources.agent_selected = rows.first().map(|row| row.identity());
                }
                KeyCode::Backspace => {
                    self.resources.agent_filter.as_mut().map(String::pop);
                }
                KeyCode::Char(c) => self
                    .resources
                    .agent_filter
                    .get_or_insert_with(String::new)
                    .push(c),
                _ => {}
            }
            return true;
        }
        if self.resources.agent_detail && key == KeyCode::Esc {
            self.resources.agent_detail = false;
            return true;
        }
        let remote_hosts: Vec<_> = self
            .remote_hosts
            .iter()
            .map(|(id, host)| (id.clone(), host.rows.clone()))
            .collect();
        let rows = crate::ui::app::render::resources::visible_rows_with_remote(
            &self.resource_snapshot,
            &remote_hosts,
            &self.resources,
        );
        let selected = self
            .resources
            .agent_selected
            .as_ref()
            .and_then(|id| rows.iter().position(|row| row.identity() == *id))
            .unwrap_or(0);
        match key {
            KeyCode::Left | KeyCode::Right => {
                self.resources.agent_sort = match (self.resources.agent_sort, key == KeyCode::Right)
                {
                    (AgentSort::Pid, true) => AgentSort::Kind,
                    (AgentSort::Kind, true) => AgentSort::Origin,
                    (AgentSort::Origin, true) => AgentSort::Workspace,
                    (AgentSort::Workspace, true) => AgentSort::Threads,
                    (AgentSort::Threads, true) => AgentSort::Memory,
                    (AgentSort::Memory, true) => AgentSort::Cpu,
                    (AgentSort::Cpu, true) => AgentSort::Runtime,
                    (AgentSort::Runtime, true) => AgentSort::Pid,
                    (AgentSort::Pid, false) => AgentSort::Runtime,
                    (AgentSort::Runtime, false) => AgentSort::Cpu,
                    (AgentSort::Cpu, false) => AgentSort::Memory,
                    (AgentSort::Memory, false) => AgentSort::Threads,
                    (AgentSort::Threads, false) => AgentSort::Workspace,
                    (AgentSort::Workspace, false) => AgentSort::Origin,
                    (AgentSort::Origin, false) => AgentSort::Kind,
                    (AgentSort::Kind, false) => AgentSort::Pid,
                };
                true
            }
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End => {
                let next = match key {
                    KeyCode::Up => selected.saturating_sub(1),
                    KeyCode::Down => (selected + 1).min(rows.len().saturating_sub(1)),
                    KeyCode::PageUp => selected.saturating_sub(10),
                    KeyCode::PageDown => (selected + 10).min(rows.len().saturating_sub(1)),
                    KeyCode::Home => 0,
                    KeyCode::End => rows.len().saturating_sub(1),
                    _ => selected,
                };
                self.resources.agent_selected = rows.get(next).map(|row| row.identity());
                if self.resources.agent_follow.is_some() {
                    self.resources.agent_follow = self.resources.agent_selected.clone();
                }
                true
            }
            KeyCode::Char('r') => {
                self.resources.agent_reverse = !self.resources.agent_reverse;
                true
            }
            KeyCode::Char('e') => {
                self.resources.agent_tree = !self.resources.agent_tree;
                true
            }
            KeyCode::Char('f') | KeyCode::Char('/') => {
                self.resources.agent_filter_editing = true;
                self.resources.agent_filter.get_or_insert_with(String::new);
                true
            }
            KeyCode::Char('F') => {
                let identity = rows
                    .get(selected)
                    .filter(|row| !row.remote)
                    .map(|row| row.identity());
                if identity.is_none() {
                    self.set_status("Follow is available for local agents only");
                    return true;
                }
                self.resources.agent_follow = match (self.resources.agent_follow.clone(), identity)
                {
                    (Some(following), Some(selected)) if following == selected => None,
                    (_, selected) => selected,
                };
                if self.resources.agent_follow.is_some() {
                    self.resources.agent_selected = self.resources.agent_follow.clone();
                }
                true
            }
            KeyCode::Enter => {
                if let Some(row) = rows.get(selected) {
                    if let Some(session) = row.session_id.as_deref() {
                        self.tab_index = crate::ui::app::types::TABS
                            .iter()
                            .position(|tab| *tab == "Sessions")
                            .unwrap_or(0);
                        self.select_session_row(session);
                    } else {
                        self.resources.agent_detail = true;
                    }
                }
                true
            }
            KeyCode::Char('t' | 'k' | 's') => {
                if let Some(row) = rows.get(selected) {
                    if !cfg!(any(target_os = "linux", target_os = "macos")) {
                        self.set_status("Process signals are disabled on this platform");
                        return true;
                    }
                    if row.remote {
                        self.set_status("Signals to remote processes are disabled");
                        return true;
                    }
                    let signal = match key {
                        KeyCode::Char('t') => AgentSignal::Terminate,
                        KeyCode::Char('k') => AgentSignal::Kill,
                        _ => AgentSignal::Stop,
                    };
                    self.resources.agent_signal_armed = Some(AgentSignalConfirmation {
                        pid: row.pid,
                        started: row.started,
                        signal,
                        stage: AgentSignalConfirmStage::First,
                    });
                }
                true
            }
            _ => false,
        }
    }

    pub(in crate::ui::app) fn resources_click(&mut self, x: u16, y: u16) -> bool {
        let hits = self.resources.hits;
        if hits.menu.contains((x, y).into()) {
            self.resources.menu_open = !self.resources.menu_open;
            return true;
        }
        let key = [
            (hits.cpu_toggle, '1'),
            (hits.mem_toggle, '2'),
            (hits.net_toggle, '3'),
            (hits.agents_toggle, '4'),
            (hits.disks_toggle, 'd'),
            (hits.io_toggle, 'i'),
            (hits.net_prev, 'b'),
            (hits.net_next, 'n'),
            (hits.net_zero, 'z'),
            (hits.net_auto, 'a'),
            (hits.net_sync, 'y'),
            (hits.preset_prev, 'p'),
            (hits.preset_next, 'P'),
            (hits.interval_down, '-'),
            (hits.interval_up, '+'),
        ]
        .into_iter()
        .find_map(|(rect, key)| rect.contains((x, y).into()).then_some(key));
        key.is_some_and(|key| self.resources_key(crossterm::event::KeyCode::Char(key)))
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_net_panel(
    frame: &mut Frame,
    area: Rect,
    snapshot: &crate::ui::resources::sampler::ResourceSnapshot,
    config: &medulla::config::ResourcesConfig,
    selected: Option<&str>,
    zero_offsets: (u64, u64),
    download_max: f64,
    upload_max: f64,
    sync: bool,
) {
    if area.is_empty() {
        return;
    }
    let selected = selected
        .and_then(|name| {
            snapshot
                .networks
                .iter()
                .find(|entry| entry.interface == name)
        })
        .or_else(|| {
            crate::ui::resources::net::default_interface(&snapshot.networks).and_then(|name| {
                snapshot
                    .networks
                    .iter()
                    .find(|entry| entry.interface == name)
            })
        });
    let Some(selected) = selected else {
        frame.render_widget(Paragraph::new("No active network interface"), area);
        return;
    };
    let (zero_received, zero_transmitted) = zero_offsets;
    let (download, upload) = if config.swap_upload_download {
        (
            selected.transmitted_bytes_per_second,
            selected.received_bytes_per_second,
        )
    } else {
        (
            selected.received_bytes_per_second,
            selected.transmitted_bytes_per_second,
        )
    };
    let (top_download, top_upload) = network_top(snapshot, &selected.interface);
    let (total_download, total_upload) = if config.swap_upload_download {
        (
            selected.total_transmitted_bytes,
            selected.total_received_bytes,
        )
    } else {
        (
            selected.total_received_bytes,
            selected.total_transmitted_bytes,
        )
    };
    let (top_download, top_upload) = if config.swap_upload_download {
        (top_upload, top_download)
    } else {
        (top_download, top_upload)
    };
    let mut lines = Vec::new();
    let narrow = area.width <= 45;
    if narrow {
        lines.push(Line::from(format!(
            "{} ↓ {}  ↑ {}",
            selected.interface,
            net_rate(download),
            net_rate(upload)
        )));
        lines.push(Line::from(format!(
            "bits ↓{} ↑{}",
            net_rate(download * 8.0),
            net_rate(upload * 8.0)
        )));
        lines.push(Line::from(format!(
            "Top ↓{} ↑{}  Total ↓{} ↑{}",
            net_rate(top_download),
            net_rate(top_upload),
            net_bytes(
                total_download.saturating_sub(if config.swap_upload_download {
                    zero_transmitted
                } else {
                    zero_received
                })
            ),
            net_bytes(total_upload.saturating_sub(if config.swap_upload_download {
                zero_received
            } else {
                zero_transmitted
            }))
        )));
    } else {
        let address = selected
            .ip_addresses
            .first()
            .map(String::as_str)
            .unwrap_or("no IP");
        lines.push(Line::from(format!("{}  {}", selected.interface, address)));
        lines.push(Line::from(format!(
            "Download {:>9}/s  {} bit/s",
            net_rate(download),
            net_rate(download * 8.0)
        )));
        lines.push(Line::from(format!(
            "Upload   {:>9}/s  {} bit/s",
            net_rate(upload),
            net_rate(upload * 8.0)
        )));
        lines.push(Line::from(format!(
            "Top ↓{} ↑{}  Total ↓{} ↑{}",
            net_rate(top_download),
            net_rate(top_upload),
            net_bytes(
                total_download.saturating_sub(if config.swap_upload_download {
                    zero_transmitted
                } else {
                    zero_received
                })
            ),
            net_bytes(total_upload.saturating_sub(if config.swap_upload_download {
                zero_received
            } else {
                zero_transmitted
            }))
        )));
    }
    let graph_rows = if area.height >= 3 { 2 } else { 0 };
    let (download_max, upload_max) =
        crate::ui::resources::net::synced_max(download_max, upload_max, sync);
    let graph_width = area.width;
    if graph_rows > 0 {
        let history: Vec<_> = snapshot.history.network_io.iter().collect();
        let downloads: Vec<f32> = history
            .iter()
            .filter_map(|sample| {
                sample
                    .iter()
                    .find(|entry| entry.interface == selected.interface)
            })
            .map(|entry| {
                let rate = if config.swap_upload_download {
                    entry.transmitted_bytes_per_second
                } else {
                    entry.received_bytes_per_second
                };
                (rate * 100.0 / download_max.max(1.0)) as f32
            })
            .collect();
        let uploads: Vec<f32> = history
            .iter()
            .filter_map(|sample| {
                sample
                    .iter()
                    .find(|entry| entry.interface == selected.interface)
            })
            .map(|entry| {
                let rate = if config.swap_upload_download {
                    entry.received_bytes_per_second
                } else {
                    entry.transmitted_bytes_per_second
                };
                (rate * 100.0 / upload_max.max(1.0)) as f32
            })
            .collect();
        let x = area.x;
        frame.render_widget(
            crate::ui::plot::Graph::new(&downloads),
            Rect::new(x, area.y, graph_width, 1),
        );
        frame.render_widget(
            crate::ui::plot::Graph::new(&uploads).invert(true),
            Rect::new(x, area.y + 1, graph_width, 1),
        );
    }
    let text_y = area.y + graph_rows;
    let text_height = area.height.saturating_sub(graph_rows);
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(Color::White)),
        Rect::new(area.x, text_y, area.width, text_height),
    );
}

fn network_top(
    snapshot: &crate::ui::resources::sampler::ResourceSnapshot,
    interface: &str,
) -> (f64, f64) {
    snapshot
        .history
        .network_io
        .iter()
        .flat_map(|sample| sample.iter())
        .filter(|entry| entry.interface == interface)
        .fold((0.0_f64, 0.0_f64), |(rx, tx), entry| {
            (
                rx.max(entry.received_bytes_per_second),
                tx.max(entry.transmitted_bytes_per_second),
            )
        })
}

fn net_rate(rate: f64) -> String {
    if rate >= 1_048_576.0 {
        format!("{:.1}M", rate / 1_048_576.0)
    } else if rate >= 1024.0 {
        format!("{:.1}K", rate / 1024.0)
    } else {
        format!("{rate:.0}B")
    }
}

fn net_bytes(value: u64) -> String {
    net_rate(value as f64)
}

fn draw_mem_panel(
    frame: &mut Frame,
    area: Rect,
    snapshot: &crate::ui::resources::sampler::ResourceSnapshot,
    config: &medulla::config::ResourcesConfig,
) {
    if area.is_empty() {
        return;
    }
    let bytes = |value: u64| {
        if value >= 1 << 30 {
            format!("{:.1}G", value as f64 / (1 << 30) as f64)
        } else if value >= 1 << 20 {
            format!("{:.0}M", value as f64 / (1 << 20) as f64)
        } else {
            format!("{:.0}K", value as f64 / 1024.0)
        }
    };
    let total = snapshot.memory_total_bytes;
    let meter = |fraction: f64, width: usize| {
        let filled = (fraction.clamp(0.0, 1.0) * width as f64).round() as usize;
        format!(
            "{}{}",
            "█".repeat(filled),
            "░".repeat(width.saturating_sub(filled))
        )
    };
    let ram_fraction = if total == 0 {
        0.0
    } else {
        snapshot.memory_used_bytes as f64 / total as f64
    };
    let ram_line = if config.mem_graphs {
        format!(
            "RAM {:>3.0}% {} / {}",
            ram_fraction * 100.0,
            bytes(snapshot.memory_used_bytes),
            bytes(total)
        )
    } else {
        let fraction = |value: u64| {
            if total == 0 {
                0.0
            } else {
                value as f64 / total as f64
            }
        };
        format!(
            "RAM {:>3.0}% U{} A{} C{} F{}",
            ram_fraction * 100.0,
            meter(ram_fraction, 4),
            meter(fraction(snapshot.memory_available_bytes), 4),
            meter(fraction(snapshot.memory_cached_bytes), 4),
            meter(fraction(snapshot.memory_free_bytes), 4),
        )
    };
    let mut lines = vec![Line::from(ram_line)];
    if config.show_swap && snapshot.swap_total_bytes > 0 {
        let swap_fraction = snapshot.swap_used_bytes as f64 / snapshot.swap_total_bytes as f64;
        lines.push(Line::from(format!(
            "Swap {:>3.0}% {} {} / {}",
            swap_fraction * 100.0,
            if config.mem_graphs {
                String::new()
            } else {
                meter(swap_fraction, 10)
            },
            bytes(snapshot.swap_used_bytes),
            bytes(snapshot.swap_total_bytes)
        )));
    }
    let mut disks = snapshot.disks.clone();
    if config.swap_disk && snapshot.swap_total_bytes > 0 {
        disks.push(crate::ui::resources::sampler::DiskSnapshot {
            device: "swap".into(),
            mount: "[swap]".into(),
            total_bytes: snapshot.swap_total_bytes,
            used_bytes: snapshot.swap_used_bytes,
            read_bytes_per_second: 0.0,
            write_bytes_per_second: 0.0,
            busy_percent: None,
        });
    }
    if config.show_disks && !config.io_mode {
        for disk in disks
            .iter()
            .take(area.height.saturating_sub(lines.len() as u16) as usize)
        {
            let used = if disk.total_bytes == 0 {
                0.0
            } else {
                disk.used_bytes as f64 / disk.total_bytes as f64
            };
            let io = if config.io_mode {
                format!(
                    " R{} W{}",
                    bytes(disk.read_bytes_per_second as u64),
                    bytes(disk.write_bytes_per_second as u64)
                )
            } else {
                String::new()
            };
            lines.push(Line::from(format!(
                "{:<8} {:>3.0}% U{}F{} {} / {} free {}{}",
                disk.mount,
                used * 100.0,
                meter(used, 4),
                meter(1.0 - used, 4),
                bytes(disk.used_bytes),
                bytes(disk.total_bytes),
                bytes(disk.total_bytes.saturating_sub(disk.used_bytes)),
                io
            )));
        }
    }
    let graph_rows = if config.io_mode && config.show_disks {
        4
    } else if config.mem_graphs {
        if config.show_swap && snapshot.swap_total_bytes > 0 {
            5
        } else {
            4
        }
    } else {
        0
    };
    if graph_rows > 0 && area.height > graph_rows {
        let graph_area = Rect::new(area.x, area.y, area.width, graph_rows);
        if config.io_mode && config.show_disks {
            let read_samples: Vec<f64> = snapshot
                .history
                .disk_read_bytes_per_second
                .iter()
                .copied()
                .collect();
            let write_samples: Vec<f64> = snapshot
                .history
                .disk_write_bytes_per_second
                .iter()
                .copied()
                .collect();
            let configured_speed = disks
                .iter()
                .map(|disk| io_graph_speed(config, &disk.mount))
                .fold(100.0_f64, f64::max);
            let scale = configured_speed * 1024.0 * 1024.0;
            let reads: Vec<f32> = read_samples
                .iter()
                .map(|v| (v * 100.0 / scale) as f32)
                .collect();
            let writes: Vec<f32> = write_samples
                .iter()
                .map(|v| (v * 100.0 / scale) as f32)
                .collect();
            if config.io_graph_combined {
                let combined_samples: Vec<f64> = read_samples
                    .iter()
                    .zip(&write_samples)
                    .map(|(r, w)| r + w)
                    .collect();
                let combined_scale = scale;
                let combined: Vec<f32> = combined_samples
                    .iter()
                    .map(|v| (v * 100.0 / combined_scale) as f32)
                    .collect();
                frame.render_widget(crate::ui::plot::Graph::new(&combined), graph_area);
            } else {
                frame.render_widget(
                    crate::ui::plot::Graph::new(&reads),
                    Rect::new(area.x, area.y, area.width, graph_rows / 2),
                );
                frame.render_widget(
                    crate::ui::plot::Graph::new(&writes),
                    Rect::new(
                        area.x,
                        area.y + graph_rows / 2,
                        area.width,
                        graph_rows - graph_rows / 2,
                    ),
                );
            }
        } else if config.mem_graphs {
            let percent = |history: &crate::ui::resources::sampler::History<u64>| {
                history
                    .iter()
                    .map(|value| {
                        if total == 0 {
                            0.0
                        } else {
                            *value as f32 * 100.0 / total as f32
                        }
                    })
                    .collect::<Vec<_>>()
            };
            let mut series = vec![
                ("USED ", percent(&snapshot.history.memory_used_bytes)),
                ("AVAIL", percent(&snapshot.history.memory_available_bytes)),
                ("CACHE", percent(&snapshot.history.memory_cached_bytes)),
                ("FREE ", percent(&snapshot.history.memory_free_bytes)),
            ];
            if config.show_swap && snapshot.swap_total_bytes > 0 {
                series.push((
                    "SWAP ",
                    snapshot
                        .history
                        .swap_used_bytes
                        .iter()
                        .map(|value| *value as f32 * 100.0 / snapshot.swap_total_bytes as f32)
                        .collect(),
                ));
            }
            for (row, (label, values)) in series.iter().take(graph_rows as usize).enumerate() {
                frame.render_widget(
                    crate::ui::plot::Graph::new(values),
                    Rect::new(area.x, area.y + row as u16, area.width, 1),
                );
                frame.render_widget(
                    Paragraph::new(*label).style(Style::default().fg(Color::DarkGray)),
                    Rect::new(area.x, area.y + row as u16, 5.min(area.width), 1),
                );
            }
        }
        lines.insert(
            0,
            Line::from(if config.io_mode && config.show_disks {
                if config.io_graph_combined {
                    "IO READ+WRITE"
                } else {
                    "IO READ            WRITE"
                }
            } else {
                if config.show_swap {
                    "RAM / SWAP HISTORY"
                } else {
                    "RAM / AVAILABLE HISTORY"
                }
            }),
        );
        let disk_start = if config.show_swap && snapshot.swap_total_bytes > 0 {
            3
        } else {
            2
        };
        lines.truncate(area.height.saturating_sub(graph_rows) as usize);
        frame.render_widget(
            Paragraph::new(lines).style(Style::default().fg(Color::White)),
            Rect::new(
                area.x,
                area.y + graph_rows,
                area.width,
                area.height - graph_rows,
            ),
        );
        if config.io_mode && config.show_disks {
            draw_disk_io_graphs(
                frame, area, snapshot, &disks, graph_rows, disk_start, config,
            );
        } else {
            draw_disk_busy_graphs(frame, area, snapshot, &disks, graph_rows, disk_start);
        }
        return;
    }
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(Color::White)),
        area,
    );
    if config.io_mode && config.show_disks {
        let disk_start = if config.show_swap && snapshot.swap_total_bytes > 0 {
            2
        } else {
            1
        };
        draw_disk_io_graphs(frame, area, snapshot, &disks, 0, disk_start, config);
    } else if config.show_disks {
        let disk_start = if config.show_swap && snapshot.swap_total_bytes > 0 {
            2
        } else {
            1
        };
        draw_disk_busy_graphs(frame, area, snapshot, &disks, 0, disk_start);
    }
}

fn draw_disk_busy_graphs(
    frame: &mut Frame,
    area: Rect,
    snapshot: &crate::ui::resources::sampler::ResourceSnapshot,
    disks: &[crate::ui::resources::sampler::DiskSnapshot],
    graph_rows: u16,
    disk_start: usize,
) {
    let x = area.x + area.width.saturating_mul(3) / 4;
    let width = area.right().saturating_sub(x);
    if width < 4 {
        return;
    }
    for (index, disk) in disks.iter().enumerate() {
        let y = area.y + graph_rows + disk_start as u16 + index as u16;
        if y >= area.bottom() {
            break;
        }
        let history: Vec<f32> = snapshot
            .history
            .disk_io
            .iter()
            .filter_map(|sample| {
                sample
                    .iter()
                    .find(|entry| entry.device == disk.device)
                    .and_then(|entry| entry.busy_percent)
                    .map(|busy| busy as f32)
            })
            .collect();
        if !history.is_empty() {
            frame.render_widget(
                crate::ui::plot::Graph::new(&history),
                Rect::new(x, y, width, 1),
            );
        }
    }
}

fn draw_disk_io_graphs(
    frame: &mut Frame,
    area: Rect,
    snapshot: &crate::ui::resources::sampler::ResourceSnapshot,
    disks: &[crate::ui::resources::sampler::DiskSnapshot],
    graph_rows: u16,
    disk_start: usize,
    config: &medulla::config::ResourcesConfig,
) {
    let x = area.x + area.width / 2;
    let width = area.right().saturating_sub(x);
    if width < 6 {
        return;
    }
    for (index, disk) in disks.iter().enumerate() {
        let y = area.y + graph_rows + disk_start as u16 + index as u16;
        if y >= area.bottom() {
            break;
        }
        let limit = io_graph_speed(config, &disk.mount) * 1024.0 * 1024.0;
        let read: Vec<f32> = snapshot
            .history
            .disk_io
            .iter()
            .filter_map(|sample| {
                sample
                    .iter()
                    .find(|entry| entry.device == disk.device)
                    .map(|entry| (entry.read_bytes_per_second * 100.0 / limit) as f32)
            })
            .collect();
        let write: Vec<f32> = snapshot
            .history
            .disk_io
            .iter()
            .filter_map(|sample| {
                sample
                    .iter()
                    .find(|entry| entry.device == disk.device)
                    .map(|entry| (entry.write_bytes_per_second * 100.0 / limit) as f32)
            })
            .collect();
        if config.io_graph_combined {
            let combined: Vec<f32> = read.iter().zip(&write).map(|(r, w)| r + w).collect();
            frame.render_widget(
                crate::ui::plot::Graph::new(&combined),
                Rect::new(x, y, width, 1),
            );
        } else {
            let half = width / 2;
            frame.render_widget(crate::ui::plot::Graph::new(&read), Rect::new(x, y, half, 1));
            frame.render_widget(
                crate::ui::plot::Graph::new(&write),
                Rect::new(x + half, y, width - half, 1),
            );
        }
    }
}

fn io_graph_speed(config: &medulla::config::ResourcesConfig, mount: &str) -> f64 {
    config
        .io_graph_speeds
        .split_whitespace()
        .find_map(|entry| {
            let (target, speed) = entry.split_once(':')?;
            if target != mount {
                return None;
            }
            speed
                .parse::<u64>()
                .ok()
                .filter(|speed| *speed > 0)
                .map(|speed| speed as f64)
        })
        .unwrap_or(100.0)
}

fn draw_cpu_panel(
    frame: &mut Frame,
    area: Rect,
    snapshot: &crate::ui::resources::sampler::ResourceSnapshot,
    config: &medulla::config::ResourcesConfig,
    colors: crate::ui::theme::Gradient,
    temp_colors: crate::ui::theme::Gradient,
) {
    if area.is_empty() {
        return;
    }
    use ratatui::layout::{Constraint, Layout};
    let history: Vec<_> = snapshot.history.cpu.iter().collect();
    let upper = medulla_monitor::CpuField::parse(&config.cpu_graph_upper)
        .unwrap_or(medulla_monitor::CpuField::Total);
    let lower = medulla_monitor::CpuField::parse(&config.cpu_graph_lower)
        .unwrap_or(medulla_monitor::CpuField::Iowait);
    let rows = if config.cpu_single_graph { 1 } else { 2 };
    let compact = area.width < 100;
    let graph_rows = if compact { 1 } else { rows.min(area.height) };
    let chunks = Layout::vertical(std::iter::repeat_n(
        Constraint::Length(1),
        graph_rows as usize,
    ))
    .split(Rect::new(area.x, area.y, area.width, graph_rows));
    let chart_gradient = crate::ui::plot::Gradient::new(vec![
        colors.start,
        colors.mid.unwrap_or(colors.start),
        colors.end.unwrap_or(colors.start),
    ]);
    let temp_gradient = crate::ui::plot::Gradient::new(vec![
        temp_colors.start,
        temp_colors.mid.unwrap_or(temp_colors.start),
        temp_colors.end.unwrap_or(temp_colors.start),
    ]);
    let values: Vec<f32> = history
        .iter()
        .map(|sample| sample.total.field(upper))
        .collect();
    frame.render_widget(
        crate::ui::plot::Graph::new(&values).gradient(&chart_gradient),
        chunks[0],
    );
    if rows > 1 && graph_rows > 1 {
        let values: Vec<f32> = history
            .iter()
            .map(|sample| sample.total.field(lower))
            .collect();
        frame.render_widget(
            crate::ui::plot::Graph::new(&values)
                .invert(config.cpu_invert_lower)
                .gradient(&chart_gradient),
            chunks[1],
        );
    }
    let mut lines = Vec::new();
    let mut details = Vec::new();
    if !compact {
        if let Some(model) = snapshot
            .cpu
            .model
            .as_deref()
            .filter(|model| !model.is_empty())
        {
            details.push(model.to_owned());
        }
    }
    let frequencies: Vec<u64> = snapshot
        .cpu
        .frequencies_mhz
        .iter()
        .flatten()
        .copied()
        .collect();
    if !frequencies.is_empty() {
        let average = frequencies.iter().sum::<u64>() / frequencies.len() as u64;
        details.push(format!(
            "Freq {}",
            format_frequency(average, &config.freq_mode)
        ));
    }
    if let Some(temp) = snapshot.cpu.package_temperature_c {
        let temp = if config.temp_scale == "fahrenheit" {
            temp * 1.8 + 32.0
        } else {
            temp
        };
        details.push(format!(
            "Pkg {temp:.0}°{}",
            if config.temp_scale == "fahrenheit" {
                "F"
            } else {
                "C"
            }
        ));
    }
    {
        details.push(format!(
            "Load avg {} {} {}",
            if compact {
                format!("{:.1}", snapshot.cpu.load_average[0])
            } else {
                format!("{:.2}", snapshot.cpu.load_average[0])
            },
            if compact {
                format!("{:.1}", snapshot.cpu.load_average[1])
            } else {
                format!("{:.2}", snapshot.cpu.load_average[1])
            },
            if compact {
                format!("{:.1}", snapshot.cpu.load_average[2])
            } else {
                format!("{:.2}", snapshot.cpu.load_average[2])
            }
        ));
    }
    if let Some(uptime) = snapshot.cpu.uptime_seconds {
        details.push(format!(
            "Up {}d {:02}h",
            uptime / 86_400,
            uptime / 3_600 % 24
        ));
    }
    if config.show_battery {
        if let Some(battery) = snapshot.cpu.battery_percent {
            details.push(format!("Bat {battery:.0}%"));
        }
    }
    if !details.is_empty() {
        lines.push(ratatui::text::Line::from(details.join("  ")));
    }
    let remaining = area
        .height
        .saturating_sub(graph_rows)
        .saturating_sub(lines.len() as u16) as usize;
    let cores = &snapshot.cpu.cores;
    let rows_per_core = if compact { 15 } else { 1 };
    let max_per_row = (area.width as usize / rows_per_core).max(1);
    let per_row = if cores.len() <= remaining.max(1) {
        1
    } else {
        cores.len().div_ceil(remaining.max(1))
    }
    .min(max_per_row);
    for group in cores
        .iter()
        .enumerate()
        .collect::<Vec<_>>()
        .chunks(per_row.max(1))
    {
        let text = group
            .iter()
            .map(|(index, cpu)| {
                let mut row = format!("C{index} {:>3.0}%", cpu.total);
                if let Some(Some(freq)) = snapshot.cpu.frequencies_mhz.get(*index) {
                    row.push_str(&format!(" {}", format_frequency(*freq, &config.freq_mode)));
                }
                if !compact {
                    if let Some((_, temp, _)) = snapshot
                        .cpu
                        .core_temperatures_c
                        .iter()
                        .find(|(core, _, _)| core == index)
                    {
                        let temp = if config.temp_scale == "fahrenheit" {
                            temp * 1.8 + 32.0
                        } else {
                            *temp
                        };
                        row.push_str(&format!(
                            " {temp:.0}°{}",
                            if config.temp_scale == "fahrenheit" {
                                "F"
                            } else {
                                "C"
                            }
                        ));
                    }
                }
                row
            })
            .collect::<Vec<_>>()
            .join("  ");
        lines.push(ratatui::text::Line::from(text));
    }
    let text_area = Rect::new(
        area.x,
        area.y + graph_rows,
        area.width,
        area.height - graph_rows,
    );
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(colors.start)),
        text_area,
    );
    if area.width >= 100 {
        let graph_x = area.x + (area.width * 2 / 3).max(20);
        let graph_width = area.right().saturating_sub(graph_x);
        let details_rows = u16::from(!details.is_empty());
        for (row_index, group) in cores
            .iter()
            .enumerate()
            .collect::<Vec<_>>()
            .chunks(per_row.max(1))
            .enumerate()
        {
            let y = area.y + graph_rows + details_rows + row_index as u16;
            if y >= area.bottom() {
                break;
            }
            let segment_width = graph_width / group.len().max(1) as u16;
            for (group_index, (core_index, _)) in group.iter().enumerate() {
                let segment_x = graph_x + segment_width * group_index as u16;
                if segment_width <= 3 {
                    continue;
                }
                let usage: Vec<f32> = history
                    .iter()
                    .filter_map(|sample| sample.cores.get(*core_index).map(|core| core.total))
                    .collect();
                let usage_width = segment_width / 2;
                frame.render_widget(
                    crate::ui::plot::Graph::new(&usage).gradient(&chart_gradient),
                    Rect::new(segment_x, y, usage_width, 1),
                );
                let temps: Vec<f32> = history
                    .iter()
                    .filter_map(|sample| {
                        sample
                            .core_temperatures_c
                            .iter()
                            .find(|(index, _, _)| index == core_index)
                            .map(|(_, temp, crit)| {
                                if *crit > 0.0 {
                                    temp * 100.0 / crit
                                } else {
                                    *temp
                                }
                            })
                    })
                    .collect();
                if !temps.is_empty() && usage_width < segment_width {
                    frame.render_widget(
                        crate::ui::plot::Graph::new(&temps).gradient(&temp_gradient),
                        Rect::new(segment_x + usage_width, y, segment_width - usage_width, 1),
                    );
                }
            }
        }
    }
}

fn format_frequency(mhz: u64, mode: &str) -> String {
    match mode {
        "mhz" => format!("{mhz}MHz"),
        "ghz" => format!("{:.1}GHz", mhz as f64 / 1000.0),
        _ if mhz >= 1000 => format!("{:.1}GHz", mhz as f64 / 1000.0),
        _ => format!("{mhz}MHz"),
    }
}

fn ratatui_toml_value_u64(value: u64) -> toml::Value {
    toml::Value::Integer(value.min(i64::MAX as u64) as i64)
}

fn clock_text() -> String {
    chrono::Local::now().format("%X").to_string()
}
