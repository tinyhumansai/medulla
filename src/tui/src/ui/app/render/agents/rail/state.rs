//! State resolution and styling for agent lanes in the Agents rail.

use std::collections::HashSet;

use ratatui::style::{Modifier, Style};

use crate::ui::agents::{AgentLane, AgentRole};

use super::super::super::super::types::App;
use super::super::super::color;
use super::status::{self, HarnessVisualState};

impl App {
    /// The presence/status glyph for a lane row.
    pub(in crate::ui::app::render) fn lane_marker(
        &self,
        item: &AgentLane,
        is_fn: bool,
    ) -> &'static str {
        if is_fn {
            "ƒ"
        } else if item.role != AgentRole::Agent {
            "●"
        } else if item.session_id.is_some() {
            match self.session_state(item).as_deref() {
                Some("ended") => "○",
                _ => "●",
            }
        } else if let Some(aid) = &item.agent_id {
            match self.snapshot.presence.get(aid) {
                Some(p) if p.online => "●",
                Some(_) => "○",
                None if item.descriptor.is_some() => "◌",
                None => "◆",
            }
        } else if item.descriptor.is_some() {
            "◌"
        } else {
            "◆"
        }
    }

    /// The state of the session backing a lane, if any.
    pub(in crate::ui::app::render) fn session_state(&self, item: &AgentLane) -> Option<String> {
        let (sid, pid) = (item.session_id.as_ref()?, item.parent_agent_id.as_ref()?);
        self.snapshot
            .sessions
            .get(pid)?
            .iter()
            .find(|s| &s.id == sid)
            .map(|s| s.state.clone())
    }

    /// A short human-readable state suffix for a lane row.
    pub(in crate::ui::app::render) fn lane_state(
        &self,
        item: &AgentLane,
        waiting_sessions: &HashSet<String>,
    ) -> String {
        if item.session_id.is_some() {
            self.session_state(item)
                .map(|_| {
                    format!(
                        " · {}",
                        self.harness_visual_state(item, waiting_sessions).label()
                    )
                })
                .unwrap_or_else(|| " · …".into())
        } else if item.role == AgentRole::Agent {
            format!(
                " · {}",
                self.harness_visual_state(item, waiting_sessions).label()
            )
        } else {
            String::new()
        }
    }

    /// Style a lane while preserving selection visibility and harness state.
    pub(super) fn lane_style(
        &self,
        item: &AgentLane,
        is_fn: bool,
        active: bool,
        waiting_sessions: &HashSet<String>,
    ) -> Style {
        let mut style = if active {
            self.theme.selection()
        } else {
            Style::default()
        };
        if item.role == AgentRole::Agent {
            let state = self.harness_visual_state(item, waiting_sessions);
            style = style.fg(if state == HarnessVisualState::NeedsInput {
                self.theme.attention
            } else {
                state.color()
            });
            if state.flashes()
                && (state != HarnessVisualState::NeedsInput || self.theme.attention_blink)
            {
                style = style.add_modifier(Modifier::SLOW_BLINK);
            }
        } else {
            style = style.fg(color(item.role.color()));
        }
        if is_fn {
            style = style.add_modifier(Modifier::DIM);
        }
        style
    }

    /// Resolve the current visual state, giving a waiting local PTY precedence.
    pub(super) fn harness_visual_state(
        &self,
        item: &AgentLane,
        waiting_sessions: &HashSet<String>,
    ) -> HarnessVisualState {
        let needs_input = self.lane_attention(item, waiting_sessions);
        classify_lane(item, self.session_state(item).as_deref(), needs_input)
    }

    /// Whether a local harness serving this lane needs the operator.
    pub(super) fn lane_attention(
        &self,
        item: &AgentLane,
        waiting_sessions: &HashSet<String>,
    ) -> bool {
        let Some(harnesses) = self.harnesses.as_ref() else {
            return false;
        };
        lane_waiting_session(
            item,
            self.harness_focus.attached_to(),
            waiting_sessions,
            |task_id| harnesses.session_for_task(task_id),
        )
        .is_some()
    }

    /// Whether the exact task row is backed by a waiting local harness.
    pub(super) fn task_attention(&self, task_id: &str, waiting_sessions: &HashSet<String>) -> bool {
        let Some(harnesses) = self.harnesses.as_ref() else {
            return false;
        };
        task_waiting_session(
            task_id,
            self.harness_focus.attached_to(),
            waiting_sessions,
            |task_id| harnesses.session_for_task(task_id),
        )
        .is_some()
    }
}

/// Classify a lane after the PTY attention snapshot has been resolved.
pub(super) fn classify_lane(
    item: &AgentLane,
    session: Option<&str>,
    needs_input: bool,
) -> HarnessVisualState {
    if needs_input {
        HarnessVisualState::NeedsInput
    } else {
        status::classify(item, session)
    }
}

/// Return the first waiting task session, skipping the pane already attached.
pub(super) fn lane_waiting_session(
    item: &AgentLane,
    attached: Option<&str>,
    waiting_sessions: &HashSet<String>,
    mut resolve: impl FnMut(&str) -> Option<String>,
) -> Option<String> {
    item.tasks
        .iter()
        .filter_map(|task| resolve(&task.task_id))
        .filter(|session| attached != Some(session.as_str()))
        .find(|session| waiting_sessions.contains(session))
}

/// Return a task's waiting session unless its pane is already attached.
pub(super) fn task_waiting_session(
    task_id: &str,
    attached: Option<&str>,
    waiting_sessions: &HashSet<String>,
    mut resolve: impl FnMut(&str) -> Option<String>,
) -> Option<String> {
    resolve(task_id)
        .filter(|session| attached != Some(session.as_str()) && waiting_sessions.contains(session))
}
