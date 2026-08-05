//! What a workflow run is doing *while* it runs, per graph node.
//!
//! A run's durable record answers what happened after the fact: which steps
//! ran, how long they took, what each returned. It cannot answer the question
//! an operator has while watching one — is the agent editing files, running the
//! suite, or stuck — because a step produces its record only once it settles,
//! and an agent step is a whole coding session long.
//!
//! So an `agent` node's harness streams its progress frames here as they
//! arrive (see [`medulla::flow_engine::NodeProgressSink`]), keyed by node, and
//! the step preview draws the tail of them under the node the graph cursor is
//! on. The buffer is per run and bounded: this is a window onto live work, not
//! a second transcript store.

use std::collections::HashMap;

/// The most frames kept per node.
///
/// A coding session emits thousands; the pane shows a tail, and the rest is
/// the harness's own transcript to keep.
const MAX_FRAMES_PER_NODE: usize = 200;

/// The live progress of one run, by node.
#[derive(Debug, Default, Clone)]
pub(in crate::ui::app) struct LiveRun {
    /// The workflow being run.
    pub(in crate::ui::app) workflow: String,
    /// Whether the run is still executing.
    pub(in crate::ui::app) running: bool,
    /// Frames per node id, oldest first.
    frames: HashMap<String, Vec<String>>,
}

impl LiveRun {
    /// A run that has just started and reported nothing yet.
    fn new(workflow: String) -> Self {
        Self {
            workflow,
            running: true,
            frames: HashMap::new(),
        }
    }

    /// Record one frame against `node`, dropping the oldest past the bound.
    fn push(&mut self, node: &str, line: String) {
        let frames = self.frames.entry(node.to_string()).or_default();
        frames.push(line);
        if frames.len() > MAX_FRAMES_PER_NODE {
            let excess = frames.len() - MAX_FRAMES_PER_NODE;
            frames.drain(..excess);
        }
    }

    /// The frames `node` has emitted, oldest first.
    pub(in crate::ui::app) fn frames(&self, node: &str) -> &[String] {
        self.frames.get(node).map(Vec::as_slice).unwrap_or_default()
    }

    /// Build a view of a run this process did not execute, from the frames its
    /// reporter sent over the control socket.
    ///
    /// A run started by a harness over MCP runs in another process, so its
    /// frames arrive as reports rather than through the sink. They describe the
    /// same thing and are drawn the same way, so they become the same shape
    /// here rather than a second one the renderer would have to know about.
    fn from_reported(run: &medulla::control_socket::HarnessRun) -> Self {
        let mut view = Self {
            workflow: run.workflow_id.clone(),
            running: !run.status.is_terminal(),
            frames: HashMap::new(),
        };
        for frame in &run.frames {
            // A frame with no node is about the run rather than about a step —
            // "started", a summary — and has no box on the graph to sit in.
            if let Some(node) = &frame.node {
                view.push(node, frame.text.clone());
            }
        }
        view
    }
}

impl super::super::types::App {
    /// Begin tracking a run this TUI just started.
    pub fn workflow_run_started(&mut self, workflow: &str, run_id: &str) {
        // One live run per workflow: a second `x` on the same workflow replaces
        // the picture rather than interleaving two runs' frames under one node.
        self.live_runs
            .retain(|_, run| run.workflow != workflow || run.running);
        self.live_runs
            .insert(run_id.to_string(), LiveRun::new(workflow.to_string()));
    }

    /// Record one streamed harness frame against the node that produced it.
    pub fn workflow_run_output(&mut self, run_id: &str, node: &str, line: String) {
        // Only for a run this process is tracking. A frame for a run that has
        // already been replaced belongs to work nobody is looking at.
        if let Some(run) = self.live_runs.get_mut(run_id) {
            run.push(node, line);
        }
    }

    /// Mark a run settled, keeping its frames for the operator to read.
    pub fn workflow_run_finished(&mut self, run_id: &str) {
        if let Some(run) = self.live_runs.get_mut(run_id) {
            run.running = false;
        }
    }

    /// The live progress of the run the graph is overlaid with, if it has any.
    ///
    /// Falls back to the newest live run of the selected workflow, so pressing
    /// `x` and watching works without first walking the rail onto a run row
    /// that did not exist when the key was pressed.
    pub(in crate::ui::app) fn live_run_for_view(&self) -> Option<&LiveRun> {
        if let Some(id) = &self.wf.overlay {
            if let Some(run) = self.live_runs.get(id) {
                return Some(run);
            }
        }
        let workflow = self.selected_workflow()?.id.clone();
        self.live_runs
            .values()
            .filter(|run| run.workflow == workflow)
            .find(|run| run.running)
    }

    /// The live view for the selected run, from either source.
    ///
    /// Owned rather than borrowed because the reported half is rebuilt from the
    /// registry: it lives in the control plane, not in the App, and a borrow of
    /// it would hold that lock across a render pass.
    pub(in crate::ui::app) fn live_run_view(&self) -> Option<LiveRun> {
        if let Some(local) = self.live_run_for_view() {
            return Some(local.clone());
        }
        let reported = self.harness_runs.find(self.wf.overlay.as_deref()?)?;
        Some(LiveRun::from_reported(&reported))
    }
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod live_tests;
