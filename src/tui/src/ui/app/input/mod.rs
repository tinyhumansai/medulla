//! Event routing for [`App`]: the top-level [`App::on_event`] dispatch and the
//! surfaces it fans out to.
//!
//! One event stream reaches three different kinds of handler, so each has a
//! module of its own: [`paste`] decides who owns a bracketed-paste payload,
//! [`mouse`] resolves pointer events against what is under them, and [`nav`]
//! holds the small navigation helpers both share with the keyboard. Keyboard
//! handling proper lives in [`super::keys`].

use crossterm::event::{Event, KeyEventKind};

use super::types::{App, Cmd};

mod mouse;
mod nav;
mod paste;
#[cfg(test)]
mod tests;

pub(in crate::ui::app) use nav::SUBTASK_PAGE;

impl App {
    /// Route a terminal event to the key, mouse, or paste handler, producing any
    /// command the event loop must run.
    ///
    /// A handler that cannot return a command itself — the mouse path answers
    /// `bool`, and the picker's key path answers nothing — leaves one on
    /// [`App::queue_cmd`] instead, and it is collected here. This is the single
    /// funnel every event passes through, so a queued command cannot be
    /// stranded by whichever surface produced it.
    pub fn on_event(&mut self, ev: Event) -> Option<Cmd> {
        let direct = match ev {
            Event::Key(k) if matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                self.on_key(k)
            }
            Event::Mouse(m) => self.on_mouse(m),
            Event::Paste(text) => {
                self.on_paste(&text);
                None
            }
            _ => None,
        };
        // A handler that returned a command directly wins: it was answering this
        // event, whereas a queued one was left by a nested surface.
        direct.or_else(|| self.take_queued_cmd())
    }
}
