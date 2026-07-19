//! The core runtime's live-stream state machinery: the connection-wide fold loop
//! that routes each event to its thread, detects `seq` gaps and rebuilds from a
//! `snapshot.get` (§3.2), serves core-issued `memory_query` calls, and the stall
//! watchdog that keeps the UI re-pulling while a silent cycle is still in flight.

use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc;

use crate::runtime::core_client::CoreEvent;
use crate::runtime::CycleResultSummary;
use crate::ui::chat_store::{now_millis, ChatMessage};
use crate::ui::events::{EventEnvelope, TuiEvent};

use super::events::{map_core_event, synth_from_snapshot};
use super::memory::serve_memory_query;
use super::types::{CoreRuntime, State};

impl CoreRuntime {
    /// The connection-wide fold loop: route each event to its thread, detect `seq`
    /// gaps, and rebuild from a `snapshot.get` on a gap.
    pub(super) fn spawn_fold_loop(&self, mut events_rx: mpsc::UnboundedReceiver<CoreEvent>) {
        let client = self.client.clone();
        let state = self.state.clone();
        let tx = self.tx.clone();
        let closed = self.closed.clone();
        let memory = self.memory.clone();
        tokio::spawn(async move {
            while let Some(ev) = events_rx.recv().await {
                // A core-issued memory tool call: serve it from the attached
                // service and reply via `memory.answer`, never folding it as a
                // display event. With no service attached the event is ignored.
                if ev.kind() == "memory_query" {
                    if let Some(service) = memory.as_ref() {
                        let (id, result) = serve_memory_query(service, &ev.event);
                        let client = client.clone();
                        tokio::spawn(async move {
                            let _ = match result {
                                Ok(ok) => client.memory_answer(&id, Some(ok), None).await,
                                Err(msg) => client.memory_answer(&id, None, Some(&msg)).await,
                            };
                        });
                    }
                    continue;
                }
                // Detect a gap under the thread's tracker before folding.
                let (gap, resync_from, core_id) = {
                    let mut s = state.lock().unwrap();
                    match s.by_core(&ev.thread_id) {
                        Some(t) => {
                            let from = t.seq_tracker.last_seq();
                            let gap = t.seq_tracker.observe(ev.seq);
                            (gap, from, ev.thread_id.clone())
                        }
                        None => (false, 0, String::new()),
                    }
                };
                if core_id.is_empty() {
                    continue; // an event for a thread we do not track
                }

                if gap {
                    {
                        let mut s = state.lock().unwrap();
                        s.resyncing = true;
                    }
                    let _ = tx.send(());
                    if let Ok(payload) = client.snapshot_get(&core_id, Some(resync_from)).await {
                        let snapshot = payload.get("snapshot").cloned().unwrap_or(payload);
                        let mut s = state.lock().unwrap();
                        let base = s.seq;
                        let mut seq = base;
                        let synth = synth_from_snapshot(&snapshot, &mut seq);
                        s.seq = seq;
                        if let Some(t) = s.by_core(&core_id) {
                            t.events.clear();
                            t.chat_events.clear();
                            t.messages.clear();
                            for env in synth {
                                if let TuiEvent::User { body } | TuiEvent::Assistant { body } =
                                    &env.event
                                {
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
                            // A visible status note that a gap was reconciled (§3.2).
                            s.seq += 1;
                            let seq = s.seq;
                            let note = EventEnvelope {
                                seq,
                                at: now_millis(),
                                event: TuiEvent::Effect {
                                    effect: json!({
                                        "kind": "resync",
                                        "note": format!("stream resynced from snapshot (seq gap after {resync_from})"),
                                    }),
                                },
                            };
                            if let Some(t) = s.by_core(&core_id) {
                                State::push_event(t, note);
                            }
                        }
                        s.resyncing = false;
                    }
                    let _ = tx.send(());
                }

                // Fold the live event itself.
                {
                    let mut s = state.lock().unwrap();
                    s.last_event_at = now_millis();
                    s.resyncing = false;
                    s.seq += 1;
                    let seq = s.seq;
                    let event = map_core_event(&ev.event, &ev.cycle_id);
                    if !ev.cycle_id.is_empty() {
                        if let Some(t) = s.by_core(&ev.thread_id) {
                            t.latest_cycle_id = Some(ev.cycle_id.clone());
                        }
                    }
                    if let Some(t) = s.by_core(&ev.thread_id) {
                        match &event {
                            TuiEvent::User { body } => t.messages.push(ChatMessage {
                                role: "user".into(),
                                content: body.clone(),
                            }),
                            TuiEvent::Assistant { body } => t.messages.push(ChatMessage {
                                role: "assistant".into(),
                                content: body.clone(),
                            }),
                            TuiEvent::CycleStart { .. } => t.running = true,
                            TuiEvent::CycleEnd { pass_count, .. } => {
                                t.running = false;
                                t.last_result = Some(CycleResultSummary {
                                    pass_count: *pass_count,
                                    task_ledger: Default::default(),
                                });
                            }
                            _ => {}
                        }
                        State::push_event(
                            t,
                            EventEnvelope {
                                seq,
                                at: ev.at,
                                event,
                            },
                        );
                    }
                }
                let _ = tx.send(());
            }
            closed.store(true, Ordering::Relaxed);
            let _ = tx.send(());
        });
    }

    /// A watchdog that pings ~every second while a cycle runs, so the UI re-pulls a
    /// snapshot and the stall indicator escalates even when the stream has gone silent.
    pub(super) fn spawn_watchdog(&self) {
        let state = self.state.clone();
        let tx = self.tx.clone();
        let closed = self.closed.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(1000));
            loop {
                tick.tick().await;
                if closed.load(Ordering::Relaxed) {
                    break;
                }
                let running = { state.lock().unwrap().active().running };
                if running {
                    let _ = tx.send(());
                }
            }
        });
    }
}
