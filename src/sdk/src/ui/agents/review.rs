//! Attribution of review-task verdict notes back to implementation tasks.

use std::collections::HashMap;

use super::types::AgentLane;

#[derive(Default)]
pub(super) struct ReviewTracker {
    targets: HashMap<String, String>,
    verdicts: HashMap<String, crate::autoreview::ReviewVerdict>,
}

impl ReviewTracker {
    pub(super) fn record_start(&mut self, task_id: &str, instruction: &str) {
        if let Some(target) = crate::autoreview::review_target(instruction) {
            self.targets.insert(task_id.to_string(), target.to_string());
        }
    }

    pub(super) fn record_note(&mut self, task_id: &str, note: &str) {
        let Some(target) = self.targets.get(task_id) else {
            return;
        };
        if let Some(verdict) = crate::autoreview::parse_verdict(note) {
            self.verdicts.insert(target.clone(), verdict);
        }
    }

    pub(super) fn apply(self, lanes: &mut [AgentLane]) {
        for task in lanes.iter_mut().flat_map(|lane| &mut lane.tasks) {
            if let Some(verdict) = self.verdicts.get(&task.task_id) {
                task.review = Some(verdict.clone());
            }
        }
    }
}
