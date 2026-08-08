    fn fold_available(
        &self,
        id: &str,
        provider: HarnessProvider,
        tailer: &mut SessionTailer,
        stream: &mut TurnStream,
        on_event: &mut Option<medulla::daemon::providers::OnEvent>,
        last_line_at: &mut i64,
    ) -> Option<RunTaskResult> {
        let poll = tailer.poll();
        // Codex cannot be told its id, so it is learned from the rollout the
        // first time the tailer locates one.
        if let Some(located) = &poll.located {
            self.sessions
                .record_session_id(id, located.harness_session_id.clone());
            if provider == HarnessProvider::Codex {
                if let Some(thread_name) = medulla::session_history::codex_thread_label(
                    &self.env,
                    &located.harness_session_id,
                ) {
                    self.sessions.record_thread_name(id, thread_name);
                }
            }
        }
        for line in poll.lines {
            *last_line_at = medulla::clock::now_millis();
            let fold = stream.observe(&line.text);
            self.workspace_context
                .lock()
                .expect("workspace context lock poisoned")
                .insert(id.to_string(), stream.workspace_context());
            // The peer watches its task through these. Dropping them would
            // leave it with an ack, silence, then a reply — which is what
            // this executor used to do.
            if let Some(callback) = on_event.as_mut() {
                for event in &fold.events {
                    callback(event);
                }
            }
            if let Some(reply) = fold.reply {
                return Some(RunTaskResult {
                    provider,
                    reply,
                    events: stream.events(),
                    usage: stream.usage(),
                    session_id: self.sessions.row(id).and_then(|row| row.session_id),
                });
            }
        }
        None
    }

    /// Poll the transcript until the harness says the turn is over.
    ///
    /// `timeout_ms` is the caller's configured idle watchdog (`[host]
    /// .taskTimeoutMs`, mirroring the headless executor's own `timeout_ms`) —
    /// the hard ceiling on how long a turn may go without producing a single
    /// transcript line. It is distinct from, and can override, the two fixed
    /// budgets below: [`LOCATE_BUDGET`] covers a harness that never starts a
    /// turn at all, and [`STALL_BUDGET_MS`] is a soft "probably finished"
    /// signal for a transcript that stops without a stated reason. A caller
    /// configuring a shorter ceiling than either means it, and is honored
    /// ahead of them.
    async fn await_turn(
        &self,
        id: &str,
