impl App {
    /// Current status-line text (observable in the header). Test/inspection seam.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Point appearance persistence at the user-global `config.toml`; injectable
    /// so feature tests avoid the real home.
    ///
    /// Also defaults [`Self::hooks_config_path`] to the same file: most tests
    /// and the common case (no project-local config in effect) have exactly one
    /// writable path, and [`Self::set_hooks_config_path`] is there for the
    /// caller that knows the two must differ.
    pub fn set_config_path(&mut self, path: std::path::PathBuf) {
        self.config_path = Some(path.clone());
        self.hooks_config_path = Some(path);
    }

    /// Point hook persistence at a file distinct from [`Self::config_path`].
    ///
    /// Call after [`Self::set_config_path`], which otherwise defaults this to
    /// the same file — needed whenever `config_path` resolved to a
    /// project-local layer, since `medulla::config::load_config` strips
    /// `[[hooks]]` from every layer but an explicit `--config` file and the
    /// user-global config. See [`super::types::App::hooks_config_path`]'s own
    /// docs for why the distinction exists at all.
    pub fn set_hooks_config_path(&mut self, path: std::path::PathBuf) {
        self.hooks_config_path = Some(path);
    }

    /// Record who the core is signed in as, for the Account subpage.
    pub fn set_account(&mut self, account: Option<medulla::auth::AuthState>) {
        self.account = account;
    }

    /// The signed-in account's opaque id, if any — read by the event loop to
    /// attribute a logout's analytics event before the account is cleared.
    pub fn account_user_id(&self) -> Option<String> {
        self.account
            .as_ref()
            .and_then(|state| state.user_id.clone())
    }

    /// Pin the sidebar's device readings to a fixed sample.
    ///
    /// Render tests need the footer to say the same thing on every machine, so
    /// they inject a snapshot instead of letting the rail sample the host.
    pub fn set_device_snapshot(&mut self, snapshot: crate::ui::resources::DeviceSnapshot) {
        self.device_monitor.inject(snapshot);
    }

    /// The device readings derived from the latest background resource snapshot.
    pub(crate) fn device_snapshot(&self) -> crate::ui::resources::DeviceSnapshot {
        self.resource_snapshot.device_snapshot()
    }

    /// Replace the cached background resources reading without sampling on the caller.
    pub fn set_resource_snapshot(
        &mut self,
        snapshot: Arc<crate::ui::resources::sampler::ResourceSnapshot>,
    ) {
        self.resources.sample_network(&snapshot);
        self.resource_snapshot = snapshot;
    }

    /// Configure Account logout with a testable home; without it, logout reports no writable location.
    pub fn set_medulla_home(&mut self, home: std::path::PathBuf) {
        self.medulla_home = Some(home);
    }

    /// Supply collected provider and customization facts for Settings.
    pub fn set_harnesses_view(&mut self, view: super::types::HarnessesView) {
        self.harnesses_view = Some(view);
    }

    /// Place the attention blink rate at an arbitrary interval. Test seam.
    ///
    /// The Appearance page only ever moves between the rates it offers, so a
    /// test covering how a *custom* rate rejoins that cycle cannot reach this
    /// state through the keyboard. `theme` stays private, as the rest of the
    /// app's state does, and this is the one door opened for that case.
    pub fn set_attention_blink_ms(&mut self, blink_ms: u64) {
        self.theme.attention_blink_ms = crate::ui::theme::clamp_blink_ms(blink_ms);
    }

    /// The active Settings subpage name. Test/inspection seam.
    pub fn settings_subpage(&self) -> &'static str {
        SETTINGS_SUBPAGES[self.settings_index.min(SETTINGS_SUBPAGES.len() - 1)]
    }

    /// Focus the Settings tab on the subpage named `name`, returning its
    /// lazy-load command. Unknown names land on the first subpage.
    ///
    /// The public counterpart to the internal index-based jump, for the event
    /// loop and tests that address subpages by name rather than position.
    pub fn focus_settings_subpage(&mut self, name: &str) -> Option<Cmd> {
        let index = SETTINGS_SUBPAGES
            .iter()
            .position(|s| *s == name)
            .unwrap_or(0);
        let cmd = self.set_settings_subpage(index);
        // "Focus" means focus: callers addressing a subpage by name want to act
        // on its contents, not to park on the nav beside it.
        self.settings_focused = true;
        cmd
    }

    /// Whether Settings focus is inside the content pane. Render/test seam.
    pub fn settings_focused(&self) -> bool {
        self.settings_focused
    }

    /// The current primary theme color. Test/inspection seam.
    pub fn theme_primary(&self) -> ratatui::style::Color {
        self.theme.primary
    }

    /// Set the persistent "update available" banner shown in the header. Called
    /// by the background update checker when a newer release is detected.
    pub fn set_update_notice(&mut self, notice: impl Into<String>) {
        self.update_notice = Some(notice.into());
    }

    /// The current update banner text, if any. Test/inspection seam.
    pub fn update_notice(&self) -> Option<&str> {
        self.update_notice.as_deref()
    }

    /// Where the Sessions rail cursor is. Test/inspection seam.
    pub fn rail_index(&self) -> usize {
        self.rail_cursor()
    }

    /// The current composer draft text. Test/inspection seam.
    pub fn draft_text(&self) -> &str {
        &self.draft.text
    }

    /// The current composer caret offset (chars). Test/inspection seam.
    pub fn draft_cursor(&self) -> usize {
        self.draft.cursor
    }

    /// How far the pane's transcript is scrolled back. Test/inspection seam.
    pub fn transcript_scroll(&self) -> usize {
        self.agent_scroll
    }

    /// Whether the resume-picker modal is open. Test/inspection seam.
    pub fn resume_open(&self) -> bool {
        self.resume_picker.is_some()
    }

    /// Whether an inline prompt overlay (Workers add/edit, task answer) is open,
    /// and its current draft text. Test/inspection seam.
    pub fn prompt_state(&self) -> Option<(String, String)> {
        self.prompt
            .as_ref()
            .map(|p| (p.title.clone(), p.draft.text.clone()))
    }

    /// The `task_id` of the currently selected Sessions rail task row, if any.
    /// Test/inspection seam for the X/A steering flows.
    pub fn selected_task_id(&self) -> Option<String> {
        self.selected_agent_task().map(|t| t.task_id)
    }

    /// Route `copy_chat` into a captured sink instead of the OS clipboard, and
    /// return that sink. Test-only: keeps `pbcopy`/OSC 52 out of the test run.
    pub fn capture_clipboard(&mut self) -> Arc<std::sync::Mutex<Vec<String>>> {
        let sink = Arc::new(std::sync::Mutex::new(Vec::new()));
        self.copy_capture = Some(sink.clone());
        sink
    }

    /// Attach the background host-link service's shared observation. Its
    /// identity, roster, and presence are merged into every snapshot refresh.
    pub fn set_link_observation(
        &mut self,
        obs: Arc<std::sync::Mutex<medulla::protocol::service::LinkObservation>>,
    ) {
        self.link_obs = Some(obs);
        self.refresh_snapshot();
    }

    /// Attach the read-only view of the host running on this device, so the
    /// host state can remain available to the runtime and routing surfaces.
    pub fn set_host_observation(&mut self, host: medulla::daemon::embedded::HostObservation) {
        self.host_obs = Some(host);
    }

    /// The host running on this device, if any.
    pub fn host_observation(&self) -> Option<&medulla::daemon::embedded::HostObservation> {
        self.host_obs.as_ref()
    }

    /// Attach the live sessions this device is running.
    ///
    /// Only called when this machine hosts: without a host nothing runs here, so
    /// there is no screen to render and no PTY to type into.
    pub fn set_local_sessions(&mut self, sessions: crate::ui::harness_pane::LocalSessions) {
        self.local_sessions = Some(sessions);
    }

    /// Read lifecycle reports out of the log the control socket writes into.
    ///
    /// Shared, not copied: the point is to render what is arriving now.
    pub fn set_hook_log(&mut self, log: medulla::harness_hooks::HookEventLog) {
        self.hook_log = log;
    }

    /// The live sessions this device is running, if it hosts.
    pub fn local_sessions(&self) -> Option<&crate::ui::harness_pane::LocalSessions> {
        self.local_sessions.as_ref()
    }

    /// Read workflow runs reported by spawned harnesses from `registry`.
    ///
    /// Shared with the control plane rather than copied: a run reported between
    /// two frames must be on screen at the next one, and a snapshot taken at
    /// startup would be permanently empty.
    pub fn set_harness_runs(&mut self, registry: medulla::control_socket::HarnessRunRegistry) {
        self.harness_runs = registry;
        #[cfg(feature = "workflows")]
        self.sync_selected_workflow_run();
    }

    /// The session the last draw resolved for the rail cursor.
    ///
    /// Inspection seam: it is set during render, so a test that wants to act on
    /// "the selected session" has to be able to see when the cursor has reached
    /// one rather than counting rows it does not control.
    pub fn pane_session_for_test(&self) -> Option<&str> {
        self.pane_session.as_deref()
    }

    /// Stand a remote session under the rail cursor, as a draw against a linked
    /// host would.
    ///
    /// Injection seam: reaching this state honestly needs a second machine, and
    /// what reads it is a *refusal* — so a test that cannot set it cannot tell
    /// the refusal apart from the silent wrong handover it exists to prevent.
    pub fn set_pane_remote_session_for_test(&mut self, agent: Option<String>) {
        self.pane_remote_session = agent;
    }

    /// Whether the "start a session" picker is on screen.
    ///
    /// Inspection seam for the pointer rules: several of them are about a click
    /// the picker must *absorb* — one off a row, one outside its box — and
    /// "nothing happened" is only distinguishable from "the modal closed" by
    /// being able to ask.
    pub fn session_picker_open_for_test(&self) -> bool {
        self.session_picker.is_some()
    }

    /// The session currently receiving the operator's keystrokes.
    pub fn attached_session(&self) -> Option<&str> {
        self.harness_focus.attached_to()
    }

    /// Where the last draw put the embedded harness pane, and whose it is.
    ///
    /// Inspection seam for pointer tests: every mouse rule in
    /// [`on_mouse`](Self::on_mouse) is stated in terms of this rect, so a test
    /// that wants to click "inside the pane" or "just outside it" has to be
    /// able to read it rather than hardcode a layout it does not control.
    pub fn harness_pane_rect_for_test(&self) -> Option<(Rect, String)> {
        self.hit_session.clone()
    }

    /// Re-read the runtime snapshot and merge in the host-link observation.
    pub fn refresh_snapshot(&mut self) {
        self.snapshot = self.runtime.snapshot();
        if let Some(obs) = &self.link_obs {
            if let Ok(obs) = obs.lock() {
                obs.merge_into(&mut self.snapshot);
            }
        }
    }
}
