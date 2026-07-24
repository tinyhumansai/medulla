//! Construction, state accessors/setters, snapshot refresh, and the small
//! tab/lane helpers for [`App`]. This is the observable-state surface: the
//! test/inspection seams and the mutators the event loop calls between ticks.

use std::sync::Arc;

use ratatui::layout::Rect;

use crate::ui::agents::{
    derive_agent_lanes, merge_worker_activity, merge_worker_roster, AgentLane,
};
use crate::ui::composer::Draft;
use crate::ui::theme::Theme;
use medulla::config::LoadedConfig;
use medulla::memory::{MemoryHit, MemoryStatus};
use medulla::runtime::{ContextItem, Runtime};

use super::types::{
    App, Cmd, MemoryEntry, ResumePicker, SETTINGS_SUBPAGES, SP_CONTEXT, SP_FEEDBACK, SP_USAGE, TABS,
};

impl App {
    /// Build a fresh screen bound to `runtime` and `loaded`, starting on the
    /// Overview tab with an empty composer and the config-derived theme.
    pub fn new(runtime: Arc<dyn Runtime>, loaded: LoadedConfig) -> Self {
        let snapshot = runtime.snapshot();
        let theme = Theme::from_config(&loaded.config.theme);
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
            agent_scroll: 0,
            chat_scroll: 0,
            worker_index: 0,
            memory_status: None,
            memory_hits: Vec::new(),
            memory_directives: Vec::new(),
            memory_index: 0,
            memory_query: None,
            memory_service: None,
            memory_ingesting: false,
            feedback: Default::default(),
            repo: Default::default(),
            lane_claims: Default::default(),
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
            medulla_home: None,
            theme,
            config_path: None,
            resume_picker: None,
            should_quit: false,
            area: Rect::new(0, 0, 80, 24),
            hit_tabs: Vec::new(),
            hit_tabs_row: 0,
            hit_agents: None,
            hit_context: None,
            hit_threads: None,
            last_events_len: 0,
            tinyplace_obs: None,
            copy_capture: None,
        }
    }

    /// Current status-line text (observable in the header). Test/inspection seam.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Point appearance persistence at a config file (the user-global
    /// `config.toml`). Wiring seam so feature tests avoid the real home.
    pub fn set_config_path(&mut self, path: std::path::PathBuf) {
        self.config_path = Some(path);
    }

    /// Point the Account subpage's logout at a Medulla home directory. Wiring
    /// seam so feature tests never clear the real credential store. Without it,
    /// logout reports that it has nowhere to write rather than guessing.
    pub fn set_medulla_home(&mut self, home: std::path::PathBuf) {
        self.medulla_home = Some(home);
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
    pub fn worker_index(&self) -> usize {
        self.worker_index
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

    /// Store the loaded persona-memory status + directives and drop back to the
    /// directive/facet overview (no active search).
    pub fn set_memory_loaded(&mut self, status: Option<MemoryStatus>, directives: Vec<String>) {
        self.memory_status = status;
        self.memory_directives = directives;
        self.memory_query = None;
        self.memory_index = 0;
    }

    /// Store persona-memory search results for `query` and select the first hit.
    pub fn set_memory_results(&mut self, hits: Vec<MemoryHit>, query: String) {
        self.memory_hits = hits;
        self.memory_query = Some(query);
        self.memory_index = 0;
    }

    /// The active persona-memory selection index. Test/inspection seam.
    pub fn memory_index(&self) -> usize {
        self.memory_index
    }

    /// Attach the persona-memory service, making the Memory tab work on every
    /// runtime path rather than only on core.
    pub fn set_memory_service(&mut self, service: Arc<medulla::memory::MemoryService>) {
        self.memory_service = Some(service);
    }

    /// The attached persona-memory service, if any. The event loop prefers it
    /// over the runtime seam when serving Memory-tab commands.
    pub fn memory_service(&self) -> Option<Arc<medulla::memory::MemoryService>> {
        self.memory_service.clone()
    }

    /// Whether a memory ingest is in flight. Render/test seam.
    pub fn memory_ingesting(&self) -> bool {
        self.memory_ingesting
    }

    /// Record that an ingest finished, and report its outcome.
    pub fn set_memory_ingest_done(&mut self, status: String) {
        self.memory_ingesting = false;
        self.set_status(status);
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
    pub(super) fn tab_enter_cmd(&self) -> Option<Cmd> {
        match self.tab() {
            "Memory" => Some(Cmd::LoadMemory),
            "Agents" | "Repo" => Some(Cmd::LoadWorkspaces(self.loaded.workflow_workspaces())),
            "Settings" => match self.settings_index {
                SP_USAGE => Some(Cmd::LoadUsage),
                SP_CONTEXT => Some(Cmd::InspectContext),
                SP_FEEDBACK => Some(Cmd::LoadFeedback(self.feedback.query.clone())),
                _ => None,
            },
            _ => None,
        }
    }

    /// Replace Repo-tab reports and keep selection within the dirty-file list.
    pub fn set_workspace_reports(&mut self, reports: Vec<medulla::workspace::WorkspaceReport>) {
        self.repo.reports = reports;
        self.repo.loading = false;
        self.repo.file_index = self
            .repo
            .file_index
            .min(self.repo_files().len().saturating_sub(1));
        if self.repo_files().is_empty() {
            self.repo.diff_key = None;
            self.repo.diff.clear();
            self.repo.diff_error = None;
            self.repo.diff_scroll = 0;
        }
    }

    /// Mark the Repo tab as refreshing without discarding its last good view.
    pub fn set_workspaces_loading(&mut self) {
        self.repo.loading = true;
    }

    /// Store the selected path's patch or its typed error.
    ///
    /// Guards against stale responses: if the operator has moved to a different
    /// file while a diff load was in flight, the now-irrelevant result is
    /// discarded so the pane does not briefly flash the wrong patch.
    pub fn set_workspace_diff(
        &mut self,
        workspace: std::path::PathBuf,
        path: std::path::PathBuf,
        result: Result<String, String>,
    ) {
        // Drop responses that arrive after the selection has moved on to a
        // different file. Accept any response when no file is currently selected
        // (the list may be empty or the index may be stale).
        let files = self.repo_files();
        if let Some((current_ws, current_change)) = files.get(self.repo.file_index) {
            if *current_ws != workspace || current_change.path != path {
                return;
            }
        }
        self.repo.diff_key = Some((workspace, path));
        self.repo.diff_scroll = 0;
        match result {
            Ok(diff) => {
                self.repo.diff = diff;
                self.repo.diff_error = None;
            }
            Err(error) => {
                self.repo.diff.clear();
                self.repo.diff_error = Some(error);
            }
        }
    }

    /// Flatten workspace dirty paths into selection order.
    pub(super) fn repo_files(&self) -> Vec<(std::path::PathBuf, medulla::workspace::FileChange)> {
        self.repo
            .reports
            .iter()
            .filter_map(|report| report.snapshot.as_ref())
            .flat_map(|snapshot| {
                snapshot
                    .files
                    .iter()
                    .cloned()
                    .map(|change| (snapshot.root.clone(), change))
            })
            .collect()
    }

    /// Command to load the currently selected dirty path's patch.
    pub fn selected_repo_diff_cmd(&self) -> Option<Cmd> {
        let (workspace, change) = self.repo_files().get(self.repo.file_index)?.clone();
        Some(Cmd::LoadWorkspaceDiff {
            workspace,
            path: change.path,
        })
    }

    /// Move the dirty-file cursor and request the newly selected patch.
    pub(super) fn move_repo_file(&mut self, up: bool) -> Option<Cmd> {
        let max = self.repo_files().len().saturating_sub(1);
        self.repo.file_index = if up {
            self.repo.file_index.saturating_sub(1)
        } else {
            (self.repo.file_index + 1).min(max)
        };
        self.selected_repo_diff_cmd()
    }

    /// Derive the current agent lanes from the snapshot, harness, and roster.
    ///
    /// The roster is the snapshot's (what the backend advertises, plus any
    /// tiny.place peers the observation overlays) merged with the runtime's own
    /// worker registry. Both are needed: a worker added at runtime lives only in
    /// the registry — which is what resolves a delegated task's address — so
    /// reading the snapshot alone left a live, dispatchable worker off this tab.
    pub(super) fn lanes(&self) -> Vec<AgentLane> {
        let roster = merge_worker_roster(&self.snapshot.roster, &self.runtime.workers());
        let mut lanes = derive_agent_lanes(&self.snapshot.events, &self.loaded.harness(), &roster);
        // The snapshot's events come from the backend, whose vocabulary says
        // nothing about delegated tasks — so a busy worker renders idle unless
        // the activity the hub observed locally is folded in.
        merge_worker_activity(&mut lanes, &self.runtime.worker_activity());
        lanes
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

    /// The current Memory-tab left-pane rows: directives + facet overview with no
    /// active search, or the ranked hits after a `/memory <query>` search.
    pub(super) fn memory_entries(&self) -> Vec<MemoryEntry> {
        if self.memory_query.is_some() {
            return self
                .memory_hits
                .iter()
                .cloned()
                .map(MemoryEntry::Hit)
                .collect();
        }
        let mut out: Vec<MemoryEntry> = self
            .memory_directives
            .iter()
            .cloned()
            .map(MemoryEntry::Directive)
            .collect();
        if let Some(st) = &self.memory_status {
            for (name, count) in &st.facet_counts {
                out.push(MemoryEntry::Facet {
                    name: name.clone(),
                    count: *count,
                });
            }
        }
        out
    }

    /// The number of selectable Memory-tab rows.
    pub(super) fn memory_entry_count(&self) -> usize {
        self.memory_entries().len()
    }
}
