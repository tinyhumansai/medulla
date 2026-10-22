use super::*;

#[test]
fn resources_interval_defaults_and_clamps_during_deserialization() {
    let defaults = ResourcesConfig::default();
    assert_eq!(defaults.update_ms, 2_000);
    assert_eq!(defaults.shown_boxes, "cpu,mem,net,agents");
    assert!(defaults.presets.starts_with("cpu:0:default"));
    assert_eq!(defaults.preset, 0);
    assert_eq!(defaults.cpu_graph_upper, "total");
    assert_eq!(defaults.cpu_graph_lower, "iowait");
    assert!(!defaults.cpu_invert_lower);
    assert!(!defaults.cpu_single_graph);
    assert_eq!(defaults.temp_scale, "celsius");
    assert_eq!(defaults.freq_mode, "auto");
    assert!(defaults.show_battery);
    assert!(defaults.mem_graphs && defaults.show_disks && defaults.show_swap);
    assert!(defaults.io_graph_speeds.is_empty());
    assert!(defaults.swap_disk && defaults.only_physical && defaults.use_fstab);
    assert!(!defaults.io_mode && !defaults.io_graph_combined);
    let low: ResourcesConfig = serde_json::from_str(r#"{"updateMs":1}"#).unwrap();
    let high: ResourcesConfig = serde_json::from_str(r#"{"updateMs":50000}"#).unwrap();
    assert_eq!(low.update_ms, MIN_RESOURCES_UPDATE_MS);
    assert_eq!(high.update_ms, MAX_RESOURCES_UPDATE_MS);
}

#[test]
fn memory_and_disk_options_deserialize_in_camel_case_and_old_configs_default() {
    let config: ResourcesConfig = serde_json::from_str(r#"{"memGraphs":false,"showDisks":false,"showSwap":false,"swapDisk":true,"onlyPhysical":true,"useFstab":true,"disksFilter":"exclude=/boot","ioMode":true,"ioGraphCombined":true,"ioGraphSpeeds":"/:20 /boot:1"}"#).unwrap();
    assert!(!config.mem_graphs && !config.show_disks && !config.show_swap);
    assert!(
        config.swap_disk
            && config.only_physical
            && config.use_fstab
            && config.io_mode
            && config.io_graph_combined
    );
    assert_eq!(config.disks_filter, "exclude=/boot");
    assert_eq!(config.io_graph_speeds, "/:20 /boot:1");
    let old: ResourcesConfig = serde_json::from_str("{}").unwrap();
    assert!(old.show_disks && old.show_swap && old.only_physical && old.use_fstab);
}

#[test]
fn cpu_sensor_options_deserialize_as_camel_case_and_old_config_keeps_defaults() {
    let config: ResourcesConfig = serde_json::from_str(
        r#"{"cpuGraphUpper":"user","cpuGraphLower":"system","cpuInvertLower":true,"cpuSingleGraph":true,"tempScale":"fahrenheit","freqMode":"ghz","showBattery":false}"#,
    )
    .unwrap();
    assert_eq!(config.cpu_graph_upper, "user");
    assert_eq!(config.cpu_graph_lower, "system");
    assert!(config.cpu_invert_lower);
    assert!(config.cpu_single_graph);
    assert_eq!(config.temp_scale, "fahrenheit");
    assert_eq!(config.freq_mode, "ghz");
    assert!(!config.show_battery);
    let old: ResourcesConfig = serde_json::from_str(r#"{"updateMs":2500}"#).unwrap();
    assert_eq!(old.cpu_graph_upper, "total");
    assert!(old.show_battery);
}

#[test]
fn resources_layout_settings_deserialize_as_camel_case() {
    let config: ResourcesConfig = serde_json::from_str(
        r#"{"shownBoxes":"cpu,agents","presets":"cpu:1:block","preset":1,"cpuBottom":true,"memBelowNet":true,"agentsLeft":true}"#,
    ).unwrap();
    assert_eq!(config.shown_boxes, "cpu,agents");
    assert_eq!(config.presets, "cpu:1:block");
    assert_eq!(config.preset, 1);
    assert_eq!(config.cpu_bottom, Some(true));
    assert_eq!(config.mem_below_net, Some(true));
    assert_eq!(config.agents_left, Some(true));
}

#[test]
fn network_options_deserialize_in_camel_case_and_old_configs_default() {
    let config: ResourcesConfig = serde_json::from_str(
        r#"{"netAuto":false,"netSync":true,"netDownload":250,"netUpload":125,"netIface":"eth0","swapUploadDownload":true}"#,
    ).unwrap();
    assert!(!config.net_auto && config.net_sync);
    assert_eq!(config.net_download, 250);
    assert_eq!(config.net_upload, 125);
    assert_eq!(config.net_iface.as_deref(), Some("eth0"));
    assert!(config.swap_upload_download);
    let old: ResourcesConfig = serde_json::from_str("{}").unwrap();
    assert!(old.net_auto);
    assert!(!old.net_sync && !old.swap_upload_download);
    assert_eq!(old.net_download, 100);
    assert_eq!(old.net_upload, 100);
    assert_eq!(old.net_iface, None);
}
