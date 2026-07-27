//! Data types for the [`super`] chat surface.

/// How a composer should present itself for one frame.
///
/// The three states are deliberately independent rather than one enum: a busy
/// composer can still hold the keyboard (the orchestrator's does, so a follow-up
/// can be typed while a turn runs), and an unfocused one still has to show what
/// was left in the draft.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ComposerChrome<'a> {
    /// Whether this composer holds the keyboard. Drives the border colour and
    /// whether the caret is drawn solid or dim.
    pub(crate) focused: bool,
    /// Whether the surface behind it is working. Colours the border yellow.
    pub(crate) busy: bool,
    /// Shown dim in place of an empty draft, e.g. `"Ask for a change"`.
    pub(crate) placeholder: Option<&'a str>,
}

/// What a transcript did with the scroll offset it was given.
///
/// Returned rather than applied, because the offset lives on the caller's state
/// and a renderer that reached in to clamp it would be writing to `App` from a
/// draw pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct TranscriptFit {
    /// The offset actually used, clamped to what the content can scroll by.
    /// Write this back so a held `PageUp` stops at the top instead of banking
    /// presses that then have to be undone one by one.
    pub(crate) scroll: usize,
    /// Lines hidden below the viewport at this offset — what a "more below"
    /// hint counts.
    pub(crate) below: usize,
}
