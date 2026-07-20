//! The Account subpage's actions: arming and performing a logout.
//!
//! Logging out clears the on-disk credential store, which cannot be undone
//! without signing in again, so it is a two-step action: the first Enter arms
//! it, the second performs it. Leaving the subpage disarms.

use super::types::App;

impl App {
    /// Handle Enter on the Account subpage.
    ///
    /// Arms the logout on the first press and performs it on the second. The
    /// returned message is the status to show.
    pub(crate) fn confirm_logout(&mut self) -> String {
        if !self.logout_armed {
            self.logout_armed = true;
            return "Account · press Enter again to log out, or move away to cancel".into();
        }
        self.logout_armed = false;
        self.perform_logout()
    }

    /// Clear the stored credentials, returning the status to show.
    ///
    /// On success this also requests a relogin: the in-memory runtime still
    /// holds a live token, so leaving the session running would leave the user
    /// signed out on disk but signed in on screen. Quitting back to the login
    /// screen makes the logout mean what it says. A failed clear leaves the
    /// session alone — there is nothing to return to the login screen for.
    fn perform_logout(&mut self) -> String {
        let Some(home) = self.medulla_home.clone() else {
            return "Account · cannot log out: no Medulla home configured".into();
        };
        let store = medulla::auth::CredentialStore::at_home(&home);
        match store.clear() {
            Ok(()) => {
                self.relogin_requested = true;
                self.should_quit = true;
                "Account · logged out. Returning to the login screen…".into()
            }
            Err(e) => format!("Account · logout failed: {e}"),
        }
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
