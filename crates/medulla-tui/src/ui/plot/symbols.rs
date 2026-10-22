//! Glyph lookup tables used by btop-style plots.

/// Symbol family used to encode two vertical samples in one terminal cell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Symbol {
    /// Braille patterns, the btop-compatible default.
    #[default]
    Braille,
    /// Unicode block elements.
    Block,
    /// Shaded blocks for broad terminal compatibility.
    Tty,
}

impl Symbol {
    pub(super) fn glyph(self, invert: bool, previous: u8, current: u8) -> char {
        let table = match (self, invert) {
            (Self::Braille, false) => BRAILLE_UP,
            (Self::Braille, true) => BRAILLE_DOWN,
            (Self::Block, false) => BLOCK_UP,
            (Self::Block, true) => BLOCK_DOWN,
            (Self::Tty, false) => TTY_UP,
            (Self::Tty, true) => TTY_DOWN,
        };
        table[usize::from(previous) * 5 + usize::from(current)]
    }
}

// Copyright (c) 2021 Aristocratos.
// Verbatim glyph tables from btop src/btop_draw.cpp Symbols::graph_symbols,
// lines 89–132 at https://github.com/aristocratos/btop/blob/89fcdf4094e89f8ac215ee6c3ee74aeffb618e86/src/btop_draw.cpp
// btop is licensed under Apache-2.0; see https://github.com/aristocratos/btop/blob/89fcdf4094e89f8ac215ee6c3ee74aeffb618e86/LICENSE
const BRAILLE_UP: [char; 25] = [
    ' ', '⢀', '⢠', '⢰', '⢸', '⡀', '⣀', '⣠', '⣰', '⣸', '⡄', '⣄', '⣤', '⣴', '⣼', '⡆', '⣆', '⣦', '⣶',
    '⣾', '⡇', '⣇', '⣧', '⣷', '⣿',
];
const BRAILLE_DOWN: [char; 25] = [
    ' ', '⠈', '⠘', '⠸', '⢸', '⠁', '⠉', '⠙', '⠹', '⢹', '⠃', '⠋', '⠛', '⠻', '⢻', '⠇', '⠏', '⠟', '⠿',
    '⢿', '⡇', '⡏', '⡟', '⡿', '⣿',
];
const BLOCK_UP: [char; 25] = [
    ' ', '▗', '▗', '▐', '▐', '▖', '▄', '▄', '▟', '▟', '▖', '▄', '▄', '▟', '▟', '▌', '▙', '▙', '█',
    '█', '▌', '▙', '▙', '█', '█',
];
const BLOCK_DOWN: [char; 25] = [
    ' ', '▝', '▝', '▐', '▐', '▘', '▀', '▀', '▜', '▜', '▘', '▀', '▀', '▜', '▜', '▌', '▛', '▛', '█',
    '█', '▌', '▛', '▛', '█', '█',
];
const TTY_UP: [char; 25] = [
    ' ', '░', '░', '▒', '▒', '░', '░', '▒', '▒', '█', '░', '▒', '▒', '▒', '█', '▒', '▒', '▒', '█',
    '█', '▒', '█', '█', '█', '█',
];
const TTY_DOWN: [char; 25] = TTY_UP;
