//! Pure view-model fold: turn the flat event stream into one lane per cognitive
//! tier plus one lane per connected roster agent / anonymous task / peer session,
//! with a row model for the Agents list and pre-wrapped transcript lines. A port
//! of the TS `deriveAgentLanes` / `agentRowModel` / `laneLines` essentials.

use std::collections::HashMap;

use crate::runtime::AgentDescriptor;
use crate::ui::events::{EventEnvelope, TuiEvent, Usage};
use crate::ui::util::wrap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRole {
    Orchestrator,
    Reasoning,
    Compress,
    Worker,
}

impl AgentRole {
    pub fn color(self) -> &'static str {
        match self {
            AgentRole::Orchestrator | AgentRole::Reasoning => "yellow",
            AgentRole::Compress => "blue",
            AgentRole::Worker => "magenta",
        }
    }
    /// A real (delegatable) agent, or a graph-invoked function (`compress`).
    pub fn is_function(self) -> bool {
        matches!(self, AgentRole::Compress)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Running,
    Done,
    Failed,
    /// §3.3(3): the wire keeps `cancelled` distinct from `failed`; the fold does too.
    Cancelled,
}

impl TaskStatus {
    /// Map a `task_complete` wire status onto the lane status. Unknown → Running is
    /// never produced here (a completion is terminal); an unrecognized string falls
    /// back to `failed` rather than silently reading as done.
    pub fn from_wire(s: &str) -> TaskStatus {
        match s {
            "done" => TaskStatus::Done,
            "cancelled" => TaskStatus::Cancelled,
            _ => TaskStatus::Failed,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            TaskStatus::Running => "running",
            TaskStatus::Done => "done",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }
    pub fn color(self) -> &'static str {
        match self {
            TaskStatus::Running => "yellow",
            TaskStatus::Done => "green",
            TaskStatus::Failed => "red",
            TaskStatus::Cancelled => "gray",
        }
    }
}

/// Split a lane task key into its `(cycleId, taskId)` parts. CoreRuntime composes a
/// lane-unique key `"<cycleId>/t:<taskId>"` (§3.3(2)/§4.4) so two cycles delegating
/// the same bare `taskId` never collide; this recovers the wire ids for steering
/// calls (`task.cancel` / `question.answer`). A key with no `/t:` marker is a bare
/// taskId with no cycle (the mock/backend runtimes), yielding `(None, key)`.
pub fn parse_task_key(key: &str) -> (Option<&str>, &str) {
    match key.split_once("/t:") {
        Some((cycle, task)) => (Some(cycle), task),
        None => (None, key),
    }
}

#[derive(Debug, Clone, Default)]
pub struct TurnBlock {
    pub at: i64,
    pub header: String,
    pub header_color: Option<String>,
    pub reasoning: Option<String>,
    pub content: Option<String>,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TaskState {
    pub task_id: String,
    pub status: TaskStatus,
    pub turns: usize,
    pub last_at: i64,
    pub turn_blocks: Vec<TurnBlock>,
    /// A pending `task_attention` prompt (reason · content), cleared on completion.
    pub attention: Option<String>,
    /// The `questionId` of a pending `task_attention` — the handle `question.answer`
    /// needs. `None` when the task has no open question.
    pub question_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AgentLane {
    pub key: String,
    pub label: String,
    pub role: AgentRole,
    pub turns: Vec<TurnBlock>,
    pub last_at: i64,
    pub tasks: Vec<TaskState>,
    pub context_tokens: Option<i64>,
    pub harness_label: Option<String>,
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
    pub parent_agent_id: Option<String>,
    pub descriptor: Option<AgentDescriptor>,
    pub active_tasks: i64,
}

/// A single pre-styled display row.
#[derive(Debug, Clone, Default)]
pub struct Line {
    pub text: String,
    pub color: Option<String>,
    pub dim: bool,
}

fn event_kind_color(kind: &str) -> Option<&'static str> {
    match kind {
        "tool" => Some("blue"),
        "prompt" => Some("cyan"),
        "stdout" => Some("gray"),
        "stderr" | "error" => Some("red"),
        "text" => Some("green"),
        "thinking" => Some("yellow"),
        _ => None,
    }
}

fn tokens_suffix(usage: &Option<Usage>) -> String {
    match usage {
        Some(u) => format!(" · {}↑ {}↓", u.input_tokens, u.output_tokens),
        None => String::new(),
    }
}

fn tool_line(name: &str, args: &serde_json::Value) -> String {
    let args = serde_json::to_string(args).unwrap_or_else(|_| "{}".into());
    let shown = if args.chars().count() > 200 {
        let mut s: String = args.chars().take(199).collect();
        s.push('…');
        s
    } else {
        args
    };
    format!("→ {name}({shown})")
}

/// Insertion-ordered lane collection.
struct Lanes {
    order: Vec<String>,
    map: HashMap<String, AgentLane>,
}

impl Lanes {
    fn new() -> Self {
        Lanes {
            order: Vec::new(),
            map: HashMap::new(),
        }
    }
    fn insert(&mut self, lane: AgentLane) {
        if !self.map.contains_key(&lane.key) {
            self.order.push(lane.key.clone());
        }
        self.map.insert(lane.key.clone(), lane);
    }
    fn get_mut(&mut self, key: &str) -> Option<&mut AgentLane> {
        self.map.get_mut(key)
    }
    fn contains(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }
    fn into_ordered(self) -> Vec<AgentLane> {
        let mut map = self.map;
        self.order
            .into_iter()
            .filter_map(|k| map.remove(&k))
            .collect()
    }
}

fn new_worker_lane(key: String, label: String) -> AgentLane {
    AgentLane {
        key,
        label,
        role: AgentRole::Worker,
        turns: Vec::new(),
        last_at: 0,
        tasks: Vec::new(),
        context_tokens: None,
        harness_label: None,
        agent_id: None,
        session_id: None,
        parent_agent_id: None,
        descriptor: None,
        active_tasks: 0,
    }
}

fn touch_task(lane: &mut AgentLane, task_id: &str, at: i64, block: Option<TurnBlock>) -> usize {
    let idx = match lane.tasks.iter().position(|t| t.task_id == task_id) {
        Some(i) => i,
        None => {
            lane.tasks.push(TaskState {
                task_id: task_id.to_string(),
                status: TaskStatus::Running,
                turns: 0,
                last_at: at,
                turn_blocks: Vec::new(),
                attention: None,
                question_id: None,
            });
            lane.tasks.len() - 1
        }
    };
    let task = &mut lane.tasks[idx];
    task.turns += 1;
    task.last_at = at;
    if let Some(b) = block {
        task.turn_blocks.push(b);
    }
    idx
}

/// Fold the event stream into lanes. Tier turns come from `inference_end`;
/// worker turns from the `task_*` events; session lanes from session/peer events.
pub fn derive_agent_lanes(
    events: &[EventEnvelope],
    harness: &str,
    roster: &[AgentDescriptor],
) -> Vec<AgentLane> {
    // Tier accumulators, in fixed order.
    let mut tier_turns: [(AgentRole, &str, Vec<TurnBlock>); 3] = [
        (AgentRole::Orchestrator, "orchestrator", Vec::new()),
        (AgentRole::Reasoning, "reasoning", Vec::new()),
        (AgentRole::Compress, "summarizer", Vec::new()),
    ];
    let mut tier_tokens: HashMap<usize, i64> = HashMap::new();
    let mut workers = Lanes::new();
    let mut task_agent: HashMap<String, String> = HashMap::new();

    // Seed one lane per connected roster agent (roster order).
    for agent in roster {
        let mut lane = new_worker_lane(format!("agent:{}", agent.id), {
            if agent.name.is_empty() {
                agent.id.clone()
            } else {
                agent.name.clone()
            }
        });
        lane.agent_id = Some(agent.id.clone());
        lane.descriptor = Some(agent.clone());
        if let Some(h) = agent
            .metadata
            .get("harness")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            lane.harness_label = Some(h.to_uppercase());
        }
        workers.insert(lane);
    }

    let tier_index = |role: &str| -> Option<usize> {
        match role {
            "orchestrator" => Some(0),
            "reasoning" => Some(1),
            "compress" => Some(2),
            _ => None,
        }
    };
    let lane_key_for = |task_id: &str, task_agent: &HashMap<String, String>| -> String {
        match task_agent.get(task_id) {
            Some(a) => format!("agent:{a}"),
            None => format!("worker:{task_id}"),
        }
    };

    for env in events {
        let at = env.at;
        match &env.event {
            TuiEvent::InferenceEnd {
                tier,
                op,
                model,
                duration_ms,
                usage,
                content,
                reasoning,
                tool_calls,
            } => {
                let Some(ti) = tier_index(tier) else { continue };
                let header = format!(
                    "{op} · {} · {duration_ms}ms{}",
                    model.as_deref().unwrap_or(tier),
                    tokens_suffix(usage)
                );
                let block = TurnBlock {
                    at,
                    header,
                    header_color: None,
                    reasoning: reasoning.clone(),
                    content: content.clone(),
                    tools: tool_calls
                        .as_ref()
                        .map(|cs| cs.iter().map(|c| tool_line(&c.name, &c.args)).collect())
                        .unwrap_or_default(),
                };
                tier_turns[ti].2.push(block);
                if let Some(u) = usage {
                    tier_tokens.insert(ti, u.input_tokens);
                }
            }
            TuiEvent::TaskStart {
                task_id,
                instruction,
                depth,
                agent_id,
            } => {
                if let Some(a) = agent_id {
                    task_agent.insert(task_id.clone(), a.clone());
                }
                let key = lane_key_for(task_id, &task_agent);
                if !workers.contains(&key) {
                    let mut lane = new_worker_lane(
                        key.clone(),
                        task_agent
                            .get(task_id)
                            .cloned()
                            .unwrap_or_else(|| task_id.clone()),
                    );
                    lane.last_at = at;
                    if let Some(a) = task_agent.get(task_id) {
                        lane.agent_id = Some(a.clone());
                    }
                    workers.insert(lane);
                }
                let lane = workers.get_mut(&key).unwrap();
                if agent_id.is_none() {
                    let label: String =
                        instruction.split_whitespace().collect::<Vec<_>>().join(" ");
                    let label: String = label.chars().take(48).collect();
                    lane.label = if label.is_empty() {
                        task_id.clone()
                    } else {
                        label
                    };
                }
                let tag = if agent_id.is_some() {
                    format!(" · {task_id}")
                } else {
                    String::new()
                };
                let block = TurnBlock {
                    at,
                    header: format!("dispatched{tag} · depth {depth}"),
                    header_color: Some("magenta".into()),
                    content: Some(instruction.clone()),
                    ..Default::default()
                };
                lane.turns.push(block.clone());
                lane.last_at = at;
                lane.active_tasks += 1;
                touch_task(lane, task_id, at, Some(block));
            }
            TuiEvent::TaskEvent {
                task_id,
                event_kind,
                content,
                harness: h,
            } => {
                let key = lane_key_for(task_id, &task_agent);
                ensure_lane(&mut workers, &key, task_id, &task_agent, at);
                let lane = workers.get_mut(&key).unwrap();
                let block = TurnBlock {
                    at,
                    header: event_kind.clone(),
                    header_color: event_kind_color(event_kind).map(str::to_string),
                    content: Some(content.clone()),
                    ..Default::default()
                };
                lane.turns.push(block.clone());
                lane.last_at = at;
                touch_task(lane, task_id, at, Some(block));
                if let Some(h) = h {
                    lane.harness_label = Some(h.clone());
                }
            }
            TuiEvent::TaskAttention {
                task_id,
                reason,
                content,
                question_id,
            } => {
                let key = lane_key_for(task_id, &task_agent);
                ensure_lane(&mut workers, &key, task_id, &task_agent, at);
                let lane = workers.get_mut(&key).unwrap();
                let block = TurnBlock {
                    at,
                    header: format!("attention · {reason}"),
                    header_color: Some("yellow".into()),
                    content: Some(content.clone()),
                    ..Default::default()
                };
                lane.turns.push(block.clone());
                lane.last_at = at;
                let idx = touch_task(lane, task_id, at, Some(block));
                lane.tasks[idx].attention = Some(format!("{reason}: {content}"));
                lane.tasks[idx].question_id = question_id.clone();
            }
            TuiEvent::TaskComplete { digest } => {
                // §3.3(4): tolerate a `task_complete` whose `task_start` was never seen
                // (or was evicted) — ensure the lane exists rather than dropping the
                // completion, the exact fold bug the contract flags.
                let key = lane_key_for(&digest.task_id, &task_agent);
                ensure_lane(&mut workers, &key, &digest.task_id, &task_agent, at);
                let lane = workers.get_mut(&key).unwrap();
                let tag = if task_agent.contains_key(&digest.task_id) {
                    format!(" · {}", digest.task_id)
                } else {
                    String::new()
                };
                // §3.3(3): map all three terminal states, keeping `cancelled` distinct.
                let status = TaskStatus::from_wire(&digest.status);
                let color = match status {
                    TaskStatus::Done => "green",
                    TaskStatus::Cancelled => "gray",
                    _ => "red",
                };
                let block = TurnBlock {
                    at,
                    header: format!("complete{tag} · {}", digest.status),
                    header_color: Some(color.into()),
                    content: if digest.digest.is_empty() {
                        None
                    } else {
                        Some(digest.digest.clone())
                    },
                    ..Default::default()
                };
                lane.turns.push(block.clone());
                lane.last_at = at;
                lane.active_tasks = (lane.active_tasks - 1).max(0);
                if let Some(u) = &digest.usage {
                    lane.context_tokens = Some(u.input_tokens);
                }
                let idx = touch_task(lane, &digest.task_id, at, Some(block));
                lane.tasks[idx].status = status;
                lane.tasks[idx].attention = None;
                lane.tasks[idx].question_id = None;
            }
            TuiEvent::SessionEvent {
                agent_id,
                session_id,
                event_kind,
                content,
            } => {
                let key = format!("session:{agent_id}#{session_id}");
                ensure_session_lane(&mut workers, &key, agent_id, session_id, at);
                let lane = workers.get_mut(&key).unwrap();
                lane.turns.push(TurnBlock {
                    at,
                    header: event_kind.clone(),
                    header_color: event_kind_color(event_kind).map(str::to_string),
                    content: Some(content.clone()),
                    ..Default::default()
                });
                lane.last_at = at;
            }
            TuiEvent::PeerSession {
                agent_id,
                session_id,
                state,
                harness: h,
            } => {
                let key = format!("session:{agent_id}#{session_id}");
                ensure_session_lane(&mut workers, &key, agent_id, session_id, at);
                let lane = workers.get_mut(&key).unwrap();
                lane.turns.push(TurnBlock {
                    at,
                    header: format!("session {state}"),
                    header_color: Some(
                        match state.as_str() {
                            "ended" => "red",
                            "idle" => "green",
                            _ => "yellow",
                        }
                        .into(),
                    ),
                    ..Default::default()
                });
                if let Some(h) = h {
                    lane.harness_label = Some(h.to_uppercase());
                }
                lane.last_at = at;
            }
            _ => {}
        }
    }

    // Build tier lanes.
    let mut tier_lanes: Vec<AgentLane> = Vec::new();
    for (ti, (role, label, turns)) in tier_turns.into_iter().enumerate() {
        let last_at = turns.last().map(|t| t.at).unwrap_or(0);
        tier_lanes.push(AgentLane {
            key: format!("tier:{label}"),
            label: label.to_string(),
            role,
            context_tokens: tier_tokens.get(&ti).copied(),
            turns,
            last_at,
            tasks: Vec::new(),
            harness_label: None,
            agent_id: None,
            session_id: None,
            parent_agent_id: None,
            descriptor: None,
            active_tasks: 0,
        });
    }

    // Tag worker lanes with their harness.
    let mut worker_lanes = workers.into_ordered();
    for lane in &mut worker_lanes {
        let tag = lane
            .harness_label
            .clone()
            .or_else(|| {
                if lane.session_id.is_some() {
                    None
                } else {
                    Some(harness.to_string())
                }
            })
            .filter(|s| !s.is_empty());
        if let Some(t) = tag {
            lane.label = format!("[{t}] {}", lane.label);
        }
    }

    // Group each machine's session lanes directly under its lane.
    let session_lanes: Vec<AgentLane> = worker_lanes
        .iter()
        .filter(|l| l.session_id.is_some())
        .cloned()
        .collect();
    let main_worker_lanes: Vec<AgentLane> = worker_lanes
        .iter()
        .filter(|l| l.session_id.is_none())
        .cloned()
        .collect();
    let mut grouped: Vec<AgentLane> = Vec::new();
    for lane in &main_worker_lanes {
        grouped.push(lane.clone());
        for s in &session_lanes {
            if s.parent_agent_id == lane.agent_id {
                grouped.push(s.clone());
            }
        }
    }
    let orphan_sessions: Vec<AgentLane> = session_lanes
        .iter()
        .filter(|s| {
            !main_worker_lanes
                .iter()
                .any(|l| l.agent_id == s.parent_agent_id)
        })
        .cloned()
        .collect();

    let agent_tiers: Vec<AgentLane> = tier_lanes
        .iter()
        .filter(|l| !l.role.is_function())
        .cloned()
        .collect();
    let function_tiers: Vec<AgentLane> = tier_lanes
        .iter()
        .filter(|l| l.role.is_function())
        .cloned()
        .collect();

    let mut out = agent_tiers;
    out.extend(grouped);
    out.extend(orphan_sessions);
    out.extend(function_tiers);
    out
}

fn ensure_lane(
    workers: &mut Lanes,
    key: &str,
    task_id: &str,
    task_agent: &HashMap<String, String>,
    at: i64,
) {
    if !workers.contains(key) {
        let mut lane = new_worker_lane(
            key.to_string(),
            task_agent
                .get(task_id)
                .cloned()
                .unwrap_or_else(|| task_id.to_string()),
        );
        lane.last_at = at;
        if let Some(a) = task_agent.get(task_id) {
            lane.agent_id = Some(a.clone());
        }
        workers.insert(lane);
    }
}

fn ensure_session_lane(workers: &mut Lanes, key: &str, agent_id: &str, session_id: &str, at: i64) {
    if !workers.contains(key) {
        let mut lane = new_worker_lane(key.to_string(), format!("↳ {session_id}"));
        lane.last_at = at;
        lane.session_id = Some(session_id.to_string());
        lane.parent_agent_id = Some(agent_id.to_string());
        workers.insert(lane);
    }
}

/// Running tasks first, then most-recently-active.
pub fn ordered_tasks(tasks: &[TaskState]) -> Vec<TaskState> {
    let mut v = tasks.to_vec();
    v.sort_by(|a, b| {
        let rank = |t: &TaskState| {
            if t.status == TaskStatus::Running {
                0
            } else {
                1
            }
        };
        rank(a).cmp(&rank(b)).then(b.last_at.cmp(&a.last_at))
    });
    v
}

/// One printed row of the Agents list.
#[derive(Debug, Clone)]
pub enum AgentRow {
    Separator,
    Lane {
        lane_index: usize,
    },
    Sub {
        lane_index: usize,
        task: TaskState,
        last: bool,
    },
    More {
        lane_index: usize,
        hidden: usize,
    },
}

impl AgentRow {
    pub fn lane_index(&self) -> Option<usize> {
        match self {
            AgentRow::Lane { lane_index }
            | AgentRow::Sub { lane_index, .. }
            | AgentRow::More { lane_index, .. } => Some(*lane_index),
            AgentRow::Separator => None,
        }
    }
    pub fn selectable(&self) -> bool {
        matches!(self, AgentRow::Lane { .. } | AgentRow::Sub { .. })
    }
}

/// Build the ordered Agents-list rows: each lane, the `── functions ──` divider
/// before the first function lane, and per-task sublanes (running first, capped).
pub fn agent_row_model(lanes: &[AgentLane], max_subtasks: usize) -> Vec<AgentRow> {
    let mut rows = Vec::new();
    let first_fn = lanes.iter().position(|l| l.role.is_function());
    for (lane_index, lane) in lanes.iter().enumerate() {
        if Some(lane_index) == first_fn {
            rows.push(AgentRow::Separator);
        }
        rows.push(AgentRow::Lane { lane_index });
        if lane.role == AgentRole::Worker
            && lane.key.starts_with("agent:")
            && !lane.tasks.is_empty()
        {
            let ordered = ordered_tasks(&lane.tasks);
            let shown = ordered.len().min(max_subtasks);
            let hidden = ordered.len() - shown;
            for (i, task) in ordered.iter().take(shown).enumerate() {
                rows.push(AgentRow::Sub {
                    lane_index,
                    task: task.clone(),
                    last: hidden == 0 && i == shown - 1,
                });
            }
            if hidden > 0 {
                rows.push(AgentRow::More { lane_index, hidden });
            }
        }
    }
    rows
}

fn blocks_to_lines(turns: &[TurnBlock], cols: usize) -> Vec<Line> {
    let mut lines = Vec::new();
    for turn in turns {
        let header = format!("{}  {}", crate::ui::util::clock(turn.at), turn.header);
        let header = if header.chars().count() > cols {
            let mut s: String = header.chars().take(cols.saturating_sub(1)).collect();
            s.push('…');
            s
        } else {
            header
        };
        lines.push(Line {
            text: header,
            color: Some(turn.header_color.clone().unwrap_or_else(|| "cyan".into())),
            dim: false,
        });
        if let Some(reasoning) = &turn.reasoning {
            lines.push(Line {
                text: "  · thinking".into(),
                color: Some("yellow".into()),
                dim: true,
            });
            for row in wrap(reasoning, cols.saturating_sub(2)) {
                lines.push(Line {
                    text: format!("  {row}"),
                    color: Some("yellow".into()),
                    dim: true,
                });
            }
        }
        if let Some(content) = &turn.content {
            lines.push(Line {
                text: "  › output".into(),
                color: Some("green".into()),
                dim: true,
            });
            for row in wrap(content, cols) {
                lines.push(Line {
                    text: row,
                    ..Default::default()
                });
            }
        }
        if !turn.tools.is_empty() {
            lines.push(Line {
                text: "  → tools".into(),
                color: Some("blue".into()),
                dim: true,
            });
            for tool in &turn.tools {
                for row in wrap(tool, cols) {
                    lines.push(Line {
                        text: row,
                        color: Some("blue".into()),
                        dim: false,
                    });
                }
            }
        }
        lines.push(Line::default());
    }
    lines
}

/// Flatten a lane's turns into pre-wrapped, styled rows. Agent-identity lanes
/// group turns under each task; others render their flat transcript.
pub fn lane_lines(lane: Option<&AgentLane>, width: usize) -> Vec<Line> {
    let Some(lane) = lane else { return Vec::new() };
    let cols = width.max(20);
    if lane.role == AgentRole::Worker && lane.key.starts_with("agent:") && !lane.tasks.is_empty() {
        let mut lines = Vec::new();
        for task in ordered_tasks(&lane.tasks) {
            lines.push(Line {
                text: format!(
                    "── {} · {} · {} turn(s) ──",
                    task.task_id,
                    task.status.label(),
                    task.turns
                ),
                color: Some(task.status.color().into()),
                dim: false,
            });
            let body = blocks_to_lines(&task.turn_blocks, cols);
            if body.is_empty() {
                lines.push(Line {
                    text: "  (no turns yet)".into(),
                    dim: true,
                    ..Default::default()
                });
            } else {
                lines.extend(body);
            }
        }
        return lines;
    }
    if lane.turns.is_empty() {
        return vec![Line {
            text: "No turns yet.".into(),
            dim: true,
            ..Default::default()
        }];
    }
    blocks_to_lines(&lane.turns, cols)
}

/// The per-task transcript for a task-focused view.
pub fn task_lines(task: &TaskState, width: usize) -> Vec<Line> {
    let cols = width.max(20);
    if task.turn_blocks.is_empty() {
        return vec![Line {
            text: "No turns yet.".into(),
            dim: true,
            ..Default::default()
        }];
    }
    blocks_to_lines(&task.turn_blocks, cols)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::events::TaskDigest;

    fn env(seq: u64, event: TuiEvent) -> EventEnvelope {
        EventEnvelope {
            seq,
            at: seq as i64 * 1000,
            event,
        }
    }

    #[test]
    fn tier_lanes_always_present_in_order() {
        let lanes = derive_agent_lanes(&[], "OPENCODE", &[]);
        // orchestrator, reasoning first; summarizer (function) last.
        assert_eq!(lanes.len(), 3);
        assert_eq!(lanes[0].label, "orchestrator");
        assert_eq!(lanes[1].label, "reasoning");
        assert_eq!(lanes[2].label, "summarizer");
        assert!(lanes[2].role.is_function());
    }

    #[test]
    fn inference_end_folds_into_tier() {
        let events = vec![env(
            1,
            TuiEvent::InferenceEnd {
                tier: "reasoning".into(),
                op: "execute_step".into(),
                model: Some("gpt".into()),
                duration_ms: 42,
                usage: Some(Usage {
                    input_tokens: 100,
                    output_tokens: 20,
                }),
                content: Some("hi".into()),
                reasoning: None,
                tool_calls: None,
            },
        )];
        let lanes = derive_agent_lanes(&events, "", &[]);
        let reasoning = &lanes[1];
        assert_eq!(reasoning.turns.len(), 1);
        assert!(reasoning.turns[0]
            .header
            .contains("execute_step · gpt · 42ms"));
        assert_eq!(reasoning.context_tokens, Some(100));
    }

    #[test]
    fn anonymous_task_lane_and_completion() {
        let events = vec![
            env(
                1,
                TuiEvent::TaskStart {
                    task_id: "t1".into(),
                    instruction: "do the thing".into(),
                    depth: 2,
                    agent_id: None,
                },
            ),
            env(
                2,
                TuiEvent::TaskEvent {
                    task_id: "t1".into(),
                    event_kind: "text".into(),
                    content: "progress".into(),
                    harness: None,
                },
            ),
            env(
                3,
                TuiEvent::TaskComplete {
                    digest: TaskDigest {
                        task_id: "t1".into(),
                        status: "done".into(),
                        digest: "result".into(),
                        result_ref: None,
                        usage: Some(Usage {
                            input_tokens: 500,
                            output_tokens: 50,
                        }),
                        depth: 2,
                    },
                },
            ),
        ];
        let lanes = derive_agent_lanes(&events, "OPENCODE", &[]);
        // orchestrator, reasoning, worker(t1), summarizer.
        let worker = lanes.iter().find(|l| l.key == "worker:t1").unwrap();
        assert_eq!(worker.label, "[OPENCODE] do the thing");
        assert_eq!(worker.active_tasks, 0);
        assert_eq!(worker.context_tokens, Some(500));
        assert_eq!(worker.tasks[0].status, TaskStatus::Done);
    }

    #[test]
    fn agent_lane_stacks_tasks_with_row_model() {
        let roster = vec![AgentDescriptor {
            id: "dev".into(),
            name: "Dev".into(),
            description: String::new(),
            availability: "online".into(),
            tags: vec![],
            metadata: serde_json::Map::new(),
        }];
        let mut events = Vec::new();
        for i in 0..10 {
            events.push(env(
                i,
                TuiEvent::TaskStart {
                    task_id: format!("t{i}"),
                    instruction: "x".into(),
                    depth: 2,
                    agent_id: Some("dev".into()),
                },
            ));
        }
        let lanes = derive_agent_lanes(&events, "TINYPLACE", &roster);
        let dev = lanes.iter().find(|l| l.key == "agent:dev").unwrap();
        assert_eq!(dev.tasks.len(), 10);
        let rows = agent_row_model(&lanes, 8);
        // Cap at 8 sublanes + a "+2 more" row for the dev lane.
        let subs = rows
            .iter()
            .filter(|r| matches!(r, AgentRow::Sub { .. }))
            .count();
        let more = rows
            .iter()
            .filter(|r| matches!(r, AgentRow::More { .. }))
            .count();
        assert_eq!(subs, 8);
        assert_eq!(more, 1);
        // The functions divider precedes the summarizer.
        assert!(rows.iter().any(|r| matches!(r, AgentRow::Separator)));
    }

    #[test]
    fn session_lanes_group_under_machine() {
        let events = vec![env(
            1,
            TuiEvent::PeerSession {
                agent_id: "m1".into(),
                session_id: "s1".into(),
                state: "working".into(),
                harness: Some("codex".into()),
            },
        )];
        let lanes = derive_agent_lanes(&events, "TINYPLACE", &[]);
        let session = lanes
            .iter()
            .find(|l| l.session_id.as_deref() == Some("s1"))
            .unwrap();
        assert_eq!(session.parent_agent_id.as_deref(), Some("m1"));
        // A session lane is tagged only with a harness it learned itself (CODEX),
        // never the global default (TINYPLACE).
        assert_eq!(session.harness_label.as_deref(), Some("CODEX"));
        assert_eq!(session.label, "[CODEX] ↳ s1");
    }
}
