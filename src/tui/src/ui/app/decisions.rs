//! Prepared-decision overlay state and actions.

use crossterm::event::KeyCode;

use crate::ui::composer::Draft;
use crate::ui::decisions::{decision_items, DecisionItem};

use super::types::{App, Prompt, PromptKind};

impl App {
    /// Current non-dismissed decisions derived from live snapshot state.
    pub fn decisions(&self) -> Vec<DecisionItem> {
        decision_items(self.snapshot.harness.as_ref(), &self.lanes())
            .into_iter()
            .filter(|item| !self.dismissed_decisions.contains(&item.id))
            .collect()
    }

    /// Open the decision overlay when actionable items exist.
    pub(super) fn open_decisions(&mut self) {
        let count = self.decisions().len();
        if count == 0 {
            self.set_status("No prepared decisions");
            return;
        }
        self.decision_index = self.decision_index.min(count - 1);
        self.decision_open = true;
        self.set_status(format!("Decisions · {count} pending"));
    }

    /// Route one key while the decision overlay owns input.
    pub(super) fn handle_decision_key(&mut self, code: KeyCode) {
        let items = self.decisions();
        self.decision_index = self.decision_index.min(items.len().saturating_sub(1));
        match code {
            KeyCode::Esc => {
                self.decision_open = false;
                self.set_status("Decision queue closed");
            }
            KeyCode::Up => {
                self.decision_index = self.decision_index.saturating_sub(1);
            }
            KeyCode::Down => {
                self.decision_index = (self.decision_index + 1).min(items.len().saturating_sub(1));
            }
            KeyCode::Char('d') => self.dismiss_selected_decision(items),
            KeyCode::Enter => self.answer_or_dismiss_selected_decision(items),
            _ => {}
        }
    }

    /// Dismiss the current item locally and keep the cursor in range.
    fn dismiss_selected_decision(&mut self, items: Vec<DecisionItem>) {
        let Some(item) = items.get(self.decision_index) else {
            self.decision_open = false;
            return;
        };
        self.dismissed_decisions.insert(item.id.clone());
        let remaining = self.decisions().len();
        self.decision_index = self.decision_index.min(remaining.saturating_sub(1));
        self.decision_open = remaining > 0;
        self.set_status(format!("Decision dismissed · {remaining} pending"));
    }

    /// Answer a routed worker question, or dismiss an informational escalation.
    fn answer_or_dismiss_selected_decision(&mut self, items: Vec<DecisionItem>) {
        let Some(item) = items.get(self.decision_index).cloned() else {
            self.decision_open = false;
            return;
        };
        let Some(target) = item.answer_target else {
            self.dismiss_selected_decision(items);
            return;
        };
        self.decision_open = false;
        self.prompt = Some(Prompt {
            kind: PromptKind::DecisionAnswer {
                decision_id: item.id,
                cycle_id: target.cycle_id,
                question_id: target.question_id,
            },
            title: format!("Answer — {}", item.question),
            draft: Draft::new(),
        });
        self.set_status("Type an answer · Enter send · Esc cancel");
    }
}
