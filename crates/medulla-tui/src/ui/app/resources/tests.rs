use super::{
    draw_net_panel,
    layout::calc_sizes,
    types::{
        AgentIdentity, AgentSignal, AgentSignalConfirmStage, AgentSignalConfirmation,
        LayoutOptions, ResourcesState, ShownBoxes,
    },
};
use crate::ui::app::App;
use crossterm::event::KeyCode;
use crossterm::event::{KeyEvent, KeyModifiers};
use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use ratatui::layout::Rect;
use ratatui::{backend::TestBackend, Terminal};
use std::sync::Arc;

fn all() -> ShownBoxes {
    ShownBoxes {
        cpu: true,
        mem: true,
        net: true,
        agents: true,
    }
}

#[test]
fn default_layout_uses_btop_shares_at_common_terminal_sizes() {
    for (area, cpu_h, mem_w, mem_h, net_h, agents_w) in [
        (Rect::new(0, 0, 170, 42), 14, 77, 16, 12, 93),
        (Rect::new(0, 0, 80, 24), 8, 36, 9, 7, 44),
    ] {
        let boxes = calc_sizes(area, all(), &LayoutOptions::default());
        assert_eq!(boxes.cpu.height, cpu_h);
        assert_eq!(boxes.mem.width, mem_w);
        assert_eq!(boxes.mem.height, mem_h);
        assert_eq!(boxes.net.height, net_h);
        assert_eq!(boxes.agents.width, agents_w);
        assert_eq!(
            boxes.mem.height + boxes.net.height,
            area.height - boxes.cpu.height
        );
    }
}

#[test]
fn hidden_agents_widens_memory_and_network_and_cpu_alone_fills_area() {
    let mut shown = all();
    shown.agents = false;
    let boxes = calc_sizes(Rect::new(0, 0, 170, 42), shown, &LayoutOptions::default());
    assert_eq!(boxes.mem.width, 170);
    assert_eq!(boxes.net.width, 170);
    assert_eq!(boxes.mem.height, 16);
    assert_eq!(boxes.net.height, 12);
    assert_eq!(boxes.mem.height + boxes.net.height, 42 - boxes.cpu.height);
    shown = ShownBoxes {
        cpu: true,
        ..Default::default()
    };
    let boxes = calc_sizes(Rect::new(0, 0, 170, 42), shown, &LayoutOptions::default());
    assert_eq!(boxes.cpu, Rect::new(0, 0, 170, 42));
}

#[test]
fn every_visible_box_combination_has_a_rect_or_a_too_small_result() {
    for area in [Rect::new(0, 0, 170, 42), Rect::new(0, 0, 80, 24)] {
        for mask in 0..16 {
            let shown = ShownBoxes {
                cpu: mask & 1 != 0,
                mem: mask & 2 != 0,
                net: mask & 4 != 0,
                agents: mask & 8 != 0,
            };
            let boxes = calc_sizes(area, shown, &LayoutOptions::default());
            let every_visible_box_has_area = [
                (shown.cpu, boxes.cpu),
                (shown.mem, boxes.mem),
                (shown.net, boxes.net),
                (shown.agents, boxes.agents),
            ]
            .into_iter()
            .all(|(shown, rect)| !shown || (rect.width > 0 && rect.height > 0));
            assert!(
                boxes.too_small.is_some() || every_visible_box_has_area,
                "shown boxes {shown:?} have an unplaced panel at {area:?}: {boxes:?}"
            );
        }
    }
}

#[test]
fn cpu_hidden_gives_mem_seventy_two_percent_and_net_the_remaining_share() {
    for agents in [false, true] {
        let shown = ShownBoxes {
            cpu: false,
            mem: true,
            net: true,
            agents,
        };
        let boxes = calc_sizes(Rect::new(0, 0, 170, 42), shown, &LayoutOptions::default());
        assert_eq!(boxes.mem.height, 30);
        assert_eq!(boxes.net.height, 12);
        assert_eq!(boxes.mem.height + boxes.net.height, 42);
        if agents {
            assert_eq!(boxes.mem.width, 77);
            assert_eq!(boxes.agents.width, 93);
        } else {
            assert_eq!(boxes.mem.width, 170);
            assert_eq!(boxes.net.width, 170);
        }
    }
}

#[test]
fn undersized_terminal_requests_too_small_message() {
    let boxes = calc_sizes(Rect::new(0, 0, 50, 7), all(), &LayoutOptions::default());
    assert_eq!(boxes.too_small, Some(60));
}

#[test]
fn agents_take_the_full_remaining_area_before_mem_and_net_are_suppressed() {
    let boxes = calc_sizes(Rect::new(0, 0, 70, 24), all(), &LayoutOptions::default());
    assert_eq!(boxes.agents.width, 70);
    assert_eq!(boxes.mem.width, 0);
    assert_eq!(boxes.net.width, 0);
}

#[test]
fn resource_keys_toggle_boxes_cycle_presets_and_clamp_interval() {
    let mut state = ResourcesState::default();
    for (key, expected) in [
        (KeyCode::Char('1'), false),
        (KeyCode::Char('2'), false),
        (KeyCode::Char('3'), false),
        (KeyCode::Char('4'), false),
    ] {
        assert!(state.key(key));
        assert_eq!(
            [
                state.shown.cpu,
                state.shown.mem,
                state.shown.net,
                state.shown.agents
            ]["1234"
                .chars()
                .position(|c| c
                    == match key {
                        KeyCode::Char(c) => c,
                        _ => ' ',
                    })
                .unwrap()],
            expected
        );
    }
    state.presets.push("cpu:1:block,agents:0:tty".into());
    assert!(state.key(KeyCode::Char('P')));
    assert_eq!(state.preset, 1);
    state.interval_ms = 9_950;
    state.key(KeyCode::Char('+'));
    assert_eq!(state.interval_ms, 10_000);
    state.interval_ms = 150;
    state.key(KeyCode::Char('-'));
    assert_eq!(state.interval_ms, 100);
}

#[test]
fn disks_and_io_keys_toggle_the_memory_panel_modes() {
    let mut state = ResourcesState::default();
    assert!(state.show_disks && !state.io_mode);
    assert!(state.key(KeyCode::Char('d')));
    assert!(!state.show_disks);
    assert!(state.key(KeyCode::Char('i')));
    assert!(state.io_mode);
    assert!(state.key(KeyCode::Char('i')));
    assert!(!state.io_mode);
}

#[test]
fn io_graph_speed_limits_override_per_mount_in_mib_per_second() {
    let config = medulla::config::ResourcesConfig {
        io_graph_speeds: "/mnt/data:1 /boot:4".into(),
        ..Default::default()
    };
    assert_eq!(super::io_graph_speed(&config, "/mnt/data"), 1.0);
    assert_eq!(super::io_graph_speed(&config, "/boot"), 4.0);
    assert_eq!(super::io_graph_speed(&config, "/"), 100.0);
}

#[test]
fn selecting_a_preset_shows_only_the_boxes_named_by_its_entries() {
    let mut state = ResourcesState {
        presets: vec![
            "cpu:0:default,mem:0:default,net:0:default,agents:1:default".into(),
            "cpu:1:block,net:0:tty".into(),
        ],
        ..ResourcesState::default()
    };
    assert!(state.key(KeyCode::Char('P')));
    assert_eq!(
        state.shown,
        ShownBoxes {
            cpu: true,
            mem: false,
            net: true,
            agents: false,
        }
    );
}

#[test]
fn tab_keys_still_cycle_all_tabs() {
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("test.json".into()),
    );
    for _ in 0..crate::ui::app::TABS
        .iter()
        .position(|tab| *tab == "Resources")
        .unwrap()
    {
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    }
    assert_eq!(app.tab(), "Resources");
    app.on_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
    assert!(!app.resources.shown.cpu);
    assert_eq!(app.loaded.config.resources.update_ms, 2_000);
    app.on_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));
    assert_eq!(app.loaded.config.resources.update_ms, 2_100);
    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let resources_index = crate::ui::app::TABS
        .iter()
        .position(|tab| *tab == "Resources")
        .unwrap();
    assert_eq!(
        app.tab(),
        crate::ui::app::TABS[(resources_index + 1) % crate::ui::app::TABS.len()]
    );
}

#[test]
fn resources_draw_empty_titled_panels_until_data_views_land() {
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("test.json".into()),
    );
    let backend = TestBackend::new(170, 42);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| app.draw_resources(frame, Rect::new(0, 0, 170, 42)))
        .unwrap();
    let text = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(text.contains("CPU [1]"));
    assert!(text.contains("Mem [2]"));
    assert!(text.contains("Net [3]"));
    assert!(text.contains("Agents [4]"));
}

#[test]
fn network_panel_renders_at_desktop_and_narrow_widths() {
    let config = medulla::config::ResourcesConfig::default();
    let mut snapshot = crate::ui::resources::sampler::ResourceSnapshot::default();
    snapshot
        .networks
        .push(crate::ui::resources::sampler::NetworkSnapshot {
            interface: "eth0".into(),
            up: true,
            connected: true,
            ip_addresses: vec!["192.0.2.2".into()],
            received_bytes_per_second: 4096.0,
            transmitted_bytes_per_second: 2048.0,
            total_received_bytes: 8192,
            total_transmitted_bytes: 4096,
        });
    let mut history = crate::ui::resources::sampler::ResourceHistory::default();
    history.push(&snapshot, 8);
    let snapshot = snapshot.with_history(Arc::new(history));
    for width in [170, 45] {
        let mut terminal = Terminal::new(TestBackend::new(width, 42)).unwrap();
        terminal
            .draw(|frame| {
                draw_net_panel(
                    frame,
                    Rect::new(0, 0, width, 8),
                    &snapshot,
                    &config,
                    Some("eth0"),
                    (0, 0),
                    10_240.0,
                    10_240.0,
                    false,
                )
            })
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("eth0"), "interface absent at width {width}");
        assert!(text.contains("Total"), "totals absent at width {width}");
        if width == 170 {
            assert!(
                text.contains("Top ↓4.0K ↑2.0K"),
                "directional receive/transmit peaks absent: {text}"
            );
            let mut swapped = config.clone();
            swapped.swap_upload_download = true;
            terminal
                .draw(|frame| {
                    draw_net_panel(
                        frame,
                        Rect::new(0, 0, width, 8),
                        &snapshot,
                        &swapped,
                        Some("eth0"),
                        (0, 0),
                        10_240.0,
                        10_240.0,
                        false,
                    )
                })
                .unwrap();
            let swapped_text = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(
                swapped_text.contains("Top ↓2.0K ↑4.0K  Total ↓4.0K ↑8.0K"),
                "swapped top/totals must follow displayed directions: {swapped_text}"
            );
        }
    }
}

#[test]
fn network_zero_offsets_totals_and_interface_keys_skip_down_links() {
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("test.json".into()),
    );
    let mut sample = crate::ui::resources::sampler::ResourceSnapshot::default();
    for (interface, up, rx) in [
        ("lo", true, 20),
        ("eth0", false, 40),
        ("wlan0", true, 100),
        ("enp1s0", true, 300),
    ] {
        sample
            .networks
            .push(crate::ui::resources::sampler::NetworkSnapshot {
                interface: interface.into(),
                up,
                connected: up && interface != "lo",
                ip_addresses: Vec::new(),
                received_bytes_per_second: 0.0,
                transmitted_bytes_per_second: 0.0,
                total_received_bytes: rx,
                total_transmitted_bytes: rx * 2,
            });
    }
    app.set_resource_snapshot(Arc::new(sample.clone()));
    app.resources.net_iface = Some("wlan0".into());
    assert!(app.resources_key(KeyCode::Char('z')));
    assert_eq!(app.resources.net_zero_offsets["wlan0"], (100, 200));
    let mut later = sample.clone();
    let wlan = later
        .networks
        .iter_mut()
        .find(|entry| entry.interface == "wlan0")
        .unwrap();
    wlan.total_received_bytes += 10;
    wlan.total_transmitted_bytes += 20;
    app.set_resource_snapshot(Arc::new(later));
    let mut terminal = Terminal::new(TestBackend::new(170, 42)).unwrap();
    terminal
        .draw(|frame| app.draw_resources(frame, Rect::new(0, 0, 170, 42)))
        .unwrap();
    let text = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(
        text.contains("Total ↓10B ↑20B"),
        "zeroed counters must continue from their offset: {text}"
    );
    assert!(app.resources_key(KeyCode::Char('n')));
    assert_eq!(app.resources.net_iface.as_deref(), Some("enp1s0"));
    assert!(app.resources_key(KeyCode::Char('z')));
    assert_eq!(app.resources.net_zero_offsets["enp1s0"], (300, 600));
    assert!(app.resources_key(KeyCode::Char('b')));
    assert_eq!(app.resources.net_iface.as_deref(), Some("wlan0"));
    assert_eq!(app.resources.net_zero_offsets["wlan0"], (100, 200));
}

#[test]
fn network_border_auto_and_sync_controls_are_clickable() {
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("test.json".into()),
    );
    let mut terminal = Terminal::new(TestBackend::new(170, 42)).unwrap();
    terminal
        .draw(|frame| app.draw_resources(frame, Rect::new(0, 0, 170, 42)))
        .unwrap();
    assert!(app.resources.net_auto);
    let auto = app.resources.hits.net_auto;
    app.resources_click(auto.x, auto.y);
    assert!(!app.resources.net_auto);
    let sync = app.resources.hits.net_sync;
    app.resources_click(sync.x, sync.y);
    assert!(app.resources.net_sync);
}

#[test]
fn memory_and_disk_panel_renders_at_desktop_and_compact_terminal_sizes() {
    let mut loaded = LoadedConfig::defaults("test.json".into());
    loaded.config.resources.shown_boxes = "mem".into();
    let mut history = crate::ui::resources::sampler::ResourceHistory::default();
    let mut sample = crate::ui::resources::sampler::ResourceSnapshot::default();
    sample.memory_total_bytes = 16_000;
    sample.memory_used_bytes = 8_000;
    sample.memory_available_bytes = 8_000;
    sample.memory_cached_bytes = 2_000;
    sample.memory_free_bytes = 1_000;
    sample.swap_total_bytes = 4_000;
    sample.swap_used_bytes = 500;
    sample.disks = vec![crate::ui::resources::sampler::DiskSnapshot {
        device: "/dev/sda1".into(),
        mount: "/mnt/data".into(),
        total_bytes: 10_000,
        used_bytes: 2_500,
        read_bytes_per_second: (32 * 1024 * 1024) as f64,
        write_bytes_per_second: (64 * 1024 * 1024) as f64,
        busy_percent: Some(15.0),
    }];
    history.push(&sample, 8);
    let sample = sample.with_history(Arc::new(history));
    for (width, height) in [(170, 42), (80, 24), (60, 16)] {
        for (mem_graphs, show_disks, io_mode, combined, speeds) in [
            (true, true, false, false, ""),
            (false, true, false, false, ""),
            (true, true, true, true, ""),
            (true, true, true, false, "/mnt/data:1"),
            (true, false, false, false, ""),
            (true, false, true, false, ""),
        ] {
            let mut configured = loaded.clone();
            configured.config.resources.mem_graphs = mem_graphs;
            configured.config.resources.show_disks = show_disks;
            configured.config.resources.io_mode = io_mode;
            configured.config.resources.io_graph_combined = combined;
            configured.config.resources.io_graph_speeds = speeds.into();
            configured.config.resources.swap_disk = true;
            let show_swap = configured.config.resources.show_swap;
            let mut app = App::new(Arc::new(MockRuntime::empty()), configured);
            app.set_resource_snapshot(Arc::new(sample.clone()));
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| app.draw_resources(frame, Rect::new(0, 0, width, height)))
                .unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(
                text.contains("RAM  50%"),
                "RAM missing at {width}x{height}: {text}"
            );
            let show_disk_rows = show_disks && !io_mode;
            assert_eq!(
                text.contains("/mnt/data"),
                show_disk_rows,
                "disk visibility at {width}x{height}: {text}"
            );
            assert!(
                text.contains("Swap  12%"),
                "swap missing at {width}x{height}: {text}"
            );
            assert_eq!(
                text.contains("[swap]"),
                show_disk_rows,
                "swap disk visibility at {width}x{height}: {text}"
            );
            if mem_graphs && (!io_mode || !show_disks) {
                let mem = app.resources.rects.mem;
                let wanted_graph_rows = if show_swap { 5 } else { 4 };
                let graph_rows = wanted_graph_rows.min(mem.height.saturating_sub(2) as usize);
                let visible_graph_rows = (mem.y + 1..mem.y + 1 + graph_rows as u16)
                    .filter(|y| {
                        (mem.x + 1..mem.right() - 1)
                            .any(|x| terminal.backend().buffer()[(x, *y)].symbol() != " ")
                    })
                    .count();
                assert_eq!(
                    visible_graph_rows, graph_rows,
                    "memory series missing at {width}x{height}"
                );
                for label in if show_swap {
                    ["USED", "AVAIL", "CACHE", "FREE", "SWAP"].as_slice()
                } else {
                    ["USED", "AVAIL", "CACHE", "FREE"].as_slice()
                } {
                    assert!(
                        text.contains(label),
                        "memory history series {label} missing at {width}x{height}: {text}"
                    );
                }
            }
            if !mem_graphs && !io_mode {
                assert!(
                    text.contains('█'),
                    "RAM meters missing at {width}x{height}: {text}"
                );
                for meter in ["U██░░", "A██░░", "C█░░░", "F░░░░"] {
                    assert!(
                        text.contains(meter),
                        "memory meter {meter} missing at {width}x{height}: {text}"
                    );
                }
            }
            if io_mode && show_disks {
                assert!(
                    text.contains(if combined { "IO READ+WRITE" } else { "IO READ" }),
                    "IO graph labels missing at {width}x{height}: {text}"
                );
                let mem = app.resources.rects.mem;
                let graph_y = mem.y + 1;
                assert!(
                    (graph_y..graph_y + 4).any(|y| {
                        (mem.x + 1..mem.right() - 1)
                            .any(|x| terminal.backend().buffer()[(x, y)].symbol() != " ")
                    }),
                    "IO graph missing at {width}x{height}"
                );
            } else if show_disk_rows {
                let mem = app.resources.rects.mem;
                let inner_y = mem.y + 1;
                let graph_rows = if mem_graphs { 5 } else { 0 };
                let disk_start = if mem_graphs { 3 } else { 2 };
                let disk_graph_y = inner_y + graph_rows + disk_start;
                let graph_x = mem.x + 1 + (mem.width - 2) * 3 / 4;
                assert!(
                    (graph_x..mem.right() - 1)
                        .any(|x| terminal.backend().buffer()[(x, disk_graph_y)].symbol() != " "),
                    "per-disk busy graph missing at {width}x{height}"
                );
            }
        }
    }
}

#[test]
fn clicking_a_panel_title_toggles_that_box() {
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("test.json".into()),
    );
    app.tab_index = crate::ui::app::TABS
        .iter()
        .position(|tab| *tab == "Resources")
        .unwrap();
    app.resources.rects = calc_sizes(Rect::new(0, 5, 170, 42), all(), &LayoutOptions::default());
    let cpu = app.resources.rects.cpu;
    app.resources.hits.cpu_toggle = Rect::new(cpu.x + 2, cpu.y, 3, 1);
    app.handle_click(cpu.x + 2, cpu.y);
    assert!(!app.resources.shown.cpu);
}

#[test]
fn cpu_title_controls_have_distinct_click_actions() {
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("test.json".into()),
    );
    app.tab_index = crate::ui::app::TABS
        .iter()
        .position(|tab| *tab == "Resources")
        .unwrap();
    let mut terminal = Terminal::new(TestBackend::new(190, 52)).unwrap();
    terminal
        .draw(|frame| app.draw_resources(frame, Rect::new(20, 5, 170, 42)))
        .unwrap();
    let menu = app.resources.hits.menu;
    let cpu_toggle = app.resources.hits.cpu_toggle;
    let cpu = app.resources.rects.cpu;
    let buffer = terminal.backend().buffer();
    for (hit, x_offset, symbol) in [
        (cpu_toggle, 6, "["),
        (menu, 10, "["),
        (app.resources.hits.preset_prev, 14, "["),
        (app.resources.hits.preset_next, 18, "["),
        (app.resources.hits.interval_down, 22, "["),
        (app.resources.hits.interval_up, 26, "["),
    ] {
        let x = cpu.x + x_offset;
        assert_eq!(buffer[(x, cpu.y)].symbol(), symbol);
        assert!(
            hit.contains((x, cpu.y).into()),
            "visible control at x={x} misses hit rect {hit:?}"
        );
    }
    assert!(menu.width > 0);
    assert!(cpu_toggle.width > 0);
    assert!(!menu.intersects(cpu_toggle));
    app.handle_click(cpu.x + 10, cpu.y);
    assert!(app.resources.menu_open);
    assert!(app.resources.shown.cpu);
    app.handle_click(cpu.x + 6, cpu.y);
    assert!(!app.resources.shown.cpu);
}

#[test]
fn cpu_panel_renders_sixteen_cores_without_clipping_at_common_sizes() {
    let mut cpu = medulla_monitor::CpuSnapshot {
        model: Some("Test CPU".to_owned()),
        cores: (0..16)
            .map(|index| medulla_monitor::CpuReading {
                total: index as f32,
                ..Default::default()
            })
            .collect(),
        frequencies_mhz: (0..16).map(|index| (index != 0).then_some(2400)).collect(),
        load_average: [0.5, 0.25, 0.1],
        uptime_seconds: Some(1234),
        ..Default::default()
    };
    let mut history = crate::ui::resources::sampler::ResourceHistory::default();
    let mut historical_cpu = cpu.clone();
    historical_cpu.core_temperatures_c = (0..16).map(|index| (index, 50.0, 90.0)).collect();
    history.push(
        &crate::ui::resources::sampler::ResourceSnapshot::default().with_cpu(historical_cpu),
        20,
    );
    let snapshot = crate::ui::resources::sampler::ResourceSnapshot::default()
        .with_cpu(std::mem::take(&mut cpu))
        .with_history(Arc::new(history));
    for (width, height) in [(170, 42), (80, 24)] {
        let mut app = App::new(
            Arc::new(MockRuntime::empty()),
            LoadedConfig::defaults("test.json".into()),
        );
        app.set_resource_snapshot(Arc::new(snapshot.clone()));
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| app.draw_resources(frame, Rect::new(0, 0, width, height)))
            .unwrap();
        let cpu_rect = app.resources.rects.cpu;
        let buffer = terminal.backend().buffer();
        let text = (cpu_rect.y..cpu_rect.bottom())
            .flat_map(|y| (cpu_rect.x..cpu_rect.right()).map(move |x| buffer[(x, y)].symbol()))
            .collect::<String>();
        for core in 0..16 {
            assert!(
                text.contains(&format!("C{core} ")),
                "missing core {core} at {width}x{height}: {text}"
            );
        }
        assert!(text.contains("2.4GHz"));
        if width >= 100 {
            assert!(text.contains("Test CPU"));
            assert!(text.contains("Load avg 0.50 0.25 0.10"));
            let graph_row = cpu_rect.y + 1 + 2 + 1;
            let first_cpu_text = (cpu_rect.x + 1..cpu_rect.x + 100)
                .map(|x| buffer[(x, graph_row)].symbol())
                .collect::<String>();
            assert!(first_cpu_text.contains("C0"));
            assert!(first_cpu_text.contains("C1"));
            assert!(first_cpu_text.contains("2.4GHz"));
            assert!(
                (cpu_rect.x + 100..cpu_rect.right() - 1)
                    .any(|x| buffer[(x, graph_row)].symbol() != " "),
                "missing per-core graphs at {width}x{height}"
            );
        } else {
            assert!(
                text.contains("Load avg 0.5"),
                "missing compact load at {width}x{height}: {text}"
            );
        }
        assert!(!text.contains("Pkg"));
        assert!(!text.contains("°C"));
    }
}

#[test]
fn cpu_panel_shows_zero_load_average_with_required_label() {
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("test.json".into()),
    );
    let snapshot = crate::ui::resources::sampler::ResourceSnapshot::default().with_cpu(
        medulla_monitor::CpuSnapshot {
            load_average: [0.0; 3],
            ..Default::default()
        },
    );
    app.set_resource_snapshot(Arc::new(snapshot));
    let mut terminal = Terminal::new(TestBackend::new(170, 42)).unwrap();
    terminal
        .draw(|frame| app.draw_resources(frame, Rect::new(0, 0, 170, 42)))
        .unwrap();
    let cpu = app.resources.rects.cpu;
    let buffer = terminal.backend().buffer();
    let text = (cpu.y..cpu.bottom())
        .flat_map(|y| (cpu.x..cpu.right()).map(move |x| buffer[(x, y)].symbol()))
        .collect::<String>();
    assert!(text.contains("Load avg 0.00 0.00 0.00"));
}

#[test]
fn cpu_frequency_mode_formats_mhz_ghz_and_auto() {
    assert_eq!(super::format_frequency(800, "auto"), "800MHz");
    assert_eq!(super::format_frequency(2400, "auto"), "2.4GHz");
    assert_eq!(super::format_frequency(2400, "mhz"), "2400MHz");
    assert_eq!(super::format_frequency(800, "ghz"), "0.8GHz");
}

#[test]
fn each_panel_title_button_toggles_only_its_box() {
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("test.json".into()),
    );
    app.tab_index = crate::ui::app::TABS
        .iter()
        .position(|tab| *tab == "Resources")
        .unwrap();
    let mut terminal = Terminal::new(TestBackend::new(190, 52)).unwrap();
    terminal
        .draw(|frame| app.draw_resources(frame, Rect::new(20, 5, 170, 42)))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let targets = [
        (
            app.resources.rects.cpu,
            app.resources.hits.cpu_toggle,
            6,
            '1',
        ),
        (
            app.resources.rects.mem,
            app.resources.hits.mem_toggle,
            6,
            '2',
        ),
        (
            app.resources.rects.net,
            app.resources.hits.net_toggle,
            6,
            '3',
        ),
        (
            app.resources.rects.agents,
            app.resources.hits.agents_toggle,
            9,
            '4',
        ),
    ];
    for (panel, hit, offset, key) in targets {
        assert!(hit.width > 0);
        let x = panel.x + offset;
        assert_eq!(buffer[(x, panel.y)].symbol(), "[");
        assert!(
            hit.contains((x, panel.y).into()),
            "visible [{key}] misses hit rect {hit:?}"
        );
        app.handle_click(x, panel.y);
        let shown = match key {
            '1' => app.resources.shown.cpu,
            '2' => app.resources.shown.mem,
            '3' => app.resources.shown.net,
            _ => app.resources.shown.agents,
        };
        assert!(!shown, "clicking [{key}] should hide its own box");
    }
}

#[test]
fn cpu_title_preset_and_interval_buttons_are_clickable() {
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("test.json".into()),
    );
    app.tab_index = crate::ui::app::TABS
        .iter()
        .position(|tab| *tab == "Resources")
        .unwrap();
    app.resources
        .presets
        .push("cpu:1:block,mem:1:block,net:0:block,agents:0:tty".into());
    let mut terminal = Terminal::new(TestBackend::new(190, 52)).unwrap();
    terminal
        .draw(|frame| app.draw_resources(frame, Rect::new(20, 5, 170, 42)))
        .unwrap();
    let hits = app.resources.hits;
    for hit in [
        hits.preset_prev,
        hits.preset_next,
        hits.interval_down,
        hits.interval_up,
    ] {
        assert!(hit.width > 0);
    }
    app.handle_click(hits.preset_next.x, hits.preset_next.y);
    assert_eq!(app.resources.preset, 1);
    app.handle_click(hits.preset_prev.x, hits.preset_prev.y);
    assert_eq!(app.resources.preset, 0);
    app.handle_click(hits.interval_up.x, hits.interval_up.y);
    assert_eq!(app.resources.interval_ms, 2_100);
    app.handle_click(hits.interval_down.x, hits.interval_down.y);
    assert_eq!(app.resources.interval_ms, 2_000);
}

#[test]
fn resources_keyboard_changes_persist_and_reload() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    let mut loaded = LoadedConfig::defaults(path.display().to_string());
    loaded.config.resources.presets = "cpu:0:default,mem:0:default,net:0:default,agents:1:default cpu:1:block,mem:1:block,net:0:block,agents:0:tty".into();
    let mut app = App::new(Arc::new(MockRuntime::empty()), loaded);
    app.set_config_path(path.clone());
    app.tab_index = crate::ui::app::TABS
        .iter()
        .position(|tab| *tab == "Resources")
        .unwrap();
    app.on_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));
    app.on_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
    app.on_key(KeyEvent::new(KeyCode::Char('P'), KeyModifiers::NONE));

    let path_text = path.to_str().unwrap().to_owned();
    let loaded =
        medulla::config::load_config(Some(&path_text), &Default::default(), temp.path()).unwrap();
    assert_eq!(loaded.config.resources.update_ms, 2_100);
    assert_eq!(loaded.config.resources.preset, 1);
    assert_eq!(loaded.config.resources.shown_boxes, "cpu,mem,net,agents");
    assert_eq!(loaded.config.resources.cpu_bottom, Some(true));
    assert_eq!(loaded.config.resources.mem_below_net, Some(true));
    assert_eq!(loaded.config.resources.agents_left, Some(true));
}

#[test]
fn box_visibility_changes_persist_and_reload_without_a_preset_change() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults(path.display().to_string()),
    );
    app.set_config_path(path.clone());
    app.tab_index = crate::ui::app::TABS
        .iter()
        .position(|tab| *tab == "Resources")
        .unwrap();
    app.on_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));

    let path_text = path.to_str().unwrap().to_owned();
    let loaded =
        medulla::config::load_config(Some(&path_text), &Default::default(), temp.path()).unwrap();
    assert_eq!(loaded.config.resources.shown_boxes, "mem,net,agents");
}

#[test]
fn explicit_layout_options_override_preset_zero() {
    let config = medulla::config::ResourcesConfig {
        preset: 0,
        cpu_bottom: Some(true),
        mem_below_net: Some(true),
        agents_left: Some(true),
        ..Default::default()
    };
    let state = ResourcesState::from_config(&config);
    assert_eq!(state.preset, 0);
    assert!(state.opts.cpu_bottom);
    assert!(state.opts.mem_below_net);
    assert!(state.opts.agents_left);
}

fn test_agent_process(
    pid: u32,
    command: &str,
    cpu: f64,
) -> medulla_monitor::sampler::ProcessSnapshot {
    medulla_monitor::sampler::ProcessSnapshot {
        pid,
        ppid: None,
        exe: Some(command.into()),
        cmd: vec![command.into()],
        cwd: Some(format!("/work/{command}")),
        start_time: 1,
        owner_uid: Some("test-user".into()),
        cpu_percent: cpu,
        memory_bytes: 2 * 1024 * 1024,
        disk_read_bytes: 0,
        disk_written_bytes: 0,
        threads: 2,
    }
}

#[test]
fn agent_rows_render_with_classification_at_desktop_and_narrow_sizes() {
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("test.json".into()),
    );
    app.set_resource_snapshot(Arc::new(
        crate::ui::resources::sampler::ResourceSnapshot::default().with_processes(vec![
            test_agent_process(10, "claude", 12.0),
            test_agent_process(20, "codex-acp", 22.0),
            test_agent_process(30, "openhuman", 32.0),
            test_agent_process(40, "aider", 42.0),
        ]),
    ));
    for (width, height) in [(170, 42), (80, 24)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| app.draw_resources(frame, Rect::new(0, 0, width, height)))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        for expected in [
            "Claude",
            "Codex-ACP",
            "OpenHuman",
            "External",
            "external",
            "PID",
        ] {
            assert!(
                text.contains(expected),
                "missing {expected} at {width}x{height}: {text}"
            );
        }
    }
}

static SIGNALS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static SIGNAL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn test_process_owned(_pid: u32, _started: u64, uid: Option<&str>) -> bool {
    uid == Some("test-user")
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn never_owned(_pid: u32, _started: u64, _uid: Option<&str>) -> bool {
    false
}
fn test_signal(
    _pid: u32,
    _started: u64,
    _uid: Option<&str>,
    _signal: AgentSignal,
) -> std::io::Result<()> {
    SIGNALS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn agent_kill_waits_for_confirmation_and_uses_injected_signal_function() {
    let _guard = SIGNAL_TEST_LOCK.lock().unwrap();
    SIGNALS.store(0, std::sync::atomic::Ordering::SeqCst);
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("test.json".into()),
    );
    app.set_resource_snapshot(Arc::new(
        crate::ui::resources::sampler::ResourceSnapshot::default()
            .with_processes(vec![test_agent_process(42, "claude", 10.0)]),
    ));
    app.resources.agent_signal_fn = test_signal;
    app.resources.agent_process_owned_fn = test_process_owned;
    assert!(app.resources_key(KeyCode::Char('k')));
    assert_eq!(SIGNALS.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(app.resources.agent_signal_armed.is_some());
    assert!(app.resources_key(KeyCode::Char('y')));
    assert_eq!(SIGNALS.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(app.resources.agent_signal_armed.is_some());
    assert!(app.resources_key(KeyCode::Char('Y')));
    assert_eq!(SIGNALS.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(app.resources.agent_signal_armed.is_none());
}

static HIDDEN_SIGNALS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn hidden_test_signal(
    _pid: u32,
    _started: u64,
    _uid: Option<&str>,
    _signal: AgentSignal,
) -> std::io::Result<()> {
    HIDDEN_SIGNALS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

#[test]
fn hidden_agents_box_cannot_arm_or_confirm_a_process_signal() {
    HIDDEN_SIGNALS.store(0, std::sync::atomic::Ordering::SeqCst);
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("test.json".into()),
    );
    app.set_resource_snapshot(Arc::new(
        crate::ui::resources::sampler::ResourceSnapshot::default()
            .with_processes(vec![test_agent_process(43, "claude", 10.0)]),
    ));
    app.resources.agent_signal_fn = hidden_test_signal;
    app.resources.shown.agents = false;

    assert!(!app.resources_key(KeyCode::Char('k')));
    assert!(app.resources.agent_signal_armed.is_none());

    app.resources.agent_signal_armed = Some(AgentSignalConfirmation {
        pid: 43,
        started: 1,
        signal: AgentSignal::Kill,
        stage: AgentSignalConfirmStage::First,
    });
    assert!(app.resources_key(KeyCode::Char('y')));
    assert!(app.resources.agent_signal_armed.is_none());
    assert_eq!(HIDDEN_SIGNALS.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn agent_signal_confirmation_stays_visible_in_detail_view() {
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("test.json".into()),
    );
    let mut process = test_agent_process(44, "aider", 10.0);
    process.exe = Some("/usr/bin/aider".into());
    process.cmd = vec![
        "API_TOKEN=envsecret".into(),
        "aider".into(),
        "--api-key".into(),
        "supersecret".into(),
        "--workspace".into(),
        "/tmp/work".into(),
    ];
    app.set_resource_snapshot(Arc::new(
        crate::ui::resources::sampler::ResourceSnapshot::default().with_processes(vec![process]),
    ));
    app.resources.agent_selected = Some(AgentIdentity {
        host: None,
        pid: 44,
        started: 1,
    });
    app.resources.agent_detail = true;
    app.resources.shown.cpu = false;
    app.resources.shown.mem = false;
    app.resources.shown.net = false;
    app.resources.agent_signal_armed = Some(AgentSignalConfirmation {
        pid: 44,
        started: 1,
        signal: AgentSignal::Kill,
        stage: AgentSignalConfirmStage::First,
    });

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal
        .draw(|frame| app.draw_resources(frame, Rect::new(0, 0, 80, 24)))
        .unwrap();
    let text = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(text.contains("EXTERNAL PROCESS"));
    for expected in [
        "Kind: External",
        "PID: 44",
        "Command: aider --api-key=[REDACTED] --workspace /tmp/work",
        "Origin: external",
        "Signal: kill",
        "Target host: local",
    ] {
        assert!(text.contains(expected), "missing {expected} in {text}");
    }
    assert!(!text.contains("supersecret"));
    assert!(!text.contains("envsecret"));
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn external_agent_signal_requires_a_second_explicit_confirmation() {
    let _guard = SIGNAL_TEST_LOCK.lock().unwrap();
    SIGNALS.store(0, std::sync::atomic::Ordering::SeqCst);
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("test.json".into()),
    );
    app.set_resource_snapshot(Arc::new(
        crate::ui::resources::sampler::ResourceSnapshot::default()
            .with_processes(vec![test_agent_process(45, "aider", 10.0)]),
    ));
    app.resources.agent_signal_fn = test_signal;
    app.resources.agent_process_owned_fn = test_process_owned;

    assert!(app.resources_key(KeyCode::Char('k')));
    assert!(app.resources_key(KeyCode::Char('y')));
    assert_eq!(SIGNALS.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(app.resources.agent_signal_armed.is_some());
    assert!(app.resources_key(KeyCode::Char('Y')));
    assert_eq!(SIGNALS.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(app.resources.agent_signal_armed.is_none());
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn unowned_process_never_reaches_the_injected_signal_function() {
    let _guard = SIGNAL_TEST_LOCK.lock().unwrap();
    SIGNALS.store(0, std::sync::atomic::Ordering::SeqCst);
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("test.json".into()),
    );
    app.set_resource_snapshot(Arc::new(
        crate::ui::resources::sampler::ResourceSnapshot::default()
            .with_processes(vec![test_agent_process(46, "claude", 10.0)]),
    ));
    app.resources.agent_signal_fn = test_signal;
    app.resources.agent_process_owned_fn = never_owned;

    assert!(app.resources_key(KeyCode::Char('k')));
    assert!(app.resources_key(KeyCode::Char('y')));
    assert_eq!(SIGNALS.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(app.resources.agent_signal_armed.is_none());
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[test]
fn process_signals_are_disabled_on_unsupported_platforms() {
    let _guard = SIGNAL_TEST_LOCK.lock().unwrap();
    SIGNALS.store(0, std::sync::atomic::Ordering::SeqCst);
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("test.json".into()),
    );
    app.set_resource_snapshot(Arc::new(
        crate::ui::resources::sampler::ResourceSnapshot::default()
            .with_processes(vec![test_agent_process(47, "claude", 10.0)]),
    ));
    app.resources.agent_signal_fn = test_signal;

    assert!(app.resources_key(KeyCode::Char('k')));
    assert!(app.resources.agent_signal_armed.is_none());
    assert_eq!(SIGNALS.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(app.resources_key(KeyCode::Char('y')));
    assert_eq!(SIGNALS.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[cfg(unix)]
#[test]
fn enter_on_a_registered_session_agent_selects_its_sessions_rail_row() {
    use medulla::protocol::HarnessProvider;

    let mut app = crate::ui::app::rail::tests::hosting_app();
    let harnesses = app.local_sessions().expect("test host").clone();
    let choice = harnesses
        .choices()
        .into_iter()
        .find(|choice| choice.provider == HarnessProvider::Codex)
        .expect("codex harness is configured");
    let session_id = harnesses
        .open_unmanaged_named(&choice, "/", false, Some("agents acceptance".into()))
        .expect("a /bin/sh session starts");

    let pid = 45_001;
    let started = 100;
    let _registration = medulla_monitor::agents::agent_registry().register_at(
        medulla_monitor::agents::AgentRegistration {
            pid,
            started_at_secs: started,
            task_id: None,
            session_id: Some(session_id.clone()),
            provider: "claude".into(),
        },
    );
    let mut process = test_agent_process(pid, "claude", 10.0);
    process.start_time = started;
    app.set_resource_snapshot(Arc::new(
        crate::ui::resources::sampler::ResourceSnapshot::default().with_processes(vec![process]),
    ));
    app.resources.agent_selected = Some(AgentIdentity {
        host: None,
        pid,
        started,
    });

    assert!(app.resources_key(KeyCode::Enter));
    let sessions_tab = crate::ui::app::types::TABS
        .iter()
        .position(|tab| *tab == "Sessions")
        .unwrap();
    assert_eq!(app.tab_index, sessions_tab);
    let rows = app.rail_rows();
    let selected_row = rows
        .iter()
        .position(|row| row.session_id() == Some(session_id.as_str()))
        .expect("registered session is on the rail");
    assert_eq!(app.rail_cursor(), selected_row);
    harnesses.sessions.shutdown();
}

#[test]
fn remote_agent_rows_participate_in_selection_and_enter_opens_the_host_session() {
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("test.json".into()),
    );
    let mut host = crate::ui::app::remote_hosts::RemoteHostState::default();
    host.rows.push(medulla::protocol::RemoteSessionRow {
        id: "w_remote".into(),
        label: "remote agent".into(),
        provider: "claude".into(),
        preset: None,
        state: medulla::protocol::RemoteSessionState::Running,
        cwd: "/remote/work".into(),
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
            kind: "Claude".into(),
            pid: 77,
            cpu: 2.0,
            rss: 1024,
            threads: 2,
            started_at: 1_700_000_000,
            cpu_history: vec![1.0, 2.0],
        }),
    });
    app.remote_hosts.insert("tower".into(), host);

    assert!(app.resources_key(KeyCode::Down));
    assert_eq!(
        app.resources.agent_selected,
        Some(AgentIdentity {
            host: Some("tower".into()),
            pid: 77,
            started: 1_700_000_000,
        })
    );
    assert!(app.resources_key(KeyCode::Enter));
    let sessions_tab = crate::ui::app::types::TABS
        .iter()
        .position(|tab| *tab == "Sessions")
        .unwrap();
    assert_eq!(app.tab_index, sessions_tab);
    let rail_rows = app.rail_rows();
    let selected_row = rail_rows
        .iter()
        .position(|row| row.session_id() == Some("tower/w_remote"))
        .expect("remote agent session is available on the Sessions rail");
    assert_eq!(app.rail_cursor(), selected_row);
}

#[test]
fn agent_follow_tracks_identity_through_sorting_and_snapshot_reordering() {
    let mut app = App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("test.json".into()),
    );
    let processes: Vec<_> = (100..120)
        .map(|pid| test_agent_process(pid, "claude", f64::from(pid)))
        .collect();
    app.set_resource_snapshot(Arc::new(
        crate::ui::resources::sampler::ResourceSnapshot::default()
            .with_processes(processes.clone()),
    ));
    app.resources.agent_selected = Some(AgentIdentity {
        host: None,
        pid: 119,
        started: 1,
    });
    assert!(app.resources_key(KeyCode::Char('F')));
    assert_eq!(
        app.resources.agent_follow,
        Some(AgentIdentity {
            host: None,
            pid: 119,
            started: 1
        })
    );

    let render = |app: &mut App| {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| app.draw_resources(frame, Rect::new(0, 0, 80, 24)))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    };
    let first = render(&mut app);
    assert_eq!(
        app.resources.agent_selected,
        Some(AgentIdentity {
            host: None,
            pid: 119,
            started: 1
        })
    );
    assert!(first.contains("FOLLOW PID 119"));
    assert!(
        first.contains("119"),
        "followed process should be visible: {first}"
    );

    assert!(app.resources_key(KeyCode::Right));
    assert!(app.resources_key(KeyCode::Right));
    app.set_resource_snapshot(Arc::new(
        crate::ui::resources::sampler::ResourceSnapshot::default()
            .with_processes(processes.into_iter().rev().collect()),
    ));
    let reordered = render(&mut app);
    assert_eq!(
        app.resources.agent_selected,
        Some(AgentIdentity {
            host: None,
            pid: 119,
            started: 1
        })
    );
    assert!(reordered.contains("FOLLOW PID 119"));
    assert!(
        reordered.contains("119"),
        "followed process should stay visible: {reordered}"
    );
}
