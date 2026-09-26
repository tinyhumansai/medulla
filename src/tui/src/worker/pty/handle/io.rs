//! Sending bytes to the child and resizing the pty + emulator together.

use std::sync::atomic::Ordering;

use portable_pty::PtySize;

use super::super::sync::lock;
use super::types::{release_queued, MAX_QUEUED_WRITE_BYTES};
use super::SessionHandle;

impl SessionHandle {
    /// Queue raw bytes for the pty — the focused pane's keystrokes, and the
    /// prompts [`inject_prompt`](super::super::inject_prompt) pastes.
    ///
    /// Returns as soon as the bytes are queued; the session's writer thread
    /// performs the write. Nothing here blocks, and that is the point twice
    /// over. `write_all` on a pty master **parks in the kernel** when the child
    /// is not draining its input — mid tool call, stopped on a modal, or simply
    /// still loading — and the tty buffer is only a few kilobytes, which an
    /// injected prompt can exceed on its own. Giving every session its own locks
    /// stops that stalling *other* sessions; handing the write to a thread stops
    /// it stalling the *caller*, which on the keystroke path is the render
    /// thread.
    ///
    /// Queueing cannot block, so the queue is bounded by *refusal* instead:
    /// [`MAX_QUEUED_WRITE_BYTES`] of unwritten bytes per session, past which a
    /// write is rejected rather than waited out. A child that never drains its
    /// stdin therefore cannot make this grow without limit.
    ///
    /// # Errors
    ///
    /// When the session has exited, when `bytes` alone exceeds the queue, when
    /// the queue is already full, or when the writer thread is gone. A failure
    /// of the *write itself* has no caller left to reach and is recorded on the
    /// row's `last_error` instead.
    pub(in super::super) fn write(&self, bytes: &[u8]) -> Result<(), String> {
        // Refused before anything is copied: a write larger than the whole budget
        // can never be admitted, however empty the queue is, and saying so here
        // beats reserving and unwinding.
        if bytes.len() > MAX_QUEUED_WRITE_BYTES {
            return Err(format!(
                "{}: {} bytes is more than the {MAX_QUEUED_WRITE_BYTES}-byte write queue holds",
                self.meta.id,
                bytes.len()
            ));
        }
        if !self.is_running() {
            return Err(format!("{} has exited", self.meta.id));
        }
        // The queue handle is cloned out and the guard dropped *before* the send,
        // so even this session's own lock is held for a clone and nothing more.
        let writes = {
            let io = lock(&self.io);
            let Some(io) = io.as_ref() else {
                return Err(format!("{} has exited", self.meta.id));
            };
            io.writes.clone()
        };
        // Reserved before queueing, and atomically, so two concurrent writers
        // cannot both observe room and both take it.
        if self
            .queued_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
                let wanted = queued + bytes.len();
                (wanted <= MAX_QUEUED_WRITE_BYTES).then_some(wanted)
            })
            .is_err()
        {
            return Err(format!(
                "{}: the write queue is full ({MAX_QUEUED_WRITE_BYTES} bytes unwritten) — \
                 the harness is not reading its input",
                self.meta.id
            ));
        }
        // Fails only if the writer thread has stopped, which means the pty stopped
        // accepting bytes. Worth reporting: the caller's keystrokes went nowhere.
        writes.send(bytes.to_vec()).map_err(|_| {
            // Saturating: the writer thread may already have zeroed the budget on
            // its way out, and these bytes were reserved before that. See
            // [`release_queued`].
            release_queued(&self.queued_bytes, bytes.len());
            format!("{}: the writer thread is gone", self.meta.id)
        })
    }

    /// Resize the pty and the emulator together.
    ///
    /// Both must move: the child reflows to the pty size, so an emulator of a
    /// different size renders a torn screen.
    pub(in super::super) fn resize(&self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        {
            let mut parser = lock(&self.screen);
            if parser.screen().size() == (rows, cols) {
                return; // already correct — skip the SIGWINCH storm
            }
            parser.set_size(rows, cols);
        }
        if let Some(io) = lock(&self.io).as_ref() {
            let _ = io.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }
}
