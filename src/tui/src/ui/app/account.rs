//! The Account subpage's actions: arming and performing a logout.
//!
//! Logging out clears the session the embedded core holds, which cannot be
//! undone without signing in again, so it is a two-step action: the first Enter
//! arms it, the second performs it. Leaving the subpage disarms.

use super::types::{App, Cmd};

impl App {
    /// Handle Enter on the Account subpage.
    ///
    /// Arms the logout on the first press and performs it on the second.
    /// Returns the status to show and, on the second press, the command that
    /// actually clears the session.
    pub(crate) fn confirm_logout(&mut self) -> (String, Option<Cmd>) {
        if !self.logout_armed {
            self.logout_armed = true;
            return (
                "Account · press Enter again to log out, or move away to cancel".into(),
                None,
            );
        }
        self.logout_armed = false;
        ("Account · logging out…".into(), Some(Cmd::Logout))
    }

    /// Record that the session was cleared, and quit back to the login screen.
    ///
    /// The runtime is still holding a live session in memory, so leaving it
    /// running would leave the operator signed out on disk but signed in on
    /// screen. Quitting is what makes the logout mean what it says.
    ///
    /// Called from the event loop when the clear succeeds — never optimistically
    /// on the keypress, because a failed clear must leave the session alone.
    pub fn logged_out(&mut self) {
        self.relogin_requested = true;
        self.should_quit = true;
    }

    /// Whether the app quit in order to re-authenticate. Read by the startup
    /// loop after [`crate::event_loop::run`] returns.
    pub fn relogin_requested(&self) -> bool {
        self.relogin_requested
    }

    /// Disarm a pending logout. Called whenever focus moves, so an armed logout
    /// never survives navigating elsewhere and returning.
    pub(crate) fn disarm_logout(&mut self) {
        self.logout_armed = false;
    }

    /// Whether a logout is currently armed. Render/test seam.
    pub(crate) fn logout_armed(&self) -> bool {
        self.logout_armed
    }
}
