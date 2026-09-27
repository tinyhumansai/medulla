//! Construction, state accessors, snapshot refresh, and tab/lane helpers for
//! [`App`], including inspection seams and event-loop mutators.

use std::sync::Arc;

use ratatui::layout::Rect;

use crate::ui::agents::{derive_agent_lanes, merge_host_activity, merge_host_roster, AgentLane};
use crate::ui::command::CommandSpec;
use crate::ui::composer::Draft;
use crate::ui::fleet::{merge_capacity, registry_capacity};
use crate::ui::theme::Theme;
use medulla::config::LoadedConfig;
use medulla::runtime::{ContextItem, Runtime};

use super::types::{
    App, Cmd, PaneView, ResumePicker, SETTINGS_SUBPAGES, SP_CONTEXT, SP_FEEDBACK, SP_SUBSCRIPTIONS,
    SP_USAGE, TABS,
};

impl App {
    /// Build a fresh screen bound to `runtime` and `loaded`, starting on the
    /// Sessions tab with an empty composer and the config-derived theme.
    pub fn new(runtime: Arc<dyn Runtime>, loaded: LoadedConfig) -> Self {
        let snapshot = runtime.snapshot();
        let theme = Theme::from_config(&loaded.config.theme);
        let harness_skip_permissions = loaded.config.harness.skip_permissions;
        let status_line_promotion_pending = loaded.config.status_line.is_none();
        App {
            runtime,
            loaded,
            snapshot,
            tab_index: 0,
            changes: super::changes::GitChangesState::capture(),
            draft: Draft::new(),
            selected: 0,
            frame: 0,
            graph: Default::default(),
            status: "Ready".into(),
            update_notice: None,
            contexts: Vec::new(),
            context_index: 0,
            rail_index: 0,
            agent_anchor: None,
            subtask_pages: std::collections::HashMap::new(),
            watching: None,
            kill_armed: None,
            workflow_delete_armed: None,
            agent_scroll: 0,
            command_index: 0,
            hook_log: medulla::harness_hooks::HookEventLog::new(),
            #[cfg(feature = "workflows")]
            workflow_index: 0,
            #[cfg(feature = "workflows")]
            workflows: Vec::new(),
            #[cfg(feature = "workflows")]
            workflow_runs: Vec::new(),
            #[cfg(feature = "workflows")]
            workflow_runs_error: None,
            #[cfg(feature = "workflows")]
            workflow_notes: Vec::new(),
            #[cfg(feature = "workflows")]
            workflow_proposals: Vec::new(),
            #[cfg(feature = "workflows")]
            wf: Default::default(),
            #[cfg(feature = "workflows")]
            workflow_store_override: None,
            tokenmaxxing_index: 0,
            tokenmaxxing_focused: false,
            feedback: Default::default(),
            decision_open: false,
            decision_index: 0,
            dismissed_decisions: Default::default(),
            prompt: None,
            mouse_capture: true,
            account_usage: None,
            settings_index: 0,
            settings_focused: false,
            appearance_index: 0,
            subscriptions_index: 0,
            subscriptions: Default::default(),
            subscriptions_loading: false,
            subscriptions_waiting_for_account: false,
            subscriptions_refresh_pending: false,
            appearance_preview: false,
            appearance_split: false,
            status_line_preview: false,
            status_line_split: false,
            resource_monitor: Default::default(),
            device_monitor: Default::default(),
            session_monitor: Default::default(),
            status_line_index: 0,
            status_line_promotion_pending,
            config_index: 0,
            logout_armed: false,
            relogin_requested: false,
            account: None,
            medulla_home: None,
            theme,
            config_path: None,
            hooks_config_path: None,
            resume_picker: None,
            should_quit: false,
            area: Rect::new(0, 0, 80, 24),
            hit_tabs: Vec::new(),
            hit_tabs_row: 0,
            hit_agents: None,
            hit_session: None,
            hit_threads: None,
            hit_started_sessions: None,
            hit_context: None,
            hit_workflow_preview: None,
            hit_nav: Default::default(),
            panes: Vec::new(),
            drag_anchor: None,
            selection: None,
            copy_selection: false,
            last_events_len: 0,
            link_obs: None,
            host_obs: None,
            local_sessions: None,
            remote_hosts: std::collections::HashMap::new(),
            pending_cmds: std::collections::VecDeque::new(),
            harness_runs: Default::default(),
            #[cfg(feature = "workflows")]
            live_runs: Default::default(),
            harness_focus: crate::ui::harness_pane::HarnessFocus::default(),
            pane_session: None,
            pane_view: Default::default(),
            pane_view_session: None,
            harness_close_armed: None,
            pane_remote_session: None,
            rail_session: None,
            session_picker: None,
            pointer_grab: None,
            hit_session_picker: None,
            help_scroll: 0,
            harness_skip_permissions,
            copy_capture: None,
        }
    }

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

    /// Configure Account logout with a testable home; without it, logout reports no writable location.
    pub fn set_medulla_home(&mut self, home: std::path::PathBuf) {
        self.medulla_home = Some(home);
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

    /// Deliver the fetched account usage payload (None = backend unavailable).
    ///
    /// Returns the subscription sample that was waiting for this payload, or a
    /// fresh sample when an independently loaded payload makes an enabled
    /// TinyHumans meter newly readable.
    pub fn set_account_usage(&mut self, data: Option<serde_json::Value>) -> Option<Cmd> {
        self.account_usage = data;
        if self.subscriptions_waiting_for_account {
            return Some(self.begin_subscription_sample());
        }
        if self
            .loaded
            .config
            .subscriptions
            .source_enabled(medulla::config::MeterSource::Tinyhumans)
        {
            if self.subscriptions_loading {
                self.subscriptions_refresh_pending = true;
                return None;
            }
            return Some(self.begin_subscription_sample());
        }
        None
    }

    /// Release a subscription refresh whose account-usage read failed.
    ///
    /// The last successful account payload remains available to the Usage page
    /// and the TinyHumans meter; a transient failure should not turn a visible
    /// balance into an unavailable row.
    pub fn usage_load_failed(&mut self) -> Option<Cmd> {
        self.subscriptions_waiting_for_account
            .then(|| self.begin_subscription_sample())
    }

    /// Deliver a fresh subscription-usage sample and start a queued refresh.
    pub fn set_subscriptions(
        &mut self,
        snapshot: medulla::subscriptions::SubscriptionSnapshot,
    ) -> Option<Cmd> {
        self.subscriptions = snapshot;
        self.subscriptions_loading = false;
        if self.subscriptions_refresh_pending {
            self.subscriptions_refresh_pending = false;
            return Some(self.begin_subscription_refresh());
        }
        None
    }

    /// The latest subscription-usage readings. Render/test seam.
    pub fn subscriptions(&self) -> &medulla::subscriptions::SubscriptionSnapshot {
        &self.subscriptions
    }

    /// The command that re-samples the meters, when one is due and none is
    /// already running.
    ///
    /// Called on the event loop's tick. Returns `None` while every meter is off
    /// — the sample would be empty and, more to the point, no request should
    /// leave the machine for a meter nobody asked for.
    pub fn due_subscription_refresh(&mut self) -> Option<Cmd> {
        let config = &self.loaded.config.subscriptions;
        if self.subscriptions_loading || !config.any_enabled() {
            return None;
        }
        let now = medulla::clock::now_millis() / 1000;
        if !self
            .subscriptions
            .is_stale(now, config.sampling_interval_seconds())
        {
            return None;
        }
        Some(self.begin_subscription_refresh())
    }

    /// Force a re-sample regardless of when the last one landed.
    ///
    /// What `r` on the Subscriptions page does: the automatic cadence is sized
    /// for windows that move over hours, and "tell me now" is the case it does
    /// not cover.
    pub(in crate::ui::app) fn refresh_subscriptions(&mut self) -> Option<Cmd> {
        if self.subscriptions_loading {
            // Account usage is fetched before the subscription command clones
            // its config, so edits made during that stage are already included.
            // Once the provider sample starts, however, one follow-up is needed.
            if !self.subscriptions_waiting_for_account {
                self.subscriptions_refresh_pending = true;
            }
            return None;
        }
        Some(self.begin_subscription_refresh())
    }

    /// Start a refresh, loading a current account payload for TinyHumans first.
    fn begin_subscription_refresh(&mut self) -> Cmd {
        self.subscriptions_loading = true;
        if self
            .loaded
            .config
            .subscriptions
            .source_enabled(medulla::config::MeterSource::Tinyhumans)
        {
            self.subscriptions_waiting_for_account = true;
            Cmd::LoadUsage
        } else {
            self.begin_subscription_sample()
        }
    }

    /// Start provider sampling with the account payload currently held in App.
    fn begin_subscription_sample(&mut self) -> Cmd {
        self.subscriptions_loading = true;
        self.subscriptions_waiting_for_account = false;
        Cmd::LoadSubscriptions(self.account_usage.clone())
    }

    /// Set the status-line text.
    pub fn set_status(&mut self, s: impl Into<String>) {
        // A destructive confirmation is valid only while its question remains
        // visible. Any asynchronous status replacement cancels it.
        self.kill_armed = None;
        self.workflow_delete_armed = None;
        // The harness close question is the same kind of promise: it is only
        // answerable while the sentence asking it is the one on screen.
        self.harness_close_armed = None;
        self.status = s.into();
    }

    /// Show and arm the session-kill confirmation as one invariant-preserving
    /// state transition.
    pub(super) fn arm_kill(&mut self, target: (String, String)) {
        self.set_status("Kill this session? y confirm · any other key cancels");
        self.kill_armed = Some(target);
    }

    /// Show the workflow deletion confirmation and retain the exact record it names.
    #[cfg(feature = "workflows")]
    pub(super) fn arm_workflow_delete(&mut self, id: String, name: String) {
        self.set_status(format!("Delete {name}? Choose Delete or Esc"));
        self.workflow_delete_armed = Some((id, name));
    }

    /// Show and arm the "close this harness" confirmation for `session`.
    ///
    /// Set after the status line, never before: [`set_status`](Self::set_status)
    /// disarms, so arming first would leave the question visible and unanswerable.
    pub(super) fn arm_harness_close(&mut self, session: String) {
        self.set_status("Close this harness? y confirm · any other key cancels");
        self.harness_close_armed = Some(session);
    }

    /// Replace the Context-tab chunks.
    pub fn set_contexts(&mut self, c: Vec<ContextItem>) {
        self.contexts = c;
    }

    /// Open the resume picker with `chats`, or report that there is nothing to
    /// resume.
    pub fn open_resume(&mut self, chats: Vec<crate::ui::chat_store::MainChatSummary>) {
        if chats.is_empty() {
            self.set_status("No saved chats to resume.");
        } else {
            self.resume_picker = Some(ResumePicker { chats, index: 0 });
            self.set_status("Resume: ↑/↓ select · Enter load · Esc cancel");
        }
    }

    /// The active tab name.
    pub fn tab(&self) -> &'static str {
        TABS[self.tab_index]
    }

    /// The lazy-load command a freshly entered tab (or Settings subpage) needs.
    ///
    /// Context, Feedback, and Usage all fetch on entry; since they are now
    /// Settings subpages rather than tabs, the Settings arm dispatches on the
    /// active subpage.
    pub(super) fn tab_enter_cmd(&mut self) -> Option<Cmd> {
        // No tab needs a focus nudge on entry: the rail forwards typing, so a
        // printable key moves focus to the composer and lands the character
        // there, and nothing is lost by not starting in it.
        match self.tab() {
            "Feedback" => Some(Cmd::LoadFeedback(self.feedback.query.clone())),
            // The diff pane is the Changes surface now, so entering the tab
            // re-loads its git data the way the removed Changes tab did on
            // entry: a repo that changed while the operator was on another tab
            // would otherwise stay stale beneath the open diff.
            "Sessions" => {
                if self.pane_view == PaneView::Diff {
                    self.refresh_changes();
                }
                None
            }
            // The workflow store is files on this machine, so entering the tab
            // reads them rather than asking the runtime for anything — which is
            // why this arm returns no command and does the work here.
            #[cfg(feature = "workflows")]
            "Workflows" => {
                self.reload_workflows();
                None
            }
            "Settings" => match self.settings_index {
                SP_USAGE => Some(Cmd::LoadUsage),
                SP_SUBSCRIPTIONS => self.refresh_subscriptions(),
                SP_CONTEXT => Some(Cmd::InspectContext),
                SP_FEEDBACK => Some(Cmd::LoadFeedback(self.feedback.query.clone())),
                _ => None,
            },
            _ => None,
        }
    }

    /// Derive the current agent lanes from the snapshot, harness, and roster.
    ///
    /// The roster is the snapshot's (what the backend advertises, plus any
    /// host-link peers the observation overlays) merged with the runtime's own
    /// worker registry. Both are needed: a worker added at runtime lives only in
    /// the registry — which is what resolves a delegated task's address — so
    /// reading the snapshot alone left a live, dispatchable worker off this tab.
    pub(super) fn lanes(&self) -> Vec<AgentLane> {
        let roster = merge_host_roster(&self.fleet_roster(), &self.runtime.workers());
        let mut lanes = derive_agent_lanes(&self.snapshot.events, &self.loaded.harness(), &roster);
        // The snapshot's events come from the backend, whose vocabulary says
        // nothing about delegated tasks — so a busy agent renders idle unless
        // the activity the hub observed locally is folded in.
        merge_host_activity(&mut lanes, &self.runtime.worker_activity());
        lanes
    }

    /// The commands offered for the current draft, or `None` when the peek is
    /// closed.
    ///
    /// Open only on the conversation surface: the composer is the only place a
    /// command can be typed, so offering the catalog anywhere else would be
    /// advertising an input that is not there.
    pub(super) fn command_suggestions(&self) -> Option<Vec<&'static CommandSpec>> {
        if self.tab() != "Sessions" || self.prompt.is_some() {
            return None;
        }
        crate::ui::command::suggestions(&self.draft.text)
    }

    /// The highlighted command's name, while the peek is open.
    /// Test/inspection seam.
    pub fn selected_command(&self) -> Option<String> {
        self.peeked_command().map(|spec| spec.name.to_string())
    }

    /// The command the peek would complete to, if it is open on a match.
    pub(super) fn peeked_command(&self) -> Option<&'static CommandSpec> {
        let specs = self.command_suggestions()?;
        specs
            .get(self.command_index.min(specs.len().saturating_sub(1)))
            .copied()
    }

    /// Scroll the transcript the Sessions cursor is reading.
    pub(super) fn scroll_transcript(&mut self, up: bool, step: usize) {
        let target = &mut self.agent_scroll;
        *target = if up {
            target.saturating_add(step)
        } else {
            target.saturating_sub(step)
        };
    }

    /// The capacity the fleet surfaces render.
    ///
    /// The runtime's own declaration wins; the operator's `[fleet]` config is
    /// the fallback, so a declared chain is still visible on a runtime that
    /// reports none (the mock, or a backend that has not answered yet). They are
    /// not merged: two half-descriptions of one fleet interleaved would be
    /// harder to trust than whichever one is authoritative.
    ///
    pub(super) fn fleet_capacity(&self) -> medulla::runtime::CapacitySnapshot {
        let mut declared = if self.snapshot.capacity.is_empty() {
            self.loaded.config.fleet.capacity()
        } else {
            self.snapshot.capacity.clone()
        };
        // Last resort, and opt-in only: with `MEDULLA_DEMO_FLEET` set and nothing
        // real declared, stand in a small fake fleet so the surfaces can be
        // exercised without a backend. It never overrides a real reading.
        if declared.is_empty() && medulla::runtime::demo_fleet_requested() {
            declared = medulla::runtime::demo_capacity();
        }
        // The locally registered peers are hosts too, and they are frequently
        // the only capacity a hub-backed session has. Declared records win on a
        // collision; the registry only ever adds machines nothing else named.
        merge_capacity(&declared, &registry_capacity(&self.runtime.workers()))
    }

    /// The agent identities the fleet surfaces place.
    ///
    /// The snapshot roster, or the stand-in one when `MEDULLA_DEMO_FLEET` is set
    /// and the runtime has none. Locally registered peers are deliberately *not*
    /// merged in here: a peer is a *host*, and it reaches these surfaces through
    /// [`fleet_capacity`](App::fleet_capacity) as one. Listing it here too would
    /// show the same machine twice — once as capacity, once as an agent with
    /// nowhere to live.
    pub(super) fn fleet_roster(&self) -> Vec<medulla::runtime::AgentDescriptor> {
        if self.snapshot.roster.is_empty() && medulla::runtime::demo_fleet_requested() {
            return medulla::runtime::demo_agents();
        }
        self.snapshot.roster.clone()
    }

    /// The index of the active thread in the snapshot's thread list.
    pub(super) fn active_thread_idx(&self) -> usize {
        self.snapshot
            .threads
            .iter()
            .position(|t| t.id == self.snapshot.active_thread_id)
            .unwrap_or(0)
    }

    /// Events-length change signal, so the loop can re-inspect context.
    pub fn events_changed(&mut self) -> bool {
        let n = self.snapshot.events.len();
        let changed = n != self.last_events_len;
        self.last_events_len = n;
        changed
    }
}
