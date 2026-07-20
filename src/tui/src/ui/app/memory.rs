//! The Memory tab's maintenance actions: starting a persona-memory ingest.
//!
//! Browsing and search state live in [`super::state`]; this module owns the
//! actions that *change* the store rather than read it.

use super::types::{App, Cmd};

impl App {
    /// Start a persona-memory ingest, returning the command that runs it.
    ///
    /// Ingest summarizes transcripts through a paid provider, so this refuses to
    /// start a second run while one is in flight — a double keypress would
    /// otherwise spend twice. Returns `None` (with an explanatory status) when
    /// it declines.
    pub(super) fn start_memory_ingest(&mut self, backfill: bool) -> Option<Cmd> {
        if self.memory_ingesting {
            self.set_status("Memory · an ingest is already running");
            return None;
        }
        self.memory_ingesting = true;
        self.set_status(if backfill {
            "Memory · backfilling (walks everything; this can take a while)…"
        } else {
            "Memory · ingesting new activity…"
        });
        Some(Cmd::IngestMemory { backfill })
    }
}
