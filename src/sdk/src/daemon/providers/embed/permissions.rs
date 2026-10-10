//! Inline approvals use Medulla's existing input forwarding channel.
//! Decisions must name the pending call; closed channels fail closed.

use openhuman_embed::seams::{ToolHookContext, ToolHookDecision};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

pub(super) struct Approvals {
    answers: Mutex<mpsc::UnboundedReceiver<String>>,
    events: mpsc::UnboundedSender<(String, Value)>,
}
impl Approvals {
    pub fn new(
        answers: mpsc::UnboundedReceiver<String>,
        events: mpsc::UnboundedSender<(String, Value)>,
    ) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(answers),
            events,
        })
    }
    pub async fn decide(&self, ctx: ToolHookContext) -> ToolHookDecision {
        let mut answers = self.answers.lock().await;
        // Input submitted before this request cannot approve a later call.
        while answers.try_recv().is_ok() {}
        if self.events.send(("approval_request".into(), json!({"call_id":ctx.call_id,
            "tool_name":ctx.tool_name, "display":ctx.tool_name, "reason":"Approve this native tool call"}))).is_err() {
            return ToolHookDecision::Deny("approval surface closed".into());
        }
        while let Some(answer) = answers.recv().await {
            let Ok(answer) = serde_json::from_str::<Value>(&answer) else {
                continue;
            };
            if answer["call_id"] != ctx.call_id {
                continue;
            }
            return match answer["decision"].as_str() {
                Some("allow") => ToolHookDecision::Proceed,
                Some("deny") => ToolHookDecision::Deny("operator denied the tool".into()),
                _ => ToolHookDecision::Deny("invalid approval decision".into()),
            };
        }
        ToolHookDecision::Deny("approval input closed".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context() -> ToolHookContext {
        serde_json::from_value(
            json!({"event":"PreToolUse","call_id":"pending-1","tool_name":"shell",
            "arguments":{"command":"pwd"},"success":null,"duration_ms":null,"cwd":"/checkout"}),
        )
        .unwrap()
    }
    #[tokio::test]
    async fn approval_requires_the_pending_call_id_and_closed_input_denies() {
        let (input, receiver) = mpsc::unbounded_channel();
        let (events, mut observed) = mpsc::unbounded_channel();
        let approvals = Approvals::new(receiver, events);
        let task = tokio::spawn(async move { approvals.decide(context()).await });
        let (_, payload) = observed.recv().await.unwrap();
        assert_eq!(payload["call_id"], "pending-1");
        input
            .send(json!({"call_id":"another-call","decision":"allow"}).to_string())
            .unwrap();
        input
            .send(json!({"call_id":"pending-1","decision":"deny"}).to_string())
            .unwrap();
        assert!(matches!(task.await.unwrap(), ToolHookDecision::Deny(_)));
        let (input, receiver) = mpsc::unbounded_channel();
        drop(input);
        let (events, mut observed) = mpsc::unbounded_channel();
        let approvals = Approvals::new(receiver, events);
        assert!(matches!(
            approvals.decide(context()).await,
            ToolHookDecision::Deny(_)
        ));
        assert_eq!(observed.recv().await.unwrap().0, "approval_request");
    }
}
