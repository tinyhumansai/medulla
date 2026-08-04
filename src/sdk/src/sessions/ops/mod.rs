//! [`SessionOp`] — the operator actions the Sessions screen dispatches, and the
//! manager entry that applies one.
//!
//! The UI stays synchronous: a keypress produces a `SessionOp` value, the event
//! loop runs it off the render thread, and the result comes back as a status
//! line. Mirrors the shape [`WorkerOp`](crate::runtime::WorkerOp) already
//! establishes for the Workers tab.

use crate::protocol::HarnessProvider;

use super::manager::{OpenSession, SessionManager};
use super::types::{SessionClass, SessionDriver, SessionOrigin};

impl SessionOp {
    /// Parse a free-text "open session" line into a [`SessionOp::Open`].
    ///
    /// Grammar: `<conversation> [provider]`. The first token is the conversation
    /// anchor; an optional second token naming a known provider selects the
    /// harness. Returns `None` for blank input so the caller can surface an
    /// "empty" notice rather than issuing a no-op.
    pub fn parse_open(input: &str, class: SessionClass) -> Option<Self> {
        let text = input.trim();
        if text.is_empty() {
            return None;
        }
        let mut parts = text.split_whitespace();
        let conversation = parts.next()?.to_string();
        let provider = parts.next().and_then(HarnessProvider::from_wire);
        Some(SessionOp::Open {
            conversation,
            class,
            provider,
        })
    }
}

impl SessionManager {
    /// Apply one operator action, returning the status line to show.
    ///
    /// Every arm reports what actually happened rather than what was asked for —
    /// "no turn in flight to interrupt" is more useful than a silent no-op.
    pub async fn apply(&self, op: SessionOp) -> Result<String, String> {
        match op {
            SessionOp::Open {
                conversation,
                class,
                provider,
            } => {
                let id = self.open(OpenSession {
                    conversation: conversation.clone(),
                    provider,
                    class: Some(class),
                    driver: SessionDriver::Task,
                    workspace: None,
                    model: None,
                    // A [`SessionOp`] is an operator action by definition — the
                    // orchestrator dispatches, it does not press keys — so the
                    // session it opens is the person's.
                    origin: SessionOrigin::User,
                    name: None,
                });
                Ok(match class {
                    SessionClass::Unbound => {
                        format!("Opened {id} · {conversation} — converse across turns")
                    }
                    // Worth saying out loud: a bounded session is torn down on
                    // its reply, so it is a staging step for one turn, not
                    // something to come back to.
                    SessionClass::Bounded => {
                        format!("Opened {id} · {conversation} — one turn, then gone")
                    }
                })
            }
            SessionOp::Submit { id, text } => {
                let outcome = self.submit(&id, &text).await?;
                Ok(if outcome.aborted {
                    format!("{id} · turn interrupted")
                } else if outcome.is_error {
                    format!("{id} · turn reported an error")
                } else {
                    format!("{id} · turn complete")
                })
            }
            SessionOp::Interrupt { id } => Ok(if self.interrupt(&id) {
                format!("{id} · interrupt sent; the session stays live")
            } else {
                format!("{id} · no turn in flight to interrupt")
            }),
            SessionOp::Reset { id } => Ok(if self.reset(&id) {
                format!("{id} · context dropped; the next turn starts fresh")
            } else {
                format!("{id} · no bound context to reset")
            }),
            SessionOp::Close { id } => {
                self.close(&id).await;
                Ok(format!("{id} · closed"))
            }
            SessionOp::Forget { id } => Ok(if self.forget(&id) {
                format!("{id} · forgotten")
            } else {
                format!("{id} · close it before forgetting it")
            }),
        }
    }
}

mod types;
pub use types::SessionOp;
