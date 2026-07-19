//! Crossterm terminal lifecycle for the TUI binary: enter/leave the alternate
//! screen, toggle raw mode and mouse capture, negotiate the kitty keyboard
//! enhancement, and guarantee a panic-safe restore. The [`TermGuard`] RAII
//! wrapper restores the terminal on drop; [`restore`] is also invoked directly
//! from the panic hook so the message prints on a clean screen.

use std::io::{self, Write};

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{execute, queue};

/// RAII guard that owns the terminal's raw-mode/alt-screen/kitty state and
/// restores it on drop. Construct with [`TermGuard::setup`].
pub(crate) struct TermGuard {
    alt_screen: bool,
    kitty: bool,
}

impl TermGuard {
    /// Enter raw mode, optionally the alternate screen, enable mouse capture,
    /// and push the kitty disambiguation flag when the terminal supports it.
    pub(crate) fn setup(alt_screen: bool) -> io::Result<Self> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        if alt_screen {
            execute!(out, EnterAlternateScreen)?;
        }
        execute!(out, EnableMouseCapture)?;
        let kitty = supports_keyboard_enhancement().unwrap_or(false);
        if kitty {
            queue!(
                out,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )?;
        }
        out.flush()?;
        Ok(TermGuard { alt_screen, kitty })
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        restore(self.alt_screen, self.kitty);
    }
}

/// Best-effort teardown of the terminal state. Safe to call from the panic hook
/// and from [`TermGuard::drop`]; every step ignores errors so a partial restore
/// still runs the remaining steps.
pub(crate) fn restore(alt_screen: bool, kitty: bool) {
    let mut out = io::stdout();
    if kitty {
        let _ = queue!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(out, DisableMouseCapture);
    if alt_screen {
        let _ = execute!(out, LeaveAlternateScreen);
    }
    let _ = disable_raw_mode();
    let _ = out.flush();
}

/// Toggle mouse capture at runtime as the app's `mouse_capture` flag changes.
pub(crate) fn set_mouse_capture(on: bool) {
    let mut out = io::stdout();
    if on {
        let _ = execute!(out, EnableMouseCapture);
    } else {
        let _ = execute!(out, DisableMouseCapture);
    }
}
