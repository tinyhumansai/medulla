//! Command execution for [`App`]: fleet/steering helpers, inline-prompt
//! submission, clipboard copy, composer-line [`App::execute`], slash-command
//! dispatch, and the Settings/Appearance mutators. These turn resolved input
//! into runtime calls and follow-up [`Cmd`]s.

use crate::ui::agents::{AgentRow, TaskState};
use crate::ui::clipboard::{copy_for_operator, copy_to_clipboard, current_platform, OSC_52};
use crate::ui::command::{self, CopyScope, SlashCommand};
use crate::ui::composer::Draft;
use medulla::runtime::{WorkerInfo, WorkerOp};

use super::types::{
    tab_pos, App, Cmd, Prompt, PromptKind, SETTINGS_SUBPAGES, SP_APPEARANCE, SP_CONFIG, SP_HELP,
    SP_USAGE,
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
    ///
    /// Indexes the rail's rows, which is what `agent_index` counts — the lane
    /// list alone is shorter than the rail and reading it here would answer for
    /// whichever row happened to share the offset.
    pub(super) fn selected_agent_task(&self) -> Option<TaskState> {
        let rows = self.rail_rows();
        match rows.get(self.agent_index.min(rows.len().saturating_sub(1))) {
            Some(super::rail::RailRow::Agent(AgentRow::Sub { task, .. })) => Some(task.clone()),
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
                        // MCP tasks run on OpenHumanRuntime's local task runner,
                        // not its backend steering API. Their cycle prefix is
                        // the capability marker for that separate cancel path.
                        if !c.starts_with("mcp:") && !self.runtime.steering_reaches_backend() {
                            self.set_status("Cancelling a task is not wired to this runtime yet");
                            return;
                        }
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
        if !self.runtime.steering_reaches_backend() {
            self.set_status("Answering a question is not wired to this runtime yet");
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
            PromptKind::ChangesBaseline => {
                self.finish_change_baseline(&text, super::changes::types::BaselineSource::Manual);
                None
            }
            PromptKind::ChangesComment { path, anchor } => {
                self.submit_changes_comment(&path, anchor, &text);
                None
            }
            PromptKind::HostAdd => match WorkerOp::parse_add(&text) {
                Some(op) => {
                    // Land on the list the add is about. Staying on Add Host
                    // leaves the operator on a page describing work they have
                    // just finished, with no sight of whether it landed.
                    self.focus_routing_subpage("Hosts");
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
            PromptKind::CustomHarnessAdd => {
                self.save_custom_harness(None, &text);
                None
            }
            PromptKind::CustomHarnessEdit(id) => {
                self.save_custom_harness(Some(&id), &text);
                None
            }
            PromptKind::LocalHostWorkspace(harness) => self.add_local_host(harness, &text),
            PromptKind::RejectProposal {
                workflow,
                proposal_id,
            } => {
                if text.is_empty() {
                    self.set_status("Proposal rejection cancelled (empty)");
                    return None;
                }
                self.set_status("Declining the proposed change…");
                Some(Cmd::RejectProposal {
                    workflow,
                    proposal_id,
                    reason: text,
                })
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
                if !self.runtime.steering_reaches_backend() {
                    self.set_status("Answering a question is not wired to this runtime yet");
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
                if !self.runtime.steering_reaches_backend() {
                    self.set_status("Answering a question is not wired to this runtime yet");
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
            #[cfg(feature = "workflows")]
            PromptKind::WorkflowInput {
                workflow_id,
                dry_run,
                mut remaining,
                mut collected,
            } => {
                // `remaining` is never empty: a prompt is only opened with a
                // field to ask about, and the last submission dispatches rather
                // than opening another.
                let field = remaining.remove(0);
                let value = match coerce_workflow_input(&field, &text) {
                    Ok(value) => value,
                    Err(message) => {
                        // Re-ask the same field rather than dropping the whole
                        // set: a mistyped number should cost one retype, not
                        // every value collected so far.
                        self.set_status(message);
                        remaining.insert(0, field);
                        self.open_workflow_input_prompt(workflow_id, dry_run, remaining, collected);
                        return None;
                    }
                };
                collected.insert(field.name.clone(), value);

                if !remaining.is_empty() {
                    self.open_workflow_input_prompt(workflow_id, dry_run, remaining, collected);
                    return None;
                }
                if dry_run {
                    self.set_status("Simulating…");
                    Some(Cmd::DryRunWorkflow {
                        id: workflow_id,
                        inputs: collected,
                    })
                } else {
                    self.set_status("Running…");
                    Some(Cmd::RunWorkflow {
                        id: workflow_id,
                        inputs: collected,
                    })
                }
            }
        }
    }

    /// Open the prompt for `remaining`'s first field, carrying the rest and
    /// what has been collected so far.
    ///
    /// Pre-filled with the field's declared default so the common answer is one
    /// keypress, and titled with the field name so a chain of several prompts
    /// says which one is on screen.
    #[cfg(feature = "workflows")]
    pub(super) fn open_workflow_input_prompt(
        &mut self,
        workflow_id: String,
        dry_run: bool,
        remaining: Vec<medulla::workflows::WorkflowInput>,
        collected: serde_json::Map<String, serde_json::Value>,
    ) {
        let Some(field) = remaining.first() else {
            return;
        };
        let title = match (&field.description, field.required) {
            (Some(description), _) if !description.is_empty() => {
                format!("{}: {description}", field.name)
            }
            (_, true) => format!("{} (required)", field.name),
            (_, false) => field.name.clone(),
        };
        let prefill = field
            .default
            .as_ref()
            .map(render_workflow_input_default)
            .unwrap_or_default();
        self.prompt = Some(Prompt::with_text(
            PromptKind::WorkflowInput {
                workflow_id,
                dry_run,
                remaining,
                collected,
            },
            title,
            prefill,
        ));
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

    /// Copy one short line — a command, an address — to the clipboard, naming it
    /// in the status line.
    ///
    /// Kept apart from [`App::copy_chat`] in two ways. It reports *what* was
    /// copied rather than a size, because for one line the size says nothing.
    /// And it goes to the terminal first
    /// ([`copy_for_operator`](medulla::clipboard::copy_for_operator)): the
    /// orchestrator itself may be running over SSH, and a short line is exactly
    /// what an operator then pastes somewhere else. A transcript keeps the
    /// local-writer-first path, since terminals cap how much an OSC 52 escape
    /// may carry and a long chat would be truncated or dropped outright.
    pub(in crate::ui::app) fn copy_line(&mut self, what: &str, text: &str) {
        if let Some(sink) = &self.copy_capture {
            sink.lock().expect("copy sink").push(text.to_string());
            self.set_status(format!("Copied {what} (captured)"));
            return;
        }
        let via = copy_for_operator(text, current_platform(), |osc| {
            use std::io::Write;
            let _ = std::io::stdout().write_all(osc.as_bytes());
            let _ = std::io::stdout().flush();
        });
        self.set_status(match via {
            Some(writer) => format!("Copied {what} → clipboard ({writer})"),
            None => format!("Sent {what} → terminal (OSC 52); check your clipboard"),
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
            SlashCommand::NewHarness { provider, path } => {
                self.start_harness_command(provider.as_deref(), path.as_deref());
            }
            SlashCommand::TakeControl => self.take_harness_control(),
            SlashCommand::HandOff { note } => self.hand_harness_back(note),
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
                self.tab_index = tab_pos("Feedback");
                self.set_status("Feedback · loading the board…");
                return self.reload_feedback();
            }
            SlashCommand::ToggleMouse => self.toggle_mouse(),
            SlashCommand::Copy(scope) => self.copy_chat(scope),
            SlashCommand::BadUsage(usage) => self.set_status(usage),
            SlashCommand::Unknown(input) => self.set_status(format!("Unknown command: {input}")),
        }
        None
    }

    /// Land on the Settings tab at subpage `index`, returning its lazy-load
    /// command (Usage and Context each fetch on entry).
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

    /// Submit a comment on a Changes tab selection, capturing context for drift detection.
    fn submit_changes_comment(
        &mut self,
        path: &std::path::Path,
        anchor: medulla::ui::git_review::CommentAnchor,
        text: &str,
    ) {
        use medulla::ui::git_review::CommentAnchor;
        let context = match anchor {
            CommentAnchor::Line(i) => self
                .changes
                .patch
                .get(i)
                .map(String::as_str)
                .unwrap_or("")
                .to_owned(),
            CommentAnchor::Hunk(i) => self
                .changes
                .hunks
                .get(i)
                .and_then(|h| self.changes.patch.get(h.header))
                .map(String::as_str)
                .unwrap_or("")
                .to_owned(),
            CommentAnchor::File => String::new(),
        };
        let kept = self
            .changes
            .comments
            .upsert_with_context(path, anchor, text, &context);
        self.set_status(if kept {
            format!(
                "Comment saved on {} · {}",
                path.display(),
                anchor.describe()
            )
        } else {
            format!("Comment cleared on {}", path.display())
        });
    }
}

/// Turn what an operator typed into a value of the input's declared type.
///
/// A prompt yields a string because a terminal line is one. The declaration is
/// what says whether `3` means the number three or the string `"3"`, so the
/// coercion is driven by it rather than by guessing from the text — otherwise a
/// version input typed as `1.0` would silently become a float.
///
/// An empty line means "leave it unset": the declared default applies, or the
/// input resolves null if it is optional. A *required* input with no default
/// cannot be left unset, and says so rather than sending an empty string that
/// would pass the type check and fail the operator's intent.
#[cfg(feature = "workflows")]
fn coerce_workflow_input(
    field: &medulla::workflows::WorkflowInput,
    text: &str,
) -> Result<serde_json::Value, String> {
    use medulla::workflows::InputType;

    if text.is_empty() {
        return match (&field.default, field.required) {
            (Some(default), _) => Ok(default.clone()),
            (None, false) => Ok(serde_json::Value::Null),
            (None, true) => Err(format!("{} is required", field.name)),
        };
    }

    match field.ty {
        InputType::String => Ok(serde_json::Value::String(text.to_string())),
        InputType::Number => text
            .parse::<serde_json::Number>()
            .map(serde_json::Value::Number)
            .map_err(|_| format!("{} expects a number, got {text:?}", field.name)),
        InputType::Boolean => text
            .parse::<bool>()
            .map(serde_json::Value::Bool)
            .map_err(|_| format!("{} expects true or false, got {text:?}", field.name)),
        InputType::Json => {
            serde_json::from_str(text).map_err(|err| format!("{} expects JSON: {err}", field.name))
        }
    }
}

/// Render a declared default as the text to pre-fill its prompt with.
///
/// A string default is shown unquoted — the operator is editing the value, not
/// its JSON encoding, and leaving the quotes in would make accepting the
/// default produce a differently-quoted string.
#[cfg(feature = "workflows")]
fn render_workflow_input_default(default: &serde_json::Value) -> String {
    match default {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}
