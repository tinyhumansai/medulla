//! Command execution for [`App`]: fleet/steering helpers, inline-prompt
//! submission, clipboard copy, composer-line [`App::execute`], slash-command
//! dispatch, and the Settings/Appearance mutators. These turn resolved input
//! into runtime calls and follow-up [`Cmd`]s.

use crate::ui::agents::{AgentRow, TaskState};
use crate::ui::clipboard::{copy_to_clipboard, current_platform, OSC_52};
use crate::ui::command::{self, CopyScope, SlashCommand};
use crate::ui::composer::Draft;
use crate::ui::theme::{color_to_string, THEME_ROLES};
use medulla::runtime::{WorkerInfo, WorkerOp};

use super::types::{
    tab_pos, App, Cmd, Prompt, PromptKind, MP_OVERVIEW, MP_SEARCH, SETTINGS_SUBPAGES,
    SP_APPEARANCE, SP_CONFIG, SP_FEEDBACK, SP_HELP, SP_USAGE,
};

impl App {
    /// The worker under the Workers-list cursor, if the fleet is non-empty.
    pub(super) fn selected_host(&self) -> Option<WorkerInfo> {
        let ws = self.runtime.workers();
        if ws.is_empty() {
            return None;
        }
        ws.get(self.host_index.min(ws.len() - 1)).cloned()
    }

    /// The task under the Agents-list cursor, when a `Sub` (task) row is selected.
    pub(super) fn selected_agent_task(&self) -> Option<TaskState> {
        let rows = self.agent_rows();
        match rows.get(self.agent_index) {
            Some(AgentRow::Sub { task, .. }) => Some(task.clone()),
            _ => None,
        }
    }

    /// Request cancellation of the selected running task, or note why it cannot.
    pub(super) fn cancel_selected_task(&mut self) {
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

    /// Open the answer prompt for the selected task's pending question.
    /// Send the composer's text as an answer when the cursor sits on a task with
    /// an open question, consuming the draft.
    ///
    /// Returns `Some(None)` when it handled the key (an answer was sent, or the
    /// draft was empty and there was nothing to send) and `None` when the caller
    /// should treat the text as an ordinary instruction instead. The distinction
    /// matters: typing into an agent's lane must not silently start a new
    /// orchestrator cycle.
    pub(super) fn answer_from_composer(&mut self, text: &str) -> Option<Option<Cmd>> {
        let task = self.selected_agent_task()?;
        let question_id = task.question_id.clone()?;
        let cycle_id = crate::ui::agents::parse_task_key(&task.task_id)
            .0?
            .to_string();
        if text.trim().is_empty() {
            self.set_status("Type an answer for the pending question");
            return Some(None);
        }
        self.runtime
            .answer_question(cycle_id, question_id, text.to_string());
        self.draft = Draft::new();
        self.set_status(format!("Answer sent to {}", task.task_id));
        Some(None)
    }

    pub(super) fn answer_selected_task(&mut self) {
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
    pub(super) fn submit_prompt(&mut self) -> Option<Cmd> {
        let p = self.prompt.take()?;
        let text = p.draft.text.trim().to_string();
        match p.kind {
            PromptKind::TaskCreate => {
                if text.is_empty() {
                    self.set_status("Tasks · title is required");
                    return None;
                }
                let now = medulla::tasks::now_timestamp();
                Some(Cmd::SaveTask(Box::new(medulla::tasks::Task {
                    id: uuid::Uuid::new_v4().to_string(),
                    title: text,
                    description: String::new(),
                    status: Default::default(),
                    source: None,
                    recurrence: None,
                    created_at: now.clone(),
                    updated_at: now,
                    last_synced_at: None,
                    dispatch: serde_json::Value::Null,
                })))
            }
            PromptKind::TaskEdit(id) => {
                if text.is_empty() {
                    self.set_status("Tasks · title is required");
                    return None;
                }
                let Some(mut task) = self.tasks.tasks.iter().find(|task| task.id == id).cloned()
                else {
                    self.set_status("Tasks · task no longer exists");
                    return None;
                };
                task.title = text;
                task.updated_at = medulla::tasks::now_timestamp();
                Some(Cmd::SaveTask(Box::new(task)))
            }
            PromptKind::SourceAdd => {
                let repository = text.trim().to_string();
                if repository.is_empty() {
                    self.set_status("Sources · repository is required");
                    return None;
                }
                let source = medulla::tasks::SourceConfig {
                    id: uuid::Uuid::new_v4().to_string(),
                    provider: "github".into(),
                    enabled: true,
                    repository,
                    state: "open".into(),
                    labels: Vec::new(),
                    filter: None,
                    token: std::env::var("GITHUB_TOKEN").ok(),
                };
                let mut document = self.tasks.clone();
                document.sources.push(source);
                Some(Cmd::SaveTasks(Box::new(document)))
            }
            PromptKind::MemorySearch => {
                if text.is_empty() {
                    self.set_status("Memory · search query is required");
                    return None;
                }
                self.memory_subpage_index = MP_SEARCH;
                self.memory_focused = true;
                self.set_status(format!("Memory · searching “{text}”…"));
                Some(Cmd::SearchMemory(text))
            }
            PromptKind::HostAdd => match WorkerOp::parse_add(&text) {
                Some(op) => {
                    self.set_status("Adding worker…");
                    Some(Cmd::WorkerOp(op))
                }
                None => {
                    self.set_status("Add cancelled (empty)");
                    None
                }
            },
            PromptKind::WorkspaceAdd => {
                self.add_workspace(&text);
                None
            }
            PromptKind::HostEditLabel(id) => {
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
            PromptKind::DecisionAnswer {
                decision_id,
                cycle_id,
                question_id,
            } => {
                if text.is_empty() {
                    self.set_status("Answer cancelled (empty)");
                    return None;
                }
                self.runtime.answer_question(cycle_id, question_id, text);
                self.dismissed_decisions.insert(decision_id);
                self.set_status("Decision answered");
                None
            }
            PromptKind::FeedbackComment { id } => {
                if text.is_empty() {
                    self.set_status("Comment cancelled (empty)");
                    return None;
                }
                self.set_status("Posting comment…");
                Some(Cmd::CommentFeedback { id, body: text })
            }
            // Step one captures the title and re-opens the prompt for the body;
            // nothing is sent until step two.
            PromptKind::FeedbackTitle { kind } => {
                if text.is_empty() {
                    self.set_status("New feedback cancelled (empty title)");
                    return None;
                }
                self.open_feedback_body(kind, text);
                None
            }
            PromptKind::FeedbackBody { kind, title } => {
                if text.is_empty() {
                    self.set_status("New feedback cancelled (empty description)");
                    return None;
                }
                self.set_status("Submitting feedback…");
                Some(Cmd::SubmitFeedback {
                    kind,
                    title,
                    body: text,
                })
            }
        }
    }

    /// Copy the requested chat scope to the clipboard (or the test capture sink),
    /// reporting the result in the status line.
    pub(super) fn copy_chat(&mut self, scope: CopyScope) {
        let text = command::copy_text(&self.snapshot.chat_events, scope);
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
    pub(super) fn execute(&mut self, value: String) -> Option<Cmd> {
        let clean = value.trim().to_string();
        if clean.is_empty() {
            return None;
        }
        self.history.push(clean.clone());
        self.history_index = -1;
        self.draft = Draft::new();
        self.chat_scroll = 0;

        if let Some(command) = SlashCommand::parse(&clean) {
            return self.dispatch_slash(command);
        }

        self.set_status("Cycle running…");
        Some(Cmd::Submit(clean))
    }

    /// Perform the side effect for a parsed [`SlashCommand`], returning any
    /// follow-up [`Cmd`] the event loop must run (e.g. a lazy load). Parsing lives
    /// in the SDK ([`crate::ui::command::parse`]); this method owns only the
    /// UI-state mutations and runtime calls.
    pub(super) fn dispatch_slash(&mut self, command: SlashCommand) -> Option<Cmd> {
        match command {
            SlashCommand::Quit => self.should_quit = true,
            SlashCommand::NewSession => {
                self.new_thread();
            }
            SlashCommand::Resume => return Some(Cmd::ListChats),
            SlashCommand::Abort => {
                self.runtime.abort();
                self.set_status("Abort requested");
            }
            SlashCommand::ClearView => {
                self.selected = 0;
                self.set_status("View reset (runtime history is retained)");
            }
            SlashCommand::Help => {
                self.set_settings_subpage(SP_HELP);
            }
            SlashCommand::Config => {
                self.enter_settings_subpage(SP_CONFIG);
            }
            SlashCommand::Settings => {
                self.enter_settings_subpage(SP_APPEARANCE);
            }
            SlashCommand::Usage => return self.set_settings_subpage(SP_USAGE),
            SlashCommand::Feedback => {
                self.enter_settings_subpage(SP_FEEDBACK);
                self.set_status("Feedback · loading the board…");
                return self.reload_feedback();
            }
            SlashCommand::Memory(query) => {
                self.tab_index = tab_pos("Memory");
                match query {
                    None => {
                        self.memory_subpage_index = MP_OVERVIEW;
                        self.memory_focused = false;
                        self.set_status("Memory · loading persona…");
                        return Some(Cmd::LoadMemory);
                    }
                    Some(query) => {
                        self.memory_subpage_index = MP_SEARCH;
                        self.memory_focused = true;
                        self.set_status(format!("Memory · searching “{query}”…"));
                        return Some(Cmd::SearchMemory(query));
                    }
                }
            }
            SlashCommand::ToggleMouse => self.toggle_mouse(),
            SlashCommand::Copy(scope) => self.copy_chat(scope),
            SlashCommand::BadUsage(usage) => self.set_status(usage),
            SlashCommand::Unknown(input) => self.set_status(format!("Unknown command: {input}")),
        }
        None
    }

    /// Land on the Settings tab at subpage `index`, returning its lazy-load
    /// command (Usage, Context, and Feedback each fetch on entry).
    pub(super) fn set_settings_subpage(&mut self, index: usize) -> Option<Cmd> {
        self.tab_index = tab_pos("Settings");
        self.settings_index = index.min(SETTINGS_SUBPAGES.len() - 1);
        // An armed logout must not survive a jump to another subpage.
        self.disarm_logout();
        // A jump lands on the nav, not inside the new page: the digit keys are a
        // way to move *between* subpages, so leaving focus in the content pane
        // would strand the next arrow key on whatever page you just left.
        self.settings_focused = false;
        self.tab_enter_cmd()
    }

    /// Jump to a Settings subpage *and* step into its content pane.
    ///
    /// Used by the slash commands: `/feedback` is a request to work with the
    /// board, not to park on the nav next to it, so it should land ready to
    /// browse.
    pub(super) fn enter_settings_subpage(&mut self, index: usize) -> Option<Cmd> {
        let cmd = self.set_settings_subpage(index);
        self.settings_focused = true;
        cmd
    }

    /// Cycle the selected Appearance role's color, apply it to the live theme,
    /// and persist the `[theme]` section.
    pub(super) fn cycle_appearance_role(&mut self, forward: bool) {
        let role = self.appearance_index.min(THEME_ROLES.len() - 1);
        self.theme.cycle_role(role, forward);
        self.persist_theme_now(THEME_ROLES[role]);
    }

    /// Persist the operator's routing strategy to config and remember it on the
    /// loaded config, so the selection survives a restart and reloads highlighted.
    ///
    /// Mirrors the theme editor: a `None` config path applies live (the strategy is
    /// still sent to the runtime) but is not written to disk.
    pub(super) fn persist_routing_strategy_now(
        &mut self,
        strategy: medulla::runtime::RoutingStrategy,
    ) {
        self.loaded.config.routing_strategy = Some(strategy);
        match &self.config_path {
            Some(path) => {
                match medulla::config::persist_routing_strategy(path, strategy.as_wire()) {
                    Ok(()) => {
                        self.set_status(format!("Applying {strategy:?} routing strategy… (saved)"))
                    }
                    Err(e) => self.set_status(format!("Routing strategy save failed: {e}")),
                }
            }
            None => self.set_status(format!(
                "Applying {strategy:?} routing strategy… (not persisted)"
            )),
        }
    }

    /// Persist and remember the provider-subscription routing strategy.
    pub(super) fn persist_subscription_strategy_now(
        &mut self,
        strategy: medulla::runtime::SubscriptionRoutingStrategy,
    ) {
        self.loaded.config.subscription_routing_strategy = Some(strategy);
        match &self.config_path {
            Some(path) => {
                match medulla::config::persist_subscription_routing_strategy(
                    path,
                    strategy.as_wire(),
                ) {
                    Ok(()) => self.set_status(format!(
                        "Applying {strategy:?} subscription strategy… (saved)"
                    )),
                    Err(e) => self.set_status(format!("Subscription strategy save failed: {e}")),
                }
            }
            None => self.set_status(format!(
                "Applying {strategy:?} subscription strategy… (not persisted)"
            )),
        }
    }

    /// Write the current theme to the injected config path, surfacing a status
    /// note on success or failure. A `None` path applies live but does not save.
    pub(super) fn persist_theme_now(&mut self, role: &str) {
        let value = color_to_string(self.theme.role(self.appearance_index));
        match &self.config_path {
            Some(path) => match crate::ui::theme::persist_theme(path, &self.theme) {
                Ok(()) => self.set_status(format!("Appearance · {role} → {value} (saved)")),
                Err(e) => self.set_status(format!("Appearance · save failed: {e}")),
            },
            None => self.set_status(format!("Appearance · {role} → {value} (not persisted)")),
        }
    }
}
