//! The event-stream fold: turn the flat event log into the orchestrator lane
//! plus one lane per connected roster agent / anonymous task / peer session.
//! A port of the TS `deriveAgentLanes` essentials. Owns [`derive_agent_lanes`]
//! and the private lane-collection machinery it drives.
//!
//! **The reasoning tier is not a lane.** Since the manager refactor the tier
//! between the orchestrator and a harness task is 0..N concurrent *managers*,
//! and the backend deliberately does not stream them: what reaches this client
//! is the orchestrator and the agents it is managing. A manager is an
//! implementation detail of how the orchestrator fanned work out, not a
//! participant the operator talks to or steers, and rendering one lane labelled
//! "reasoning" for N of them described the pre-4.0.0 topology anyway. Inference
//! turns on the `reasoning` and `compress` tiers are therefore dropped here
//! rather than folded into a lane nothing feeds. They remain visible verbatim
//! under Settings › Trace, which is where a hidden layer belongs.

use std::collections::HashMap;

use crate::runtime::AgentDescriptor;
use crate::ui::events::{EventEnvelope, TuiEvent};

use super::fmt::{event_kind_color, tokens_suffix, tool_line};
use super::types::{AgentLane, AgentRole, TaskState, TaskStatus, TurnBlock};

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

/// Build a fresh worker-role lane with all optional fields cleared.
fn new_agent_lane(key: String, label: String) -> AgentLane {
    AgentLane {
        key,
        label,
        role: AgentRole::Agent,
        turns: Vec::new(),
        last_at: 0,
        tasks: Vec::new(),
        context_tokens: None,
        usage: Default::default(),
        harness_label: None,
        agent_id: None,
        session_id: None,
        parent_agent_id: None,
        descriptor: None,
        active_tasks: 0,
        work: None,
    }
}

/// Find-or-create the task by id, bump its turn count/timestamp, optionally push a
/// block, and return its index.
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
                work: None,
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
    // Tier accumulators. Only the orchestrator gets one: the manager tier and
    // the compress function are deliberately hidden from this view (see the
    // module doc), so their inference turns are folded nowhere.
    let mut tier_turns: [(AgentRole, &str, Vec<TurnBlock>); 1] =
        [(AgentRole::Orchestrator, "orchestrator", Vec::new())];
    let mut tier_tokens: HashMap<usize, i64> = HashMap::new();
    let mut tier_usage: HashMap<usize, crate::ui::meters::LaneUsage> = HashMap::new();
    let mut workers = Lanes::new();
    let mut task_agent: HashMap<String, String> = HashMap::new();
    // Per-task work folds, applied onto the tasks once the lanes are built.
    let mut task_work: HashMap<String, crate::harness_work::WorkFold> = HashMap::new();

    // Seed one lane per connected roster agent (roster order).
    for agent in roster {
        // The lane *key* keeps the full id — identity must stay exact, or two
        // peers sharing four leading characters would fold into one lane. Only
        // the label is shortened, and only when it is a bare address: a name
        // someone chose is meaningful text and survives whole.
        let mut lane = new_agent_lane(format!("agent:{}", agent.id), {
            let shown = if agent.name.is_empty() {
                agent.id.as_str()
            } else {
                agent.name.as_str()
            };
            crate::ui::util::short_if_address(shown)
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

    // `None` for every hidden tier, which drops that inference turn.
    let tier_index = |role: &str| -> Option<usize> {
        match role {
            "orchestrator" => Some(0),
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
                    tier_usage.entry(ti).or_default().accumulate(u);
                }
            }
            TuiEvent::TaskStart {
                task_id,
                instruction,
                depth,
                agent_id,
                ..
            } => {
                if let Some(a) = agent_id {
                    task_agent.insert(task_id.clone(), a.clone());
                }
                let key = lane_key_for(task_id, &task_agent);
                if !workers.contains(&key) {
                    let mut lane = new_agent_lane(
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
                // A structured work event carries its payload as the event's
                // JSON content. Folding it here is what lets a backend that
                // forwards a harness's todo list reach the same panel a
                // locally-hosted worker fills — one surface, either route.
                //
                // It is folded *instead of* being appended to the transcript:
                // rendered as a turn it is a wall of raw JSON, and the panel is
                // where that same content is legible.
                if crate::harness_work::kinds::is_work_kind(event_kind) {
                    if let Ok(payload) = serde_json::from_str::<serde_json::Value>(content) {
                        task_work
                            .entry(task_id.clone())
                            .or_default()
                            .apply(event_kind, &payload, at);
                        let lane = workers.get_mut(&key).unwrap();
                        lane.last_at = at;
                        // The task must still exist for the fold to attach to,
                        // and its clock must move — work is activity.
                        touch_task(lane, task_id, at, None);
                        continue;
                    }
                }
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
                    lane.usage.accumulate(u);
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
            usage: tier_usage.get(&ti).copied().unwrap_or_default(),
            turns,
            last_at,
            tasks: Vec::new(),
            harness_label: None,
            agent_id: None,
            session_id: None,
            parent_agent_id: None,
            descriptor: None,
            active_tasks: 0,
            work: None,
        });
    }

    // Tag worker lanes with their harness, and attach each task's folded work
    // to the task it belongs to — plus the freshest of them to the lane, so a
    // rail row can show progress without the operator opening anything.
    let mut worker_lanes = workers.into_ordered();
    for lane in &mut worker_lanes {
        for task in lane.tasks.iter_mut() {
            let snapshot = task_work.get(&task.task_id).map(|fold| fold.snapshot());
            if let Some(snapshot) = snapshot.filter(|snapshot| !snapshot.is_empty()) {
                task.work = Some(Box::new(snapshot.clone()));
            }
        }
        lane.work = lane
            .tasks
            .iter()
            .filter(|task| task.work.is_some())
            .max_by_key(|task| task.last_at)
            .and_then(|task| task.work.clone());
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

/// Ensure a worker lane exists for `key`, seeding it from `task_agent` when absent.
fn ensure_lane(
    workers: &mut Lanes,
    key: &str,
    task_id: &str,
    task_agent: &HashMap<String, String>,
    at: i64,
) {
    if !workers.contains(key) {
        // The fallback is an agent id, which for a host-link peer *is* its
        // address — so it gets the same treatment as one.
        let mut lane = new_agent_lane(
            key.to_string(),
            task_agent
                .get(task_id)
                .map(|id| crate::ui::util::short_if_address(id))
                .unwrap_or_else(|| task_id.to_string()),
        );
        lane.last_at = at;
        if let Some(a) = task_agent.get(task_id) {
            lane.agent_id = Some(a.clone());
        }
        workers.insert(lane);
    }
}

/// Ensure a session lane exists for `key`, tagging its parent machine.
fn ensure_session_lane(workers: &mut Lanes, key: &str, agent_id: &str, session_id: &str, at: i64) {
    if !workers.contains(key) {
        // A session id is a UUID — 36 characters of the same opaque noise an
        // address is, and it sits in the same rail measuring the same sidebar.
        // Shortened unconditionally rather than sniffed: we know what this is.
        let mut lane = new_agent_lane(
            key.to_string(),
            format!("↳ {}", crate::ui::util::short_address(session_id)),
        );
        lane.last_at = at;
        lane.session_id = Some(session_id.to_string());
        lane.parent_agent_id = Some(agent_id.to_string());
        workers.insert(lane);
    }
}

mod types;
use types::Lanes;
