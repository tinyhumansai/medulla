//! The welcome screen's state machine: [`WelcomeScreen::handle_key`] folds key
//! events into step transitions and optional async [`WelcomeCmd`]s, and
//! [`WelcomeScreen::apply`] folds [`WelcomeEvent`]s from spawned work back into
//! state. Rendering lives in [`super::draw`]; the data model in [`super::types`].
//!
//! The consent gate is the load-bearing rule here: [`WelcomeCmd::UploadAndClaim`]
//! is only ever emitted from [`Step::Consent`] on an explicit Enter. No other
//! key, event, or step transition can cause an upload.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::types::{Step, WelcomeCmd, WelcomeEvent, WelcomeOutcome, WelcomeScreen};

impl WelcomeScreen {
    /// Handle one key event, optionally emitting an async command.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<WelcomeCmd> {
        // Ctrl-C skips from anywhere — declining is always one keypress away.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.outcome = Some(WelcomeOutcome::Skipped);
            return None;
        }

        match self.step {
            Step::Intro => match key.code {
                KeyCode::Enter => {
                    self.error = None;
                    self.step = Step::Scanning;
                    Some(WelcomeCmd::Scan)
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.outcome = Some(WelcomeOutcome::Skipped);
                    None
                }
                _ => None,
            },
            // Scanning and uploading are uninterruptible except by skipping.
            Step::Scanning | Step::Uploading => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                    self.outcome = Some(WelcomeOutcome::Skipped);
                }
                None
            }
            Step::Consent => match key.code {
                KeyCode::Enter => {
                    self.error = None;
                    self.upload_total = self.scan.session_count;
                    self.uploaded = 0;
                    self.step = Step::Uploading;
                    Some(WelcomeCmd::UploadAndClaim)
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.outcome = Some(WelcomeOutcome::Skipped);
                    None
                }
                _ => None,
            },
            Step::Reveal => match key.code {
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => {
                    self.outcome = Some(WelcomeOutcome::Completed {
                        awarded_usd: self.awarded_usd,
                        tier: self.tier.clone(),
                    });
                    None
                }
                _ => None,
            },
            Step::Empty => match key.code {
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => {
                    // Not a skip: nothing was ever offered, so the caller keeps
                    // the offer open for once this user has sessions.
                    self.outcome = Some(WelcomeOutcome::NothingToShare);
                    None
                }
                _ => None,
            },
        }
    }

    /// Fold an async event back into screen state.
    pub fn apply(&mut self, ev: WelcomeEvent) {
        match ev {
            WelcomeEvent::ScanReady(summary) => {
                let found_something = summary.session_count > 0;
                self.scan = summary;
                // Nothing to share is a dead end, not an error — say so plainly
                // rather than asking the user to consent to sending zero files.
                self.step = if found_something {
                    Step::Consent
                } else {
                    Step::Empty
                };
            }
            WelcomeEvent::UploadProgress {
                uploaded,
                total,
                redactions,
            } => {
                self.uploaded = uploaded;
                self.upload_total = total;
                self.redactions = redactions;
            }
            WelcomeEvent::Claimed {
                awarded_usd,
                tier,
                breakdown,
                max_reward_usd,
                already_claimed,
            } => {
                self.awarded_usd = awarded_usd;
                self.tier = tier;
                self.breakdown = breakdown;
                if max_reward_usd > 0.0 {
                    self.max_reward_usd = max_reward_usd;
                }
                self.already_claimed = already_claimed;
                self.error = None;
                self.step = Step::Reveal;
            }
            WelcomeEvent::Failed(msg) => {
                self.error = Some(msg);
                // Land on the reveal so the user sees the failure and can leave;
                // any credit already granted is reported by the backend anyway.
                self.step = Step::Reveal;
            }
        }
    }
}
