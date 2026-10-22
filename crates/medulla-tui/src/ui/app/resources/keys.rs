use super::types::ResourcesState;
use crossterm::event::KeyCode;

impl ResourcesState {
    pub(in crate::ui::app) fn key(&mut self, key: KeyCode) -> bool {
        match key {
            KeyCode::Char('1') => self.shown.cpu = !self.shown.cpu,
            KeyCode::Char('2') => self.shown.mem = !self.shown.mem,
            KeyCode::Char('3') => self.shown.net = !self.shown.net,
            KeyCode::Char('4') => {
                self.shown.agents = !self.shown.agents;
                if !self.shown.agents {
                    self.agent_signal_armed = None;
                }
            }
            KeyCode::Char('d') => self.show_disks = !self.show_disks,
            KeyCode::Char('i') => self.io_mode = !self.io_mode,
            KeyCode::Char('z') => self.net_zeroed = true,
            KeyCode::Char('a') => self.net_auto = !self.net_auto,
            KeyCode::Char('y') => self.net_sync = !self.net_sync,
            KeyCode::Char('b') | KeyCode::Char('n') => return true,
            KeyCode::Char('p') => self.preset = self.preset.saturating_sub(1),
            KeyCode::Char('P') => self.preset = (self.preset + 1) % self.presets.len().max(1),
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.interval_ms = (self.interval_ms + 100).min(10_000)
            }
            KeyCode::Char('-') => self.interval_ms = self.interval_ms.saturating_sub(100).max(100),
            _ => return false,
        }
        if matches!(key, KeyCode::Char('p') | KeyCode::Char('P')) {
            // An interactive preset choice replaces the loaded placement
            // overrides. The caller persists this resulting layout alongside
            // the selected preset, so reload preserves what the operator saw.
            self.apply_preset();
        }
        true
    }
}
