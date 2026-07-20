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
    /// The in-memory runtime keeps whatever token it already holds, so the
    /// current session continues; the next launch starts signed out. Saying so
    /// is better than implying the session ended.
    fn perform_logout(&mut self) -> String {
        let Some(home) = self.medulla_home.clone() else {
            return "Account · cannot log out: no Medulla home configured".into();
        };
        let store = medulla::auth::CredentialStore::at_home(&home);
        match store.clear() {
            Ok(()) => "Account · logged out. Stored credentials cleared; this session continues until you quit.".into(),
            Err(e) => format!("Account · logout failed: {e}"),
        }
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
