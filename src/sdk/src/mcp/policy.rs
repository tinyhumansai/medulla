//! Translate loaded Medulla configuration into the policy MCP tools enforce.

/// Build the local tool-call policy from a loaded configuration.
///
/// Presets assigned to another host must neither be advertised by
/// `workflow_host` nor used by the local daemon that `workflow_run` starts.
pub(super) fn policy_from_loaded(
    loaded: crate::config::LoadedConfig,
) -> crate::workflows::ops::HostPolicy {
    let local_host_id = loaded.config.host.effective_address();
    let custom_harness_configs = crate::config::load_layered_custom_harnesses(&loaded.sources)
        .unwrap_or_default()
        .into_iter()
        .filter(|preset| preset.host_id == local_host_id)
        .collect::<Vec<_>>();
    let custom_harnesses = custom_harness_configs
        .iter()
        .map(|preset| preset.id.clone())
        .collect();
    crate::workflows::ops::HostPolicy {
        launch: crate::harness_hooks::LaunchPolicy::from_config(&loaded.config),
        workflows: loaded.config.workflows,
        custom_harnesses,
        custom_harness_configs,
    }
}
