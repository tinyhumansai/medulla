//! Worker-contract and manual lane claims projected onto the SDK blast-radius fold.

use crate::ui::agents::{
    claimed_dirty_paths, contract_permitted_paths, evaluate_lane_claims, validate_claim_patterns,
    AgentLane, AgentRow, ClaimedPath, LaneClaim, LaneGuardReport,
};
use crate::ui::composer::Draft;

use super::types::{App, Prompt, PromptKind};

impl App {
    /// The lane under the Agents-list cursor, including a selected task's parent.
    pub(super) fn selected_agent_lane(&self) -> Option<AgentLane> {
        let lanes = self.lanes();
        let lane_index = self
            .agent_rows()
            .get(self.agent_index)
            .and_then(AgentRow::lane_index)?;
        lanes.get(lane_index).cloned()
    }

    /// Open the selected lane's comma-separated manual claim editor.
    pub(super) fn open_lane_claim_prompt(&mut self) {
        let Some(lane) = self.selected_agent_lane() else {
            self.set_status("Select a lane before editing its path claim");
            return;
        };
        let existing = self
            .lane_claims
            .get(&lane.key)
            .map(|patterns| patterns.join(", "))
            .unwrap_or_default();
        self.prompt = Some(Prompt {
            kind: PromptKind::LaneClaim { lane_key: lane.key },
            title: format!("Claim paths for {} — comma-separated globs", lane.label),
            draft: Draft {
                cursor: existing.chars().count(),
                text: existing,
            },
        });
        self.set_status("Agents · Enter save claim · empty clears · Esc cancel");
    }

    /// Validate and store a manual lane claim. Empty input removes the claim.
    pub(super) fn submit_lane_claim(&mut self, lane_key: String, text: &str) {
        let patterns = text
            .split(',')
            .map(str::trim)
            .filter(|pattern| !pattern.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if patterns.is_empty() {
            self.lane_claims.remove(&lane_key);
            self.set_status("Agents · lane claim cleared");
            return;
        }
        if let Err(error) = validate_claim_patterns(&patterns) {
            self.set_status(format!("Agents · {error}"));
            return;
        }
        self.lane_claims.insert(lane_key, patterns);
        self.set_status("Agents · lane claim saved");
    }

    /// Current dirty paths from all successfully loaded local workspaces.
    fn dirty_claim_paths(&self) -> Vec<ClaimedPath> {
        self.repo_files()
            .into_iter()
            .map(|(workspace, change)| ClaimedPath {
                workspace,
                path: change.path,
            })
            .collect()
    }

    /// Contract paths take precedence; manual claims support older task events.
    pub(super) fn effective_lane_claim(&self, lane: &AgentLane) -> Option<(Vec<String>, bool)> {
        contract_permitted_paths(lane)
            .map(|paths| (paths, true))
            .or_else(|| {
                self.lane_claims
                    .get(&lane.key)
                    .cloned()
                    .map(|paths| (paths, false))
            })
    }

    /// Evaluate every visible contract/manual claim against live repository state.
    pub(super) fn lane_guard_report(&self) -> LaneGuardReport {
        let dirty = self.dirty_claim_paths();
        let claims = self
            .lanes()
            .iter()
            .filter_map(|lane| {
                let (permitted_paths, _) = self.effective_lane_claim(lane)?;
                claimed_dirty_paths(&permitted_paths, &dirty)
                    .ok()
                    .map(|touched_paths| LaneClaim {
                        lane_key: lane.key.clone(),
                        permitted_paths,
                        touched_paths,
                    })
            })
            .collect::<Vec<_>>();
        evaluate_lane_claims(&claims, &self.loaded.config.workflow.shared_path_denylist)
            .unwrap_or_default()
    }
}
