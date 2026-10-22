use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use super::{
    spawn_sampler, spawn_sampler_with_disk_options, update_disk_options, update_interval,
    DiskOptions, DiskSnapshot, History, ResourceSnapshot, ResourceSource,
};

#[derive(Clone)]
struct SequenceSource(Arc<AtomicUsize>);

impl ResourceSource for SequenceSource {
    fn sample(&mut self) -> ResourceSnapshot {
        ResourceSnapshot {
            cpu_percent: self.0.fetch_add(1, Ordering::Relaxed) as f64,
            ..Default::default()
        }
    }
}

#[tokio::test]
async fn sampler_posts_injected_samples_in_order() {
    let (interval_tx, interval_rx) = watch::channel(Some(Duration::from_millis(5)));
    let (output_tx, mut output_rx) = mpsc::unbounded_channel();
    let task = spawn_sampler(
        SequenceSource(Arc::new(AtomicUsize::new(0))),
        interval_rx,
        2,
        move |snapshot| output_tx.send(snapshot).is_ok(),
    );
    let first = tokio::time::timeout(Duration::from_secs(1), output_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let second = tokio::time::timeout(Duration::from_secs(1), output_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.cpu_percent, 0.0);
    assert_eq!(second.cpu_percent, 1.0);
    assert_eq!(
        second
            .history
            .cpu_percent
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [0.0, 1.0]
    );
    drop(interval_tx);
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn changed_interval_controls_the_next_sample_tick() {
    let (interval_tx, interval_rx) = watch::channel(Some(Duration::from_secs(5)));
    let (output_tx, mut output_rx) = mpsc::unbounded_channel();
    let task = spawn_sampler(
        SequenceSource(Arc::new(AtomicUsize::new(0))),
        interval_rx,
        2,
        move |snapshot| output_tx.send(snapshot).is_ok(),
    );

    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(4)).await;
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
    assert!(output_rx.try_recv().is_err());

    interval_tx.send(Some(Duration::from_millis(20))).unwrap();
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_millis(19)).await;
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
    assert!(output_rx.try_recv().is_err());
    tokio::time::advance(Duration::from_millis(1)).await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
        if output_rx.try_recv().is_ok() {
            task.abort();
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    task.abort();
    panic!("updated resource interval did not produce a sample");
}

#[tokio::test(start_paused = true)]
async fn repeated_unchanged_interval_updates_do_not_starve_samples() {
    let period = Duration::from_millis(20);
    let (interval_tx, interval_rx) = watch::channel(Some(period));
    let (output_tx, mut output_rx) = mpsc::unbounded_channel();
    let task = spawn_sampler(
        SequenceSource(Arc::new(AtomicUsize::new(0))),
        interval_rx,
        2,
        move |snapshot| output_tx.send(snapshot).is_ok(),
    );
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
    for _ in 0..5 {
        assert!(!update_interval(&interval_tx, Some(period)));
        tokio::task::yield_now().await;
    }
    tokio::time::advance(period).await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
        if output_rx.try_recv().is_ok() {
            task.abort();
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    task.abort();
    panic!("unchanged interval updates starved the scheduled sample");
}

#[derive(Clone)]
struct DiskOptionsSource(Arc<std::sync::Mutex<DiskOptions>>);

impl ResourceSource for DiskOptionsSource {
    fn sample(&mut self) -> ResourceSnapshot {
        let options = self.0.lock().unwrap().clone();
        let disks = [
            ("loop0", "/boot", false, false),
            ("sda", "/", true, true),
            ("sdb", "/boot", true, true),
            ("sdc", "/data", true, false),
        ]
        .into_iter()
        .filter(|(device, mount, physical, in_fstab)| {
            (!options.only_physical || *physical)
                && (!options.use_fstab || *in_fstab)
                && !options
                    .filter
                    .split(',')
                    .filter_map(|filter| filter.strip_prefix("exclude="))
                    .any(|excluded| excluded == *mount)
                && (*device != "loop0" || !options.only_physical)
        })
        .map(|(device, mount, _, _)| DiskSnapshot {
            device: device.into(),
            mount: mount.into(),
            total_bytes: 0,
            used_bytes: 0,
            read_bytes_per_second: 0.0,
            write_bytes_per_second: 0.0,
            busy_percent: None,
        })
        .collect();
        ResourceSnapshot {
            disks,
            ..Default::default()
        }
    }

    fn update_disk_options(&mut self, options: DiskOptions) {
        *self.0.lock().unwrap() = options;
    }
}

#[tokio::test]
async fn disk_options_change_samples_without_restarting_the_sampler() {
    let initial = DiskOptions::default();
    let (options_tx, options_rx) = watch::channel(initial.clone());
    let (interval_tx, interval_rx) = watch::channel(Some(Duration::from_millis(5)));
    let (output_tx, mut output_rx) = mpsc::unbounded_channel();
    let task = spawn_sampler_with_disk_options(
        DiskOptionsSource(Arc::new(std::sync::Mutex::new(initial))),
        interval_rx,
        options_rx,
        2,
        move |snapshot| output_tx.send(snapshot).is_ok(),
    );

    async fn sample_matching(
        output_rx: &mut mpsc::UnboundedReceiver<Arc<ResourceSnapshot>>,
        matches: impl Fn(&ResourceSnapshot) -> bool,
    ) -> Arc<ResourceSnapshot> {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = output_rx.recv().await.expect("sampler should stay alive");
                if matches(&snapshot) {
                    return snapshot;
                }
            }
        })
        .await
        .expect("sampler should publish a matching snapshot within one second")
    }

    assert_eq!(
        sample_matching(&mut output_rx, |snapshot| snapshot.disks.len() == 4)
            .await
            .disks
            .len(),
        4
    );
    assert!(update_disk_options(
        &options_tx,
        DiskOptions {
            only_physical: true,
            ..Default::default()
        }
    ));
    assert_eq!(
        sample_matching(&mut output_rx, |snapshot| snapshot.disks.len() == 3)
            .await
            .disks
            .len(),
        3
    );
    assert!(update_disk_options(
        &options_tx,
        DiskOptions {
            only_physical: true,
            use_fstab: true,
            ..Default::default()
        }
    ));
    assert_eq!(
        sample_matching(&mut output_rx, |snapshot| snapshot.disks.len() == 2)
            .await
            .disks
            .len(),
        2
    );
    assert!(update_disk_options(
        &options_tx,
        DiskOptions {
            only_physical: true,
            use_fstab: true,
            filter: "exclude=/boot".into(),
        }
    ));
    let final_sample = sample_matching(&mut output_rx, |snapshot| {
        snapshot.disks.len() == 1 && snapshot.disks[0].mount == "/"
    })
    .await;
    assert_eq!(final_sample.disks.len(), 1);
    assert_eq!(final_sample.disks[0].mount, "/");

    drop(interval_tx);
    task.abort();
}

#[test]
fn history_trims_to_requested_capacity_and_clones_cheaply() {
    let mut history = History::new(3);
    history.push(1, 2);
    history.push(2, 2);
    let clone = history.clone();
    history.push(3, 2);
    assert_eq!(history.iter().copied().collect::<Vec<_>>(), [2, 3]);
    assert_eq!(clone.iter().copied().collect::<Vec<_>>(), [1, 2]);
    history.push(4, 0);
    assert!(history.is_empty());
}

#[test]
fn resource_history_keeps_each_memory_series_for_graphs() {
    let sample = ResourceSnapshot {
        memory_used_bytes: 10,
        memory_available_bytes: 20,
        memory_cached_bytes: 30,
        memory_free_bytes: 40,
        swap_used_bytes: 50,
        ..Default::default()
    };
    let mut history = super::ResourceHistory::default();
    history.push(&sample, 8);

    assert_eq!(
        history
            .memory_used_bytes
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [10]
    );
    assert_eq!(
        history
            .memory_available_bytes
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [20]
    );
    assert_eq!(
        history
            .memory_cached_bytes
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [30]
    );
    assert_eq!(
        history
            .memory_free_bytes
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [40]
    );
    assert_eq!(
        history.swap_used_bytes.iter().copied().collect::<Vec<_>>(),
        [50]
    );
}

#[test]
fn process_io_peak_decays_instead_of_resetting_to_each_sample() {
    let mut source = super::SystemResourceSource::default();
    assert_eq!(source.update_disk_peak(100.0, 0.0), 100.0);
    assert_eq!(source.update_disk_peak(0.0, 0.0), 90.0);
}

#[cfg(target_os = "linux")]
#[test]
fn process_snapshots_skip_executable_and_command_for_unclassified_processes() {
    let mut source = super::SystemResourceSource::default();
    let snapshot = source.sample();
    let pid = std::process::id();
    let process = snapshot
        .processes
        .iter()
        .find(|process| process.pid == pid)
        .unwrap_or_else(|| panic!("sampler omitted its own process {pid}"));

    assert!(process.exe.is_none());
    assert!(process.cmd.is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn process_snapshots_include_executable_and_command_for_agent_candidate_children() {
    let mut child = std::process::Command::new("bash")
        .args(["-c", "read -r line"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take();
    let pid = child.id();
    let snapshot = super::SystemResourceSource::default().sample();
    let process = snapshot
        .processes
        .iter()
        .find(|process| process.pid == pid)
        .cloned();
    let _ = child.kill();
    let _ = child.wait();

    let process = process.unwrap_or_else(|| panic!("sampler omitted external fixture PID {pid}"));
    assert!(
        process.exe.as_deref().is_some_and(|exe| {
            std::path::Path::new(exe)
                .file_name()
                .is_some_and(|name| name == "bash")
        }),
        "agent candidate snapshot must include its executable: {process:?}"
    );
    assert!(
        process
            .cmd
            .iter()
            .any(|argument| argument == "read -r line"),
        "agent candidate snapshot must include argv: {process:?}"
    );
    drop(stdin);
}

#[cfg(target_os = "linux")]
#[test]
fn thread_details_are_refreshed_only_for_agent_candidates() {
    use std::ffi::OsString;
    use std::path::Path;

    assert!(super::should_refresh_task_details(
        Some(Path::new("/usr/local/bin/claude")),
        &[OsString::from("claude")],
    ));
    assert!(super::should_refresh_task_details(
        Some(Path::new("/usr/bin/node")),
        &[
            OsString::from("node"),
            OsString::from("/opt/@agentclientprotocol/claude-agent-acp"),
        ],
    ));
    assert!(!super::should_refresh_task_details(
        Some(Path::new("/usr/bin/sleep")),
        &[OsString::from("sleep"), OsString::from("120")],
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn process_details_are_refreshed_only_for_possible_agent_processes() {
    assert!(super::may_be_agent_process("claude"));
    assert!(super::may_be_agent_process("node"));
    assert!(super::may_be_agent_process("bash"));
    assert!(!super::may_be_agent_process("sleep"));
    assert!(!super::may_be_agent_process("postgres"));
}

#[cfg(target_os = "linux")]
#[test]
fn agent_metric_refresh_includes_descendants_without_refreshing_unrelated_trees() {
    use std::collections::HashSet;
    use sysinfo::Pid;

    let processes = [
        (Pid::from_u32(10), None),
        (Pid::from_u32(11), Some(Pid::from_u32(10))),
        (Pid::from_u32(12), Some(Pid::from_u32(11))),
        (Pid::from_u32(20), None),
        (Pid::from_u32(21), Some(Pid::from_u32(20))),
    ];
    let refreshed = super::pids_in_process_trees(processes, [Pid::from_u32(10)])
        .into_iter()
        .collect::<HashSet<_>>();

    assert_eq!(
        refreshed,
        [Pid::from_u32(10), Pid::from_u32(11), Pid::from_u32(12)]
            .into_iter()
            .collect()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn shell_candidates_do_not_make_every_child_an_agent_metric() {
    use std::ffi::OsString;
    use std::path::Path;

    assert!(!super::should_refresh_process_tree(
        Some(Path::new("/usr/bin/bash")),
        &[
            OsString::from("bash"),
            OsString::from("/app/tests_resources.sh")
        ],
    ));
    assert!(super::should_refresh_process_tree(
        Some(Path::new("/usr/local/bin/codex")),
        &[OsString::from("codex")],
    ));
}

#[test]
fn device_disk_selection_prefers_the_deepest_mount_containing_cwd() {
    let disks = [
        super::DiskSnapshot {
            device: "sda1".into(),
            mount: "/".into(),
            total_bytes: 10_000,
            used_bytes: 9_000,
            read_bytes_per_second: 0.0,
            write_bytes_per_second: 0.0,
            busy_percent: None,
        },
        super::DiskSnapshot {
            device: "sdb1".into(),
            mount: "/mnt/data".into(),
            total_bytes: 1_000,
            used_bytes: 250,
            read_bytes_per_second: 0.0,
            write_bytes_per_second: 0.0,
            busy_percent: None,
        },
    ];

    let selected =
        super::types::most_specific_mount(&disks, std::path::Path::new("/mnt/data/project"))
            .unwrap();
    assert_eq!(selected.mount, "/mnt/data");
}

#[test]
fn disk_filters_exclude_mount_subtrees_and_virtual_filesystems() {
    let options = super::DiskOptions {
        filter: "exclude=/boot".into(),
        ..Default::default()
    };
    assert!(!super::mount_is_visible(
        "/boot",
        "/dev/sda1",
        "ext4",
        &options,
        ""
    ));
    assert!(!super::mount_is_visible(
        "/boot/efi",
        "/dev/sda2",
        "vfat",
        &options,
        ""
    ));
    assert!(super::mount_is_visible(
        "/",
        "/dev/sda3",
        "ext4",
        &options,
        ""
    ));
    let physical = super::DiskOptions {
        only_physical: true,
        ..Default::default()
    };
    assert!(!super::mount_is_visible(
        "/run", "tmpfs", "tmpfs", &physical, ""
    ));
    assert!(!super::mount_is_visible(
        "/app", "overlay", "overlay", &physical, ""
    ));
    assert!(super::mount_is_visible(
        "/",
        "/dev/sda3",
        "ext4",
        &physical,
        ""
    ));
}

#[test]
fn fstab_mode_keeps_only_configured_mounts() {
    let options = super::DiskOptions {
        use_fstab: true,
        ..Default::default()
    };
    let fstab = "/dev/root / ext4 defaults 0 1\n/dev/data /mnt/data\\040drive ext4 defaults 0 2\n";
    assert!(super::mount_is_visible(
        "/",
        "/dev/root",
        "ext4",
        &options,
        fstab
    ));
    assert!(super::mount_is_visible(
        "/mnt/data drive",
        "/dev/data",
        "ext4",
        &options,
        fstab
    ));
    assert!(!super::mount_is_visible(
        "/var",
        "/dev/root",
        "ext4",
        &options,
        fstab
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn linux_meminfo_reports_available_and_cached_with_legacy_fallback() {
    let memory = super::parse_memory("MemTotal: 1000 kB\nMemFree: 100 kB\nCached: 300 kB\nMemAvailable: 700 kB\nSwapTotal: 50 kB\n");
    assert_eq!(
        (memory.total, memory.available, memory.cached),
        (1_024_000, 716_800, 307_200)
    );
    let fallback = super::parse_memory("MemTotal: 1000 kB\nMemFree: 100 kB\nCached: 300 kB\n");
    assert_eq!(fallback.available, 409_600);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_block_stat_deltas_report_read_write_rates_and_busy_percent() {
    let before = super::parse_block_stat("8 0 100 500 8 0 200 900 0 300 1400").unwrap();
    let after = super::parse_block_stat("8 0 110 700 8 0 220 1300 0 450 2000").unwrap();
    let reading = super::io_delta(before, after, 2.0);
    assert_eq!(reading.read_rate, 2_560.0);
    assert_eq!(reading.write_rate, 5_120.0);
    assert_eq!(reading.busy, 7.5);
    assert_eq!(super::io_delta((0, 0, 0), (0, 0, 10_000), 1.0).busy, 100.0);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_partition_block_stats_are_read_from_class_block() {
    let root = tempfile::tempdir().unwrap();
    let partition_stat = root.path().join("sys/class/block/nvme0n1p1/stat");
    std::fs::create_dir_all(partition_stat.parent().unwrap()).unwrap();
    std::fs::write(&partition_stat, "259 1 100 500 259 1 200 900 0 300 1400").unwrap();

    assert_eq!(
        super::read_block_stat(root.path(), "nvme0n1p1"),
        Some((100, 200, 300))
    );
}
