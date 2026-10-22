use ratatui::layout::Rect;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::ui::app) struct ShownBoxes {
    pub cpu: bool,
    pub mem: bool,
    pub net: bool,
    pub agents: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::ui::app) struct LayoutOptions {
    pub cpu_bottom: bool,
    pub mem_below_net: bool,
    pub agents_left: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::ui::app) struct BoxRects {
    pub cpu: Rect,
    pub mem: Rect,
    pub net: Rect,
    pub agents: Rect,
    pub too_small: Option<u16>,
}

#[derive(Debug, Clone)]
pub(in crate::ui::app) struct ResourcesState {
    pub shown: ShownBoxes,
    pub opts: LayoutOptions,
    pub rects: BoxRects,
    pub interval_ms: u64,
    pub preset: usize,
    pub presets: Vec<String>,
    pub hits: ResourceHits,
    pub menu_open: bool,
    pub help_open: bool,
    pub show_disks: bool,
    pub io_mode: bool,
    pub net_auto: bool,
    pub net_sync: bool,
    pub net_download: u64,
    pub net_upload: u64,
    pub net_iface: Option<String>,
    pub swap_upload_download: bool,
    pub net_zeroed: bool,
    pub net_zero_offsets: std::collections::HashMap<String, (u64, u64)>,
    pub net_download_scaler: crate::ui::resources::net::NetScaler,
    pub net_upload_scaler: crate::ui::resources::net::NetScaler,
    pub agent_sort: AgentSort,
    pub agent_reverse: bool,
    pub agent_tree: bool,
    pub agent_filter: Option<String>,
    pub agent_filter_editing: bool,
    pub agent_selected: Option<AgentIdentity>,
    pub agent_follow: Option<AgentIdentity>,
    pub agent_detail: bool,
    pub agent_signal_armed: Option<AgentSignalConfirmation>,
    pub agent_process_owned_fn: fn(u32, u64, Option<&str>) -> bool,
    pub agent_signal_fn: fn(u32, u64, Option<&str>, AgentSignal) -> std::io::Result<()>,
    pub agent_cpu_history: std::collections::HashMap<AgentIdentity, Vec<f64>>,
    pub agent_last_sample: Option<std::time::Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::ui::app) struct AgentIdentity {
    pub host: Option<String>,
    pub pid: u32,
    pub started: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub(in crate::ui::app) struct ResourceHits {
    pub cpu_toggle: Rect,
    pub mem_toggle: Rect,
    pub net_toggle: Rect,
    pub agents_toggle: Rect,
    pub disks_toggle: Rect,
    pub io_toggle: Rect,
    pub net_prev: Rect,
    pub net_next: Rect,
    pub net_zero: Rect,
    pub net_auto: Rect,
    pub net_sync: Rect,
    pub menu: Rect,
    pub preset_prev: Rect,
    pub preset_next: Rect,
    pub interval_down: Rect,
    pub interval_up: Rect,
}

impl Default for ResourcesState {
    fn default() -> Self {
        Self {
            shown: ShownBoxes {
                cpu: true,
                mem: true,
                net: true,
                agents: true,
            },
            opts: LayoutOptions::default(),
            rects: BoxRects::default(),
            interval_ms: 2_000,
            preset: 0,
            presets: vec!["cpu:0:default,mem:0:default,net:0:default,agents:1:default".into()],
            hits: ResourceHits::default(),
            menu_open: false,
            help_open: false,
            show_disks: true,
            io_mode: false,
            net_auto: true,
            net_sync: false,
            net_download: 100,
            net_upload: 100,
            net_iface: None,
            swap_upload_download: false,
            net_zeroed: false,
            net_zero_offsets: std::collections::HashMap::new(),
            net_download_scaler: Default::default(),
            net_upload_scaler: Default::default(),
            agent_sort: AgentSort::Cpu,
            agent_reverse: false,
            agent_tree: false,
            agent_filter: None,
            agent_filter_editing: false,
            agent_selected: None,
            agent_follow: None,
            agent_detail: false,
            agent_signal_armed: None,
            agent_process_owned_fn: process_owned_by_current_user,
            agent_signal_fn: send_process_signal,
            agent_cpu_history: std::collections::HashMap::new(),
            agent_last_sample: None,
        }
    }
}

impl ResourcesState {
    pub fn sample_network(&mut self, snapshot: &crate::ui::resources::sampler::ResourceSnapshot) {
        let selected = self
            .net_iface
            .as_deref()
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
        if let Some(selected) = selected {
            self.net_download_scaler
                .sample(selected.received_bytes_per_second);
            self.net_upload_scaler
                .sample(selected.transmitted_bytes_per_second);
        }
    }

    pub fn from_config(config: &medulla::config::ResourcesConfig) -> Self {
        let shown = config
            .shown_boxes
            .split(',')
            .map(str::trim)
            .collect::<Vec<_>>();
        let parsed = config
            .presets
            .split_whitespace()
            .filter(|preset| valid_preset(preset))
            .map(str::to_owned)
            .take(10)
            .collect::<Vec<_>>();
        let presets = if parsed.is_empty() {
            Self::default().presets
        } else {
            parsed
        };
        let mut state = Self {
            shown: ShownBoxes {
                cpu: shown.contains(&"cpu"),
                mem: shown.contains(&"mem"),
                net: shown.contains(&"net"),
                agents: shown.contains(&"agents"),
            },
            opts: LayoutOptions::default(),
            interval_ms: config.update_interval_ms(),
            show_disks: config.show_disks,
            io_mode: config.io_mode,
            net_auto: config.net_auto,
            net_sync: config.net_sync,
            net_download: config.net_download.max(1),
            net_upload: config.net_upload.max(1),
            net_iface: config.net_iface.clone().filter(|name| !name.is_empty()),
            swap_upload_download: config.swap_upload_download,
            net_zeroed: false,
            presets,
            ..Self::default()
        };
        state.preset = config.preset.min(state.presets.len().saturating_sub(1));
        state.apply_preset();
        // `shownBoxes` is an explicit startup choice. Interactive preset
        // changes replace visibility with the boxes listed by that preset.
        state.shown = ShownBoxes {
            cpu: shown.contains(&"cpu"),
            mem: shown.contains(&"mem"),
            net: shown.contains(&"net"),
            agents: shown.contains(&"agents"),
        };
        if let Some(value) = config.cpu_bottom {
            state.opts.cpu_bottom = value;
        }
        if let Some(value) = config.mem_below_net {
            state.opts.mem_below_net = value;
        }
        if let Some(value) = config.agents_left {
            state.opts.agents_left = value;
        }
        state
    }

    pub fn apply_preset(&mut self) {
        let Some(preset) = self.presets.get(self.preset) else {
            return;
        };
        let (mut cpu_bottom, mut mem_below_net, mut agents_left) = (false, false, false);
        let mut shown = ShownBoxes::default();
        for item in preset.split(',') {
            let mut parts = item.split(':');
            let (Some(name), Some(position), Some(_graph)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            match name {
                "cpu" => {
                    shown.cpu = true;
                    cpu_bottom = position == "1";
                }
                "mem" => {
                    shown.mem = true;
                    mem_below_net = position == "1";
                }
                "net" => shown.net = true,
                "agents" => agents_left = position == "0",
                _ => {}
            }
            if name == "agents" {
                shown.agents = true;
            }
        }
        self.opts = LayoutOptions {
            cpu_bottom,
            mem_below_net,
            agents_left,
        };
        self.shown = shown;
    }
}

fn valid_preset(preset: &str) -> bool {
    let mut seen = std::collections::HashSet::new();
    let mut any = false;
    for item in preset.split(',') {
        let mut parts = item.split(':');
        let (Some(name), Some(position), Some(graph)) = (parts.next(), parts.next(), parts.next())
        else {
            return false;
        };
        if parts.next().is_some()
            || !["cpu", "mem", "net", "agents"].contains(&name)
            || !seen.insert(name)
            || !["0", "1"].contains(&position)
            || !["default", "braille", "block", "tty"].contains(&graph)
        {
            return false;
        }
        any = true;
    }
    any
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::ui::app) enum AgentSort {
    Pid,
    Kind,
    Origin,
    Workspace,
    Threads,
    Memory,
    #[default]
    Cpu,
    Runtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::ui::app) enum AgentSignal {
    Terminate,
    Kill,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::ui::app) enum AgentSignalConfirmStage {
    First,
    ExternalSecond,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::ui::app) struct AgentSignalConfirmation {
    pub pid: u32,
    pub started: u64,
    pub signal: AgentSignal,
    pub stage: AgentSignalConfirmStage,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn current_user_uid() -> String {
    // SAFETY: geteuid has no preconditions and returns the current effective uid.
    unsafe { libc::geteuid() }.to_string()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn process_owned_by_current_user(pid: u32, started: u64, sampled_uid: Option<&str>) -> bool {
    let Some(sampled_uid) = sampled_uid else {
        return false;
    };
    let Some((current_started, current_uid)) =
        medulla_monitor::agents::current_process_identity(pid)
    else {
        return false;
    };
    current_started == started && current_uid == sampled_uid && current_uid == current_user_uid()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_owned_by_current_user(_pid: u32, _started: u64, _sampled_uid: Option<&str>) -> bool {
    false
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn send_process_signal(
    pid: u32,
    started: u64,
    sampled_uid: Option<&str>,
    signal: AgentSignal,
) -> std::io::Result<()> {
    if !process_owned_by_current_user(pid, started, sampled_uid) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "process identity or owner changed before signal",
        ));
    }
    let signal = match signal {
        AgentSignal::Terminate => libc::SIGTERM,
        AgentSignal::Kill => libc::SIGKILL,
        AgentSignal::Stop => libc::SIGSTOP,
    };
    // SAFETY: kill only receives the integer pid and signal values validated above.
    if unsafe { libc::kill(pid as libc::pid_t, signal) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn send_process_signal(
    _pid: u32,
    _started: u64,
    _sampled_uid: Option<&str>,
    _signal: AgentSignal,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "process signals are unsupported on this platform",
    ))
}
