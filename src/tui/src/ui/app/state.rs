//! Construction, state accessors, snapshot refresh, and tab/lane helpers for
//! [`App`], including inspection seams and event-loop mutators.

use std::sync::Arc;

use ratatui::layout::Rect;

use crate::ui::agents::{
    derive_agent_lanes, merge_host_activity, merge_host_roster, AgentLane, AgentRole,
};
use crate::ui::command::CommandSpec;
use crate::ui::composer::Draft;
use crate::ui::fleet::{merge_capacity, registry_capacity};
use crate::ui::theme::Theme;
use medulla::config::LoadedConfig;
use medulla::runtime::{ContextItem, Runtime};

use super::types::{
    App, Cmd, HandbackPolicy, ResumePicker, ROUTING_SUBPAGES, SETTINGS_SUBPAGES, SP_CONTEXT,
    SP_USAGE, TABS, TASKS_SUBPAGES,
};

impl App {
    /// Build a fresh screen bound to `runtime` and `loaded`, starting on the
    /// Overview tab with an empty composer and the config-derived theme.
    pub fn new(runtime: Arc<dyn Runtime>, loaded: LoadedConfig) -> Self {
        let snapshot = runtime.snapshot();
        let theme = Theme::from_config(&loaded.config.theme);
        // Load the persisted routing strategy onto the chooser so the operator's
        // saved selection is highlighted on start rather than always Manual.
        let routing_strategy_index = loaded
            .config
            .routing_strategy
            .and_then(|strategy| {
                super::types::ROUTING_STRATEGIES
                    .iter()
                    .position(|option| option.strategy == strategy)
            })
            .unwrap_or(0);
        let subscription_strategy_index = loaded
            .config
            .subscription_routing_strategy
            .and_then(|strategy| {
                super::types::SUBSCRIPTION_STRATEGIES
                    .iter()
                    .position(|option| option.strategy == strategy)
            })
            .unwrap_or(0);
        // Read before `loaded` is moved into the struct below.
        let handback_policy = HandbackPolicy::from_config(&loaded.config.harness.handback);
        let harness_skip_permissions = loaded.config.harness.skip_permissions;
        App {
            runtime,
            loaded,
            snapshot,
            tab_index: 0,
            draft: Draft::new(),
            history: Vec::new(),
            history_index: -1,
            selected: 0,
            status: "Ready".into(),
            update_notice: None,
            contexts: Vec::new(),
            context_index: 0,
            agent_index: 0,
            watching: None,
            agents_focus: super::types::AgentsFocus::default(),
            agent_scroll: 0,
            chat_scroll: 0,
            command_index: 0,
            host_index: 0,
            workspace_index: 0,
            template_index: 0,
            custom_harnesses: Vec::new(),
            custom_harness_index: 0,
            template_scroll: 0,
            template_modal: false,
            #[cfg(feature = "workflows")]
            workflow_index: 0,
            #[cfg(feature = "workflows")]
            workflows: Vec::new(),
            #[cfg(feature = "workflows")]
            workflow_runs: Vec::new(),
            #[cfg(feature = "workflows")]
            workflow_runs_error: None,
            #[cfg(feature = "workflows")]
            wf: Default::default(),
            #[cfg(feature = "workflows")]
            workflow_store_override: None,
            routing_index: 0,
            routing_focused: false,
            routing_strategy_index,
            subscription_strategy_index,
            subscription_strategy_focused: false,
            credential_status: super::credentials::detect_credential_status(),
            tasks_index: 0,
            tasks_focused: false,
            task_source_index: 0,
            tasks_detail_open: false,
            tokenmaxxing_index: 0,
            tokenmaxxing_focused: false,
            tasks: medulla::tasks::TaskDocument::default(),
            decision_open: false,
            decision_index: 0,
            dismissed_decisions: Default::default(),
            prompt: None,
            frame: 0,
            mouse_capture: true,
            account_usage: None,
            settings_index: 0,
            settings_focused: false,
            appearance_index: 0,
            config_index: 0,
            logout_armed: false,
            relogin_requested: false,
            account: None,
            medulla_home: None,
            theme,
            config_path: None,
            resume_picker: None,
            should_quit: false,
            area: Rect::new(0, 0, 80, 24),
            hit_tabs: Vec::new(),
            hit_tabs_row: 0,
            hit_agents: None,
            hit_harness: None,
            hit_threads: None,
            hit_context: None,
            hit_nav: Default::default(),
            panes: Vec::new(),
            drag_anchor: None,
            selection: None,
            copy_selection: false,
            last_events_len: 0,
            tinyplace_obs: None,
            host_obs: None,
            harnesses: None,
            harness_focus: crate::ui::harness_pane::HarnessFocus::default(),
            harness_pane_session: None,
            harness_picker: None,
            handback_prompt: None,
            help_scroll: 0,
            handback_policy,
            harness_took_control: false,
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
    pub fn set_config_path(&mut self, path: std::path::PathBuf) {
        self.config_path = Some(path);
        self.reload_custom_harnesses();
    }

    /// Record who the core is signed in as, for the Account subpage.
    pub fn set_account(&mut self, account: Option<medulla::core_host::auth::AuthState>) {
        self.account = account;
    }

    /// Configure Account logout with a testable home; without it, logout reports no writable location.
    pub fn set_medulla_home(&mut self, home: std::path::PathBuf) {
        self.medulla_home = Some(home);
        if let Some(home) = &self.medulla_home {
            if let Ok(repository) = medulla::tasks::TaskRepository::in_home(home) {
                self.tasks = repository.document().clone();
            }
        }
    }

    /// Replace the task document returned by background persistence/sync work.
    pub fn set_tasks(&mut self, document: medulla::tasks::TaskDocument) {
        self.tasks = document;
        self.selected = self.selected.min(self.tasks.tasks.len().saturating_sub(1));
    }

    /// The active Tasks subpage name. Test/inspection seam.
    pub fn tasks_subpage(&self) -> &'static str {
        TASKS_SUBPAGES[self.tasks_index.min(TASKS_SUBPAGES.len() - 1)]
    }

    /// Whether Tasks focus is inside the active content pane.
    pub fn tasks_focused(&self) -> bool {
        self.tasks_focused
    }

    /// Whether a selected task or source is open in the detail modal.
    pub fn tasks_detail_open(&self) -> bool {
        self.tasks_detail_open
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

    /// Where the Agents rail cursor is. Test/inspection seam.
    pub fn agent_index(&self) -> usize {
        self.agent_index
    }

    /// The current composer draft text. Test/inspection seam.
    pub fn draft_text(&self) -> &str {
        &self.draft.text
    }

    /// The current composer caret offset (chars). Test/inspection seam.
    pub fn draft_cursor(&self) -> usize {
        self.draft.cursor
    }

    /// The chat transcript scroll offset from the bottom. Test/inspection seam.
    pub fn chat_scroll(&self) -> usize {
        self.chat_scroll
    }

    /// Whether the resume-picker modal is open. Test/inspection seam.
    pub fn resume_open(&self) -> bool {
        self.resume_picker.is_some()
    }

    /// Whether an inline prompt overlay (Workers add/edit, Agents answer) is open,
    /// and its current draft text. Test/inspection seam.
    pub fn prompt_state(&self) -> Option<(String, String)> {
        self.prompt
            .as_ref()
            .map(|p| (p.title.clone(), p.draft.text.clone()))
    }

    /// The `task_id` of the currently selected Agents-list task row, if any.
    /// Test/inspection seam for the X/A steering flows.
    pub fn selected_task_id(&self) -> Option<String> {
        self.selected_agent_task().map(|t| t.task_id)
    }

    /// The active worker-selection index. Test/inspection seam.
    pub fn host_index(&self) -> usize {
        self.host_index
    }

    /// The active Routing subpage name. Test/inspection seam.
    pub fn routing_subpage(&self) -> &'static str {
        ROUTING_SUBPAGES[self.routing_index.min(ROUTING_SUBPAGES.len() - 1)]
    }

    /// Whether Routing focus is inside the active content pane.
    pub fn routing_focused(&self) -> bool {
        self.routing_focused
    }

    /// Focus Routing on a named subpage and enter its content pane.
    pub fn focus_routing_subpage(&mut self, name: &str) {
        self.tab_index = super::types::tab_pos("Routing");
        self.routing_index = ROUTING_SUBPAGES
            .iter()
            .position(|page| *page == name)
            .unwrap_or(0);
        self.routing_focused = true;
        self.refresh_credential_status_if_needed();
    }

    /// Route `copy_chat` into a captured sink instead of the OS clipboard, and
    /// return that sink. Test-only: keeps `pbcopy`/OSC 52 out of the test run.
    pub fn capture_clipboard(&mut self) -> Arc<std::sync::Mutex<Vec<String>>> {
        let sink = Arc::new(std::sync::Mutex::new(Vec::new()));
        self.copy_capture = Some(sink.clone());
        sink
    }

    /// Attach the background tinyplace service's shared observation. Its identity,
    /// roster, and presence are merged into every snapshot refresh.
    pub fn set_tinyplace_observation(
        &mut self,
        obs: Arc<std::sync::Mutex<medulla::tinyplace::service::TinyplaceObservation>>,
    ) {
        self.tinyplace_obs = Some(obs);
        self.refresh_snapshot();
    }

    /// Attach the read-only view of the host running on this device, so the
    /// Overview tab can say what this machine is doing with the work it is sent.
    pub fn set_host_observation(&mut self, host: medulla::daemon::embedded::HostObservation) {
        self.host_obs = Some(host);
    }

    /// The host running on this device, if any.
    pub fn host_observation(&self) -> Option<&medulla::daemon::embedded::HostObservation> {
        self.host_obs.as_ref()
    }

    /// Attach the live harness sessions this device is running.
    ///
    /// Only called when this machine hosts: without a host nothing runs here, so
    /// there is no screen to render and no PTY to type into.
    pub fn set_local_harnesses(&mut self, harnesses: crate::ui::harness_pane::LocalHarnesses) {
        self.harnesses = Some(harnesses);
    }

    /// The live harness sessions this device is running, if it hosts.
    pub fn local_harnesses(&self) -> Option<&crate::ui::harness_pane::LocalHarnesses> {
        self.harnesses.as_ref()
    }

    /// The harness session the last draw resolved for the rail cursor.
    ///
    /// Inspection seam: it is set during render, so a test that wants to act on
    /// "the selected harness" has to be able to see when the cursor has reached
    /// one rather than counting rows it does not control.
    pub fn harness_pane_session_for_test(&self) -> Option<&str> {
        self.harness_pane_session.as_deref()
    }

    /// The harness session currently receiving the operator's keystrokes.
    pub fn attached_harness(&self) -> Option<&str> {
        self.harness_focus.attached_to()
    }

    /// Re-read the runtime snapshot and merge in the tiny.place observation.
    pub fn refresh_snapshot(&mut self) {
        self.snapshot = self.runtime.snapshot();
        if let Some(obs) = &self.tinyplace_obs {
            if let Ok(obs) = obs.lock() {
                obs.merge_into(&mut self.snapshot);
            }
        }
    }

    /// Deliver the fetched account usage payload (None = backend unavailable).
    pub fn set_account_usage(&mut self, data: Option<serde_json::Value>) {
        self.account_usage = data;
    }

    /// Set the status-line text.
    pub fn set_status(&mut self, s: impl Into<String>) {
        self.status = s.into();
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
        match self.tab() {
            "Tasks" => Some(Cmd::LoadTasks),
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
                SP_CONTEXT => Some(Cmd::InspectContext),
                _ => None,
            },
            _ => None,
        }
    }

    /// Derive the current agent lanes from the snapshot, harness, and roster.
    ///
    /// The roster is the snapshot's (what the backend advertises, plus any
    /// tiny.place peers the observation overlays) merged with the runtime's own
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
        if self.tab() != "Agents" || self.prompt.is_some() {
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

    /// Move the peek's cursor, wrapping at both ends so a short list is quick to
    /// walk in either direction.
    pub(super) fn move_command_index(&mut self, up: bool) {
        let Some(len) = self.command_suggestions().map(|s| s.len()) else {
            return;
        };
        if len == 0 {
            return;
        }
        self.command_index = if up {
            (self.command_index + len - 1) % len
        } else {
            (self.command_index + 1) % len
        };
    }

    /// Replace the draft with the peeked command, ready for its argument.
    ///
    /// A command that takes an argument gets a trailing space — which also
    /// closes the peek, since the choice has been made.
    pub(super) fn complete_command(&mut self) {
        let Some(spec) = self.peeked_command() else {
            return;
        };
        let text = if spec.args.is_empty() {
            format!("/{}", spec.name)
        } else {
            format!("/{} ", spec.name)
        };
        self.draft = Draft {
            cursor: text.chars().count(),
            text,
        };
        self.command_index = 0;
    }

    /// Whether the Agents cursor sits on the orchestrator lane.
    ///
    /// That lane is the conversation with the operator, so it scrolls the chat
    /// transcript and its composer submits an instruction; every other lane
    /// shows an agent's own turns and answers its questions.
    pub fn on_orchestrator_lane(&self) -> bool {
        let lanes = self.lanes();
        let rows = self.agent_rows();
        rows.get(self.agent_index.min(rows.len().saturating_sub(1)))
            .and_then(|row| row.lane_index())
            .and_then(|index| lanes.get(index))
            .map(|lane| lane.role == AgentRole::Orchestrator)
            // An empty lane list means the orchestrator lane is all there is.
            .unwrap_or(true)
    }

    /// Scroll whichever transcript the Agents cursor is reading.
    pub(super) fn scroll_transcript(&mut self, up: bool, step: usize) {
        let target = if self.on_orchestrator_lane() {
            &mut self.chat_scroll
        } else {
            &mut self.agent_scroll
        };
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
    /// The template catalog is the exception. This client's own catalog — the
    /// `.medulla/agents` store, or the built-in coding roles when nothing is
    /// installed — is merged into whatever wins, by id, because it is what this
    /// client declares it can provision. A runtime that names the same id keeps
    /// its own record.
    pub(super) fn fleet_capacity(&self) -> medulla::runtime::CapacitySnapshot {
        let mut declared = if self.snapshot.capacity.is_empty() {
            self.loaded.config.fleet.capacity()
        } else {
            self.snapshot.capacity.clone()
        };
        // Last resort, and opt-in only: with `MEDULLA_DEMO_FLEET` set and nothing
        // real declared, stand in a small fake fleet so the surfaces can be
        // exercised without a backend. It never overrides a real reading.
        if (declared.is_empty() || self.loaded.config.fleet.declares_only_templates())
            && medulla::runtime::demo_fleet_requested()
        {
            declared = medulla::runtime::demo_capacity();
        }
        // The locally registered peers are hosts too, and they are frequently
        // the only capacity a hub-backed session has. Declared records win on a
        // collision; the registry only ever adds machines nothing else named.
        let merged = merge_capacity(&declared, &registry_capacity(&self.runtime.workers()));
        // This client's catalog comes last and only fills gaps: a template the
        // runtime already declares under the same id wins, so the page is never
        // empty on a fresh install and never overrides an authoritative record.
        medulla::agents::merge_templates(&merged, &self.loaded.config.fleet.agent_templates)
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
