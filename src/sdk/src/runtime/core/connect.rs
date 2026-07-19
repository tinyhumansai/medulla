//! The [`CoreRuntime`] connection lifecycle: the handshake/adopt/subscribe/seed
//! constructor that stands the runtime up, plus the small change-ping and the
//! stall-threshold test seam. The live fold loop it spawns lives in [`super::fold`].

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use serde_json::Value;
use tokio::sync::{broadcast, mpsc};

use crate::memory::MemoryService;
use crate::runtime::core_client::{CoreClient, CoreEvent, SeqTracker};
use crate::ui::chat_store::{now_millis, ChatMessage};
use crate::ui::events::TuiEvent;

use super::events::synth_from_snapshot;
use super::memory::memory_capability;
use super::types::{CoreRuntime, State, Thread, STALL_MS};
use super::workers::workers_from_payload;

impl CoreRuntime {
    /// Connect: handshake, adopt (or create) an active thread, subscribe, seed its
    /// snapshot, then spawn the fold loop and a stall watchdog.
    pub async fn connect(
        client: CoreClient,
        events_rx: mpsc::UnboundedReceiver<CoreEvent>,
        client_version: &str,
        memory: Option<Arc<MemoryService>>,
    ) -> anyhow::Result<Self> {
        // Advertise the memory toolset in the handshake when a service is
        // attached and enabled; otherwise the reasoning layer never emits
        // `memory_query` events.
        let capability = memory
            .as_ref()
            .filter(|m| m.settings().enabled)
            .map(|m| memory_capability(m));
        client
            .initialize(client_version, capability)
            .await
            .map_err(|e| anyhow!("core handshake failed: {e}"))?;

        // Adopt the first existing thread, or create one.
        let listed = client
            .thread_list()
            .await
            .map_err(|e| anyhow!(e.to_string()))?;
        let core_id = listed
            .get("threads")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(|t| t.get("threadId"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let core_id = match core_id {
            Some(id) => id,
            None => client
                .thread_create(Some("main"), Some("app"))
                .await
                .map_err(|e| anyhow!(e.to_string()))?,
        };

        let mut state = State {
            threads: vec![Thread::new("t1", "main", core_id.clone())],
            active_id: "t1".into(),
            next_thread: 2,
            seq: 0,
            workers: Vec::new(),
            resyncing: false,
            last_event_at: now_millis(),
            stall_ms: STALL_MS,
            async_mode: false,
        };

        // Subscribe + seed the active thread's snapshot before any live event lands.
        let sub = client
            .thread_subscribe(&core_id, None)
            .await
            .map_err(|e| anyhow!(e.to_string()))?;
        let baseline = sub.get("baselineSeq").and_then(Value::as_u64).unwrap_or(0);
        if let Some(snapshot) = sub.get("snapshot") {
            let synth = synth_from_snapshot(snapshot, &mut state.seq);
            let t = &mut state.threads[0];
            for env in synth {
                if let TuiEvent::User { body } | TuiEvent::Assistant { body } = &env.event {
                    let role = if matches!(env.event, TuiEvent::User { .. }) {
                        "user"
                    } else {
                        "assistant"
                    };
                    t.messages.push(ChatMessage {
                        role: role.into(),
                        content: body.clone(),
                    });
                }
                State::push_event(t, env);
            }
        }
        state.threads[0].seq_tracker = SeqTracker::new(baseline);

        // Best-effort worker registry seed (a core with no worker surface just errors).
        if let Ok(list) = client.worker_list().await {
            state.workers = workers_from_payload(&list);
        }

        // Only keep an enabled service attached; a disabled one is dropped so
        // the runtime never advertises or serves memory.
        let memory = memory.filter(|m| m.settings().enabled);

        let (tx, _rx) = broadcast::channel(256);
        let rt = CoreRuntime {
            client: Arc::new(client),
            state: Arc::new(Mutex::new(state)),
            tx,
            closed: Arc::new(AtomicBool::new(false)),
            memory,
        };

        rt.spawn_fold_loop(events_rx);
        rt.spawn_watchdog();
        Ok(rt)
    }

    /// Fire a change notification on the broadcast channel, waking every subscriber.
    pub(super) fn ping(&self) {
        let _ = self.tx.send(());
    }

    /// Test seam: shorten the stall-detection threshold (ms). No behavior change at
    /// the [`STALL_MS`] default; exists so tests can exercise the `Stalled` state
    /// without waiting out the production silence window.
    #[doc(hidden)]
    pub fn set_stall_ms(&self, ms: i64) {
        self.state.lock().unwrap().stall_ms = ms;
    }
}
