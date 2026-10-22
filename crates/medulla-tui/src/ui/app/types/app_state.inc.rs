/// The interactive TUI screen: all tab state, input focus, and render geometry.
pub struct App {
    /// The runtime this screen drives.
    pub runtime: Arc<dyn Runtime>,
    /// The loaded configuration (for the Config and settings surfaces).
    pub loaded: LoadedConfig,
    /// The most recent runtime snapshot, refreshed each loop tick.
    pub snapshot: RuntimeSnapshot,
    /// The active top-level tab index (into [`TABS`]).
    pub tab_index: usize,
    /// Git changes from the selected session or operator-chosen commit.
    pub(in crate::ui::app) changes: super::super::changes::GitChangesState,
    pub(in crate::ui::app) draft: Draft,
    pub(in crate::ui::app) selected: usize,
    /// The Subconscious tab's animated signal graph. Held on the app because its
    /// simulation has to survive between frames; it is advanced by the draw
    /// path, which is the only thing that looks at it.
    pub(in crate::ui::app) graph: super::super::render::graph::Graph,
    pub(in crate::ui::app) status: String,
    /// A persistent "update vX.Y.Z available" banner, set by the background
    /// update checker; shown in the header until the app exits.
    pub(in crate::ui::app) update_notice: Option<String>,
    pub(in crate::ui::app) contexts: Vec<ContextItem>,
    pub(in crate::ui::app) context_index: usize,
    pub(in crate::ui::app) rail_index: usize,
    /// Which selectable rail row remains selected while the live rail is rebuilt.
    pub(in crate::ui::app) agent_anchor: Option<super::super::rail::RailAnchor>,
    /// Extra pages of sublanes revealed under an agent lane, keyed by lane key.
    ///
    /// Keyed by [`AgentLane::key`](crate::ui::agents::AgentLane::key) rather than
    /// by the lane's rail position, because lanes are re-folded from events every
    /// tick and a lane that appears or ends shifts every index below it — an
    /// expansion tied to a position would silently jump to a different agent.
    /// Absent means the lane shows its first page, which is the default every
    /// lane starts at.
    pub(in crate::ui::app) subtask_pages: std::collections::HashMap<String, usize>,
    /// The `(worker address, task id)` whose screen is currently subscribed.
    ///
    /// Held so a selection change can stop the old stream as well as start the
    /// new one: a subscription nobody is looking at costs the worker a sample,
    /// a ratchet advance and a send on every tick.
    pub(in crate::ui::app) watching: Option<(String, String)>,
    /// The watched `(worker, task)` awaiting destructive-action confirmation.
    pub(in crate::ui::app) kill_armed: Option<(String, String)>,
    /// The workflow awaiting a destructive-action confirmation.
    pub(in crate::ui::app) workflow_delete_armed: Option<(String, String)>,
    pub(in crate::ui::app) agent_scroll: usize,
    /// Selected row in the command peek, while it is open.
    pub(in crate::ui::app) command_index: usize,
    /// Lifecycle reports arriving from the harnesses this Medulla launched.
    ///
    /// Written by the control socket's `hook.report` handler and read here; an
    /// app with no control plane bound simply renders an empty log.
    pub(in crate::ui::app) hook_log: medulla::harness_hooks::HookEventLog,
    /// Selected row on the Workflows tab.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) workflow_index: usize,
    /// The installed workflows, as last read from disk.
    ///
    /// Cached rather than re-read every frame: the store is files, and a render
    /// pass should not do I/O. `r` re-reads it, as it does for templates.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) workflows: Vec<medulla::workflows::WorkflowSummary>,
    /// The selected workflow's runs, read when the selection changes rather
    /// than on every frame.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) workflow_runs: Vec<medulla::workflows::RunRecord>,
    /// Why the run history could not be read, if it could not.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) workflow_runs_error: Option<String>,
    /// What the selected workflow has learned, newest first.
    ///
    /// Cached beside the runs and refreshed with them, for the same reason: a
    /// render pass must not touch the disk.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) workflow_notes: Vec<medulla::workflows::WorkflowNote>,
    /// Changes proposed for the selected workflow, newest first.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) workflow_proposals: Vec<medulla::workflows::WorkflowProposal>,
    /// The Workflows tab's panes, cursors, and copilot threads.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) wf: WorkflowsState,
    /// A workflow store attached directly, overriding the layered one this
    /// client would otherwise resolve.
    ///
    /// The layered store always reads the current directory's
    /// `.medulla/workflows` as repository defaults, then overlays the
    /// user-global workflow directory. That is useful in a real session and
    /// wrong under test, where it makes the catalogue depend on the developer's
    /// checkout. `None` resolves the layered store, as a real session does.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) workflow_store_override:
        Option<Arc<dyn medulla::workflows::WorkflowStore>>,
    /// The active TokenMaxxxing sidebar page.
    pub(in crate::ui::app) tokenmaxxing_index: usize,
    /// Whether keyboard focus is inside the TokenMaxxxing content pane.
    pub(in crate::ui::app) tokenmaxxing_focused: bool,
    /// Feedback-board state (lazily loaded on entry / refresh).
    pub(in crate::ui::app) feedback: FeedbackState,
    /// Feedback-board tab state (lazily loaded on tab entry / refresh).
    /// Whether the prepared-decision modal is visible.
    pub(in crate::ui::app) decision_open: bool,
    /// Highlighted decision row.
    pub(in crate::ui::app) decision_index: usize,
    /// Session-local ids intentionally hidden by the operator.
    pub(in crate::ui::app) dismissed_decisions: std::collections::BTreeSet<String>,
    pub(in crate::ui::app) prompt: Option<Prompt>,
    /// The animation frame counter: one per event-loop tick (~90ms).
    ///
    /// Drives the spinner and the workflow canvas's flowing wires. Held on the
    /// app rather than read from a clock so a test that draws frames explicitly
    /// sees the same animation the terminal does.
    pub frame: usize,
    /// Whether the app currently captures the mouse.
    pub mouse_capture: bool,
    /// Account-level usage payload (`/teams/me/usage` data), when fetched.
    pub account_usage: Option<serde_json::Value>,
    /// The active Settings subpage (index into [`SETTINGS_SUBPAGES`]).
    pub(in crate::ui::app) settings_index: usize,
    /// Whether keyboard focus is inside the Settings content pane rather than on
    /// the left-hand subpage nav.
    ///
    /// Subpages whose content is a list of *actions* (Feedback especially) bind
    /// enough single letters that they swallow the keys you would otherwise use
    /// to get around, and `↑↓` moving the nav meant arrow keys jumped you off
    /// the page entirely. Entering the pane hands `↑↓` to the content and makes
    /// the letter bindings deliberate rather than ambient.
    pub(in crate::ui::app) settings_focused: bool,
    /// Selected option on the Resources settings page.
    pub(in crate::ui::app) resources_settings_index: usize,
    /// State owned by the Resources tab.
    pub(in crate::ui::app) resources: super::super::resources::ResourcesState,
    /// The selected row on the Appearance subpage.
    pub(in crate::ui::app) appearance_index: usize,
    /// The selected meter row on the Subscriptions subpage.
    pub(in crate::ui::app) subscriptions_index: usize,
    /// Provider and customization facts cached for the Harnesses page.
    pub(in crate::ui::app) harnesses_view: Option<super::HarnessesView>,
    /// The latest subscription-usage readings, shown in the sidebar and on the
    /// Subscriptions subpage. Empty until the first sample lands.
    pub(in crate::ui::app) compression: Option<medulla::juice::EmbeddedCompressionStats>,
    pub(in crate::ui::app) compression_loading: bool,
    pub(in crate::ui::app) compression_refreshed_at: u64,
    pub(in crate::ui::app) subscriptions: medulla::subscriptions::SubscriptionSnapshot,
    /// Whether a subscription sample is in flight, so a slow probe cannot be
    /// queued behind itself by the refresh tick.
    pub(in crate::ui::app) subscriptions_loading: bool,
    /// Whether the current refresh is fetching the account payload needed by
    /// the TinyHumans meter before it starts the provider sample.
    pub(in crate::ui::app) subscriptions_waiting_for_account: bool,
    /// Whether a setting changed after the active provider sample cloned its
    /// configuration, requiring one follow-up sample when it completes.
    pub(in crate::ui::app) subscriptions_refresh_pending: bool,
    /// Whether Appearance is showing its Preview tab rather than its Options
    /// tab. Two tabs rather than one long page because the controls and what
    /// they do to the interface are two different things to look at, and
    /// stacking them meant neither fitted on a short terminal.
    pub(in crate::ui::app) appearance_preview: bool,
    /// Whether Appearance last drew as two columns rather than as two tabs.
    /// Written by the renderer, read by the key handler to decide whether `Tab`
    /// has anything to switch.
    pub(in crate::ui::app) appearance_split: bool,
    /// Whether the Status line page is showing its Preview tab rather than its
    /// Switches tab. Only consulted while the pane is too narrow to show both
    /// halves side by side.
    pub(in crate::ui::app) status_line_preview: bool,
    /// Whether the Status line page last drew as two columns rather than as two
    /// tabs. Written by the renderer, which is the only thing that knows the
    /// pane's width, and read by the key handler to decide whether `Tab` has
    /// anything to switch.
    pub(in crate::ui::app) status_line_split: bool,
    /// Throttled sampler backing the optional whole-device sidebar indicators.
    pub(in crate::ui::app) device_monitor: crate::ui::resources::DeviceMonitor,
    /// Latest immutable host snapshot consumed by the Resources tab.
    pub(in crate::ui::app) resource_snapshot:
        std::sync::Arc<crate::ui::resources::sampler::ResourceSnapshot>,
    /// The selected field row on the Status line subpage.
    pub(in crate::ui::app) status_line_index: usize,
    /// Whether the next persisted status-line edit must write the complete
    /// legacy-derived section rather than one field.
    pub(in crate::ui::app) status_line_promotion_pending: bool,
    /// The selected editable row on the Config subpage.
    pub(in crate::ui::app) config_index: usize,
    /// Whether the Account subpage's logout is armed. Logging out clears stored
    /// credentials, so the first Enter arms and the second confirms; any other
    /// navigation disarms it.
    pub(in crate::ui::app) logout_armed: bool,
    /// Whether the app is quitting in order to re-authenticate rather than to
    /// exit. Set by a successful logout so the caller tears the session down and
    /// returns to the login screen instead of returning to the shell.
    pub(in crate::ui::app) relogin_requested: bool,
    /// Who the embedded core is signed in as, for the Account subpage.
    pub(in crate::ui::app) account: Option<medulla::auth::AuthState>,
    /// The Medulla home directory, used to locate the credential store the
    /// Account subpage clears. Injectable so feature tests never touch the real
    /// home; `None` disables logout.
    pub(in crate::ui::app) medulla_home: Option<std::path::PathBuf>,
    /// The resolved color theme; selection highlighting + chrome draw from it.
    pub(in crate::ui::app) theme: Theme,
    /// Where appearance changes are persisted (the user-global `config.toml`).
    /// Injectable so feature tests never touch the real home. `None` disables
    /// persistence (changes still apply live).
    pub(in crate::ui::app) config_path: Option<std::path::PathBuf>,
    /// Where hook edits are persisted — deliberately not always [`Self::config_path`].
    ///
    /// `config_path` may resolve to a project-local file
    /// (`.medulla/config.toml`/`medulla.toml`), which is exactly the layer
    /// `medulla::config::load_config` strips `[[hooks]]` from on every load that
    /// is not an explicit `--config` (project configuration must not authorize
    /// shell commands in the operator's environment). Saving a hook there would
    /// show "Hook saved" and apply for the rest of this session while writing
    /// to a file the next launch ignores. Defaulted to [`Self::config_path`] by
    /// [`Self::set_config_path`] and overridden by
    /// [`Self::set_hooks_config_path`] whenever the caller knows the two must
    /// differ — see `app_loop::run_tui` in the `medulla-tui` crate.
    pub(in crate::ui::app) hooks_config_path: Option<std::path::PathBuf>,
    pub(in crate::ui::app) resume_picker: Option<ResumePicker>,
    /// Whether the event loop should exit after this tick.
    pub should_quit: bool,

    // Render geometry, recorded each draw for click hit-testing.
    pub(in crate::ui::app) area: Rect,
    pub(in crate::ui::app) hit_tabs: Vec<(u16, u16)>,
    pub(in crate::ui::app) hit_tabs_row: u16,
    /// Where the Sessions rail drew, and the rendered row each visible line
    /// belongs to. A row may wrap onto several lines, so a click resolves
    /// through this snapshot rather than by adding an offset to a freshly
    /// rebuilt row list that may have changed since the frame was drawn.
    pub(in crate::ui::app) hit_agents: Option<(Rect, Vec<RailHit>)>,
    // Where the embedded session screen landed, and whose it is. Recorded so a
    // wheel event can be routed to the terminal under the pointer and given
    // coordinates relative to *its* origin rather than the screen's.
    pub(in crate::ui::app) hit_session: Option<(Rect, String)>,
    /// The threads strip's hit box and its first visible row, for click-to-switch.
    pub(in crate::ui::app) hit_threads: Option<(Rect, usize)>,
    /// Where the orchestrator's conversation drew, and the task each of its
    /// visible lines opens (§A7) — `None` for the lines that are transcript
    /// rather than a session entry.
    ///
    /// One slot per drawn row rather than a dense list, because the entries are
    /// interleaved with the conversation: each one sits under the turn that
    /// started it, so the block is no longer contiguous and an offset from its
    /// top no longer identifies an entry.
    ///
    /// Tasks rather than row indices: the rail is rebuilt every frame, so an
    /// index recorded during the draw can name a different row by the time the
    /// click lands. A task id either still has a session or does not.
    pub(in crate::ui::app) hit_started_sessions: Option<(Rect, Vec<Option<String>>)>,
    pub(in crate::ui::app) hit_context: Option<Rect>,
    /// The selected workflow step's preview, for pointer-wheel scrolling.
    pub(in crate::ui::app) hit_workflow_preview: Option<Rect>,
    /// Where the active tab's subpage nav drew its page rows. Only one nav is on
    /// screen at a time, so one field serves Routing and Settings.
    pub(in crate::ui::app) hit_nav: crate::ui::multi_pane::NavHits,
    /// Every pane drawn this frame, in draw order. A pointer selection is
    /// clamped to whichever of these it started in, so a drag reads one pane's
    /// text instead of splicing its neighbour's columns into every row.
    pub(in crate::ui::app) panes: Vec<Rect>,
    /// A drag in progress: where the button went down. Kept apart from
    /// [`Self::selection`] so a click that never moves leaves no selection.
    pub(in crate::ui::app) drag_anchor: Option<(u16, u16)>,
    /// The block of cells the pointer has swept, normalized to
    /// `(left, top, right, bottom)` inclusive.
    pub(in crate::ui::app) selection: Option<(u16, u16, u16, u16)>,
    /// Set when the button is released over a live selection: the next draw
    /// copies what the selection covers, since only then is the buffer readable.
    pub(in crate::ui::app) copy_selection: bool,
    pub(in crate::ui::app) last_events_len: usize,

    // Test-only clipboard capture: when set, `copy_chat` records the copied text
    // here and skips the platform writers (no `pbcopy`/OSC subprocess in tests).
    pub(in crate::ui::app) copy_capture: Option<Arc<std::sync::Mutex<Vec<String>>>>,

    // Optional observational overlay from the background host-link service:
    // this endpoint's own identity, its peer roster, and peer presence. Merged
    // into the snapshot on every refresh so the routing and Agents lanes
    // light up without the runtime having to know about the link.
    pub(in crate::ui::app) link_obs:
        Option<Arc<std::sync::Mutex<medulla::protocol::service::LinkObservation>>>,
    // A read-only view of the task host running on this device, when one is.
    // Read live at render rather than merged into the snapshot: its counters
    // move on the host's own schedule, and the snapshot is the *runtime's*
    // picture of the world — the host is a peer to it, not part of it.
    pub(in crate::ui::app) host_obs: Option<medulla::daemon::embedded::HostObservation>,
    // The live sessions this device is running. `None` when this machine
    // does not host, in which case the Sessions tab has no local screen to show
    // and falls back to a remote worker's streamed one, or to the transcript.
    pub(in crate::ui::app) local_sessions: Option<crate::ui::harness_pane::LocalSessions>,
    /// What each `[[remoteHosts]]` machine last said about itself, by host id.
    ///
    /// A cache rather than a connection: bootstrapping one runs `ssh` and waits,
    /// which the render pass must never do. Populated lazily — a configured host
    /// costs nothing until somebody picks it. See
    /// [`remote_hosts`](crate::ui::app::remote_hosts).
    pub(in crate::ui::app) remote_hosts:
        std::collections::HashMap<String, crate::ui::app::remote_hosts::RemoteHostState>,
    /// Commands left by a handler that had no way to return one.
    ///
    /// The mouse path answers `bool` and the picker's key path answers nothing,
    /// so neither can hand a [`Cmd`] back up. Drained by
    /// [`App::on_event`](crate::ui::app::App::on_event), which every event passes
    /// through.
    pub(in crate::ui::app) pending_cmds: std::collections::VecDeque<Cmd>,
    // Workflow runs the harnesses on this device started over MCP, keyed by the
    // grant session the launcher recorded on each PTY row. Read at render so a
    // run reported a moment ago is on screen at the next frame, and empty on a
    // build or a host with no control plane bound.
    pub(in crate::ui::app) harness_runs: medulla::control_socket::HarnessRunRegistry,
    // Live per-node harness output for runs this TUI started, keyed by run id.
    // Only ever a handful: a settled run keeps its frames until the next run of
    // the same workflow replaces it.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) live_runs:
        std::collections::HashMap<String, super::super::workflows::LiveRun>,
    // Which of the TUI and the selected session owns the keyboard. Reset to
    // `Chrome` whenever the attached session stops being the selected one, so
    // the operator's keys can never land in a session they are not looking at.
    pub(in crate::ui::app) harness_focus: crate::ui::harness_pane::HarnessFocus,
    // The session the Sessions pane resolved on the last draw, and the
    // only one the attach chord can act on. Recorded during render because that
    // is where the rail cursor is turned into a selection; cleared at the top of
    // every draw so it can never name a pane that is no longer on screen.
    pub(in crate::ui::app) pane_session: Option<String>,
    // What the pane is showing for that session: its terminal, or one of the
    // views that replace it. Not cleared per frame — it is the operator's
    // choice, not a record of what the last draw resolved — so it is reset by
    // the selection moving instead.
    pub(in crate::ui::app) pane_view: PaneView,
    // The session `pane_view` was chosen for. `pane_session` cannot answer this:
    // it is cleared at the top of every draw, so comparing against it would
    // reset the view on every frame.
    pub(in crate::ui::app) pane_view_session: Option<String>,
    // The session whose close is awaiting confirmation, if one is. Killing a
    // harness ends whatever it was in the middle of, so `k` asks first — the
    // same one-keypress contract `kill_armed` has for a dispatched task.
    pub(in crate::ui::app) harness_close_armed: Option<String>,
    // The session that took the press of the button currently held down, if any.
    // A terminal grabs the pointer for the whole gesture: whoever received the
    // press receives the drags and the release too, wherever the pointer has
    // wandered to since. Without the grab a release outside the pane — or one
    // swallowed by a modal the click itself opened — never reaches the child,
    // which goes on believing the button is still down and misplaces everything
    // it draws in response to the pointer afterwards.
    pub(in crate::ui::app) pointer_grab: Option<PointerGrab>,
    // The "start a session" picker's outer box, and where each offered row was
    // drawn with the index it stands for in that step's list. The harness step
    // windows a long list, so screen position and list index are not the same
    // number, and only the draw knows which window it used.
    pub(in crate::ui::app) hit_session_picker: Option<(Rect, Vec<(Rect, usize)>)>,
    // The agent behind a selected session row that this device does NOT run,
    // recorded alongside `pane_session` on the same draw.
    //
    // Its only purpose is to tell "the cursor is not on a session" apart from
    // "the cursor is on somebody else's session", which `pane_session` cannot:
    // both leave it `None`. Taking control resolves through the local workspace
    // path, so a remote session can be watched but not taken (§E7), and an
    // operator who presses the take chord on one deserves that answer rather
    // than "no session on this row".
    pub(in crate::ui::app) pane_remote_session: Option<String>,
    // The session selected on the Sessions rail, retained while another tab is
    // visible. Unlike `pane_session`, this is navigation state rather than a
    // keyboard-routing capability: Changes uses it to keep following
    // the repository the operator selected after an intervening tab draw.
    pub(in crate::ui::app) rail_session: Option<String>,
    /// The "start a session" picker, while it is open.
    pub(in crate::ui::app) session_picker: Option<SessionPicker>,
    /// How far the Help page is scrolled, in lines.
    pub(in crate::ui::app) help_scroll: u16,
    /// Whether operator-started sessions launch with the permission-bypass
    /// flag, from `[harness].skipPermissions`.
    pub(in crate::ui::app) harness_skip_permissions: bool,
}
