//! The interactive TUI: app state, key/mouse handling, slash commands, and the
//! ratatui render for every tab. A port of the Ink `App.tsx` behavior.

use std::sync::Arc;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::agents::{
    agent_row_model, derive_agent_lanes, lane_lines, task_lines, AgentLane, AgentRole, AgentRow,
    Line as StyledLine, TaskState, TaskStatus,
};
use crate::ui::clipboard::{copy_text, copy_to_clipboard, current_platform, CopyScope, OSC_52};
use crate::ui::composer::{caret_row_col, delete_before, insert_at, move_caret_row, Draft};
use crate::ui::events::{describe_event, EventEnvelope, TuiEvent};
use crate::ui::stream;
use crate::ui::theme::{color_to_string, Theme, THEME_ROLES};
use crate::ui::util::{clip, clock, fmt_tokens, wrap};
use medulla::config::LoadedConfig;
use medulla::memory::{MemoryHit, MemoryStatus};
use medulla::runtime::{ContextItem, Runtime, RuntimeSnapshot, WorkerInfo, WorkerOp};

pub const TABS: [&str; 8] = [
    "Overview", "Chat", "Agents", "Workers", "Trace", "Context", "Memory", "Settings",
];
/// The Settings tab's left-nav subpages, in order (number keys 1-4 jump to them).
pub const SETTINGS_SUBPAGES: [&str; 4] = ["Usage", "Appearance", "Config", "Help"];
pub use crate::ui::util::SPINNER;

// Settings subpage indices.
const SP_USAGE: usize = 0;
const SP_APPEARANCE: usize = 1;
const SP_CONFIG: usize = 2;
const SP_HELP: usize = 3;

/// The index of a tab by name, or 0 if unknown. Keeps tab jumps robust as the tab
/// list grows.
fn tab_pos(name: &str) -> usize {
    TABS.iter().position(|t| *t == name).unwrap_or(0)
}

/// An async action the event loop must run on the app's behalf.
#[derive(Debug)]
pub enum Cmd {
    Quit,
    Submit(String),
    Resume(String),
    ListChats,
    InspectContext,
    WorkerOp(WorkerOp),
    /// Load the persona-memory status + directives for the Memory tab.
    LoadMemory,
    /// Fetch account-level usage from the backend for the Usage tab.
    LoadUsage,
    /// Run a persona-memory search and land on the Memory tab.
    SearchMemory(String),
}

struct ResumePicker {
    chats: Vec<crate::ui::chat_store::MainChatSummary>,
    index: usize,
}

/// One selectable row in the Memory tab's left pane: either the directive/facet
/// overview (no active search) or a ranked search hit.
enum MemoryEntry {
    Directive(String),
    Facet { name: String, count: usize },
    Hit(MemoryHit),
}

/// The action a small inline prompt (Workers add/edit, Agents answer) submits.
enum PromptKind {
    WorkerAdd,
    WorkerEditLabel(String),
    AnswerQuestion {
        cycle_id: String,
        question_id: String,
    },
}

/// A single-line inline input overlay, composer-styled, reused for the fleet and
/// steering prompts.
struct Prompt {
    kind: PromptKind,
    title: String,
    draft: Draft,
}

pub struct App {
    pub runtime: Arc<dyn Runtime>,
    pub loaded: LoadedConfig,
    pub snapshot: RuntimeSnapshot,
    pub tab_index: usize,
    draft: Draft,
    history: Vec<String>,
    history_index: i64,
    selected: usize,
    status: String,
    /// A persistent "update vX.Y.Z available" banner, set by the background
    /// update checker; shown in the header until the app exits.
    update_notice: Option<String>,
    contexts: Vec<ContextItem>,
    context_index: usize,
    agent_index: usize,
    agent_scroll: usize,
    chat_scroll: usize,
    worker_index: usize,
    // Persona-memory tab state (lazily loaded on tab entry / search).
    memory_status: Option<MemoryStatus>,
    memory_hits: Vec<MemoryHit>,
    memory_directives: Vec<String>,
    memory_index: usize,
    memory_query: Option<String>,
    prompt: Option<Prompt>,
    pub frame: usize,
    pub mouse_capture: bool,
    /// Account-level usage payload (`/teams/me/usage` data), when fetched.
    pub account_usage: Option<serde_json::Value>,
    /// The active Settings subpage (index into [`SETTINGS_SUBPAGES`]).
    settings_index: usize,
    /// The selected theme role on the Appearance subpage.
    appearance_index: usize,
    /// The resolved color theme; selection highlighting + chrome draw from it.
    theme: Theme,
    /// Where appearance changes are persisted (the user-global `config.toml`).
    /// Injectable so feature tests never touch the real home. `None` disables
    /// persistence (changes still apply live).
    config_path: Option<std::path::PathBuf>,
    resume_picker: Option<ResumePicker>,
    pub should_quit: bool,

    // Render geometry, recorded each draw for click hit-testing.
    area: Rect,
    hit_tabs: Vec<(u16, u16)>,
    hit_tabs_row: u16,
    hit_agents: Option<(Rect, usize)>,
    hit_context: Option<Rect>,
    hit_threads: Option<(Rect, usize)>,
    last_events_len: usize,

    // Test-only clipboard capture: when set, `copy_chat` records the copied text
    // here and skips the platform writers (no `pbcopy`/OSC subprocess in tests).
    copy_capture: Option<Arc<std::sync::Mutex<Vec<String>>>>,

    // Optional observational overlay from the background tinyplace service:
    // this TUI's own identity, its peer roster, and peer presence. Merged into
    // the snapshot on every refresh so the Overview panel and Agents lanes light
    // up without the runtime having to know about tiny.place.
    tinyplace_obs:
        Option<Arc<std::sync::Mutex<medulla::tinyplace_support::service::TinyplaceObservation>>>,
}

fn color(name: &str) -> Color {
    match name {
        "yellow" => Color::Yellow,
        "green" => Color::Green,
        "red" => Color::Red,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "cyanBright" => Color::LightCyan,
        "gray" | "grey" => Color::DarkGray,
        "white" => Color::White,
        _ => Color::Reset,
    }
}

fn styled_to_tline(line: &StyledLine) -> TLine<'static> {
    let mut style = Style::default();
    if let Some(c) = &line.color {
        style = style.fg(color(c));
    }
    if line.dim {
        style = style.add_modifier(Modifier::DIM);
    }
    let text = if line.text.is_empty() {
        " ".to_string()
    } else {
        line.text.clone()
    };
    TLine::from(Span::styled(text, style))
}

fn event_color(env: &EventEnvelope) -> Option<&'static str> {
    match &env.event {
        TuiEvent::Error { .. } => Some("red"),
        TuiEvent::TaskStart { .. } | TuiEvent::TaskComplete { .. } | TuiEvent::TaskEvent { .. } => {
            Some("magenta")
        }
        TuiEvent::User { .. } => Some("cyan"),
        TuiEvent::Assistant { .. } => Some("green"),
        TuiEvent::AgentStatus { availability, .. } => Some(if availability == "online" {
            "green"
        } else {
            "red"
        }),
        TuiEvent::InferenceStart { .. } | TuiEvent::InferenceEnd { .. } => Some("blue"),
        _ => None,
    }
}

/// Fold the chat event stream into a wrapped conversational transcript.
fn chat_lines(events: &[EventEnvelope], width: usize) -> Vec<StyledLine> {
    let cols = width.max(20);
    let mut out = Vec::new();
    for env in events {
        match &env.event {
            TuiEvent::User { body } => {
                out.push(StyledLine::default());
                for (i, row) in wrap(body, cols.saturating_sub(2)).into_iter().enumerate() {
                    out.push(StyledLine {
                        text: if i == 0 {
                            format!("❯ {row}")
                        } else {
                            format!("  {row}")
                        },
                        color: Some("cyan".into()),
                        dim: false,
                    });
                }
            }
            TuiEvent::Assistant { body } => {
                for (i, row) in wrap(body, cols.saturating_sub(2)).into_iter().enumerate() {
                    out.push(StyledLine {
                        text: if i == 0 {
                            format!("⏺ {row}")
                        } else {
                            format!("  {row}")
                        },
                        color: Some("green".into()),
                        dim: false,
                    });
                }
            }
            TuiEvent::Error { source, message } => {
                for row in wrap(&format!("{source}: {message}"), cols) {
                    out.push(StyledLine {
                        text: row,
                        color: Some("red".into()),
                        dim: false,
                    });
                }
            }
            _ => {}
        }
    }
    out
}

impl App {
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
            prompt: None,
            frame: 0,
            mouse_capture: true,
            account_usage: None,
            settings_index: 0,
            appearance_index: 0,
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

    /// The active Settings subpage name. Test/inspection seam.
    pub fn settings_subpage(&self) -> &'static str {
        SETTINGS_SUBPAGES[self.settings_index.min(SETTINGS_SUBPAGES.len() - 1)]
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
        obs: Arc<std::sync::Mutex<medulla::tinyplace_support::service::TinyplaceObservation>>,
    ) {
        self.tinyplace_obs = Some(obs);
        self.refresh_snapshot();
    }

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

    pub fn set_status(&mut self, s: impl Into<String>) {
        self.status = s.into();
    }

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

    pub fn open_resume(&mut self, chats: Vec<crate::ui::chat_store::MainChatSummary>) {
        if chats.is_empty() {
            self.set_status("No saved chats to resume.");
        } else {
            self.resume_picker = Some(ResumePicker { chats, index: 0 });
            self.set_status("Resume: ↑/↓ select · Enter load · Esc cancel");
        }
    }

    pub fn tab(&self) -> &'static str {
        TABS[self.tab_index]
    }

    /// The lazy-load command a freshly entered tab needs, if any.
    fn tab_enter_cmd(&self) -> Option<Cmd> {
        match self.tab() {
            "Context" => Some(Cmd::InspectContext),
            "Memory" => Some(Cmd::LoadMemory),
            "Settings" if self.settings_index == SP_USAGE => Some(Cmd::LoadUsage),
            _ => None,
        }
    }

    fn lanes(&self) -> Vec<AgentLane> {
        derive_agent_lanes(
            &self.snapshot.events,
            &self.loaded.harness(),
            &self.snapshot.roster,
        )
    }

    fn active_thread_idx(&self) -> usize {
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

    // --- input --------------------------------------------------------------

    pub fn on_event(&mut self, ev: Event) -> Option<Cmd> {
        match ev {
            Event::Key(k) if matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                self.on_key(k)
            }
            Event::Mouse(m) => self.on_mouse(m),
            _ => None,
        }
    }

    fn on_mouse(&mut self, m: crossterm::event::MouseEvent) -> Option<Cmd> {
        if self.resume_picker.is_some() {
            return None; // modal swallows mouse
        }
        let tab = self.tab();
        match m.kind {
            MouseEventKind::ScrollUp => match tab {
                "Chat" => self.chat_scroll += 3,
                "Agents" => self.agent_scroll += 3,
                "Trace" => self.selected = self.selected.saturating_sub(3),
                "Context" => self.context_index = self.context_index.saturating_sub(1),
                "Memory" => self.memory_index = self.memory_index.saturating_sub(1),
                _ => {}
            },
            MouseEventKind::ScrollDown => match tab {
                "Chat" => self.chat_scroll = self.chat_scroll.saturating_sub(3),
                "Agents" => self.agent_scroll = self.agent_scroll.saturating_sub(3),
                "Trace" => self.selected += 3,
                "Context" => {
                    let max = self.contexts.len().saturating_sub(1);
                    self.context_index = (self.context_index + 1).min(max);
                }
                "Memory" => {
                    let max = self.memory_entry_count().saturating_sub(1);
                    self.memory_index = (self.memory_index + 1).min(max);
                }
                _ => {}
            },
            MouseEventKind::Down(MouseButton::Left) => return self.handle_click(m.column, m.row),
            _ => {}
        }
        None
    }

    fn handle_click(&mut self, x: u16, y: u16) -> Option<Cmd> {
        // Tab bar.
        if y == self.hit_tabs_row {
            for (i, (start, end)) in self.hit_tabs.clone().into_iter().enumerate() {
                if x >= start && x <= end {
                    self.tab_index = i;
                    self.selected = 0;
                    return self.tab_enter_cmd();
                }
            }
            return None;
        }
        let tab = self.tab();
        if tab == "Agents" {
            if let Some((rect, window_start)) = self.hit_agents {
                if rect.contains((x, y).into()) {
                    let rel = (y - rect.y) as usize;
                    let rows = self.agent_rows();
                    let idx = window_start + rel;
                    if let Some(row) = rows.get(idx) {
                        if row.selectable() {
                            self.agent_scroll = 0;
                            self.agent_index = idx;
                        }
                    }
                }
            }
        } else if tab == "Context" {
            if let Some(rect) = self.hit_context {
                if rect.contains((x, y).into()) {
                    let rel = (y - rect.y) as usize;
                    if rel < self.contexts.len() {
                        self.context_index = rel;
                    }
                }
            }
        } else if tab == "Chat" {
            if let Some((rect, window_start)) = self.hit_threads {
                if rect.contains((x, y).into()) {
                    let rel = (y - rect.y) as usize;
                    let idx = window_start + rel;
                    if let Some(t) = self.snapshot.threads.get(idx) {
                        let id = t.id.clone();
                        self.runtime.set_active_thread(id);
                        self.chat_scroll = 0;
                        self.refresh_snapshot();
                    }
                }
            }
        }
        None
    }

    fn agent_rows(&self) -> Vec<AgentRow> {
        agent_row_model(&self.lanes(), 8)
    }

    fn visible_count(&self) -> usize {
        (self.area.height as usize).saturating_sub(13).max(5)
    }

    fn on_key(&mut self, k: KeyEvent) -> Option<Cmd> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let alt = k.modifiers.contains(KeyModifiers::ALT);

        // Resume picker owns navigation while open.
        if self.resume_picker.is_some() {
            match k.code {
                KeyCode::Char('c') if ctrl => self.should_quit = true,
                KeyCode::Esc => self.resume_picker = None,
                KeyCode::Up => {
                    if let Some(p) = &mut self.resume_picker {
                        p.index = p.index.saturating_sub(1);
                    }
                }
                KeyCode::Down => {
                    if let Some(p) = &mut self.resume_picker {
                        p.index = (p.index + 1).min(p.chats.len().saturating_sub(1));
                    }
                }
                KeyCode::Enter => {
                    if let Some(p) = self.resume_picker.take() {
                        if let Some(chat) = p.chats.get(p.index) {
                            return Some(Cmd::Resume(chat.session_id.clone()));
                        }
                    }
                }
                _ => {}
            }
            return None;
        }

        // The inline prompt (Workers add/edit, Agents answer) owns input while open.
        if self.prompt.is_some() {
            match k.code {
                KeyCode::Char('c') if ctrl => self.should_quit = true,
                KeyCode::Esc => {
                    self.prompt = None;
                    self.set_status("Cancelled");
                }
                KeyCode::Enter => return self.submit_prompt(),
                KeyCode::Backspace | KeyCode::Delete => {
                    if let Some(p) = &mut self.prompt {
                        p.draft = delete_before(&p.draft.text, p.draft.cursor);
                    }
                }
                KeyCode::Left => {
                    if let Some(p) = &mut self.prompt {
                        p.draft.cursor = p.draft.cursor.saturating_sub(1);
                    }
                }
                KeyCode::Right => {
                    if let Some(p) = &mut self.prompt {
                        p.draft.cursor = (p.draft.cursor + 1).min(p.draft.text.chars().count());
                    }
                }
                KeyCode::Char(c) if !ctrl && !alt => {
                    if let Some(p) = &mut self.prompt {
                        p.draft = insert_at(&p.draft.text, p.draft.cursor, &c.to_string());
                    }
                }
                _ => {}
            }
            return None;
        }

        let tab = self.tab();

        // Global control chords.
        if ctrl {
            match k.code {
                KeyCode::Char('c') => {
                    self.should_quit = true;
                    return None;
                }
                KeyCode::Char('o') => {
                    self.toggle_mouse();
                    return None;
                }
                KeyCode::Char('y') => {
                    self.copy_chat(CopyScope::All);
                    return None;
                }
                KeyCode::Char('x') => {
                    self.runtime.abort();
                    self.set_status("Abort requested");
                    return None;
                }
                KeyCode::Char('n') => {
                    self.runtime.new_session();
                    self.refresh_snapshot();
                    self.set_status("Started a fresh conversation session");
                    return None;
                }
                KeyCode::Char('f') => {
                    self.fork_thread(None);
                    if tab != "Chat" {
                        self.tab_index = tab_pos("Chat");
                    }
                    return None;
                }
                KeyCode::Up | KeyCode::Down if tab == "Chat" => {
                    let idx = self.active_thread_idx();
                    let next = if matches!(k.code, KeyCode::Up) {
                        idx.checked_sub(1)
                    } else {
                        Some(idx + 1)
                    };
                    if let Some(n) = next {
                        if let Some(t) = self.snapshot.threads.get(n) {
                            let id = t.id.clone();
                            self.runtime.set_active_thread(id);
                            self.chat_scroll = 0;
                            self.refresh_snapshot();
                        }
                    }
                    return None;
                }
                _ => {}
            }
        }

        match k.code {
            KeyCode::Tab => {
                self.tab_index = (self.tab_index + 1) % TABS.len();
                self.selected = 0;
                return self.tab_enter_cmd();
            }
            KeyCode::BackTab => {
                self.tab_index = (self.tab_index + TABS.len() - 1) % TABS.len();
                self.selected = 0;
                return self.tab_enter_cmd();
            }
            // Settings tab: subpage navigation + the Appearance theme editor.
            KeyCode::Char(d @ '1'..='4') if tab == "Settings" => {
                return self.set_settings_subpage(d as usize - '1' as usize);
            }
            KeyCode::Char('r') if tab == "Settings" && self.settings_index == SP_USAGE => {
                self.set_status("Usage · refreshing…");
                return Some(Cmd::LoadUsage);
            }
            KeyCode::Char('c') if tab == "Settings" && self.settings_index == SP_USAGE => {
                self.settings_index = SP_CONFIG;
            }
            KeyCode::Up | KeyCode::Down if tab == "Settings" => {
                let up = matches!(k.code, KeyCode::Up);
                self.settings_index = if up {
                    self.settings_index.saturating_sub(1)
                } else {
                    (self.settings_index + 1).min(SETTINGS_SUBPAGES.len() - 1)
                };
                return self.tab_enter_cmd();
            }
            KeyCode::Char('j') | KeyCode::Char('k')
                if tab == "Settings" && self.settings_index == SP_APPEARANCE =>
            {
                let up = matches!(k.code, KeyCode::Char('k'));
                self.appearance_index = if up {
                    self.appearance_index.saturating_sub(1)
                } else {
                    (self.appearance_index + 1).min(THEME_ROLES.len() - 1)
                };
            }
            KeyCode::Char('j') | KeyCode::Char('k') if tab == "Settings" => {
                let up = matches!(k.code, KeyCode::Char('k'));
                self.settings_index = if up {
                    self.settings_index.saturating_sub(1)
                } else {
                    (self.settings_index + 1).min(SETTINGS_SUBPAGES.len() - 1)
                };
                return self.tab_enter_cmd();
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Enter
                if tab == "Settings" && self.settings_index == SP_APPEARANCE =>
            {
                let forward = !matches!(k.code, KeyCode::Left);
                self.cycle_appearance_role(forward);
            }
            KeyCode::PageUp if tab == "Chat" => {
                self.chat_scroll += self.visible_count().saturating_sub(1).max(1);
            }
            KeyCode::PageDown if tab == "Chat" => {
                let step = self.visible_count().saturating_sub(1).max(1);
                self.chat_scroll = self.chat_scroll.saturating_sub(step);
            }
            KeyCode::Enter if tab == "Chat" && (shift || alt) => {
                self.draft = insert_at(&self.draft.text, self.draft.cursor, "\n");
            }
            KeyCode::Enter if tab == "Chat" => {
                let value = self.draft.text.clone();
                return self.execute(value);
            }
            KeyCode::Backspace | KeyCode::Delete if tab == "Chat" => {
                self.draft = delete_before(&self.draft.text, self.draft.cursor);
            }
            KeyCode::Esc if tab == "Chat" => {
                self.draft = Draft::new();
            }
            KeyCode::Left if tab == "Chat" => {
                self.draft.cursor = self.draft.cursor.saturating_sub(1);
            }
            KeyCode::Right if tab == "Chat" => {
                self.draft.cursor = (self.draft.cursor + 1).min(self.draft.text.chars().count());
            }
            KeyCode::Up | KeyCode::Down if tab == "Agents" && self.draft.text.is_empty() => {
                self.agent_scroll = 0;
                self.move_agent_index(matches!(k.code, KeyCode::Up));
            }
            KeyCode::Char('j') if tab == "Agents" && self.draft.text.is_empty() => {
                self.agent_scroll += 1;
            }
            KeyCode::Char('k') if tab == "Agents" && self.draft.text.is_empty() => {
                self.agent_scroll = self.agent_scroll.saturating_sub(1);
            }
            // Agents steering: cancel the selected running task, answer a pending question.
            KeyCode::Char('X') if tab == "Agents" => {
                self.cancel_selected_task();
                return None;
            }
            KeyCode::Char('A') if tab == "Agents" => {
                self.answer_selected_task();
                return None;
            }
            // Workers fleet ops.
            KeyCode::Up if tab == "Workers" => {
                self.worker_index = self.worker_index.saturating_sub(1);
            }
            KeyCode::Down if tab == "Workers" => {
                let max = self.runtime.workers().len().saturating_sub(1);
                self.worker_index = (self.worker_index + 1).min(max);
            }
            KeyCode::Char('a') if tab == "Workers" => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::WorkerAdd,
                    title: "Add worker — address or @handle, optional label".into(),
                    draft: Draft::new(),
                });
                self.set_status("Add worker · Enter save · Esc cancel");
            }
            KeyCode::Char('s') | KeyCode::Enter if tab == "Workers" => {
                if let Some(w) = self.selected_worker() {
                    self.set_status(format!(
                        "Selecting {}",
                        w.label.as_deref().unwrap_or(&w.address)
                    ));
                    return Some(Cmd::WorkerOp(WorkerOp::Select { id: w.id }));
                }
            }
            KeyCode::Char('d') | KeyCode::Char('x') if tab == "Workers" => {
                if let Some(w) = self.selected_worker() {
                    self.set_status(format!(
                        "Removing {}",
                        w.label.as_deref().unwrap_or(&w.address)
                    ));
                    return Some(Cmd::WorkerOp(WorkerOp::Remove { id: w.id }));
                }
            }
            KeyCode::Char('e') if tab == "Workers" => {
                if let Some(w) = self.selected_worker() {
                    let mut draft = Draft::new();
                    if let Some(l) = &w.label {
                        draft = insert_at("", 0, l);
                    }
                    self.prompt = Some(Prompt {
                        kind: PromptKind::WorkerEditLabel(w.id.clone()),
                        title: format!("Edit label — {}", w.address),
                        draft,
                    });
                    self.set_status("Edit label · Enter save · Esc cancel");
                }
            }
            // Memory browse.
            KeyCode::Up if tab == "Memory" => {
                self.memory_index = self.memory_index.saturating_sub(1);
            }
            KeyCode::Down if tab == "Memory" => {
                let max = self.memory_entry_count().saturating_sub(1);
                self.memory_index = (self.memory_index + 1).min(max);
            }
            KeyCode::Char('j') if tab == "Memory" && self.draft.text.is_empty() => {
                let max = self.memory_entry_count().saturating_sub(1);
                self.memory_index = (self.memory_index + 1).min(max);
            }
            KeyCode::Char('k') if tab == "Memory" && self.draft.text.is_empty() => {
                self.memory_index = self.memory_index.saturating_sub(1);
            }
            KeyCode::Up => {
                if tab == "Chat" {
                    if let Some(moved) = move_caret_row(&self.draft.text, self.draft.cursor, -1) {
                        self.draft.cursor = moved;
                    } else {
                        self.recall_older();
                    }
                } else {
                    self.selected = self.selected.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if tab == "Chat" {
                    if let Some(moved) = move_caret_row(&self.draft.text, self.draft.cursor, 1) {
                        self.draft.cursor = moved;
                    } else {
                        self.recall_newer();
                    }
                } else {
                    self.selected += 1;
                }
            }
            KeyCode::Char('j') if tab == "Context" && self.draft.text.is_empty() => {
                let max = self.contexts.len().saturating_sub(1);
                self.context_index = (self.context_index + 1).min(max);
            }
            KeyCode::Char('k') if tab == "Context" && self.draft.text.is_empty() => {
                self.context_index = self.context_index.saturating_sub(1);
            }
            KeyCode::Char(c) if tab == "Chat" && !ctrl && !alt => {
                self.draft = insert_at(&self.draft.text, self.draft.cursor, &c.to_string());
            }
            _ => {}
        }
        None
    }

    fn move_agent_index(&mut self, up: bool) {
        let rows = self.agent_rows();
        if rows.is_empty() {
            return;
        }
        let clamped = self.agent_index.min(rows.len() - 1);
        let step: i64 = if up { -1 } else { 1 };
        let mut next = clamped as i64 + step;
        while next >= 0 && (next as usize) < rows.len() && !rows[next as usize].selectable() {
            next += step;
        }
        self.agent_index = if next < 0 || next as usize >= rows.len() {
            clamped
        } else {
            next as usize
        };
    }

    fn recall_older(&mut self) {
        let next = (self.history.len() as i64 - 1).min(self.history_index + 1);
        if next >= 0 {
            self.history_index = next;
            let recalled = self
                .history
                .get(self.history.len() - 1 - next as usize)
                .cloned()
                .unwrap_or_default();
            self.draft = Draft {
                cursor: recalled.chars().count(),
                text: recalled,
            };
        }
    }

    fn recall_newer(&mut self) {
        if self.history_index >= 0 {
            let next = self.history_index - 1;
            self.history_index = next;
            let recalled = if next >= 0 {
                self.history
                    .get(self.history.len() - 1 - next as usize)
                    .cloned()
                    .unwrap_or_default()
            } else {
                String::new()
            };
            self.draft = Draft {
                cursor: recalled.chars().count(),
                text: recalled,
            };
        }
    }

    fn toggle_mouse(&mut self) {
        self.mouse_capture = !self.mouse_capture;
        self.set_status(if self.mouse_capture {
            "Mouse captured — click tabs/lanes to navigate, wheel scrolls (Shift/Option-drag to copy)"
        } else {
            "Mouse released — native click-drag selection & copy restored"
        });
    }

    fn fork_thread(&mut self, name: Option<String>) {
        let label = name.clone().unwrap_or_else(|| "new thread".into());
        self.runtime.fork(name);
        self.chat_scroll = 0;
        self.refresh_snapshot();
        self.set_status(format!("Forked → {label} (inherits history; fresh fleet)"));
    }

    // --- fleet & steering helpers -------------------------------------------

    fn selected_worker(&self) -> Option<WorkerInfo> {
        let ws = self.runtime.workers();
        if ws.is_empty() {
            return None;
        }
        ws.get(self.worker_index.min(ws.len() - 1)).cloned()
    }

    /// The task under the Agents-list cursor, when a `Sub` (task) row is selected.
    fn selected_agent_task(&self) -> Option<TaskState> {
        let rows = self.agent_rows();
        match rows.get(self.agent_index) {
            Some(AgentRow::Sub { task, .. }) => Some(task.clone()),
            _ => None,
        }
    }

    fn cancel_selected_task(&mut self) {
        match self.selected_agent_task() {
            Some(t) => {
                let (cycle, task) = crate::ui::agents::parse_task_key(&t.task_id);
                match cycle {
                    Some(c) => {
                        self.runtime.cancel_task(c.to_string(), task.to_string());
                        self.set_status(format!("Cancel requested · {task}"));
                    }
                    None => self.set_status("Selected task has no cycle to cancel"),
                }
            }
            None => self.set_status("Select a running task (↑↓) to cancel with X"),
        }
    }

    fn answer_selected_task(&mut self) {
        match self.selected_agent_task() {
            Some(t) => match (
                t.question_id.clone(),
                crate::ui::agents::parse_task_key(&t.task_id).0,
            ) {
                (Some(qid), Some(cycle)) => {
                    self.prompt = Some(Prompt {
                        kind: PromptKind::AnswerQuestion {
                            cycle_id: cycle.to_string(),
                            question_id: qid,
                        },
                        title: t
                            .attention
                            .clone()
                            .map(|a| format!("Answer — {a}"))
                            .unwrap_or_else(|| "Answer the pending question".into()),
                        draft: Draft::new(),
                    });
                    self.set_status("Type an answer · Enter send · Esc cancel");
                }
                _ => self.set_status("Selected task has no pending question"),
            },
            None => self.set_status("Select a task (↑↓) with a pending question to answer"),
        }
    }

    /// Submit the open inline prompt, producing the follow-up command (if any) and
    /// closing the overlay.
    fn submit_prompt(&mut self) -> Option<Cmd> {
        let p = self.prompt.take()?;
        let text = p.draft.text.trim().to_string();
        match p.kind {
            PromptKind::WorkerAdd => match WorkerOp::parse_add(&text) {
                Some(op) => {
                    self.set_status("Adding worker…");
                    Some(Cmd::WorkerOp(op))
                }
                None => {
                    self.set_status("Add cancelled (empty)");
                    None
                }
            },
            PromptKind::WorkerEditLabel(id) => {
                let mut patch = serde_json::Map::new();
                patch.insert("label".into(), serde_json::Value::String(text));
                self.set_status("Updating label…");
                Some(Cmd::WorkerOp(WorkerOp::Update { id, patch }))
            }
            PromptKind::AnswerQuestion {
                cycle_id,
                question_id,
            } => {
                if text.is_empty() {
                    self.set_status("Answer cancelled (empty)");
                    return None;
                }
                self.runtime.answer_question(cycle_id, question_id, text);
                self.set_status("Answer sent");
                None
            }
        }
    }

    fn copy_chat(&mut self, scope: CopyScope) {
        let text = copy_text(&self.snapshot.chat_events, &scope);
        if text.trim().is_empty() {
            self.set_status(match scope {
                CopyScope::Last => "No assistant reply to copy yet.",
                CopyScope::All => "Nothing to copy yet.",
            });
            return;
        }
        if let Some(sink) = &self.copy_capture {
            sink.lock().expect("copy sink").push(text.clone());
            let rows = text.split('\n').count();
            let what = match scope {
                CopyScope::Last => "last reply",
                CopyScope::All => "chat",
            };
            self.set_status(format!(
                "Copied {what} · {rows} line{} · {} chars (captured)",
                if rows == 1 { "" } else { "s" },
                text.len()
            ));
            return;
        }
        let via = copy_to_clipboard(&text, current_platform(), |osc| {
            use std::io::Write;
            let _ = std::io::stdout().write_all(osc.as_bytes());
            let _ = std::io::stdout().flush();
        });
        let rows = text.split('\n').count();
        let what = match scope {
            CopyScope::Last => "last reply",
            CopyScope::All => "chat",
        };
        let size = format!(
            "{rows} line{} · {} chars",
            if rows == 1 { "" } else { "s" },
            text.len()
        );
        self.set_status(if via == OSC_52 {
            format!("Sent {what} · {size} → terminal (OSC 52); check your clipboard")
        } else {
            format!("Copied {what} · {size} → clipboard ({via})")
        });
    }

    /// Handle a submitted composer line (a plain turn or a slash command).
    fn execute(&mut self, value: String) -> Option<Cmd> {
        let clean = value.trim().to_string();
        if clean.is_empty() {
            return None;
        }
        self.history.push(clean.clone());
        self.history_index = -1;
        self.draft = Draft::new();
        self.chat_scroll = 0;

        if let Some(rest) = clean.strip_prefix('/') {
            let lower = rest.trim().to_lowercase();
            let (cmd, arg) = match lower.split_once(' ') {
                Some((c, a)) => (c.to_string(), a.trim().to_string()),
                None => (lower.clone(), String::new()),
            };
            match cmd.as_str() {
                "quit" | "q" => self.should_quit = true,
                "new" => {
                    self.runtime.new_session();
                    self.refresh_snapshot();
                    self.set_status("Started a fresh conversation session");
                }
                "fork" => {
                    // Preserve original case for the name.
                    let name = clean[1..].trim()["fork".len()..].trim().to_string();
                    self.fork_thread(if name.is_empty() { None } else { Some(name) });
                    self.tab_index = tab_pos("Chat");
                }
                "resume" => return Some(Cmd::ListChats),
                "abort" => {
                    self.runtime.abort();
                    self.set_status("Abort requested");
                }
                "clear" => {
                    self.selected = 0;
                    self.set_status("View reset (runtime history is retained)");
                }
                "help" => {
                    self.set_settings_subpage(SP_HELP);
                }
                "config" => {
                    self.set_settings_subpage(SP_CONFIG);
                }
                "settings" | "theme" => {
                    self.set_settings_subpage(SP_APPEARANCE);
                }
                "usage" => return self.set_settings_subpage(SP_USAGE),
                "memory" | "mem" => {
                    self.tab_index = tab_pos("Memory");
                    // Preserve original case for the query.
                    let query = clean[1..].trim()[cmd.len()..].trim().to_string();
                    if query.is_empty() {
                        self.set_status("Memory · loading persona…");
                        return Some(Cmd::LoadMemory);
                    }
                    self.set_status(format!("Memory · searching “{query}”…"));
                    return Some(Cmd::SearchMemory(query));
                }
                "mouse" => self.toggle_mouse(),
                "copy" => {
                    if !arg.is_empty() && arg != "all" && arg != "last" {
                        self.set_status("Usage: /copy [all|last]");
                    } else {
                        self.copy_chat(if arg == "last" {
                            CopyScope::Last
                        } else {
                            CopyScope::All
                        });
                    }
                }
                "async" => {
                    if !arg.is_empty() && arg != "on" && arg != "off" {
                        self.set_status("Usage: /async [on|off]");
                    } else {
                        let on = if arg.is_empty() {
                            !self.snapshot.async_mode
                        } else {
                            arg == "on"
                        };
                        self.runtime.set_async_mode(on);
                        self.refresh_snapshot();
                        self.set_status(if on {
                            "async ON — delegations detach; chat stays free while sub-agents work"
                        } else {
                            "async OFF — delegations await their results before the reply"
                        });
                    }
                }
                _ => self.set_status(format!("Unknown command: {clean}")),
            }
            return None;
        }

        self.set_status("Cycle running…");
        Some(Cmd::Submit(clean))
    }

    // --- settings -----------------------------------------------------------

    /// Land on the Settings tab at subpage `index`, returning its lazy-load
    /// command (Usage fetches account usage on entry).
    fn set_settings_subpage(&mut self, index: usize) -> Option<Cmd> {
        self.tab_index = tab_pos("Settings");
        self.settings_index = index.min(SETTINGS_SUBPAGES.len() - 1);
        self.tab_enter_cmd()
    }

    /// Cycle the selected Appearance role's color, apply it to the live theme,
    /// and persist the `[theme]` section.
    fn cycle_appearance_role(&mut self, forward: bool) {
        let role = self.appearance_index.min(THEME_ROLES.len() - 1);
        self.theme.cycle_role(role, forward);
        self.persist_theme_now(THEME_ROLES[role]);
    }

    /// Write the current theme to the injected config path, surfacing a status
    /// note on success or failure. A `None` path applies live but does not save.
    fn persist_theme_now(&mut self, role: &str) {
        let value = color_to_string(self.theme.role(self.appearance_index));
        match &self.config_path {
            Some(path) => match crate::ui::theme::persist_theme(path, &self.theme) {
                Ok(()) => self.set_status(format!("Appearance · {role} → {value} (saved)")),
                Err(e) => self.set_status(format!("Appearance · save failed: {e}")),
            },
            None => self.set_status(format!("Appearance · {role} → {value} (not persisted)")),
        }
    }

    // --- render -------------------------------------------------------------

    pub fn draw(&mut self, f: &mut Frame) {
        self.area = f.area();
        let chat = self.tab() == "Chat";
        let has_prompt = self.prompt.is_some();
        let extra = if has_prompt {
            3
        } else if chat {
            self.extra_height()
        } else {
            0
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Length(1), // tabs
                Constraint::Min(0),    // content
                Constraint::Length(extra),
                Constraint::Length(1), // footer
            ])
            .split(self.area);

        self.draw_header(f, rows[0]);
        self.draw_tabs(f, rows[1]);
        self.draw_content(f, rows[2]);
        if has_prompt {
            self.draw_prompt(f, rows[3]);
        } else if chat {
            if self.resume_picker.is_some() {
                self.draw_resume(f, rows[3]);
            } else {
                self.draw_composer(f, rows[3]);
            }
        }
        self.draw_footer(f, rows[4]);
    }

    fn extra_height(&self) -> u16 {
        if let Some(p) = &self.resume_picker {
            let cap = ((self.area.height as usize).saturating_sub(9)).max(3);
            (p.chats.len().min(cap) as u16 + 3).min(self.area.height / 2)
        } else {
            let lines = self.draft.text.split('\n').count() as u16;
            lines.max(1) + 2
        }
    }

    fn draw_header(&mut self, f: &mut Frame, area: Rect) {
        let halves = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);
        let mut spans = vec![
            Span::styled(
                "MEDULLA",
                Style::default()
                    .fg(self.theme.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
        ];
        if self.snapshot.async_mode {
            spans.push(Span::styled(
                "⚡ ASYNC ON",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                "async off",
                Style::default().add_modifier(Modifier::DIM),
            ));
        }
        if let Some(notice) = &self.update_notice {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                notice.clone(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        f.render_widget(Paragraph::new(TLine::from(spans)), halves[0]);
        // Stream health sits right next to the status when a cycle runs under a
        // runtime that tracks one (the core runtime); otherwise just the status.
        let mut right: Vec<Span> = Vec::new();
        if self.snapshot.running {
            if let Some(st) = self.runtime.stream_state() {
                let c = match st {
                    medulla::runtime::StreamState::Live => Color::Green,
                    medulla::runtime::StreamState::Resyncing => Color::Yellow,
                    medulla::runtime::StreamState::Stalled => Color::Red,
                };
                right.push(Span::styled(
                    format!("{} {}  ", st.glyph(), st.label()),
                    Style::default().fg(c),
                ));
            }
        }
        right.push(Span::styled(
            self.status.clone(),
            Style::default().add_modifier(Modifier::DIM),
        ));
        f.render_widget(
            Paragraph::new(TLine::from(right)).alignment(Alignment::Right),
            halves[1],
        );
    }

    fn draw_tabs(&mut self, f: &mut Frame, area: Rect) {
        self.hit_tabs.clear();
        self.hit_tabs_row = area.y;
        let mut spans = Vec::new();
        let mut col = area.x;
        for (i, name) in TABS.iter().enumerate() {
            let label = format!(" {name} ");
            let w = label.chars().count() as u16;
            self.hit_tabs.push((col, col + w - 1));
            let mut style = Style::default();
            if i == self.tab_index {
                style = self.theme.selection();
            }
            spans.push(Span::styled(label, style));
            spans.push(Span::raw(" "));
            col += w + 1;
        }
        f.render_widget(Paragraph::new(TLine::from(spans)), area);
    }

    fn draw_footer(&mut self, f: &mut Frame, area: Rect) {
        let text = format!(
            "Tab views · ↑↓ history/nav · ⇧⏎ newline · ^Y copy · ^F fork · ^↑↓ thread · ^X abort · ^O mouse {} · /async {} · /help",
            if self.mouse_capture { "●" } else { "○" },
            if self.snapshot.async_mode { "on" } else { "off" },
        );
        f.render_widget(
            Paragraph::new(TLine::from(Span::styled(
                text,
                Style::default().add_modifier(Modifier::DIM),
            )))
            .wrap(Wrap { trim: true }),
            area,
        );
    }

    fn panel<'a>(&self, title: impl Into<String>) -> Block<'a> {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.theme.dim_border))
            .title(Span::styled(
                title.into(),
                Style::default()
                    .fg(self.theme.primary)
                    .add_modifier(Modifier::BOLD),
            ))
    }

    fn draw_content(&mut self, f: &mut Frame, area: Rect) {
        match self.tab() {
            "Overview" => self.draw_overview(f, area),
            "Chat" => self.draw_chat(f, area),
            "Agents" => self.draw_agents(f, area),
            "Workers" => self.draw_workers(f, area),
            "Trace" => self.draw_trace(f, area),
            "Context" => self.draw_context(f, area),
            "Memory" => self.draw_memory(f, area),
            "Settings" => self.draw_settings(f, area),
            _ => self.draw_overview(f, area),
        }
    }

    fn draw_settings(&mut self, f: &mut Frame, area: Rect) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(16), Constraint::Min(0)])
            .split(area);

        // Left nav: subpage list.
        let block = self.panel("Settings");
        let inner = block.inner(cols[0]);
        f.render_widget(block, cols[0]);
        let mut lines: Vec<TLine> = Vec::new();
        for (i, name) in SETTINGS_SUBPAGES.iter().enumerate() {
            let style = if i == self.settings_index {
                self.theme.selection()
            } else {
                Style::default()
            };
            lines.push(TLine::from(Span::styled(
                format!(" {} {name} ", i + 1),
                style,
            )));
        }
        lines.push(TLine::from(Span::styled(
            "↑↓ nav · 1-4 jump",
            Style::default().add_modifier(Modifier::DIM),
        )));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);

        // Right content: the active subpage.
        match self.settings_index {
            SP_USAGE => self.draw_usage(f, cols[1]),
            SP_APPEARANCE => self.draw_appearance(f, cols[1]),
            SP_CONFIG => self.draw_config(f, cols[1]),
            _ => self.draw_help(f, cols[1]),
        }
    }

    fn draw_appearance(&mut self, f: &mut Frame, area: Rect) {
        let block = self.panel("Appearance");
        let inner = block.inner(area);
        f.render_widget(block, area);
        let sel = self.appearance_index.min(THEME_ROLES.len() - 1);
        let mut lines: Vec<TLine> = Vec::new();
        for (i, role) in THEME_ROLES.iter().enumerate() {
            let c = self.theme.role(i);
            let text_style = if i == sel {
                self.theme.selection()
            } else {
                Style::default()
            };
            let marker = if i == sel { "▸ " } else { "  " };
            lines.push(TLine::from(vec![
                Span::styled(marker, text_style),
                Span::styled("███ ", Style::default().fg(c)),
                Span::styled(format!("{role:<13} {}", color_to_string(c)), text_style),
            ]));
        }
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled(
            "j/k select role · ←/→ or Enter cycle color · applies live",
            Style::default().add_modifier(Modifier::DIM),
        )));
        let where_saved = match &self.config_path {
            Some(p) => format!("saved to {}", p.display()),
            None => "changes apply live (no config path set)".into(),
        };
        lines.push(TLine::from(Span::styled(
            where_saved,
            Style::default().add_modifier(Modifier::DIM),
        )));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    fn draw_overview(&mut self, f: &mut Frame, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4),
                Constraint::Length(7),
                Constraint::Length(5),
                Constraint::Min(0),
            ])
            .split(area);
        let logo: Vec<TLine> = crate::ui::LOGO
            .iter()
            .map(|row| {
                TLine::from(Span::styled(
                    *row,
                    Style::default()
                        .fg(self.theme.primary)
                        .add_modifier(Modifier::BOLD),
                ))
            })
            .collect();
        f.render_widget(Paragraph::new(Text::from(logo)), rows[0]);
        let rows = &rows[1..];
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(33),
                Constraint::Percentage(33),
                Constraint::Percentage(34),
            ])
            .split(rows[0]);

        // Session panel.
        let mut session = vec![
            TLine::from(format!("id {}", clip(&self.snapshot.session_id, 24))),
            TLine::from(format!(
                "turns {}",
                self.snapshot.messages.len().div_ceil(2)
            )),
            TLine::from(Span::styled(
                if self.snapshot.running {
                    "● running"
                } else {
                    "● idle"
                },
                Style::default().fg(if self.snapshot.running {
                    Color::Yellow
                } else {
                    Color::Green
                }),
            )),
        ];
        session.push(if self.snapshot.async_mode {
            TLine::from(Span::styled(
                "async ● on",
                Style::default().fg(Color::Magenta),
            ))
        } else {
            TLine::from(Span::styled(
                "async ○ off",
                Style::default().add_modifier(Modifier::DIM),
            ))
        });
        session.push(if self.snapshot.tracing {
            TLine::from(Span::styled(
                "langfuse ● tracing",
                Style::default().fg(Color::Green),
            ))
        } else {
            TLine::from(Span::styled(
                "langfuse ○ off",
                Style::default().add_modifier(Modifier::DIM),
            ))
        });
        f.render_widget(
            Paragraph::new(Text::from(session)).block(self.panel("Session")),
            top[0],
        );

        // Orchestration panel.
        let running_calls = stream::running_calls(&self.snapshot.events);
        let completed = self
            .snapshot
            .last_result
            .as_ref()
            .map(|r| r.task_ledger.len())
            .unwrap_or(0);
        let passes = self
            .snapshot
            .last_result
            .as_ref()
            .map(|r| r.pass_count.to_string())
            .unwrap_or_else(|| "—".into());
        let orch = vec![
            TLine::from(format!("passes {passes}")),
            TLine::from(format!("agents {completed}")),
            TLine::from(format!("active model calls {running_calls}")),
        ];
        f.render_widget(
            Paragraph::new(Text::from(orch)).block(self.panel("Orchestration")),
            top[1],
        );

        // Third panel: tinyplace or opencode.
        self.draw_overview_third(f, top[2]);

        // Model routing: inference is server-managed, so show the runtime we
        // are attached to plus the models actually observed on the stream.
        let workers_val = if let Some(tp) = &self.loaded.config.tinyplace {
            format!("tiny.place · {} peer(s)", tp.peers.len())
        } else {
            self.loaded
                .config
                .opencode
                .as_ref()
                .map(|o| o.model.clone())
                .unwrap_or_default()
        };
        let mut routing = vec![TLine::from(vec![
            Span::styled("runtime ", Style::default().fg(self.theme.primary)),
            Span::raw(self.runtime.describe()),
        ])];
        for (label, tier, color) in [
            ("orchestrator ", "orchestrator", Color::Yellow),
            ("reasoning ", "reasoning", Color::Yellow),
            ("summarizer ", "compress", Color::Blue),
        ] {
            routing.push(TLine::from(vec![
                Span::styled(label, Style::default().fg(color)),
                Span::raw(
                    stream::observed_model(&self.snapshot.events, tier)
                        .unwrap_or("—")
                        .to_string(),
                ),
            ]));
        }
        routing.push(TLine::from(vec![
            Span::styled("workers ", Style::default().fg(Color::Magenta)),
            Span::raw(workers_val),
        ]));
        f.render_widget(
            Paragraph::new(Text::from(routing)).block(self.panel("Model routing")),
            rows[1],
        );

        // Live activity.
        let take = self.visible_count().saturating_sub(7).max(5);
        let start = self.snapshot.events.len().saturating_sub(take);
        let recent: Vec<TLine> = self.snapshot.events[start..]
            .iter()
            .map(|e| self.event_line(e, area.width.saturating_sub(6) as usize, false))
            .collect();
        let body = if recent.is_empty() {
            Text::from(TLine::from(Span::styled(
                "No events yet.",
                Style::default().add_modifier(Modifier::DIM),
            )))
        } else {
            Text::from(recent)
        };
        f.render_widget(
            Paragraph::new(body).block(self.panel("Live activity")),
            rows[2],
        );
    }

    fn draw_overview_third(&self, f: &mut Frame, area: Rect) {
        if let Some(tp) = &self.loaded.config.tinyplace {
            let peers: Vec<_> = self
                .snapshot
                .roster
                .iter()
                .filter(|a| a.metadata.get("harness").and_then(|v| v.as_str()) == Some("tinyplace"))
                .collect();
            let readings = peers
                .iter()
                .filter(|a| self.snapshot.presence.contains_key(&a.id))
                .count();
            let online = peers
                .iter()
                .filter(|a| {
                    self.snapshot
                        .presence
                        .get(&a.id)
                        .map(|p| p.online)
                        .unwrap_or(false)
                })
                .count();
            let all_sessions: Vec<_> = self.snapshot.sessions.values().flatten().collect();
            let live = all_sessions.iter().filter(|s| s.state != "ended").count();
            let mut lines = vec![TLine::from(tp.base_url.clone())];
            if readings > 0 {
                lines.push(TLine::from(Span::styled(
                    format!("agents {online}/{} online", peers.len()),
                    Style::default().fg(if online > 0 { Color::Green } else { Color::Red }),
                )));
            } else {
                lines.push(TLine::from(format!(
                    "agents {} · presence pending",
                    peers.len()
                )));
            }
            if !all_sessions.is_empty() {
                lines.push(TLine::from(format!(
                    "sessions {live} live / {} known",
                    all_sessions.len()
                )));
            }
            if let Some(me) = &self.snapshot.tinyplace {
                let who = me.handle.clone().unwrap_or_else(|| clip(&me.agent_id, 24));
                lines.push(TLine::from(format!("me {who}")));
            } else {
                lines.push(TLine::from(Span::styled(
                    "me · connecting…",
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
            f.render_widget(
                Paragraph::new(Text::from(lines)).block(self.panel("tiny.place")),
                area,
            );
        } else {
            let oc = self.loaded.config.opencode.clone().unwrap_or_default();
            let lines = vec![
                TLine::from(oc.model),
                TLine::from(format!("agent {}", oc.agent)),
                TLine::from(format!("concurrency {}", oc.max_concurrency)),
            ];
            f.render_widget(
                Paragraph::new(Text::from(lines)).block(self.panel("OpenCode workers")),
                area,
            );
        }
    }

    fn event_line(&self, env: &EventEnvelope, width: usize, selected: bool) -> TLine<'static> {
        let mut style = Style::default().fg(color(event_color(env).unwrap_or("white")));
        if selected {
            style = self.theme.selection();
        }
        let text = format!(
            "{} {}",
            clock(env.at),
            clip(&describe_event(&env.event), width.saturating_sub(11))
        );
        TLine::from(Span::styled(text, style))
    }

    fn draw_chat(&mut self, f: &mut Frame, area: Rect) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(26), Constraint::Min(0)])
            .split(area);

        // Threads sidebar.
        let block = self.panel(format!("Threads · {}", self.snapshot.threads.len()));
        let inner = block.inner(cols[0]);
        f.render_widget(block, cols[0]);
        let cap = (inner.height as usize).saturating_sub(1).max(1);
        let active_idx = self.active_thread_idx();
        let window_start = active_idx
            .saturating_sub(cap / 2)
            .min(self.snapshot.threads.len().saturating_sub(cap));
        self.hit_threads = Some((inner, window_start));
        let depth = stream::thread_depths(&self.snapshot.threads);
        let mut lines: Vec<TLine> = Vec::new();
        for t in self.snapshot.threads.iter().skip(window_start).take(cap) {
            let d = *depth.get(&t.id).unwrap_or(&0);
            let indent = if d == 0 {
                String::new()
            } else {
                format!("{}⑃ ", "  ".repeat(d - 1))
            };
            let marker = if t.running { "▶" } else { "●" };
            let mut badges = Vec::new();
            if t.running_tasks > 0 {
                badges.push(format!("{} run", t.running_tasks));
            }
            if t.attention > 0 {
                badges.push(format!("{}⚠", t.attention));
            }
            let badge = if badges.is_empty() {
                String::new()
            } else {
                format!(" · {}", badges.join(" "))
            };
            let text = format!("{indent}{marker} {} · {}t{badge}", t.name, t.turns);
            let mut style = Style::default();
            if t.running {
                style = style.fg(Color::Yellow);
            }
            if t.id == self.snapshot.active_thread_id {
                style = self.theme.selection();
            }
            lines.push(TLine::from(Span::styled(text, style)));
        }
        lines.push(TLine::from(Span::styled(
            "^F fork · ^↑↓ switch",
            Style::default().add_modifier(Modifier::DIM),
        )));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);

        // Transcript.
        let name = self
            .snapshot
            .threads
            .get(active_idx)
            .map(|t| t.name.clone())
            .unwrap_or_else(|| "main".into());
        let title = format!(
            "{name} · {} turns",
            self.snapshot.messages.len().div_ceil(2)
        );
        let block = self.panel(title);
        let inner = block.inner(cols[1]);
        f.render_widget(block, cols[1]);
        let width = inner.width as usize;
        let capacity = (inner.height as usize).saturating_sub(1).max(4);
        let lines = chat_lines(&self.snapshot.chat_events, width.saturating_sub(2));
        let max_scroll = lines.len().saturating_sub(capacity);
        let eff = self.chat_scroll.min(max_scroll);
        self.chat_scroll = eff;
        let end = lines.len() - eff;
        let view = &lines[end.saturating_sub(capacity)..end];
        let mut out: Vec<TLine> = if view.is_empty() {
            vec![TLine::from(Span::styled(
                "No messages yet — type below to start.",
                Style::default().add_modifier(Modifier::DIM),
            ))]
        } else {
            view.iter().map(styled_to_tline).collect()
        };
        // Status row.
        if eff > 0 {
            out.push(TLine::from(Span::styled(
                format!("↑ {eff} line(s) below · scroll down / PageDown to catch up"),
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else if self.snapshot.running {
            let rc = stream::running_calls(&self.snapshot.events);
            let msg = if rc > 0 {
                format!(
                    "thinking · {rc} model call{} in flight",
                    if rc == 1 { "" } else { "s" }
                )
            } else {
                "working…".into()
            };
            out.push(TLine::from(Span::styled(
                format!("{} {msg}", SPINNER[self.frame % SPINNER.len()]),
                Style::default().fg(Color::Yellow),
            )));
        }
        f.render_widget(Paragraph::new(Text::from(out)), inner);
    }

    fn draw_agents(&mut self, f: &mut Frame, area: Rect) {
        let lanes = self.lanes();
        let rows = self.agent_rows();
        let active = self.agent_index.min(rows.len().saturating_sub(1));
        self.agent_index = active;
        let selected_row = rows.get(active);
        let active_lane_index = selected_row.and_then(|r| r.lane_index()).unwrap_or(0);
        let selected_task: Option<TaskState> = match selected_row {
            Some(AgentRow::Sub { task, .. }) => Some(task.clone()),
            _ => None,
        };

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(36), Constraint::Min(0)])
            .split(area);

        let running_tasks: usize = lanes
            .iter()
            .map(|l| {
                l.tasks
                    .iter()
                    .filter(|t| t.status == TaskStatus::Running)
                    .count()
            })
            .sum();
        let agent_count = lanes.iter().filter(|l| !l.role.is_function()).count();
        let title = if running_tasks > 0 {
            format!("Agents · {agent_count} · {running_tasks} running")
        } else {
            format!("Agents · {agent_count}")
        };
        let block = self.panel(title);
        let inner = block.inner(cols[0]);
        f.render_widget(block, cols[0]);
        let capacity = (inner.height as usize).max(1);
        let window_start = active
            .saturating_sub(capacity / 2)
            .min(rows.len().saturating_sub(capacity));
        self.hit_agents = Some((inner, window_start));
        let mut lines: Vec<TLine> = Vec::new();
        for (offset, row) in rows.iter().skip(window_start).take(capacity).enumerate() {
            let idx = window_start + offset;
            lines.push(self.agent_row_line(row, &lanes, idx == active));
        }
        f.render_widget(Paragraph::new(Text::from(lines)), inner);

        // Transcript pane.
        let lane = lanes.get(active_lane_index);
        let pane_width = ((cols[1].width as usize).saturating_sub(4)).max(24);
        let content_lines: Vec<StyledLine> = if let Some(t) = &selected_task {
            task_lines(t, pane_width)
        } else {
            lane_lines(lane, pane_width)
        };
        let title = if let Some(t) = &selected_task {
            format!(
                "{} › {} · {} turns",
                lane.map(|l| l.label.as_str()).unwrap_or("task"),
                t.task_id,
                t.turns
            )
        } else if let Some(l) = lane {
            format!("{} · {} turns", l.label, l.turns.len())
        } else {
            "Transcript".into()
        };
        let block = self.panel(title);
        let inner = block.inner(cols[1]);
        f.render_widget(block, cols[1]);
        let mut header: Vec<TLine> = Vec::new();
        // Context bar.
        if let Some(l) = lane {
            if let Some(used) = l.context_tokens {
                let window = self.loaded.config.medulla.context_window() as i64;
                let pct = ((used as f64 / window as f64) * 100.0).round().min(100.0) as i64;
                let filled = ((pct as f64 / 100.0) * 16.0).round() as usize;
                let bar = format!(
                    "{}{}",
                    "█".repeat(filled),
                    "░".repeat(16usize.saturating_sub(filled))
                );
                let c = if pct >= 90 {
                    Color::Red
                } else if pct >= 70 {
                    Color::Yellow
                } else {
                    Color::DarkGray
                };
                let detail = if l.role == AgentRole::Worker {
                    format!("{} tokens", fmt_tokens(used))
                } else {
                    format!("{}/{} ({pct}%)", fmt_tokens(used), fmt_tokens(window))
                };
                header.push(TLine::from(Span::styled(
                    format!("context {bar} {detail}"),
                    Style::default().fg(c),
                )));
            }
        }
        let capacity = (inner.height as usize).saturating_sub(header.len()).max(4);
        let max_scroll = content_lines.len().saturating_sub(capacity);
        let eff = self.agent_scroll.min(max_scroll);
        let end = content_lines.len() - eff;
        let view = &content_lines[end.saturating_sub(capacity)..end];
        let mut out = header;
        out.extend(view.iter().map(styled_to_tline));
        if eff > 0 {
            out.push(TLine::from(Span::styled(
                format!("↑ {eff} more line(s) below · k to catch up"),
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        f.render_widget(Paragraph::new(Text::from(out)), inner);
    }

    fn agent_row_line(&self, row: &AgentRow, lanes: &[AgentLane], active: bool) -> TLine<'static> {
        match row {
            AgentRow::Separator => TLine::from(Span::styled(
                "── functions ──",
                Style::default().add_modifier(Modifier::DIM),
            )),
            AgentRow::More { hidden, .. } => TLine::from(Span::styled(
                format!("   └ +{hidden} more"),
                Style::default().add_modifier(Modifier::DIM),
            )),
            AgentRow::Sub { task, last, .. } => {
                let branch = if *last { "└" } else { "├" };
                let mut style = Style::default();
                if active {
                    style = self.theme.selection();
                }
                let status_style = if active {
                    style
                } else {
                    style.fg(color(task.status.color()))
                };
                TLine::from(vec![
                    Span::styled(format!("   {branch} {} · ", task.task_id), style),
                    Span::styled(task.status.label().to_string(), status_style),
                    Span::styled(format!(" · {} turns", task.turns), style),
                ])
            }
            AgentRow::Lane { lane_index } => {
                let Some(item) = lanes.get(*lane_index) else {
                    return TLine::from("");
                };
                let window = self.loaded.config.medulla.context_window() as i64;
                let is_fn = item.role.is_function();
                let ctx = match item.context_tokens {
                    None => String::new(),
                    Some(used) if item.role == AgentRole::Worker => {
                        format!(" · ctx {}", fmt_tokens(used))
                    }
                    Some(used) => format!(
                        " · ctx {}/{} {}%",
                        fmt_tokens(used),
                        fmt_tokens(window),
                        ((used as f64 / window as f64) * 100.0).round() as i64
                    ),
                };
                let marker = self.lane_marker(item, is_fn);
                let state = self.lane_state(item);
                let sessions_note = if let Some(aid) = &item.agent_id {
                    let list = self.snapshot.sessions.get(aid).cloned().unwrap_or_default();
                    if list.is_empty() {
                        String::new()
                    } else {
                        let live = list.iter().filter(|s| s.state != "ended").count();
                        format!(" · {}/{} sess", live, list.len())
                    }
                } else {
                    String::new()
                };
                let mut style = Style::default().fg(color(item.role.color()));
                if is_fn {
                    style = style.add_modifier(Modifier::DIM);
                }
                if active {
                    style = self.theme.selection();
                }
                let text = format!(
                    "{marker} {} · {}{ctx}{state}{sessions_note}",
                    item.label,
                    item.turns.len()
                );
                TLine::from(Span::styled(text, style))
            }
        }
    }

    fn lane_marker(&self, item: &AgentLane, is_fn: bool) -> &'static str {
        if is_fn {
            "ƒ"
        } else if item.role != AgentRole::Worker {
            "●"
        } else if item.session_id.is_some() {
            let state = self.session_state(item);
            match state.as_deref() {
                Some("ended") => "○",
                _ => "●",
            }
        } else if let Some(aid) = &item.agent_id {
            match self.snapshot.presence.get(aid) {
                Some(p) => {
                    if p.online {
                        "●"
                    } else {
                        "○"
                    }
                }
                None if item.descriptor.is_some() => "◌",
                None => "◆",
            }
        } else if item.descriptor.is_some() {
            "◌"
        } else {
            "◆"
        }
    }

    fn session_state(&self, item: &AgentLane) -> Option<String> {
        let (sid, pid) = (item.session_id.as_ref()?, item.parent_agent_id.as_ref()?);
        self.snapshot
            .sessions
            .get(pid)?
            .iter()
            .find(|s| &s.id == sid)
            .map(|s| s.state.clone())
    }

    fn lane_state(&self, item: &AgentLane) -> String {
        if item.session_id.is_some() {
            let s = self.session_state(item);
            match s.as_deref() {
                Some("ended") => " · inactive".into(),
                Some(other) => format!(" · {other}"),
                None => " · …".into(),
            }
        } else if item.role == AgentRole::Worker {
            if item.active_tasks > 0 {
                " · busy".into()
            } else if item.turns.is_empty() {
                " · idle".into()
            } else {
                String::new()
            }
        } else {
            String::new()
        }
    }

    fn draw_workers(&mut self, f: &mut Frame, area: Rect) {
        let workers = self.runtime.workers();
        let selected = if workers.is_empty() {
            0
        } else {
            self.worker_index.min(workers.len() - 1)
        };
        self.worker_index = selected;
        let title = format!("Workers · {}", workers.len());
        let block = self.panel(title);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let mut lines: Vec<TLine> = Vec::new();
        if workers.is_empty() {
            lines.push(TLine::from(Span::styled(
                "No workers registered. Press a to add a remote peer (address or @handle).",
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else {
            let vis = self.visible_count();
            let start = selected
                .saturating_sub(vis / 2)
                .min(workers.len().saturating_sub(vis));
            for (i, w) in workers.iter().enumerate().skip(start).take(vis) {
                let marker = if w.selected { "●" } else { " " };
                let handle = w.handle.as_deref().unwrap_or(&w.address);
                let label = w.label.as_deref().unwrap_or("");
                let harness = w
                    .harness
                    .as_deref()
                    .map(|h| format!(" · {}", h.to_uppercase()))
                    .unwrap_or_default();
                let text = format!(
                    "{marker} {} · {}{}{}",
                    w.id,
                    handle,
                    if label.is_empty() {
                        String::new()
                    } else {
                        format!(" · {label}")
                    },
                    harness,
                );
                let mut style = Style::default();
                if w.selected {
                    style = style.fg(Color::Green);
                }
                if i == selected {
                    style = self.theme.selection();
                }
                lines.push(TLine::from(Span::styled(text, style)));
            }
        }
        lines.push(TLine::from(Span::styled(
            "a add · Enter/s select · e edit label · d/x remove",
            Style::default().add_modifier(Modifier::DIM),
        )));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    fn draw_prompt(&mut self, f: &mut Frame, area: Rect) {
        let Some(prompt) = &self.prompt else { return };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.theme.accent))
            .title(Span::styled(
                prompt.title.clone(),
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let chars: Vec<char> = prompt.draft.text.chars().collect();
        let before: String = chars.iter().take(prompt.draft.cursor).collect();
        let at: String = chars
            .get(prompt.draft.cursor)
            .map(|c| c.to_string())
            .unwrap_or_else(|| " ".into());
        let after: String = chars.iter().skip(prompt.draft.cursor + 1).collect();
        let spans = vec![
            Span::styled("❯ ", Style::default().fg(Color::Magenta)),
            Span::raw(before),
            Span::styled(at, Style::default().add_modifier(Modifier::REVERSED)),
            Span::raw(after),
        ];
        f.render_widget(Paragraph::new(TLine::from(spans)), inner);
    }

    fn draw_trace(&mut self, f: &mut Frame, area: Rect) {
        let source: Vec<&EventEnvelope> = self
            .snapshot
            .events
            .iter()
            .filter(|e| matches!(e.event, TuiEvent::Trace { .. }))
            .collect();
        let block = self.panel(format!("Trace · {} events", source.len()));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let vis = self.visible_count();
        let start = self.selected.min(source.len().saturating_sub(vis));
        let page: Vec<&EventEnvelope> = source.into_iter().skip(start).take(vis).collect();
        let mut lines: Vec<TLine> = if page.is_empty() {
            vec![TLine::from(Span::styled(
                "No events yet.",
                Style::default().add_modifier(Modifier::DIM),
            ))]
        } else {
            page.iter()
                .map(|e| self.event_line(e, area.width.saturating_sub(6) as usize, false))
                .collect()
        };
        if let Some(first) = page.first() {
            if let Ok(json) = serde_json::to_string(&first.event) {
                lines.push(TLine::from(Span::styled(
                    json,
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
        }
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    fn draw_context(&mut self, f: &mut Frame, area: Rect) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
            .split(area);
        let block = self.panel(format!("Environment · {} chunks", self.contexts.len()));
        let inner = block.inner(cols[0]);
        f.render_widget(block, cols[0]);
        self.hit_context = Some(inner);
        let idx = self
            .context_index
            .min(self.contexts.len().saturating_sub(1));
        let vis = self.visible_count();
        let mut lines: Vec<TLine> = Vec::new();
        for (i, item) in self.contexts.iter().take(vis).enumerate() {
            let mut style = Style::default();
            if i == idx {
                style = self.theme.selection();
            }
            lines.push(TLine::from(Span::styled(
                format!("{} · {}b · {}", item.kind, item.bytes, item.ref_),
                style,
            )));
        }
        if self.contexts.is_empty() {
            lines.push(TLine::from(Span::styled(
                "No context chunks yet.",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        f.render_widget(Paragraph::new(Text::from(lines)), inner);

        let selected = self.contexts.get(idx);
        let title = selected
            .map(|c| c.ref_.clone())
            .unwrap_or_else(|| "Chunk detail".into());
        let content = selected
            .map(|c| c.content.clone())
            .unwrap_or_else(|| "Select a chunk with j/k.".into());
        f.render_widget(
            Paragraph::new(content)
                .wrap(Wrap { trim: false })
                .block(self.panel(title)),
            cols[1],
        );
    }

    /// The current Memory-tab left-pane rows: directives + facet overview with no
    /// active search, or the ranked hits after a `/memory <query>` search.
    fn memory_entries(&self) -> Vec<MemoryEntry> {
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

    fn memory_entry_count(&self) -> usize {
        self.memory_entries().len()
    }

    fn draw_memory(&mut self, f: &mut Frame, area: Rect) {
        // Disabled / not wired: a single helpful hint panel.
        let enabled = self
            .memory_status
            .as_ref()
            .map(|s| s.enabled)
            .unwrap_or(false);
        if !enabled {
            let mut lines = vec![TLine::from(Span::styled(
                "Persona memory is not enabled.",
                Style::default().fg(Color::Yellow),
            ))];
            lines.push(TLine::from(Span::styled(
                "Enable it in config (memory.enabled = true) with an OpenRouter key,",
                Style::default().add_modifier(Modifier::DIM),
            )));
            lines.push(TLine::from(Span::styled(
                "then run `medulla memory backfill` to distil your persona pack.",
                Style::default().add_modifier(Modifier::DIM),
            )));
            f.render_widget(
                Paragraph::new(Text::from(lines))
                    .wrap(Wrap { trim: true })
                    .block(self.panel("Persona memory")),
                area,
            );
            return;
        }

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(6), Constraint::Min(0)])
            .split(area);

        // Status header.
        let st = self.memory_status.clone().unwrap_or(MemoryStatus {
            enabled: true,
            workspace: String::new(),
            pack_exists: false,
            pack_path: String::new(),
            entry_count: 0,
            directives_count: 0,
            facet_counts: Default::default(),
        });
        let mut header = vec![
            TLine::from(vec![
                Span::styled("● enabled", Style::default().fg(Color::Green)),
                Span::raw(format!(" · {}", clip(&st.workspace, 48))),
            ]),
            if st.pack_exists {
                TLine::from(Span::styled(
                    format!("pack ● present · {}", clip(&st.pack_path, 52)),
                    Style::default().fg(Color::Green),
                ))
            } else {
                TLine::from(Span::styled(
                    "pack ○ absent · run `medulla memory backfill`",
                    Style::default().add_modifier(Modifier::DIM),
                ))
            },
            TLine::from(format!(
                "{} observation(s) · {} directive(s)",
                st.entry_count, st.directives_count
            )),
        ];
        let facets = if st.facet_counts.is_empty() {
            "facets: (none)".to_string()
        } else {
            let joined = st
                .facet_counts
                .iter()
                .map(|(f, n)| format!("{f}={n}"))
                .collect::<Vec<_>>()
                .join(" ");
            format!("facets: {joined}")
        };
        header.push(TLine::from(Span::styled(
            facets,
            Style::default().fg(self.theme.primary),
        )));
        f.render_widget(
            Paragraph::new(Text::from(header))
                .wrap(Wrap { trim: true })
                .block(self.panel("Persona memory")),
            rows[0],
        );

        // Left list + right detail.
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
            .split(rows[1]);

        let entries = self.memory_entries();
        let idx = self.memory_index.min(entries.len().saturating_sub(1));
        let searching = self.memory_query.is_some();
        let left_title = match &self.memory_query {
            Some(q) => format!("Search “{}” · {} hit(s)", clip(q, 18), entries.len()),
            None => "Directives & facets".to_string(),
        };
        let block = self.panel(left_title);
        let inner = block.inner(cols[0]);
        f.render_widget(block, cols[0]);
        let vis = (inner.height as usize).max(1);
        let start = idx
            .saturating_sub(vis / 2)
            .min(entries.len().saturating_sub(vis));
        let mut lines: Vec<TLine> = Vec::new();
        for (i, entry) in entries.iter().enumerate().skip(start).take(vis) {
            let (label, base) = match entry {
                MemoryEntry::Directive(text) => (
                    format!("◆ {}", clip(text, 30)),
                    Style::default().fg(Color::Yellow),
                ),
                MemoryEntry::Facet { name, count } => (
                    format!("▪ {name} · {count}"),
                    Style::default().add_modifier(Modifier::DIM),
                ),
                MemoryEntry::Hit(hit) => (
                    format!("{} · {} · {:.2}", hit.facet, hit.tier, hit.score),
                    Style::default().fg(Color::Magenta),
                ),
            };
            let mut style = base;
            if i == idx {
                style = self.theme.selection();
            }
            lines.push(TLine::from(Span::styled(label, style)));
        }
        if entries.is_empty() {
            let hint = if searching {
                "No hits for that query."
            } else {
                "No directives or observations yet. Run `medulla memory backfill`."
            };
            lines.push(TLine::from(Span::styled(
                hint,
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        f.render_widget(Paragraph::new(Text::from(lines)), inner);

        // Detail pane.
        let (title, body) = self.memory_detail(entries.get(idx));
        f.render_widget(
            Paragraph::new(Text::from(body))
                .wrap(Wrap { trim: false })
                .block(self.panel(title)),
            cols[1],
        );
    }

    /// The detail title + wrapped body for the selected Memory entry.
    fn memory_detail(&self, entry: Option<&MemoryEntry>) -> (String, Vec<TLine<'static>>) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        match entry {
            None => (
                "Detail".into(),
                vec![TLine::from(Span::styled(
                    "Select an entry with ↑/↓ (or search with /memory <query>).",
                    dim,
                ))],
            ),
            Some(MemoryEntry::Directive(text)) => {
                ("Directive".into(), vec![TLine::from(text.clone())])
            }
            Some(MemoryEntry::Facet { name, count }) => (
                name.clone(),
                vec![
                    TLine::from(format!("{count} observation(s) in this facet.")),
                    TLine::from(Span::styled(
                        "Run /memory <query> to rank observations across facets.",
                        dim,
                    )),
                ],
            ),
            Some(MemoryEntry::Hit(hit)) => {
                let mut body = vec![TLine::from(hit.text.clone()), TLine::from("")];
                if let Some(q) = &hit.quote {
                    body.push(TLine::from(Span::styled(format!("“{q}”"), dim)));
                    body.push(TLine::from(""));
                }
                body.push(TLine::from(Span::styled(
                    format!(
                        "facet {} · tier {} · score {:.3}",
                        hit.facet, hit.tier, hit.score
                    ),
                    dim,
                )));
                body.push(TLine::from(Span::styled(hit.timestamp.clone(), dim)));
                (format!("{} · {}", hit.facet, hit.tier), body)
            }
        }
    }

    fn draw_usage(&mut self, f: &mut Frame, area: Rect) {
        let fold = stream::usage_fold(&self.snapshot.events);
        let dim = Style::default().add_modifier(Modifier::DIM);
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let mut lines: Vec<TLine> = Vec::new();
        lines.push(TLine::from(Span::styled("This session", bold)));
        let mut tiers: Vec<(&String, &stream::TierUsage)> = fold.tiers.iter().collect();
        tiers.sort_by(|a, b| a.0.cmp(b.0));
        if tiers.is_empty() && fold.subagent.calls == 0 {
            lines.push(TLine::from(Span::styled("no model calls yet", dim)));
        }
        for (tier, t) in tiers {
            lines.push(TLine::from(format!(
                "{tier:<14} in {:<10} out {:<10} calls {}",
                t.input_tokens, t.output_tokens, t.calls
            )));
        }
        if fold.subagent.calls > 0 {
            lines.push(TLine::from(format!(
                "{:<14} in {:<10} out {:<10} tasks {}",
                "sub-agents",
                fold.subagent.input_tokens,
                fold.subagent.output_tokens,
                fold.subagent.calls
            )));
            for (task, input, output) in fold.tasks.iter().take(12) {
                lines.push(TLine::from(Span::styled(
                    format!("  {} in {input} out {output}", clip(task, 28)),
                    dim,
                )));
            }
        }
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled("Account", bold)));
        match &self.account_usage {
            None => lines.push(TLine::from(Span::styled(
                "account usage requires backend login (medulla login) · r to refresh",
                dim,
            ))),
            Some(data) => {
                let g = |path: &[&str]| -> Option<serde_json::Value> {
                    let mut cur = data;
                    for key in path {
                        cur = cur.get(key)?;
                    }
                    Some(cur.clone())
                };
                if let Some(plan) = g(&["plan"]).and_then(|v| v.as_str().map(str::to_string)) {
                    lines.push(TLine::from(format!("plan       {plan}")));
                }
                if let Some(spent) = g(&["inferenceTotals", "spent"]).and_then(|v| v.as_f64()) {
                    let calls = g(&["inferenceTotals", "calls"])
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    lines.push(TLine::from(format!(
                        "cycle      ${spent:.4} spent · {calls} calls"
                    )));
                }
                if let Some(remaining) = g(&["remainingUsd"]).and_then(|v| v.as_f64()) {
                    lines.push(TLine::from(format!("remaining  ${remaining:.4}")));
                }
                if let Some(models) = g(&["inferenceByModel"]).and_then(|v| match v {
                    serde_json::Value::Array(rows) => Some(rows),
                    _ => None,
                }) {
                    for row in models.iter().take(8) {
                        let model = row
                            .get("model")
                            .or_else(|| row.get("_id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("?");
                        let spent = row.get("spent").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        lines.push(TLine::from(Span::styled(
                            format!("  {model:<24} ${spent:.4}"),
                            dim,
                        )));
                    }
                }
            }
        }
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled(
            "r refresh · c effective config · 1-4 switch settings pages",
            dim,
        )));
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .block(self.panel("Usage")),
            area,
        );
    }

    fn draw_config(&mut self, f: &mut Frame, area: Rect) {
        let sources = if self.loaded.sources.is_empty() {
            "built-in defaults".to_string()
        } else {
            self.loaded.sources.join(" < ")
        };
        let body = format!("Sources: {sources}\n\n{}", self.loaded.pretty_json());
        let block = self.panel(format!("Configuration · {}", self.loaded.path));
        f.render_widget(
            Paragraph::new(body).wrap(Wrap { trim: false }).block(block),
            area,
        );
    }

    fn draw_help(&mut self, f: &mut Frame, area: Rect) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let lines = vec![
            TLine::from("Tab / Shift-Tab switch views · Chat: type to compose, ↑↓ recall prompt history"),
            TLine::from(Span::styled(
                "In a multi-line draft ↑↓ walk the caret between rows; history recalls from the edge rows",
                dim,
            )),
            TLine::from("Chat pins to the latest reply; the composer is shown only on this view"),
            TLine::from("Enter sends · Shift-Enter inserts a newline (Option-Enter if Shift-Enter sends)"),
            TLine::from("PageUp / PageDown scrolls the Chat and Agents transcripts"),
            TLine::from("Agents: ↑↓ pick an agent · j / k scroll · X cancel task · A answer a question"),
            TLine::from("Workers: a add peer · Enter/s select · e edit label · d/x remove"),
            TLine::from("Context: j / k select chunks · Esc clear input · Ctrl-X abort cycle"),
            TLine::from("Memory: ↑↓ / j k browse directives, facets & hits · /memory <query> to search"),
            TLine::from("Settings: ↑↓ nav subpages · 1-4 jump · Usage/Appearance/Config/Help live here"),
            TLine::from("Appearance: j / k pick a theme role · ←/→ or Enter cycle its color (saved live)"),
            TLine::from("Ctrl-N new session · Ctrl-C quit (nav keys act only when the input is empty)"),
            TLine::from(" "),
            TLine::from(Span::styled("Copy", bold)),
            TLine::from("Ctrl-Y copies the whole chat · /copy last copies just the latest reply"),
            TLine::from(" "),
            TLine::from(Span::styled("Mouse", bold)),
            TLine::from("Click a tab to switch views · in Agents/Context click a row to select · wheel scrolls"),
            TLine::from("Ctrl-O / /mouse release the mouse to the terminal for native drag-select"),
            TLine::from(" "),
            TLine::from(Span::styled("Commands", bold)),
            TLine::from("/new · /fork [name] · /resume · /abort · /clear · /config · /copy [all|last]"),
            TLine::from("/usage · /settings · /theme · /memory [query] · /mouse · /async [on|off] · /help · /quit"),
        ];
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: true })
                .block(self.panel("Keyboard & REPL help")),
            area,
        );
    }

    fn draw_composer(&mut self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(if self.snapshot.running {
                Color::Yellow
            } else {
                self.theme.primary
            }));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let caret = caret_row_col(&self.draft.text, self.draft.cursor);
        let mut lines: Vec<TLine> = Vec::new();
        for (index, row) in self.draft.text.split('\n').enumerate() {
            let prefix = if index == 0 { "❯ " } else { "  " };
            let mut spans = vec![Span::styled(
                prefix,
                Style::default().fg(self.theme.primary),
            )];
            if index == caret.row {
                let chars: Vec<char> = row.chars().collect();
                let before: String = chars.iter().take(caret.col).collect();
                let at: String = chars
                    .get(caret.col)
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| " ".into());
                let after: String = chars.iter().skip(caret.col + 1).collect();
                spans.push(Span::raw(before));
                spans.push(Span::styled(
                    at,
                    Style::default().add_modifier(Modifier::REVERSED),
                ));
                spans.push(Span::raw(after));
            } else {
                spans.push(Span::raw(row.to_string()));
            }
            lines.push(TLine::from(spans));
        }
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    fn draw_resume(&mut self, f: &mut Frame, area: Rect) {
        let Some(picker) = &self.resume_picker else {
            return;
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.theme.accent))
            .title(Span::styled(
                format!(
                    "Resume a chat — ↑/↓ select · Enter load · Esc cancel ({})",
                    picker.chats.len()
                ),
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let cap = (inner.height as usize).max(1);
        let start = picker
            .index
            .saturating_sub(cap / 2)
            .min(picker.chats.len().saturating_sub(cap));
        let mut lines = Vec::new();
        for (i, chat) in picker.chats.iter().enumerate().skip(start).take(cap) {
            let marker = if i == picker.index { "❯ " } else { "  " };
            let mut style = Style::default();
            if i == picker.index {
                style = self.theme.selection();
            }
            let text = format!(
                "{marker}{} · {}t · {} thread{} · {}",
                chat.name,
                chat.turns,
                chat.thread_count,
                if chat.thread_count == 1 { "" } else { "s" },
                chat.updated_at,
            );
            lines.push(TLine::from(Span::styled(text, style)));
        }
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla::runtime::mock::MockRuntime;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn app() -> App {
        let rt: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
        let loaded = {
            let mut l = LoadedConfig::defaults("medulla.tui.json".into());
            l.config.tinyplace = Some(medulla::config::TinyplaceConfig::default());
            l
        };
        App::new(rt, loaded)
    }

    fn render(app: &mut App) -> String {
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let buf = terminal.backend().buffer().clone();
        buf.content().iter().map(|c| c.symbol()).collect::<String>()
    }

    #[test]
    fn every_tab_renders() {
        for (i, name) in TABS.iter().enumerate() {
            let mut a = app();
            a.tab_index = i;
            let out = render(&mut a);
            assert!(out.contains("MEDULLA"), "tab {name} missing header");
        }
    }

    #[test]
    fn header_shows_async_toggle() {
        let mut a = app();
        a.runtime.set_async_mode(true);
        a.refresh_snapshot();
        let out = render(&mut a);
        assert!(out.contains("ASYNC ON"));
    }

    #[test]
    fn slash_help_switches_tab() {
        let mut a = app();
        a.tab_index = 1;
        let _ = a.execute("/help".into());
        assert_eq!(a.tab(), "Settings");
        assert_eq!(a.settings_subpage(), "Help");
    }

    #[test]
    fn unknown_command_sets_status() {
        let mut a = app();
        let _ = a.execute("/bogus".into());
        assert!(a.status.contains("Unknown command"));
    }

    #[test]
    fn plain_text_returns_submit_cmd() {
        let mut a = app();
        a.tab_index = 1;
        let cmd = a.execute("hello world".into());
        assert!(matches!(cmd, Some(Cmd::Submit(s)) if s == "hello world"));
        assert_eq!(a.status, "Cycle running…");
    }

    #[test]
    fn typing_inserts_into_draft() {
        let mut a = app();
        a.tab_index = 1;
        for ch in "hi".chars() {
            a.on_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        assert_eq!(a.draft.text, "hi");
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        assert_eq!(a.draft.text, "hi\n");
    }
}
